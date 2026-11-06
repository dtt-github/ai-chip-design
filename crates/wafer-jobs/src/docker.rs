use std::path::Path;
use std::process::Stdio;

use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::{Backend, BoxFuture, CancellationToken, JobError, JobSpec, MAX_LOG_BYTES, Outcome, RUN_TAG};

#[derive(Debug, Clone)]
pub struct DockerConfig {
    /// `docker` (or a compatible CLI such as `podman`).
    pub docker: String,
    /// LibreLane image, pinned by tag or digest.
    pub image: String,
    /// Named volume (or host path) holding the PDKs, mounted read-only at `/pdk`.
    pub pdk_volume: String,
    pub cpus: f32,
    pub memory: String,
    /// `uid:gid` to run as, so results are owned by the API's user.
    pub user: Option<String>,
    /// Wall-clock limit per container; the container is killed after it.
    pub timeout: Option<std::time::Duration>,
}

/// Runs LibreLane in a throwaway container with no network access.
pub struct DockerBackend {
    config: DockerConfig,
    digest: String,
    execs: std::sync::atomic::AtomicU64,
}

impl DockerBackend {
    /// Checks the image is present and records its digest for provenance.
    pub async fn new(config: DockerConfig) -> Result<Self, JobError> {
        let out = Command::new(&config.docker)
            .args(["image", "inspect", "--format", "{{if .RepoDigests}}{{index .RepoDigests 0}}{{else}}{{.Id}}{{end}}"])
            .arg(&config.image)
            .output()
            .await
            .map_err(|e| JobError::Setup(format!("cannot run {}: {e}", config.docker)))?;
        if !out.status.success() {
            return Err(JobError::Setup(format!(
                "image {} is not available locally; pull it first ({})",
                config.image,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let digest = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(Self { config, digest, execs: Default::default() })
    }

    fn container_name(spec: &JobSpec) -> String {
        format!("wafer-job-{}", spec.id)
    }

    async fn run_inner(&self, spec: &JobSpec, cancel: CancellationToken) -> Result<Outcome, JobError> {
        let argv: Vec<String> = ["librelane", "--manual-pdk", "--pdk-root", "/pdk", "--pdk", &spec.pdk]
            .into_iter()
            .chain(["--run-tag", RUN_TAG, "--overwrite", &format!("./{}", spec.config)])
            .map(String::from)
            .collect();
        self.container(spec, &Self::container_name(spec), &argv, &spec.log_path, cancel).await
    }

    /// One throwaway container over the job's work directory, output appended to `log`.
    async fn container(
        &self,
        spec: &JobSpec,
        name: &str,
        argv: &[String],
        log: &Path,
        cancel: CancellationToken,
    ) -> Result<Outcome, JobError> {
        let c = &self.config;
        let workdir = spec.workdir.canonicalize()?;
        let mut cmd = Command::new(&c.docker);
        cmd.args(["run", "--rm", "--name", name, "--network", "none"])
            .args(["--cpus", &c.cpus.to_string(), "--memory", &c.memory, "--pids-limit", "4096"])
            .args(["--security-opt", "no-new-privileges", "--cap-drop", "ALL"])
            .args(["-e", "HOME=/tmp", "-e", "PDK_ROOT=/pdk"])
            .args(["-v", &format!("{}:/pdk:ro", c.pdk_volume)])
            .args(["-v", &format!("{}:/work", workdir.display())])
            .args(["-w", "/work"]);
        if let Some(user) = &c.user {
            cmd.args(["--user", user]);
        }
        cmd.arg(&c.image)
            .args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let log = OpenOptions::new().create(true).append(true).open(log).await?;
        let log = std::sync::Arc::new(Mutex::new((log, 0u64)));
        let mut child = cmd.spawn()?;
        let stdout = pump(child.stdout.take().expect("piped"), log.clone());
        let stderr = pump(child.stderr.take().expect("piped"), log.clone());

        let limit = async {
            match c.timeout {
                Some(t) => tokio::time::sleep(t).await,
                None => std::future::pending().await,
            }
        };
        // Killing the CLI client does not stop the container; kill it by name.
        let kill = || async {
            let _ = Command::new(&c.docker).args(["kill", name]).output().await;
        };
        let outcome = tokio::select! {
            status = child.wait() => Outcome::Exited(status?.code().unwrap_or(-1)),
            _ = cancel.cancelled() => {
                kill().await;
                let _ = child.wait().await;
                Outcome::Canceled
            }
            _ = limit => {
                kill().await;
                let _ = child.wait().await;
                Outcome::TimedOut
            }
        };
        let _ = tokio::join!(stdout, stderr);
        log.lock().await.0.flush().await?;
        Ok(outcome)
    }
}

/// Copies lines from a child pipe into the shared log, up to `MAX_LOG_BYTES`.
fn pump(
    pipe: impl AsyncRead + Unpin + Send + 'static,
    log: std::sync::Arc<Mutex<(tokio::fs::File, u64)>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(pipe).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut guard = log.lock().await;
            let (file, written) = &mut *guard;
            if *written >= MAX_LOG_BYTES {
                continue;
            }
            let mut bytes = line.into_bytes();
            bytes.push(b'\n');
            *written += bytes.len() as u64;
            if *written >= MAX_LOG_BYTES {
                bytes.extend_from_slice(b"\n[log truncated]\n");
            }
            if file.write_all(&bytes).await.is_err() {
                break;
            }
            let _ = file.flush().await;
        }
    })
}

impl Backend for DockerBackend {
    fn toolchain(&self) -> String {
        if self.digest.contains('@') { self.digest.clone() } else { format!("{} ({})", self.config.image, self.digest) }
    }

    fn cleanup_orphans(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let docker = &self.config.docker;
            let Ok(out) = Command::new(docker).args(["ps", "-q", "--filter", "name=^wafer-job-"]).output().await else {
                return;
            };
            let ids: Vec<String> = String::from_utf8_lossy(&out.stdout).split_whitespace().map(String::from).collect();
            if !ids.is_empty() {
                tracing::warn!(count = ids.len(), "killing job containers left by a previous run");
                let _ = Command::new(docker).arg("kill").args(&ids).output().await;
            }
        })
    }

    fn run<'a>(&'a self, spec: &'a JobSpec, cancel: CancellationToken) -> BoxFuture<'a, Result<Outcome, JobError>> {
        Box::pin(self.run_inner(spec, cancel))
    }

    fn exec<'a>(
        &'a self,
        spec: &'a JobSpec,
        argv: &'a [String],
        log: &'a Path,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Outcome, JobError>> {
        Box::pin(async move {
            // Unique per call; the `wafer-job-` prefix keeps orphan cleanup working.
            let n = self.execs.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let name = format!("{}-x{n}", Self::container_name(spec));
            self.container(spec, &name, argv, log, cancel).await
        })
    }
}
