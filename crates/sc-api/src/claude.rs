//! Minimal Claude Messages API client over raw HTTP (there is no official Rust SDK).
//!
//! Requests use Claude Opus 5.5 with adaptive thinking (summarised, for the
//! evidence log), an explicit effort, automatic prompt caching, and the
//! server-side refusal fallback (`fallbacks: "default"`).

use std::time::Duration;

use serde_json::{Value, json};

pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
const API_VERSION: &str = "2023-06-01";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MAX_ATTEMPTS: u32 = 4;

#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    pub api_key: String,
    /// `https://api.anthropic.com` unless overridden (tests point it at a mock).
    pub base_url: String,
    pub model: String,
    /// `low` | `medium` | `high` | `xhigh` | `max` (Opus 5.5 defaults to `medium`; we set it).
    pub effort: String,
    pub max_tokens: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum ClaudeError {
    #[error("Claude API error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("could not reach the Claude API: {0}")]
    Transport(String),
}

pub struct Claude {
    pub cfg: ClaudeConfig,
    http: reqwest::Client,
}

impl Claude {
    pub fn new(cfg: ClaudeConfig) -> Self {
        let http = reqwest::Client::builder()
            // Non-streaming requests with large outputs and thinking can take minutes.
            .timeout(Duration::from_secs(600))
            .build()
            .expect("http client");
        Claude { cfg, http }
    }

    pub fn request_body(&self, system: &str, tools: &Value, messages: &[Value]) -> Value {
        json!({
            "model": self.cfg.model,
            "max_tokens": self.cfg.max_tokens,
            "system": system,
            "tools": tools,
            "messages": messages,
            "thinking": { "type": "adaptive", "display": "summarized" },
            "output_config": { "effort": self.cfg.effort },
            // Cache the growing conversation prefix between turns.
            "cache_control": { "type": "ephemeral" },
            "fallbacks": "default",
        })
    }

    /// One Messages API call; retries rate limits, overload and server errors.
    pub async fn create(&self, system: &str, tools: &Value, messages: &[Value]) -> Result<Value, ClaudeError> {
        let body = self.request_body(system, tools, messages);
        let url = format!("{}/v1/messages", self.cfg.base_url.trim_end_matches('/'));
        let mut attempt = 0;
        loop {
            attempt += 1;
            let sent = self
                .http
                .post(&url)
                .header("x-api-key", &self.cfg.api_key)
                .header("anthropic-version", API_VERSION)
                .header("anthropic-beta", FALLBACK_BETA)
                .json(&body)
                .send()
                .await;
            let res = match sent {
                Ok(r) => r,
                Err(e) if attempt < MAX_ATTEMPTS => {
                    tracing::warn!(error = %e, attempt, "Claude request failed; retrying");
                    tokio::time::sleep(backoff(attempt, None)).await;
                    continue;
                }
                Err(e) => return Err(ClaudeError::Transport(e.to_string())),
            };
            let status = res.status().as_u16();
            let retry_after =
                res.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
            let text = res.text().await.map_err(|e| ClaudeError::Transport(e.to_string()))?;
            if (200..300).contains(&status) {
                return serde_json::from_str(&text).map_err(|e| ClaudeError::Transport(format!("bad JSON: {e}")));
            }
            let retryable = matches!(status, 408 | 409 | 429 | 500..=599);
            if retryable && attempt < MAX_ATTEMPTS {
                tracing::warn!(status, attempt, "Claude API busy; retrying");
                tokio::time::sleep(backoff(attempt, retry_after)).await;
                continue;
            }
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                .unwrap_or(text);
            return Err(ClaudeError::Api { status, message });
        }
    }
}

fn backoff(attempt: u32, retry_after: Option<u64>) -> Duration {
    match retry_after {
        Some(s) => Duration::from_secs(s.min(60)),
        None => Duration::from_millis(500 * 2u64.pow(attempt.min(6))),
    }
}
