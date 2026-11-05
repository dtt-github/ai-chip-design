//! SiliconCommons HTTP API.

pub mod agent;
pub mod auth;
pub mod blocks;
pub mod chip;
pub mod claude;
pub mod collab;
pub mod design;
pub mod error;
pub mod gate;
pub mod gls;
pub mod ip;
pub mod jobs;
pub mod licence;
pub mod lsp;
pub mod projects;
pub mod spec;
pub mod spend;
mod template;
pub mod workspace;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
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
    pub jobs: Arc<jobs::JobService>,
    pub collab: Arc<collab::Collab>,
    pub agent: Arc<agent::AgentService>,
}

pub fn router(state: AppState) -> Router {
    use projects as p;
    let api = Router::new()
        .route("/me", get(me))
        .route("/me/ai-usage", get(spend::mine))
        .route("/projects", get(p::list).post(p::create))
        .route("/projects/{id}", get(p::get))
        .route("/projects/{id}/tree", get(p::tree))
        .route("/projects/{id}/snapshot", get(p::snapshot))
        .route("/projects/{id}/files/{*path}", get(p::read_file))
        .route("/projects/{id}/commits", get(p::log).post(p::commit))
        .route("/projects/{id}/members", get(p::members).post(p::add_member))
        .route("/projects/{id}/members/{user}", axum::routing::delete(p::remove_member))
        .route("/projects/{id}/lsp", get(lsp::handler))
        .route("/projects/{id}/collab", get(collab::handler))
        .route("/projects/{id}/jobs", get(jobs::list).post(jobs::create))
        .route("/projects/{id}/agent", get(agent::list).post(agent::create))
        .route("/projects/{id}/blocks", get(chip::get).put(chip::save))
        .route("/projects/{id}/blocks/check", post(chip::check))
        .route("/projects/{id}/blocks/review", post(chip::review))
        .route("/projects/{id}/spec", get(design::get).put(design::save))
        .route("/projects/{id}/spec/run", post(design::run))
        .route("/projects/{id}/spec/draft", post(design::draft))
        .route("/ip", get(ip::list))
        .route("/projects/{id}/ip", post(ip::publish))
        .route("/projects/{id}/ip/imports", get(ip::imports))
        .route("/projects/{id}/ip/check", get(ip::check))
        .route("/projects/{id}/ip/import", post(ip::import))
        .route("/agent/{id}", get(agent::get))
        .route("/agent/{id}/merge", post(agent::merge))
        .route("/agent/{id}/discard", post(agent::discard))
        .route("/agent/{id}/regate", post(agent::regate))
        .route("/jobs/{id}", get(jobs::get))
        .route("/jobs/{id}/log", get(jobs::log))
        .route("/jobs/{id}/cancel", post(jobs::cancel))
        .route("/jobs/{id}/artifacts", get(jobs::artifacts))
        .route("/jobs/{id}/drc", get(jobs::drc))
        .route("/jobs/{id}/report", get(jobs::report))
        .route("/jobs/{id}/artifacts/{*path}", get(jobs::artifact));

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
