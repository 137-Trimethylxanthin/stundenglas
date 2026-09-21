//! Signing in. Supabase keepeth the accounts and checketh the passwords; we
//! never see a password twice, and the browser never seeth a Supabase token.
//!
//! What the browser carrieth is our own cookie, of which only a hash is kept,
//! so a stolen database yieldeth no usable session.

use anyhow::{Context, Result, bail};
use axum_extra::extract::cookie::{Cookie, SameSite};
use chrono::{Duration, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::AppState;

pub const COOKIE: &str = "stundenglas_session";
const SESSION_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy)]
pub struct CurrentUser {
    pub id: Uuid,
}

#[derive(Deserialize)]
struct GoTrueUser {
    id: Uuid,
}

#[derive(Deserialize)]
struct GoTrueSession {
    #[serde(default)]
    user: Option<GoTrueUser>,
    #[serde(default)]
    access_token: Option<String>,
}

#[derive(Deserialize)]
struct GoTrueError {
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

fn complain(body: &str, fallback: &str) -> String {
    serde_json::from_str::<GoTrueError>(body)
        .ok()
        .and_then(|e| e.msg.or(e.error_description).or(e.message))
        .unwrap_or_else(|| fallback.to_owned())
}

/// Hand the credentials to Supabase and, if it is content, open a session.
pub async fn sign_up(state: &AppState, email: &str, password: &str) -> Result<Uuid> {
    if password.chars().count() < 10 {
        bail!("choose a password of at least ten characters");
    }
    let reply = post(state, "/auth/v1/signup", email, password).await?;
    let (status, body) = reply;
    if !status.is_success() {
        bail!("{}", complain(&body, "that sign-up was refused"));
    }
    let session: GoTrueSession = serde_json::from_str(&body).context("reading the sign-up")?;
    match session.user {
        // No session comes back when the address is already taken, or when
        // confirmation by email is demanded. Say so without revealing which.
        Some(user) if session.access_token.is_some() => Ok(user.id),
        _ => bail!("that address cannot be used to sign up here"),
    }
}

pub async fn sign_in(state: &AppState, email: &str, password: &str) -> Result<Uuid> {
    let (status, body) = post(state, "/auth/v1/token?grant_type=password", email, password).await?;
    if !status.is_success() {
        bail!("{}", complain(&body, "those details were not accepted"));
    }
    let session: GoTrueSession = serde_json::from_str(&body).context("reading the sign-in")?;
    session.user.map(|u| u.id).context("no account came back")
}

async fn post(
    state: &AppState,
    path: &str,
    email: &str,
    password: &str,
) -> Result<(reqwest::StatusCode, String)> {
    let reply = state
        .http
        .post(format!("{}{path}", state.config.supabase_url))
        .header("apikey", &state.config.supabase_anon_key)
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send()
        .await
        .context("reaching the account service")?;
    let status = reply.status();
    Ok((status, reply.text().await.unwrap_or_default()))
}

// --------------------------------------------------------------- sessions ---

fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

pub async fn open_session(pool: &PgPool, user: Uuid, agent: Option<&str>) -> Result<String> {
    let token = crate::admin::mint_token()?;
    sqlx::query(
        "insert into sessions (token_hash, user_id, expires_at, user_agent)
         values ($1, $2, $3, $4)",
    )
    .bind(hash(&token))
    .bind(user)
    .bind(Utc::now() + Duration::days(SESSION_DAYS))
    .bind(agent.map(|a| a.chars().take(200).collect::<String>()))
    .execute(pool)
    .await
    .context("opening a session")?;
    Ok(token)
}

pub async fn user_of(pool: &PgPool, token: &str) -> Option<CurrentUser> {
    let row =
        sqlx::query("select user_id from sessions where token_hash = $1 and expires_at > now()")
            .bind(hash(token))
            .fetch_optional(pool)
            .await
            .ok()??;
    Some(CurrentUser { id: row.get("user_id") })
}

pub async fn close_session(pool: &PgPool, token: &str) {
    let _ = sqlx::query("delete from sessions where token_hash = $1")
        .bind(hash(token))
        .execute(pool)
        .await;
}

/// Sweep away what hath expired, so the table doth not grow for ever.
pub async fn sweep(pool: &PgPool) {
    let _ = sqlx::query("delete from sessions where expires_at < now()").execute(pool).await;
}

pub fn cookie_for(token: String, secure: bool) -> Cookie<'static> {
    let mut cookie = Cookie::new(COOKIE, token);
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_path("/");
    cookie.set_secure(secure);
    cookie.set_max_age(time::Duration::days(SESSION_DAYS));
    cookie
}

pub fn cookie_gone(secure: bool) -> Cookie<'static> {
    let mut cookie = Cookie::new(COOKIE, "");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_path("/");
    cookie.set_secure(secure);
    cookie.set_max_age(time::Duration::seconds(0));
    cookie
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cookie_cannot_be_read_by_script_nor_sent_across_sites() {
        let c = cookie_for("abc".to_owned(), true);
        assert!(c.http_only().unwrap(), "script must not reach the session");
        assert_eq!(c.same_site(), Some(SameSite::Lax), "guards the forms against other sites");
        assert!(c.secure().unwrap());
        assert_eq!(c.path(), Some("/"));
    }

    #[test]
    fn over_plain_http_the_secure_flag_is_dropped_else_it_would_never_arrive() {
        assert!(!cookie_for("abc".to_owned(), false).secure().unwrap());
    }

    #[test]
    fn logging_out_expires_the_cookie_at_once() {
        let c = cookie_gone(true);
        assert_eq!(c.value(), "");
        assert_eq!(c.max_age(), Some(time::Duration::seconds(0)));
    }

    #[test]
    fn only_the_hash_of_a_token_is_ever_stored() {
        let token = "a-session-token-of-some-length";
        let stored = hash(token);
        assert_eq!(stored.len(), 32);
        assert!(!String::from_utf8_lossy(&stored).contains("session"));
        assert_eq!(stored, hash(token), "must be stable, or nobody stays logged in");
        assert_ne!(stored, hash("a-session-token-of-some-lengtH"));
    }

    #[test]
    fn gotrue_complaints_are_passed_on_when_it_gives_one() {
        assert_eq!(
            complain(r#"{"msg":"Invalid login credentials"}"#, "x"),
            "Invalid login credentials"
        );
        assert_eq!(complain(r#"{"error_description":"nope"}"#, "x"), "nope");
        assert_eq!(complain("not json at all", "fallback"), "fallback");
        assert_eq!(complain("{}", "fallback"), "fallback");
    }
}
