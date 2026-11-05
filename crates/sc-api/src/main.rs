use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use sc_api::auth::{JwksVerifier, Verifier};
use sc_api::lsp::LspConfig;
use sc_api::{AppState, MIGRATOR, router};
use sc_git::RepoStore;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

fn env(key: &str) -> anyhow::Result<String> {
    std::env::var(key).with_context(|| format!("environment variable {key} is not set (see .env.example)"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let bind: SocketAddr = env("SC_BIND").unwrap_or_else(|_| "127.0.0.1:8090".into()).parse()?;
    let data_dir = PathBuf::from(env("SC_DATA_DIR").unwrap_or_else(|_| "./data".into()));
    let issuer = env("SC_OIDC_ISSUER")?;
    let audiences: Vec<String> =
        env("SC_OIDC_AUDIENCES")?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();

    let db = PgPoolOptions::new()
        .max_connections(20)
        .connect(&env("DATABASE_URL")?)
        .await
        .context("connecting to Postgres")?;
    MIGRATOR.run(&db).await.context("running migrations")?;

    let verible_ls =
        PathBuf::from(env("SC_VERIBLE_LS").unwrap_or_else(|_| ".tools/verible/bin/verible-verilog-ls".into()));
    let verible_ls = match verible_ls.canonicalize() {
        Ok(path) => Some(path),
        Err(_) => {
            tracing::warn!(path = %verible_ls.display(), "Verible language server not found; run scripts/fetch-verible.sh");
            None
        }
    };
    let max_lsp: usize = env("SC_LSP_MAX_SESSIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(32);

    let state = AppState {
        db,
        repos: Arc::new(RepoStore::new(data_dir.join("repos"))?),
        verifier: Arc::new(Verifier::Jwks(JwksVerifier::new(issuer, audiences))),
        lsp: Arc::new(LspConfig::new(verible_ls, max_lsp)),
    };

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "siliconcommons-api listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
