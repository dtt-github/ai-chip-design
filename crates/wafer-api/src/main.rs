use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;
use wafer_api::auth::{JwksVerifier, Verifier};
use wafer_api::jobs::JobService;
use wafer_api::lsp::LspConfig;
use wafer_api::{AppState, MIGRATOR, router};
use wafer_git::RepoStore;
use wafer_jobs::{Backend, DockerBackend, DockerConfig};

fn env(key: &str) -> anyhow::Result<String> {
    std::env::var(key).with_context(|| format!("environment variable {key} is not set (see .env.example)"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let bind: SocketAddr = env("WAFER_BIND").unwrap_or_else(|_| "127.0.0.1:8090".into()).parse()?;
    let data_dir = PathBuf::from(env("WAFER_DATA_DIR").unwrap_or_else(|_| "./data".into()));
    let issuer = env("WAFER_OIDC_ISSUER")?;
    let audiences: Vec<String> =
        env("WAFER_OIDC_AUDIENCES")?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();

    let db = PgPoolOptions::new()
        .max_connections(20)
        .connect(&env("DATABASE_URL")?)
        .await
        .context("connecting to Postgres")?;
    MIGRATOR.run(&db).await.context("running migrations")?;

    let verible_ls =
        PathBuf::from(env("WAFER_VERIBLE_LS").unwrap_or_else(|_| ".tools/verible/bin/verible-verilog-ls".into()));
    let verible_ls = match verible_ls.canonicalize() {
        Ok(path) => Some(path),
        Err(_) => {
            tracing::warn!(path = %verible_ls.display(), "Verible language server not found; run scripts/fetch-verible.sh");
            None
        }
    };
    let max_lsp: usize = env("WAFER_LSP_MAX_SESSIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(32);

    let docker = DockerConfig {
        docker: env("WAFER_DOCKER").unwrap_or_else(|_| "docker".into()),
        image: env("WAFER_LIBRELANE_IMAGE").unwrap_or_else(|_| "ghcr.io/librelane/librelane:3.0.16".into()),
        pdk_volume: env("WAFER_PDK_VOLUME").unwrap_or_else(|_| "wafersmith_pdk".into()),
        cpus: env("WAFER_JOB_CPUS").ok().and_then(|v| v.parse().ok()).unwrap_or(4.0),
        memory: env("WAFER_JOB_MEMORY").unwrap_or_else(|_| "8g".into()),
        user: current_user().await,
        timeout: Some(std::time::Duration::from_secs(
            60 * env("WAFER_JOB_TIMEOUT_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(120u64),
        )),
    };
    let backend: Option<Arc<dyn Backend>> = match DockerBackend::new(docker).await {
        Ok(b) => {
            tracing::info!(toolchain = %b.toolchain(), "flow runs enabled");
            Some(Arc::new(b))
        }
        Err(e) => {
            tracing::warn!(error = %e, "flow runs disabled");
            None
        }
    };
    // The Anthropic API directly, or OpenRouter's Anthropic-compatible endpoint.
    let key = |k: &str| env(k).ok().filter(|v| !v.trim().is_empty());
    let provider = match (key("ANTHROPIC_API_KEY"), key("OPENROUTER_API_KEY")) {
        (Some(k), _) => Some((k, "https://api.anthropic.com", wafer_api::claude::DEFAULT_MODEL, false)),
        (None, Some(k)) => Some((k, wafer_api::claude::OPENROUTER_BASE_URL, wafer_api::claude::OPENROUTER_MODEL, true)),
        (None, None) => None,
    };
    let claude = match provider {
        Some((api_key, base_url, model, openrouter)) => {
            let cfg = wafer_api::claude::ClaudeConfig {
                api_key,
                base_url: env("WAFER_ANTHROPIC_BASE_URL").unwrap_or_else(|_| base_url.into()),
                model: env("WAFER_AGENT_MODEL").unwrap_or_else(|_| model.into()),
                effort: env("WAFER_AGENT_EFFORT").unwrap_or_else(|_| "high".into()),
                max_tokens: 16_000,
                bearer: openrouter,
                fallbacks: !openrouter,
            };
            tracing::info!(model = %cfg.model, effort = %cfg.effort, base = %cfg.base_url, "agent enabled");
            if let Ok(v) = env("WAFER_AI_PRICES")
                && wafer_api::spend::parse_prices(&v).is_none()
            {
                anyhow::bail!(
                    "WAFER_AI_PRICES must be four numbers: input,output,cache_read,cache_write (US$ per million tokens)"
                );
            }
            let (p, known) = wafer_api::spend::prices(&cfg.model);
            if known {
                tracing::info!(model = %cfg.model, input = p.input, output = p.output, "AI prices (US$ per million tokens)");
            } else {
                tracing::warn!(model = %cfg.model, "no prices known for this model: costs are counted at the dearest known prices; set WAFER_AI_PRICES");
            }
            Some(wafer_api::claude::Claude::new(cfg))
        }
        None => {
            tracing::warn!("agent disabled: set ANTHROPIC_API_KEY or OPENROUTER_API_KEY to enable it");
            None
        }
    };
    let workers: usize = env("WAFER_JOB_WORKERS").ok().and_then(|v| v.parse().ok()).unwrap_or(2);

    // Per-person daily AI budget in US dollars; 0 turns the limit off.
    let daily_usd = match std::env::var("WAFER_AI_DAILY_USD") {
        Ok(v) => v.trim().parse::<f64>().map_err(|_| anyhow::anyhow!("WAFER_AI_DAILY_USD must be a number"))?,
        Err(_) => 3.0,
    };
    let daily_usd = (daily_usd > 0.0).then_some(daily_usd);
    tracing::info!(?daily_usd, "AI budget per person per day");
    let state = AppState {
        db,
        repos: Arc::new(RepoStore::new(data_dir.join("repos"))?),
        verifier: Arc::new(Verifier::Jwks(
            JwksVerifier::new(issuer, audiences).with_internal_url(env("WAFER_OIDC_INTERNAL_URL").ok()),
        )),
        lsp: Arc::new(LspConfig::new(verible_ls, max_lsp)),
        jobs: Arc::new(JobService::new(backend, data_dir.join("jobs")).with_retention(
            env("WAFER_JOB_RETENTION_DAYS").ok().and_then(|v| v.parse().ok()).filter(|d: &u32| *d > 0),
        )),
        collab: Arc::new(wafer_api::collab::Collab::new(data_dir.join("collab"))),
        agent: Arc::new(wafer_api::agent::AgentService::new(claude, 2).with_daily_limit(daily_usd)),
    };
    wafer_api::agent::recover(&state).await?;
    wafer_api::jobs::start_workers(state.clone(), workers).await?;

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "wafersmith-api listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

/// `uid:gid` of this process, so job containers write files we can read and delete.
async fn current_user() -> Option<String> {
    let id = |flag: &'static str| async move {
        let out = tokio::process::Command::new("id").arg(flag).output().await.ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    Some(format!("{}:{}", id("-u").await?, id("-g").await?))
}
