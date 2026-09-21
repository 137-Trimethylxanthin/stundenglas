//! A security key as a second factor.
//!
//! Supabase holdeth the credential and judgeth the ceremony; we only carry the
//! challenge to the browser and the answer back, so the browser still speaketh
//! to nobody but us. WebAuthn itself must happen in the page — it is a browser
//! API and there is no doing it server-side.
//!
//! This is a *second* factor, not a replacement for the password: GoTrue
//! v2.196 offereth no passwordless passkey sign-in over its REST API.

use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::AppState;
use crate::crypto::{KEY_VERSION, Sealed};

/// What a user hath enrolled.
#[derive(Debug, Clone, Serialize)]
pub struct Factor {
    pub id: Uuid,
    pub friendly_name: String,
    pub verified: bool,
}

#[derive(Deserialize)]
struct RawFactor {
    id: Uuid,
    #[serde(default)]
    friendly_name: Option<String>,
    #[serde(default)]
    factor_type: String,
    #[serde(default)]
    status: String,
}

/// Every security key on the account, verified or not.
///
/// Read from `/user`, not `/factors`: the latter answers with an empty body
/// in this build, which once silently became "no keys at all" and so quietly
/// disabled the second factor for everyone.
pub async fn factors(state: &AppState, bearer: &str) -> Result<Vec<Factor>> {
    let body = call(state, reqwest::Method::GET, "/auth/v1/user", Some(bearer), None).await?;
    #[derive(Deserialize)]
    struct WithFactors {
        #[serde(default)]
        factors: Vec<RawFactor>,
    }
    let raw = serde_json::from_str::<WithFactors>(&body)
        .context("reading the account's security keys")?
        .factors;
    Ok(raw
        .into_iter()
        .filter(|f| f.factor_type == "webauthn")
        .map(|f| Factor {
            id: f.id,
            friendly_name: f.friendly_name.unwrap_or_else(|| "Security key".to_owned()),
            verified: f.status == "verified",
        })
        .collect())
}

/// Only a verified key may stand between a password and a session; an
/// abandoned half-enrolment must never lock anybody out.
pub async fn verified_factor(state: &AppState, bearer: &str) -> Option<Factor> {
    factors(state, bearer).await.ok()?.into_iter().find(|f| f.verified)
}

pub async fn enrol(state: &AppState, bearer: &str, name: &str) -> Result<Uuid> {
    let body = call(
        state,
        reqwest::Method::POST,
        "/auth/v1/factors",
        Some(bearer),
        Some(json!({ "factor_type": "webauthn", "friendly_name": name })),
    )
    .await?;
    #[derive(Deserialize)]
    struct Made {
        id: Uuid,
    }
    Ok(serde_json::from_str::<Made>(&body).context("reading the new factor")?.id)
}

/// The options the browser handeth to `navigator.credentials`.
#[derive(Serialize)]
pub struct Ceremony {
    pub factor_id: Uuid,
    pub challenge_id: Uuid,
    pub options: Value,
}

pub async fn challenge(state: &AppState, bearer: &str, factor: Uuid) -> Result<Ceremony> {
    let origin = state.config.public_url.clone();
    let rp_id = rp_id_of(&origin);
    let body = call(
        state,
        reqwest::Method::POST,
        &format!("/auth/v1/factors/{factor}/challenge"),
        Some(bearer),
        Some(json!({ "webauthn": { "rp_id": rp_id, "rp_origins": [origin] } })),
    )
    .await?;

    let reply: Value = serde_json::from_str(&body).context("reading the challenge")?;
    let challenge_id = reply
        .get("id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .context("the challenge had no id")?;
    let options = reply
        .pointer("/webauthn/credential_options")
        .cloned()
        .context("the challenge carried no credential options")?;
    Ok(Ceremony { factor_id: factor, challenge_id, options })
}

/// Which ceremony is being answered. GoTrue insisteth on being told, and
/// refuseth with "WebAuthn type must be create or request" otherwise.
#[derive(Debug, Clone, Copy)]
pub enum Ceremonial {
    /// Registering a new key.
    Create,
    /// Presenting a key already registered.
    Request,
}

impl Ceremonial {
    fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Request => "request",
        }
    }
}

