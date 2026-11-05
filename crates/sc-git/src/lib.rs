//! Per-project Git storage.
//!
//! Every SiliconCommons project is a bare Git repository on disk. The API never
//! checks out a worktree: reads go straight to tree objects and writes build a
//! new tree with [`git2::build::TreeUpdateBuilder`] and commit it onto the
//! project's default branch. Writes are optimistic: the caller passes the commit
//! it based its edits on and gets [`GitError::Conflict`] if the branch moved.

use std::path::{Path, PathBuf};

use git2::build::TreeUpdateBuilder;
use git2::{ErrorCode, FileMode, ObjectType, Oid, Repository, Signature, TreeWalkMode, TreeWalkResult};
use serde::Serialize;

pub const DEFAULT_BRANCH: &str = "main";
const HEAD_REF: &str = "refs/heads/main";
/// Largest single file the text API accepts (layouts go to object storage, not Git).
pub const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("file too large: {0} bytes")]
    TooLarge(usize),
    #[error(transparent)]
    Git(#[from] git2::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, GitError>;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CommitInfo {
    pub id: String,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    /// Seconds since the Unix epoch.
    pub time: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub size: usize,
}

#[derive(Debug, Clone)]
pub struct Author {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone)]
pub enum Change {
    Write { path: String, content: Vec<u8> },
    Delete { path: String },
}

/// Directory of bare repositories, one per project, named `<key>.git`.
#[derive(Debug, Clone)]
pub struct RepoStore {
    root: PathBuf,
}

impl RepoStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn path(&self, key: &str) -> PathBuf {
        self.root.join(format!("{key}.git"))
    }

    /// Creates the bare repository and an initial commit holding `files`.
    pub fn create(&self, key: &str, files: Vec<(String, Vec<u8>)>, author: &Author) -> Result<CommitInfo> {
        let path = self.path(key);
        if path.exists() {
            return Err(GitError::Conflict(format!("repository {key} already exists")));
        }
        let repo = Repository::init_bare(&path)?;
        repo.set_head(HEAD_REF)?;
        let project = ProjectRepo { repo };
        let changes = files.into_iter().map(|(path, content)| Change::Write { path, content }).collect();
        project.commit(changes, "Initial commit", author, None)
    }

    pub fn open(&self, key: &str) -> Result<ProjectRepo> {
        match Repository::open_bare(self.path(key)) {
            Ok(repo) => Ok(ProjectRepo { repo }),
            Err(e) if e.code() == ErrorCode::NotFound => Err(GitError::NotFound(format!("repository {key}"))),
            Err(e) => Err(e.into()),
        }
    }
}

pub struct ProjectRepo {
    repo: Repository,
}

impl ProjectRepo {
    pub fn path(&self) -> &Path {
        self.repo.path()
    }

    /// Commit id of the default branch tip, or `None` for an empty repository.
    pub fn head(&self) -> Result<Option<String>> {
        Ok(self.head_oid()?.map(|o| o.to_string()))
    }

