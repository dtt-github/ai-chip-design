//! Bearer-token authentication against the OIDC provider (Rauthy).
//!
//! Access tokens are JWTs; we verify them locally with the provider's JWKS and
//! map `(iss, sub)` to a row in `users`, creating it on first sight.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;

/// Claims we read from an access token.
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    /// Absent in userinfo responses; the JWT validation requires it on tokens.
    #[serde(default)]
    pub iss: String,
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub given_name: Option<String>,
    #[serde(default)]
    pub family_name: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
}

impl Claims {
    fn has_name(&self) -> bool {
        [&self.name, &self.given_name, &self.family_name, &self.preferred_username]
            .iter()
            .any(|n| n.as_deref().is_some_and(|n| !n.is_empty()))
    }

    fn display_name(&self) -> String {
        if let Some(n) = self.name.as_deref().filter(|n| !n.is_empty()) {
            return n.to_string();
        }
        let full = [self.given_name.as_deref(), self.family_name.as_deref()]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if !full.is_empty() {
            return full;
        }
        self.preferred_username
            .clone()
            .or_else(|| self.email.as_deref().and_then(|e| e.split('@').next()).map(str::to_string))
            .unwrap_or_else(|| self.sub.clone())
    }
}

pub enum Verifier {
    Jwks(JwksVerifier),
    /// HS256 with a shared secret. Only for tests: never configured from the environment.
    SharedSecret {
        secret: Vec<u8>,
        issuer: String,
        audiences: Vec<String>,
    },
}

impl Verifier {
    pub async fn verify(&self, token: &str) -> Result<Claims, ApiError> {
        match self {
            Verifier::Jwks(v) => v.verify(token).await,
            Verifier::SharedSecret { secret, issuer, audiences } => {
                let mut validation = Validation::new(Algorithm::HS256);
                validation.set_issuer(&[issuer]);
                validation.set_audience(audiences);
                decode::<Claims>(token, &DecodingKey::from_secret(secret), &validation)
                    .map(|d| d.claims)
                    .map_err(|e| ApiError::Unauthorized(format!("invalid token: {e}")))
            }
        }
    }

    /// Profile claims from the provider's userinfo endpoint. Access tokens
    /// from some providers (Rauthy included) carry no name claims.
    async fn userinfo(&self, token: &str) -> Option<Claims> {
        match self {
            Verifier::Jwks(v) => v.userinfo(token).await,
            Verifier::SharedSecret { .. } => None,
        }
    }
}

pub struct JwksVerifier {
    issuer: String,
    /// Where the server itself reaches the provider, when that differs from the public issuer
    /// (e.g. `http://rauthy:8080/auth/v1` inside Compose); discovered URLs are rewritten to it.
    internal: Option<String>,
    audiences: Vec<String>,
    http: reqwest::Client,
    keys: RwLock<KeyCache>,
}

struct KeyCache {
    keys: HashMap<String, (Algorithm, DecodingKey)>,
    userinfo_endpoint: Option<String>,
    fetched_at: Option<Instant>,
}

/// Don't hammer the provider when tokens with unknown `kid`s arrive.
const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct Discovery {
    jwks_uri: String,
    userinfo_endpoint: Option<String>,
}

impl JwksVerifier {
    pub fn new(issuer: String, audiences: Vec<String>) -> Self {
        Self {
            issuer,
            internal: None,
            audiences,
            http: reqwest::Client::builder().timeout(Duration::from_secs(10)).build().expect("http client"),
            keys: RwLock::new(KeyCache { keys: HashMap::new(), userinfo_endpoint: None, fetched_at: None }),
        }
    }

    pub fn with_internal_url(mut self, url: Option<String>) -> Self {
        self.internal = url.map(|u| u.trim_end_matches('/').to_string());
        self
    }

    /// `url` under the public issuer, as the server reaches it.
    fn local(&self, url: &str) -> String {
        match (&self.internal, url.strip_prefix(self.issuer.trim_end_matches('/'))) {
            (Some(base), Some(rest)) => format!("{base}{rest}"),
            _ => url.to_string(),
        }
    }

