//! What the AI costs, per person: every assistant (spec drafting) and agent
//! (proposal) call records its token usage priced in US dollars, and a
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

/// US dollars per million tokens: Claude Opus 5.5 (cache writes are 5-minute, 1.25x input).
const INPUT: f64 = 4.0;
const OUTPUT: f64 = 20.0;
const CACHE_READ: f64 = 0.20;
const CACHE_WRITE: f64 = 5.0;

pub fn usd(usage: &BTreeMap<String, u64>) -> f64 {
    let n = |k: &str| *usage.get(k).unwrap_or(&0) as f64 / 1e6;
    n("input_tokens") * INPUT
        + n("output_tokens") * OUTPUT
        + n("cache_read_input_tokens") * CACHE_READ
        + n("cache_creation_input_tokens") * CACHE_WRITE
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

pub async fn record(state: &AppState, user: Uuid, project: Uuid, kind: &str, usage: &BTreeMap<String, u64>) {
    let cost = usd(usage);
    let res = sqlx::query("INSERT INTO ai_usage (user_id, project_id, kind, usage, usd) VALUES ($1, $2, $3, $4, $5)")
        .bind(user)
        .bind(project)
        .bind(kind)
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
        assert!((super::usd(&u) - 0.1828).abs() < 0.001, "{}", super::usd(&u));
    }
}