    fn head_oid(&self) -> Result<Option<Oid>> {
        match self.repo.refname_to_id(HEAD_REF) {
            Ok(oid) => Ok(Some(oid)),
            Err(e) if e.code() == ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Full commit id for a revision (commit id, prefix, branch or tag).
    pub fn resolve(&self, rev: &str) -> Result<String> {
        Ok(self.resolve_commit(Some(rev))?.id().to_string())
    }

    fn resolve_commit(&self, rev: Option<&str>) -> Result<git2::Commit<'_>> {
        let oid = match rev {
            Some(r) => self
                .repo
                .revparse_single(r)
                .map_err(|_| GitError::NotFound(format!("revision {r}")))?
                .peel_to_commit()?
                .id(),
            None => self.head_oid()?.ok_or_else(|| GitError::NotFound("empty repository".into()))?,
        };
        Ok(self.repo.find_commit(oid)?)
    }

    /// All files (blobs) at `rev`, sorted by path.
    pub fn list_files(&self, rev: Option<&str>) -> Result<Vec<FileEntry>> {
        let tree = self.resolve_commit(rev)?.tree()?;
        let mut files = Vec::new();
        let mut walk_err = None;
        tree.walk(TreeWalkMode::PreOrder, |dir, entry| {
            if entry.kind() == Some(ObjectType::Blob) {
                let name = entry.name().unwrap_or_default();
                match self.repo.find_blob(entry.id()) {
                    Ok(blob) => files.push(FileEntry { path: format!("{dir}{name}"), size: blob.size() }),
                    Err(e) => {
                        walk_err = Some(e);
                        return TreeWalkResult::Abort;
                    }
                }
            }
            TreeWalkResult::Ok
        })?;
        if let Some(e) = walk_err {
            return Err(e.into());
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    pub fn read_file(&self, rev: Option<&str>, path: &str) -> Result<Vec<u8>> {
        let path = normalize_path(path)?;
        let tree = self.resolve_commit(rev)?.tree()?;
        let entry = tree.get_path(Path::new(&path)).map_err(|_| GitError::NotFound(format!("file {path}")))?;
        let blob = entry.to_object(&self.repo)?.into_blob().map_err(|_| GitError::NotFound(format!("file {path}")))?;
        Ok(blob.content().to_vec())
    }

    /// Applies `changes` on top of the branch tip and commits them.
    ///
    /// `expected_parent` is the commit the caller based its edits on; if the
    /// branch has moved since, nothing is written and `Conflict` is returned.
    pub fn commit(
        &self,
        changes: Vec<Change>,
        message: &str,
        author: &Author,
        expected_parent: Option<&str>,
    ) -> Result<CommitInfo> {
        if changes.is_empty() {
            return Err(GitError::InvalidPath("no changes to commit".into()));
        }
        let head = self.head_oid()?;
        if let Some(expected) = expected_parent {
            let expected = Oid::from_str(expected).map_err(|_| GitError::NotFound(format!("revision {expected}")))?;
            if head != Some(expected) {
                return Err(GitError::Conflict(format!(
                    "branch {DEFAULT_BRANCH} moved: expected {expected}, now {}",
                    head.map(|h| h.to_string()).unwrap_or_else(|| "empty".into())
                )));
            }
        }

        let parent = head.map(|oid| self.repo.find_commit(oid)).transpose()?;
        let base_tree = match &parent {
            Some(c) => c.tree()?,
            None => self.repo.find_tree(self.repo.treebuilder(None)?.write()?)?,
        };

        let mut update = TreeUpdateBuilder::new();
        for change in changes {
            match change {
                Change::Write { path, content } => {
                    let path = normalize_path(&path)?;
                    if content.len() > MAX_FILE_BYTES {
                        return Err(GitError::TooLarge(content.len()));
                    }
                    let blob = self.repo.blob(&content)?;
                    update.upsert(path.as_str(), blob, FileMode::Blob);
                }
                Change::Delete { path } => {
                    let path = normalize_path(&path)?;
                    if base_tree.get_path(Path::new(&path)).is_err() {
                        return Err(GitError::NotFound(format!("file {path}")));
                    }
                    update.remove(path.as_str());
                }
            }
        }
        let tree_id = update.create_updated(&self.repo, &base_tree)?;
        if parent.as_ref().is_some_and(|p| p.tree_id() == tree_id) {
            return Err(GitError::InvalidPath("changes leave the tree unchanged".into()));
        }
        let tree = self.repo.find_tree(tree_id)?;
        let sig = Signature::now(&author.name, &author.email)?;
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        // libgit2 refuses the ref update if the tip is no longer `parent`, which
        // closes the race between the check above and this write.
        let oid =
            self.repo.commit(Some(HEAD_REF), &sig, &sig, message, &tree, &parents).map_err(|e| match e.code() {
                ErrorCode::Modified | ErrorCode::Locked => GitError::Conflict(e.message().to_string()),
                _ => e.into(),
            })?;
        Ok(commit_info(&self.repo.find_commit(oid)?))
    }

    /// Most recent commits on the default branch, newest first.
    pub fn log(&self, limit: usize) -> Result<Vec<CommitInfo>> {
        let Some(head) = self.head_oid()? else {
            return Ok(Vec::new());
        };
        let mut walk = self.repo.revwalk()?;
        walk.push(head)?;
        walk.take(limit).map(|oid| Ok(commit_info(&self.repo.find_commit(oid?)?))).collect()
    }
}

fn commit_info(c: &git2::Commit) -> CommitInfo {
    let author = c.author();
    CommitInfo {
        id: c.id().to_string(),
        message: c.message().unwrap_or_default().to_string(),
        author_name: author.name().unwrap_or_default().to_string(),
        author_email: author.email().unwrap_or_default().to_string(),
        time: author.when().seconds(),
    }
}

/// Validates a repository-relative path and returns it in canonical form
/// (`a/b/c.v`, no leading slash, no `.`/`..` segments, nothing under `.git`).
pub fn normalize_path(path: &str) -> Result<String> {
    let bad = || GitError::InvalidPath(path.to_string());
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() || trimmed.len() > 512 || trimmed.ends_with('/') {
        return Err(bad());
    }
    for seg in trimmed.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.eq_ignore_ascii_case(".git") {
            return Err(bad());
        }
        if seg.chars().any(|c| c.is_control() || c == '\\') {
            return Err(bad());
        }
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn author() -> Author {
        Author { name: "Ada".into(), email: "ada@example.org".into() }
    }

    fn store() -> (tempfile::TempDir, RepoStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = RepoStore::new(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn create_list_read() {
        let (_d, store) = store();
        let first = store
            .create(
                "p1",
                vec![("README.md".into(), b"hi".to_vec()), ("src/top.v".into(), b"module top; endmodule".to_vec())],
                &author(),
            )
            .unwrap();
        let repo = store.open("p1").unwrap();
        assert_eq!(repo.head().unwrap(), Some(first.id.clone()));
        let files = repo.list_files(None).unwrap();
        assert_eq!(
            files,
            vec![FileEntry { path: "README.md".into(), size: 2 }, FileEntry { path: "src/top.v".into(), size: 21 },]
        );
        assert_eq!(repo.read_file(None, "/src/top.v").unwrap(), b"module top; endmodule");
        assert!(matches!(repo.read_file(None, "nope.v"), Err(GitError::NotFound(_))));
    }

    #[test]
    fn commit_write_delete_and_log() {
        let (_d, store) = store();
        let c1 = store.create("p", vec![("a.v".into(), b"1".to_vec())], &author()).unwrap();
        let repo = store.open("p").unwrap();
        let c2 = repo
            .commit(
                vec![
                    Change::Write { path: "rtl/b.v".into(), content: b"2".to_vec() },
                    Change::Delete { path: "a.v".into() },
                ],
                "edit",
                &author(),
                Some(&c1.id),
            )
            .unwrap();
        let paths: Vec<_> = repo.list_files(None).unwrap().into_iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["rtl/b.v"]);
        // Old revision still readable.
        assert_eq!(repo.read_file(Some(&c1.id), "a.v").unwrap(), b"1");
        let log = repo.log(10).unwrap();
        assert_eq!(log.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec![c2.id.as_str(), c1.id.as_str()]);
        assert_eq!(log[0].message, "edit");
    }

    #[test]
    fn stale_parent_conflicts() {
        let (_d, store) = store();
        let c1 = store.create("p", vec![("a.v".into(), b"1".to_vec())], &author()).unwrap();
        let repo = store.open("p").unwrap();
        repo.commit(vec![Change::Write { path: "a.v".into(), content: b"2".to_vec() }], "m", &author(), Some(&c1.id))
            .unwrap();
        let err = repo
            .commit(vec![Change::Write { path: "a.v".into(), content: b"3".to_vec() }], "m", &author(), Some(&c1.id))
            .unwrap_err();
        assert!(matches!(err, GitError::Conflict(_)), "{err:?}");
        assert_eq!(repo.read_file(None, "a.v").unwrap(), b"2");
    }

    #[test]
    fn rejects_bad_paths_and_noops() {
        let (_d, store) = store();
        store.create("p", vec![("a.v".into(), b"1".to_vec())], &author()).unwrap();
        let repo = store.open("p").unwrap();
        for bad in ["", "../x", "a/../b", ".git/config", "a//b", "dir/", "a\\b"] {
            let r = repo.commit(vec![Change::Write { path: bad.into(), content: vec![] }], "m", &author(), None);
            assert!(matches!(r, Err(GitError::InvalidPath(_))), "{bad}: {r:?}");
        }
        let same =
            repo.commit(vec![Change::Write { path: "a.v".into(), content: b"1".to_vec() }], "m", &author(), None);
        assert!(matches!(same, Err(GitError::InvalidPath(_))));
        let missing = repo.commit(vec![Change::Delete { path: "zz.v".into() }], "m", &author(), None);
        assert!(matches!(missing, Err(GitError::NotFound(_))));
    }

    #[test]
    fn open_missing_and_duplicate_create() {
        let (_d, store) = store();
        assert!(matches!(store.open("nope"), Err(GitError::NotFound(_))));
        store.create("p", vec![("a".into(), vec![1])], &author()).unwrap();
        assert!(matches!(store.create("p", vec![("a".into(), vec![1])], &author()), Err(GitError::Conflict(_))));
    }
}
