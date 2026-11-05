use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use sc_git::{Author, Change, CommitInfo, FileEntry};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::template;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Editor,
    Owner,
}

impl Role {
    fn parse(s: &str) -> Option<Role> {
        match s {
            "viewer" => Some(Role::Viewer),
            "editor" => Some(Role::Editor),
            "owner" => Some(Role::Owner),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Editor => "editor",
            Role::Owner => "owner",
        }
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Project {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub pdk: String,
    pub visibility: String,
    /// SPDX identifier the project's own files are released under.
    pub licence: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ProjectView {
    #[serde(flatten)]
    pub project: Project,
    /// The caller's role; `null` when they can read a public project without being a member.
    pub role: Option<Role>,
    pub head: Option<String>,
}

/// Loads a project and checks the caller has at least `needed` on it.
/// Public projects are readable by every signed-in user.
pub(crate) async fn authorize(
    state: &AppState,
    user: &AuthUser,
    id: Uuid,
    needed: Role,
) -> ApiResult<(Project, Option<Role>)> {
    let project: Option<Project> =
        sqlx::query_as("SELECT * FROM projects WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
    let Some(project) = project else {
        return Err(ApiError::NotFound("project not found".into()));
    };
    let role: Option<String> =
        sqlx::query_scalar("SELECT role FROM project_members WHERE project_id = $1 AND user_id = $2")
            .bind(id)
            .bind(user.id)
            .fetch_optional(&state.db)
            .await?;
    let role = role.as_deref().and_then(Role::parse);
    let allowed = match role {
        Some(r) => r >= needed,
        None => needed == Role::Viewer && project.visibility == "public",
    };
    if !allowed {
        // Don't reveal that a private project exists.
        return Err(if role.is_none() && project.visibility != "public" {
            ApiError::NotFound("project not found".into())
        } else {
            ApiError::Forbidden
        });
    }
    Ok((project, role))
}

/// Runs a blocking Git operation on the project's repository.
async fn with_repo<T, F>(state: &AppState, id: Uuid, f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(sc_git::ProjectRepo) -> sc_git::Result<T> + Send + 'static,
{
    let repos = state.repos.clone();
    Ok(tokio::task::spawn_blocking(move || f(repos.open(&id.to_string())?)).await??)
}

pub async fn list(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Vec<serde_json::Value>>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        #[sqlx(flatten)]
        project: Project,
        role: String,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT p.*, m.role FROM projects p
         JOIN project_members m ON m.project_id = p.id
         WHERE m.user_id = $1
         ORDER BY p.updated_at DESC",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                let mut v = serde_json::to_value(&r.project).expect("serializable");
                v["role"] = r.role.into();
                v
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub struct CreateProject {
    pub name: String,
    pub slug: Option<String>,
    #[serde(default)]
    pub description: String,
    pub visibility: Option<String>,
    /// SPDX identifier; defaults to Apache-2.0.
    pub licence: Option<String>,
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    slug.chars().take(63).collect::<String>().trim_end_matches('-').to_string()
}

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(req): Json<CreateProject>,
) -> ApiResult<(StatusCode, Json<ProjectView>)> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    let slug = req.slug.map(|s| s.trim().to_string()).unwrap_or_else(|| slugify(&name));
    if slug.is_empty() {
        return Err(ApiError::BadRequest("could not derive a slug from the name; pass one explicitly".into()));
    }
    let visibility = req.visibility.unwrap_or_else(|| "private".into());
    let licence = req.licence.map(|l| l.trim().to_string()).unwrap_or_else(|| "Apache-2.0".into());
    if !crate::licence::is_known(&licence) {
        return Err(ApiError::BadRequest(format!(
            "unsupported licence '{licence}'; use one of {}",
            crate::licence::project_licences().join(", ")
        )));
    }
    let id = Uuid::new_v4();

    let mut tx = state.db.begin().await?;
    let project: Project = sqlx::query_as(
        "INSERT INTO projects (id, owner_id, slug, name, description, visibility, licence)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *",
    )
    .bind(id)
    .bind(user.id)
    .bind(&slug)
    .bind(&name)
    .bind(req.description.trim())
    .bind(&visibility)
    .bind(&licence)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match ApiError::from(e) {
        ApiError::Conflict(_) => ApiError::Conflict(format!("you already have a project with slug '{slug}'")),
        other => other,
    })?;
    sqlx::query("INSERT INTO project_members (project_id, user_id, role) VALUES ($1, $2, 'owner')")
        .bind(id)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;

    let repos = state.repos.clone();
    let files = template::starter_files(&name, &slug, &licence);
    let author = Author { name: user.display_name.clone(), email: user.email.clone() };
    let initial = tokio::task::spawn_blocking(move || repos.create(&id.to_string(), files, &author)).await??;

    if let Err(e) = tx.commit().await {
        let path = state.repos.path(&id.to_string());
        let _ = tokio::fs::remove_dir_all(path).await;
        return Err(e.into());
    }
    tracing::info!(%id, %slug, owner = %user.id, "project created");
    Ok((StatusCode::CREATED, Json(ProjectView { project, role: Some(Role::Owner), head: Some(initial.id) })))
}

pub async fn get(State(state): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Json<ProjectView>> {
    let (project, role) = authorize(&state, &user, id, Role::Viewer).await?;
    let head = with_repo(&state, id, |r| r.head()).await?;
    Ok(Json(ProjectView { project, role, head }))
}

#[derive(Debug, Deserialize)]
pub struct RevQuery {
    pub rev: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TreeView {
    pub commit: Option<String>,
    pub files: Vec<FileEntry>,
}

pub async fn tree(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<RevQuery>,
) -> ApiResult<Json<TreeView>> {
    authorize(&state, &user, id, Role::Viewer).await?;
    let view = with_repo(&state, id, move |r| {
        let commit = match &q.rev {
            Some(rev) => Some(rev.clone()),
            None => r.head()?,
        };
        let files = if commit.is_some() { r.list_files(q.rev.as_deref())? } else { Vec::new() };
        Ok(TreeView { commit, files })
    })
    .await?;
    Ok(Json(view))
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub commit: Option<String>,
    /// Text files (UTF-8, up to 1 MiB) by path; binary and larger files are only in `tree`.
    pub files: std::collections::HashMap<String, String>,
}

/// Every text file at a revision in one response (the base the working copy is compared to).
pub async fn snapshot(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<RevQuery>,
) -> ApiResult<Json<Snapshot>> {
    authorize(&state, &user, id, Role::Viewer).await?;
    let commit = match q.rev {
        Some(rev) => {
            let repos = state.repos.clone();
            Some(tokio::task::spawn_blocking(move || repos.open(&id.to_string())?.resolve(&rev)).await??)
        }
        None => crate::workspace::head(state.repos.clone(), id).await?,
    };
    let files = match &commit {
        Some(c) => crate::workspace::text_files(state.repos.clone(), id, c.clone(), 1024 * 1024).await?,
        None => Default::default(),
    };
    Ok(Json(Snapshot { commit, files }))
}

#[derive(Debug, Serialize)]
pub struct FileView {
    pub path: String,
    /// Commit the content was read from.
    pub commit: String,
    /// `false` when the file is not valid UTF-8; `content` is then empty.
    pub text: bool,
    pub content: String,
}

pub async fn read_file(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, path)): Path<(Uuid, String)>,
    Query(q): Query<RevQuery>,
) -> ApiResult<Json<FileView>> {
    authorize(&state, &user, id, Role::Viewer).await?;
    let path = sc_git::normalize_path(&path)?;
    let view = with_repo(&state, id, move |r| {
        let commit = match q.rev {
            Some(rev) => rev,
            None => r.head()?.ok_or_else(|| sc_git::GitError::NotFound("empty repository".into()))?,
        };
        let bytes = r.read_file(Some(&commit), &path)?;
        let (text, content) = match String::from_utf8(bytes) {
            Ok(s) => (true, s),
            Err(_) => (false, String::new()),
        };
        Ok(FileView { path, commit, text, content })
    })
    .await?;
    Ok(Json(view))
}

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    pub limit: Option<usize>,
}

pub async fn log(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<LogQuery>,
) -> ApiResult<Json<Vec<CommitInfo>>> {
    authorize(&state, &user, id, Role::Viewer).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    Ok(Json(with_repo(&state, id, move |r| r.log(limit)).await?))
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum ChangeReq {
    Write { path: String, content: String },
    Delete { path: String },
}

#[derive(Debug, Deserialize)]
pub struct CommitReq {
    pub message: String,
    /// Commit the edits are based on. Omit to commit onto whatever the tip is.
    pub parent: Option<String>,
    pub changes: Vec<ChangeReq>,
}

pub async fn commit(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<CommitReq>,
) -> ApiResult<(StatusCode, Json<CommitInfo>)> {
    authorize(&state, &user, id, Role::Editor).await?;
    let message = req.message.trim().to_string();
    if message.is_empty() {
        return Err(ApiError::BadRequest("commit message is required".into()));
    }
    if req.changes.len() > 1000 {
        return Err(ApiError::BadRequest("too many changes in one commit".into()));
    }
    let changes = req
        .changes
        .into_iter()
        .map(|c| match c {
            ChangeReq::Write { path, content } => Change::Write { path, content: content.into_bytes() },
            ChangeReq::Delete { path } => Change::Delete { path },
        })
        .collect();
    let author = Author { name: user.display_name.clone(), email: user.email.clone() };
    let parent = req.parent;
    let info = with_repo(&state, id, move |r| r.commit(changes, &message, &author, parent.as_deref())).await?;
    sqlx::query("UPDATE projects SET updated_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
    // Everyone with the project open moves to the new head.
    state.collab.committed(&state, id, &info.id).await;
    Ok((StatusCode::CREATED, Json(info)))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Member {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
}

pub async fn members(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Member>>> {
    authorize(&state, &user, id, Role::Viewer).await?;
    let rows = sqlx::query_as(
        "SELECT u.id AS user_id, u.email, u.display_name, m.role
         FROM project_members m JOIN users u ON u.id = m.user_id
         WHERE m.project_id = $1 ORDER BY m.added_at",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
pub struct AddMember {
    pub email: String,
    pub role: Role,
}

/// Adds or updates a member. The user must have signed in at least once.
pub async fn add_member(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<AddMember>,
) -> ApiResult<Json<Member>> {
    let (project, _) = authorize(&state, &user, id, Role::Owner).await?;
    if req.role == Role::Owner {
        return Err(ApiError::BadRequest("a project has exactly one owner".into()));
    }
    let target: Option<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, email, display_name FROM users WHERE lower(email) = lower($1)")
            .bind(req.email.trim())
            .fetch_optional(&state.db)
            .await?;
    let Some((target_id, email, display_name)) = target else {
        return Err(ApiError::NotFound("no user with that email has signed in yet".into()));
    };
    if target_id == project.owner_id {
        return Err(ApiError::BadRequest("cannot change the owner's role".into()));
    }
    sqlx::query(
        "INSERT INTO project_members (project_id, user_id, role) VALUES ($1, $2, $3)
         ON CONFLICT (project_id, user_id) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(id)
    .bind(target_id)
    .bind(req.role.as_str())
    .execute(&state.db)
    .await?;
    Ok(Json(Member { user_id: target_id, email, display_name, role: req.role.as_str().into() }))
}

/// Takes a person out of a project (owner only; the owner can't be removed).
pub async fn remove_member(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, member)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let (project, _) = authorize(&state, &user, id, Role::Owner).await?;
    if member == project.owner_id {
        return Err(ApiError::BadRequest("the owner can't be removed".into()));
    }
    let n = sqlx::query("DELETE FROM project_members WHERE project_id = $1 AND user_id = $2")
        .bind(id)
        .bind(member)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::NotFound("not a member of this project".into()));
    }
    // Their open co-editing sessions are closed so the removal takes effect now.
    state.collab.disconnect(id, member).await;
    tracing::info!(project = %id, %member, by = %user.id, "member removed");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::slugify;

    #[test]
    fn slugs() {
        assert_eq!(slugify("  My RISC-V Core!  "), "my-risc-v-core");
        assert_eq!(slugify("___"), "");
        assert_eq!(slugify(&"x".repeat(100)).len(), 63);
    }
}
