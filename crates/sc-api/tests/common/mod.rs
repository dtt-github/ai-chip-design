//! Shared test setup: app state with a shared-secret token verifier.

use std::sync::Arc;

use jsonwebtoken::{EncodingKey, Header, encode};
use sc_api::AppState;
use sc_api::auth::Verifier;
use sc_api::lsp::LspConfig;
use sc_git::RepoStore;
use serde_json::json;
use sqlx::PgPool;

const SECRET: &[u8] = b"test-secret";
const ISSUER: &str = "https://issuer.test/";

/// App state backed by `db` and a temporary repository directory (kept alive by the returned guard).
pub fn state(db: PgPool, lsp: LspConfig) -> (AppState, tempfile::TempDir) {
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
