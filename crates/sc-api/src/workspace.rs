//! Writing a project's files out to a plain directory (for tools that need a filesystem).

use std::path::PathBuf;

use sc_git::RepoStore;
use std::sync::Arc;
use uuid::Uuid;

/// Most files a project may have when exported.
pub const MAX_EXPORT_FILES: usize = 5000;

/// Writes the files of `project` at `rev` (default: branch tip) into `dir` and
/// returns their paths. An empty repository exports nothing.
pub async fn export(
    repos: Arc<RepoStore>,
    project: Uuid,
    rev: Option<String>,
    dir: PathBuf,
) -> anyhow::Result<Vec<String>> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<String>> {
        let repo = repos.open(&project.to_string())?;
        if rev.is_none() && repo.head()?.is_none() {
            return Ok(Vec::new());
        }
        let files = repo.list_files(rev.as_deref())?;
        anyhow::ensure!(files.len() <= MAX_EXPORT_FILES, "project has more than {MAX_EXPORT_FILES} files");
        let mut paths = Vec::with_capacity(files.len());
        for entry in files {
            let target = dir.join(&entry.path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, repo.read_file(rev.as_deref(), &entry.path)?)?;
            paths.push(entry.path);
        }
        Ok(paths)
    })
    .await?
}

/// The branch tip, or `None` for an empty repository.
pub async fn head(repos: Arc<RepoStore>, project: Uuid) -> anyhow::Result<Option<String>> {
    Ok(tokio::task::spawn_blocking(move || repos.open(&project.to_string())?.head()).await??)
}

/// UTF-8 files up to `max_bytes` at `rev`, by path. Binary and larger files are left out.
pub async fn text_files(
    repos: Arc<RepoStore>,
    project: Uuid,
    rev: String,
    max_bytes: usize,
) -> anyhow::Result<std::collections::HashMap<String, String>> {
    tokio::task::spawn_blocking(move || {
        let repo = repos.open(&project.to_string())?;
        let mut out = std::collections::HashMap::new();
        for f in repo.list_files(Some(&rev))?.into_iter().take(MAX_EXPORT_FILES) {
            if f.size > max_bytes {
                continue;
            }
            if let Ok(text) = String::from_utf8(repo.read_file(Some(&rev), &f.path)?) {
                out.insert(f.path, text);
            }
        }
        Ok(out)
    })
    .await?
}
