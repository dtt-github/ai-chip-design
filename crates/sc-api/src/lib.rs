//! SiliconCommons HTTP API.

pub mod auth;
pub mod error;
pub mod lsp;
pub mod projects;
mod template;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use axum::{Json, Router};
use sc_git::RepoStore;
use serde_json::json;
use sqlx::PgPool;
use tower_http::trace::TraceLayer;

use crate::auth::{AuthUser, Verifier};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub repos: Arc<RepoStore>,
    pub verifier: Arc<Verifier>,
    pub lsp: Arc<lsp::LspConfig>,
}

pub fn router(state: AppState) -> Router {
    use projects as p;
    let api = Router::new()
        .route("/me", get(me))
        .route("/projects", get(p::list).post(p::create))
        .route("/projects/{id}", get(p::get))
        .route("/projects/{id}/tree", get(p::tree))
        .route("/projects/{id}/files/{*path}", get(p::read_file))
        .route("/projects/{id}/commits", get(p::log).post(p::commit))
        .route("/projects/{id}/members", get(p::members).post(p::add_member))
        .route("/projects/{id}/lsp", get(lsp::handler));

    Router::new()
        .route("/healthz", get(|| async { Json(json!({ "status": "ok" })) }))
        .nest("/api", api)
        // Commits carry whole files; sc_git enforces the per-file limit.
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn me(user: AuthUser) -> Json<AuthUser> {
    Json(user)
}
