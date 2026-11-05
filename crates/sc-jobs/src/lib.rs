//! Job backends: run an EDA flow over a prepared work directory and stream
//! its output to a log file.
//!
//! A backend gets a work directory that already holds the project files at
//! the job's commit. LibreLane writes its run into `<workdir>/runs/<RUN_TAG>`;
//! the final views (GDS, DEF, netlists, `metrics.json`, a layout render) end
//! up in [`final_dir`].

mod docker;
pub mod reports;
mod summary;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

pub use docker::{DockerBackend, DockerConfig};
pub use summary::{parse_metrics, summarize};
pub use tokio_util::sync::CancellationToken;

/// LibreLane `--run-tag`; fixed so results are at a known path.
pub const RUN_TAG: &str = "sc";
/// Logs beyond this are cut off (the flow keeps running).
pub const MAX_LOG_BYTES: u64 = 32 * 1024 * 1024;

pub fn run_dir(workdir: &Path) -> PathBuf {
    workdir.join("runs").join(RUN_TAG)
}

pub fn final_dir(workdir: &Path) -> PathBuf {
    run_dir(workdir).join("final")
}

/// Reads `path`, which must be a regular file inside `root` with no symlink anywhere on the way.
/// Job directories are written by flow containers running user-controlled tools, so the server
/// never follows a link they could have planted.
pub async fn read_inside(root: &Path, path: &Path) -> std::io::Result<Vec<u8>> {
    let denied = || std::io::Error::new(std::io::ErrorKind::PermissionDenied, "not a plain file inside the job");
    let rel = path.strip_prefix(root).map_err(|_| denied())?;
    let mut cur = root.to_path_buf();
    for part in rel.components() {
        let std::path::Component::Normal(p) = part else { return Err(denied()) };
        cur.push(p);
        if tokio::fs::symlink_metadata(&cur).await?.file_type().is_symlink() {
            return Err(denied());
        }
    }
    if !tokio::fs::symlink_metadata(&cur).await?.is_file() {
        return Err(denied());
    }
    tokio::fs::read(&cur).await
}

/// Removes every symlink under `root` that points outside it (or nowhere). Called once the flow
/// container has exited, before the server reads anything the flow wrote.
pub fn remove_escaping_links(root: &Path) -> usize {
    let Ok(root) = root.canonicalize() else { return 0 };
    let mut removed = 0;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if meta.file_type().is_symlink() {
                let inside = path.canonicalize().is_ok_and(|t| t.starts_with(&root));
                if !inside && std::fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            } else if meta.is_dir() {
                stack.push(path);
            }
        }
    }
    removed
}

#[derive(Debug, Clone)]
pub struct JobSpec {
    /// Unique, filesystem- and container-name-safe job id.
    pub id: String,
    pub workdir: PathBuf,
    /// Flow config, relative to `workdir`.
    pub config: String,
    pub pdk: String,
    /// Combined stdout/stderr of the flow is appended here.
    pub log_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The flow process exited; `0` means success.
    Exited(i32),
    Canceled,
    /// Stopped after running longer than the backend's time limit.
    TimedOut,
}

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("{0}")]
    Setup(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("this backend cannot run extra commands")]
    Unsupported,
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Backend: Send + Sync {
    /// Human-readable description of the toolchain (image and digest), stored with each job.
    fn toolchain(&self) -> String;

    /// Stops work left over from a previous process (called once at startup, before any run).
    fn cleanup_orphans(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }

    /// Runs the flow to completion, or until `cancel` fires.
    fn run<'a>(&'a self, spec: &'a JobSpec, cancel: CancellationToken) -> BoxFuture<'a, Result<Outcome, JobError>>;

    /// Runs `argv` in the flow's environment (same toolchain, PDK and work directory, `/work` as the
    /// current directory), appending its output to `log`. Used for steps after the flow, such as
    /// gate-level simulation. Backends without it report [`JobError::Unsupported`].
    fn exec<'a>(
        &'a self,
        spec: &'a JobSpec,
        argv: &'a [String],
        log: &'a Path,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Outcome, JobError>> {
        let _ = (spec, argv, log, cancel);
        Box::pin(async { Err(JobError::Unsupported) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn links_out_of_the_job_are_refused_and_removed() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "host file").unwrap();
        let job = tempfile::tempdir().unwrap();
        let root = job.path();
        std::fs::create_dir_all(root.join("runs/sc/final")).unwrap();
        std::fs::write(root.join("runs/sc/final/metrics.json"), "{}").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), root.join("runs/sc/final/leak.json")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("runs/sc/out")).unwrap();
        std::os::unix::fs::symlink("metrics.json", root.join("runs/sc/final/inside.json")).unwrap();

        assert_eq!(read_inside(root, &root.join("runs/sc/final/metrics.json")).await.unwrap(), b"{}");
        for bad in ["runs/sc/final/leak.json", "runs/sc/out/secret", "runs/sc/final/inside.json", "runs/sc/final"] {
            assert!(read_inside(root, &root.join(bad)).await.is_err(), "{bad}");
        }
        assert!(read_inside(root, &outside.path().join("secret")).await.is_err());

        assert_eq!(remove_escaping_links(root), 2);
        assert!(!root.join("runs/sc/final/leak.json").exists());
        assert!(std::fs::symlink_metadata(root.join("runs/sc/final/inside.json")).is_ok(), "links within the job stay");
        assert!(outside.path().join("secret").exists());
    }
}
