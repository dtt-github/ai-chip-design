//! What the AI costs, per person: every assistant (spec drafting) and agent
//! (proposal) call records its token usage priced in US dollars for the model
//! that served it, and a
//! person who has spent their daily limit gets a clear "come back tomorrow"
//! instead of another call. Days are UTC.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};

/// US dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prices {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// Known models, by name without the provider prefix, dots as dashes and any date suffix
/// (`anthropic/claude-opus-5.5` and `claude-opus-5-5-20260921` are both `claude-opus-5-5`).
/// Anthropic cache writes are 5-minute (1.25x input); OpenRouter lists no cache-write price for
/// Gemini, so it is charged as input.
const KNOWN: &[(&str, Prices)] = &[
    ("claude-opus-5-5", Prices { input: 4.0, output: 20.0, cache_read: 0.20, cache_write: 5.0 }),
    ("gemini-4-argon", Prices { input: 2.0, output: 10.0, cache_read: 0.10, cache_write: 2.0 }),
];

/// For a model this table doesn't know (and no `WAFER_AI_PRICES`): the dearest known prices, so a
/// daily limit errs towards stopping early rather than overspending.
const UNKNOWN: Prices = Prices { input: 4.0, output: 20.0, cache_read: 0.20, cache_write: 5.0 };

/// The table's key for a model id as the API or OpenRouter reports it.
fn model_key(model: &str) -> String {
    let name = model.rsplit('/').next().unwrap_or(model).to_lowercase().replace('.', "-");
    // A trailing date: -20260921 or @20260921.
    let parts: Vec<&str> = name.split(['-', '@']).collect();
    match parts.last() {
        Some(d) if d.len() == 8 && d.chars().all(|c| c.is_ascii_digit()) => {
            name[..name.len() - d.len() - 1].to_string()
        }
        _ => name,
    }
}

/// `WAFER_AI_PRICES`: "input,output,cache_read,cache_write" in US dollars per million tokens, for every
/// model (a model the table doesn't know, or a negotiated price).
fn configured() -> Option<Prices> {
    static P: std::sync::OnceLock<Option<Prices>> = std::sync::OnceLock::new();
    *P.get_or_init(|| std::env::var("WAFER_AI_PRICES").ok().and_then(|v| parse_prices(&v)))
}

pub fn parse_prices(v: &str) -> Option<Prices> {
    let n: Vec<f64> = v.split(',').map(|x| x.trim().parse::<f64>()).collect::<Result<_, _>>().ok()?;
    match n[..] {
        [input, output, cache_read, cache_write] if n.iter().all(|x| x.is_finite() && *x >= 0.0) => {
            Some(Prices { input, output, cache_read, cache_write })
        }
        _ => None,
    }
}

/// The prices for a model, and whether they are known (from `WAFER_AI_PRICES` or the table).
pub fn prices(model: &str) -> (Prices, bool) {
    if let Some(p) = configured() {
        return (p, true);
    }
    let key = model_key(model);
    match KNOWN.iter().find(|(k, _)| *k == key) {
        Some((_, p)) => (*p, true),
        None => (UNKNOWN, false),
    }
}

/// What `usage` cost on `model`, in US dollars.
pub fn usd(model: &str, usage: &BTreeMap<String, u64>) -> f64 {
    let (p, _) = prices(model);
    let n = |k: &str| *usage.get(k).unwrap_or(&0) as f64 / 1e6;
    n("input_tokens") * p.input
        + n("output_tokens") * p.output
        + n("cache_read_input_tokens") * p.cache_read
        + n("cache_creation_input_tokens") * p.cache_write
}

async fn spent_today(state: &AppState, user: Uuid) -> ApiResult<f64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(SUM(usd), 0)::float8 FROM ai_usage WHERE user_id = $1 AND created_at >= date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'",
    )
    .bind(user)
    .fetch_one(&state.db)
    .await?)
}

/// Refuses when the person has used up today's limit (if the server sets one).
pub async fn check(state: &AppState, user: &AuthUser) -> ApiResult<()> {
    let Some(limit) = state.agent.daily_usd else { return Ok(()) };
    let spent = spent_today(state, user.id).await?;
    if spent >= limit {
        return Err(ApiError::TooManyRequests(format!(
            "you've used today's AI budget (${spent:.2} of ${limit:.2}); it resets at midnight UTC"
        )));
    }
    Ok(())
}

/// Records a call's usage, priced for the model that served it.
pub async fn record(
    state: &AppState,
    user: Uuid,
    project: Uuid,
    kind: &str,
    model: &str,
    usage: &BTreeMap<String, u64>,
) {
    let cost = usd(model, usage);
    let res = sqlx::query(
        "INSERT INTO ai_usage (user_id, project_id, kind, model, usage, usd) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(user)
    .bind(project)
    .bind(kind)
    .bind(model)
    .bind(json!(usage))
    .bind(cost)
    .execute(&state.db)
    .await;
    if let Err(e) = res {
        tracing::error!(error = ?e, %user, "recording AI usage failed");
    }
}

/// Today's spend and the limit, for the person asking.
pub async fn mine(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    let spent = spent_today(&state, user.id).await?;
    Ok(Json(json!({ "today_usd": spent, "limit_usd": state.agent.daily_usd })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn prices_match_a_real_bill() {
        // The first live agent run: OpenRouter billed $0.183.
        let u = [
            ("input_tokens", 14),
            ("output_tokens", 6335),
            ("cache_read_input_tokens", 22142),
            ("cache_creation_input_tokens", 10319),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        assert!((super::usd("anthropic/claude-opus-5.5", &u) - 0.1828).abs() < 0.001);
        // Gemini 4 Argon: half the price per token, cache writes as input.
        let g = super::usd("google/gemini-4-argon", &u);
        assert!((g - 0.0862).abs() < 0.001, "{g}");
    }

    #[test]
    fn model_names_and_configured_prices() {
        use super::{model_key, parse_prices, prices};
        assert_eq!(model_key("anthropic/claude-opus-5.5"), "claude-opus-5-5");
        assert_eq!(model_key("claude-opus-5-5-20260921"), "claude-opus-5-5");
        assert_eq!(model_key("google/gemini-4-argon"), "gemini-4-argon");
        assert_eq!(model_key("google/gemini-4-argon@20261001"), "gemini-4-argon");
        assert!(prices("claude-opus-5-5").1);
        // Unknown models are priced as the dearest known one, and flagged.
        let (p, known) = prices("someone/new-model");
        assert!(!known);
        assert_eq!(p.output, 20.0);
        assert_eq!(parse_prices("1, 2, 0.1, 1.25").map(|p| p.cache_write), Some(1.25));
        assert_eq!(parse_prices("1,2,3"), None);
        assert_eq!(parse_prices("1,2,x,4"), None);
        assert_eq!(parse_prices("1,-2,3,4"), None);
    }
}