    async fn verify(&self, token: &str) -> Result<Claims, ApiError> {
        let header = decode_header(token).map_err(|e| ApiError::Unauthorized(format!("malformed token: {e}")))?;
        let kid = header.kid.ok_or_else(|| ApiError::Unauthorized("token has no kid".into()))?;
        let (alg, key) = self.key_for(&kid).await?;
        let mut validation = Validation::new(alg);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&self.audiences);
        decode::<Claims>(token, &key, &validation)
            .map(|d| d.claims)
            .map_err(|e| ApiError::Unauthorized(format!("invalid token: {e}")))
    }

    async fn key_for(&self, kid: &str) -> Result<(Algorithm, DecodingKey), ApiError> {
        if let Some(k) = self.keys.read().await.keys.get(kid) {
            return Ok(k.clone());
        }
        let mut cache = self.keys.write().await;
        if let Some(k) = cache.keys.get(kid) {
            return Ok(k.clone());
        }
        if cache.fetched_at.is_some_and(|t| t.elapsed() < MIN_REFRESH_INTERVAL) {
            return Err(ApiError::Unauthorized("unknown signing key".into()));
        }
        (cache.keys, cache.userinfo_endpoint) = self.fetch_keys().await?;
        cache.fetched_at = Some(Instant::now());
        cache.keys.get(kid).cloned().ok_or_else(|| ApiError::Unauthorized("unknown signing key".into()))
    }

    async fn userinfo(&self, token: &str) -> Option<Claims> {
        let url = self.local(&self.keys.read().await.userinfo_endpoint.clone()?);
        let res = self.http.get(url).bearer_auth(token).send().await.ok()?.error_for_status().ok()?;
        res.json().await.inspect_err(|e| tracing::warn!(error = %e, "bad userinfo response")).ok()
    }

    #[allow(clippy::type_complexity)]
    async fn fetch_keys(&self) -> Result<(HashMap<String, (Algorithm, DecodingKey)>, Option<String>), ApiError> {
        let discovery_url =
            self.local(&format!("{}/.well-known/openid-configuration", self.issuer.trim_end_matches('/')));
        let fetch = async {
            let d: Discovery = self.http.get(&discovery_url).send().await?.error_for_status()?.json().await?;
            let set: JwkSet = self.http.get(self.local(&d.jwks_uri)).send().await?.error_for_status()?.json().await?;
            anyhow::Ok((set, d.userinfo_endpoint))
        };
        let (set, userinfo_endpoint) =
            fetch.await.map_err(|e| ApiError::Internal(e.context("fetching JWKS from OIDC provider")))?;
        let mut keys = HashMap::new();
        for jwk in &set.keys {
            let (Some(kid), Some(alg)) = (jwk.common.key_id.clone(), jwk.common.key_algorithm) else {
                continue;
            };
            let Ok(alg) = alg.to_string().parse::<Algorithm>() else { continue };
            if let Ok(key) = DecodingKey::from_jwk(jwk) {
                keys.insert(kid, (alg, key));
            }
        }
        tracing::info!(count = keys.len(), "loaded OIDC signing keys");
        Ok((keys, userinfo_endpoint))
    }
}

/// The authenticated caller. Add it as a handler argument to require login.
#[derive(Debug, Clone, Serialize)]
pub struct AuthUser {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| ApiError::Unauthorized("missing bearer token".into()))?;
        authenticate(state, token).await
    }
}

/// Verifies an access token and returns the matching user, creating it on first sight.
pub async fn authenticate(state: &AppState, token: &str) -> Result<AuthUser, ApiError> {
    let mut claims = state.verifier.verify(token).await?;
    let email = claims.email.clone().unwrap_or_default();

    // Without name claims, ask userinfo for new users and about once a day
    // (to pick up renames); otherwise keep the stored name.
    let mut has_name = claims.has_name();
    if !has_name {
        let fresh: Option<bool> = sqlx::query_scalar(
            "SELECT last_seen_at > now() - interval '1 day' FROM users
                 WHERE oidc_issuer = $1 AND oidc_subject = $2",
        )
        .bind(&claims.iss)
        .bind(&claims.sub)
        .fetch_optional(&state.db)
        .await?;
        if fresh != Some(true)
            && let Some(info) = state.verifier.userinfo(token).await.filter(|i| i.sub == claims.sub)
        {
            claims.name = info.name;
            claims.given_name = info.given_name;
            claims.family_name = info.family_name;
            claims.preferred_username = info.preferred_username;
            has_name = claims.has_name();
        }
    }
    let display_name = claims.display_name();
    let (id, display_name): (Uuid, String) = sqlx::query_as(
        "INSERT INTO users (id, oidc_issuer, oidc_subject, email, display_name)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (oidc_issuer, oidc_subject) DO UPDATE
               SET email = EXCLUDED.email,
                   display_name = CASE WHEN $6 THEN EXCLUDED.display_name ELSE users.display_name END,
                   last_seen_at = now()
             RETURNING id, display_name",
    )
    .bind(Uuid::new_v4())
    .bind(&claims.iss)
    .bind(&claims.sub)
    .bind(&email)
    .bind(&display_name)
    .bind(has_name)
    .fetch_one(&state.db)
    .await?;
    Ok(AuthUser { id, email, display_name })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims() -> Claims {
        Claims {
            iss: "i".into(),
            sub: "s".into(),
            email: Some("ada@example.org".into()),
            name: None,
            given_name: None,
            family_name: None,
            preferred_username: None,
        }
    }

    #[test]
    fn display_name_fallbacks() {
        let mut c = claims();
        assert_eq!(c.display_name(), "ada");
        c.given_name = Some("Ada".into());
        c.family_name = Some("Lovelace".into());
        assert_eq!(c.display_name(), "Ada Lovelace");
        c.name = Some("Countess".into());
        assert_eq!(c.display_name(), "Countess");
    }
}
