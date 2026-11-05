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
