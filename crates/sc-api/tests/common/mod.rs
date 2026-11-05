//! Shared test setup: app state with a shared-secret token verifier.
#![allow(dead_code)] // each test binary uses a different subset

use std::sync::Arc;

use jsonwebtoken::{EncodingKey, Header, encode};
use sc_api::AppState;
use sc_api::auth::Verifier;
use sc_api::jobs::JobService;
use sc_api::lsp::LspConfig;
use sc_git::RepoStore;
use serde_json::json;
use sqlx::PgPool;

const SECRET: &[u8] = b"test-secret";
const ISSUER: &str = "https://issuer.test/";

/// App state backed by `db` and a temporary data directory (kept alive by the returned guard).
pub fn state(db: PgPool, lsp: LspConfig) -> (AppState, tempfile::TempDir) {
    state_with_backend(db, lsp, None)
}

pub fn state_with_backend(
    db: PgPool,
    lsp: LspConfig,
    backend: Option<Arc<dyn sc_jobs::Backend>>,
) -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState {
        db,
        repos: Arc::new(RepoStore::new(dir.path().join("repos")).unwrap()),
        verifier: Arc::new(Verifier::SharedSecret {
            secret: SECRET.to_vec(),
            issuer: ISSUER.into(),
            audiences: vec!["web".into()],
        }),
        lsp: Arc::new(lsp),
        jobs: Arc::new(JobService::new(backend, dir.path().join("jobs"))),
        collab: Arc::new(sc_api::collab::Collab::new(dir.path().join("collab"))),
    };
    (state, dir)
}

pub fn token(sub: &str, email: &str) -> String {
    let claims = json!({
        "iss": ISSUER, "aud": "web", "sub": sub, "email": email,
        "given_name": sub, "exp": chrono::Utc::now().timestamp() + 600,
    });
    encode(&Header::default(), &claims, &EncodingKey::from_secret(SECRET)).unwrap()
}