/// Hand the signed response back to Supabase. On success it yieldeth a session
/// raised to the second level.
///
/// The kind is decided here, never taken from the page: a browser claiming a
/// registration were it presenting a key would be answering the wrong question.
pub async fn verify(
    state: &AppState,
    bearer: &str,
    factor: Uuid,
    challenge_id: Uuid,
    kind: Ceremonial,
    credential: Value,
) -> Result<String> {
    let body = call(
        state,
        reqwest::Method::POST,
        &format!("/auth/v1/factors/{factor}/verify"),
        Some(bearer),
        Some(json!({
            "challenge_id": challenge_id,
            "webauthn": { "type": kind.as_str(), "credential_response": credential },
        })),
    )
    .await?;
    #[derive(Deserialize)]
    struct Raised {
        #[serde(default)]
        access_token: Option<String>,
    }
    serde_json::from_str::<Raised>(&body)
        .ok()
        .and_then(|r| r.access_token)
        // A verified enrolment answers without a session; the caller keeps the
        // one it already had.
        .map_or_else(|| Ok(bearer.to_owned()), Ok)
}

pub async fn forget(state: &AppState, bearer: &str, factor: Uuid) -> Result<()> {
    call(state, reqwest::Method::DELETE, &format!("/auth/v1/factors/{factor}"), Some(bearer), None)
        .await?;
    Ok(())
}

async fn call(
    state: &AppState,
    method: reqwest::Method,
    path: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> Result<String> {
    let mut req = state
        .http
        .request(method, format!("{}{path}", state.config.supabase_url))
        .header("apikey", &state.config.supabase_anon_key);
    if let Some(token) = bearer {
        req = req.bearer_auth(token);
    }
    if let Some(payload) = body {
        req = req.json(&payload);
    }
    let reply = req.send().await.context("reaching the account service")?;
    let status = reply.status();
    let text = reply.text().await.unwrap_or_default();
    if !status.is_success() {
        let why = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("msg").or_else(|| v.get("message")).and_then(Value::as_str).map(str::to_owned)
            })
            .unwrap_or_else(|| text.chars().take(160).collect());
        bail!("{why}");
    }
    Ok(text)
}

/// The relying party is the bare host: no scheme, no port. A key enrolled
/// under one is worthless under another, so this must not drift.
pub fn rp_id_of(public_url: &str) -> String {
    let without = public_url
        .strip_prefix("https://")
        .or_else(|| public_url.strip_prefix("http://"))
        .unwrap_or(public_url);
    without.split('/').next().unwrap_or("").split(':').next().unwrap_or("").to_ascii_lowercase()
}

// ------------------------------------------------------- half-done logins ---

fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

pub async fn park(state: &AppState, user: Uuid, factor: Uuid, gotrue: &str) -> Result<String> {
    let token = crate::admin::mint_token()?;
    let sealed = state.config.sealer.seal(gotrue)?;
    sqlx::query(
        "insert into pending_logins
            (token_hash, user_id, factor_id, gotrue_secret, gotrue_nonce, key_version, expires_at)
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(hash(&token))
    .bind(user)
    .bind(factor)
    .bind(&sealed.ciphertext)
    .bind(&sealed.nonce)
    .bind(KEY_VERSION)
    .bind(Utc::now() + Duration::minutes(10))
    .execute(&state.pool)
    .await
    .context("parking the half-finished sign-in")?;
    Ok(token)
}

pub struct Parked {
    pub user: Uuid,
    pub factor: Uuid,
    pub gotrue: String,
}

/// Read without consuming: the browser may need several tries at the key.
pub async fn peek(state: &AppState, token: &str) -> Option<Parked> {
    let row = sqlx::query(
        "select user_id, factor_id, gotrue_secret, gotrue_nonce
           from pending_logins where token_hash = $1 and expires_at > now()",
    )
    .bind(hash(token))
    .fetch_optional(&state.pool)
    .await
    .ok()??;
    use sqlx::Row as _;
    let sealed = Sealed { ciphertext: row.get("gotrue_secret"), nonce: row.get("gotrue_nonce") };
    Some(Parked {
        user: row.get("user_id"),
        factor: row.get("factor_id"),
        gotrue: state.config.sealer.unseal(&sealed).ok()?,
    })
}

pub async fn unpark(state: &AppState, token: &str) {
    let _ = sqlx::query("delete from pending_logins where token_hash = $1")
        .bind(hash(token))
        .execute(&state.pool)
        .await;
}

pub async fn sweep(state: &AppState) {
    let _ = sqlx::query("delete from pending_logins where expires_at < now()")
        .execute(&state.pool)
        .await;
}

#[cfg(test)]
mod tests {
    use super::rp_id_of;

    #[test]
    fn the_relying_party_is_the_bare_host() {
        assert_eq!(rp_id_of("https://stundenglas.example"), "stundenglas.example");
        assert_eq!(rp_id_of("http://localhost:8080"), "localhost");
        assert_eq!(rp_id_of("https://Example.TEST:443/some/path"), "example.test");
    }

    #[test]
    fn nothing_odd_slips_through() {
        assert_eq!(rp_id_of(""), "");
        assert_eq!(rp_id_of("https://"), "");
    }
}
