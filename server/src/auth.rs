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
pub const PENDING: &str = "stundenglas_pending";
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
/// A signed-in Supabase session: who, and the token we may act with.
pub struct Admitted {
    pub user: Uuid,
    pub gotrue: String,
}

pub async fn sign_up(state: &AppState, email: &str, password: &str) -> Result<Admitted> {
    if password.chars().count() < 10 {
        bail!("choose a password of at least ten characters");
    }
    let reply = post(state, "/auth/v1/signup", email, password).await?;
    let (status, body) = reply;
    if !status.is_success() {
        bail!("{}", complain(&body, "that sign-up was refused"));
    }
    let session: GoTrueSession = serde_json::from_str(&body).context("reading the sign-up")?;
    match (session.user, session.access_token) {
        // No session comes back when the address is already taken, or when
        // confirmation by email is demanded. Say so without revealing which.
        (Some(user), Some(token)) => Ok(Admitted { user: user.id, gotrue: token }),
        _ => bail!("that address cannot be used to sign up here"),
    }
}

pub async fn sign_in(state: &AppState, email: &str, password: &str) -> Result<Admitted> {
    let (status, body) = post(state, "/auth/v1/token?grant_type=password", email, password).await?;
    if !status.is_success() {
        bail!("{}", complain(&body, "those details were not accepted"));
    }
    let session: GoTrueSession = serde_json::from_str(&body).context("reading the sign-in")?;
    match (session.user, session.access_token) {
        (Some(user), Some(token)) => Ok(Admitted { user: user.id, gotrue: token }),
        _ => bail!("no session came back"),
    }
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

/// Erase the account itself. The service key is wanted here and nowhere else:
/// only an administrator of the project may delete a user.
///
/// Everything this service holdeth hangeth off `auth.users` by a cascading
/// key, so the sealed passwords, the feeds and the cached timetables go with
/// it. There is no second sweep to forget.
pub async fn delete_user(state: &AppState, who: uuid::Uuid) -> Result<()> {
    let reply = state
        .http
        .delete(format!("{}/auth/v1/admin/users/{who}", state.config.supabase_url))
        .header("apikey", &state.config.supabase_service_key)
        .bearer_auth(&state.config.supabase_service_key)
        .send()
        .await
        .context("reaching the account service")?;

    if !reply.status().is_success() {
        let status = reply.status();
        let body = reply.text().await.unwrap_or_default();
        bail!("{}", complain(&body, &format!("the account service refused to delete ({status})")));
    }
    Ok(())
}

// ----------------------------------------------------------------- tokens ---

/// An address as we keep and compare it: trimmed, and in lower case.
pub fn normalise_address(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// The two errands a one-time token may serve, named as GoTrue nameth them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Confirm,
    Reset,
}

impl Token {
    fn gotrue_type(self) -> &'static str {
        match self {
            Token::Confirm => "magiclink",
            Token::Reset => "recovery",
        }
    }
}

/// What came of spending a token.
pub enum Spent {
    Good(Admitted),
    Expired,
}

#[derive(Deserialize)]
struct GoTrueCode {
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    msg: Option<String>,
}

/// Doth this refusal from `/verify` mean the token is spent or stale, rather
/// than the service being broken? Anything unreadable is taken as broken.
fn verify_error_is_expired(body: &str) -> bool {
    serde_json::from_str::<GoTrueCode>(body).is_ok_and(|e| {
        e.error_code.as_deref() == Some("otp_expired")
            || e.msg.is_some_and(|m| m.contains("invalid or has expired"))
    })
}

/// Make an account that is not yet confirmed and hath no password, for the
/// service key alone may do so. An address already taken is an error: the
/// callers look first.
pub async fn create_unconfirmed(state: &AppState, email: &str) -> Result<Uuid> {
    let reply = state
        .http
        .post(format!("{}/auth/v1/admin/users", state.config.supabase_url))
        .header("apikey", &state.config.supabase_service_key)
        .bearer_auth(&state.config.supabase_service_key)
        .json(&serde_json::json!({ "email": email, "email_confirm": false }))
        .send()
        .await
        .context("reaching the account service")?;
    let status = reply.status();
    let body = reply.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("{}", complain(&body, &format!("the account service refused to create ({status})")));
    }
    let user: GoTrueUser = serde_json::from_str(&body).context("reading the new account")?;
    Ok(user.id)
}

/// Ask for a one-time token for an existing address. `None` when no such
/// account is known.
pub async fn mint(state: &AppState, kind: Token, email: &str) -> Result<Option<String>> {
    #[derive(Deserialize)]
    struct Link {
        hashed_token: String,
    }
    let reply = state
        .http
        .post(format!("{}/auth/v1/admin/generate_link", state.config.supabase_url))
        .header("apikey", &state.config.supabase_service_key)
        .bearer_auth(&state.config.supabase_service_key)
        .json(&serde_json::json!({ "type": kind.gotrue_type(), "email": email }))
        .send()
        .await
        .context("reaching the account service")?;
    let status = reply.status();
    let body = reply.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::NOT_FOUND {
        let unknown = serde_json::from_str::<GoTrueCode>(&body)
            .is_ok_and(|e| e.error_code.as_deref() == Some("user_not_found"));
        if unknown {
            return Ok(None);
        }
    }
    if !status.is_success() {
        bail!("{}", complain(&body, &format!("the account service refused a token ({status})")));
    }
    let link: Link = serde_json::from_str(&body).context("reading the token")?;
    Ok(Some(link.hashed_token))
}

/// Spend a token. It confirmeth the address and yieldeth a session; a second
/// spending, or a stale token, is `Expired`.
pub async fn spend(state: &AppState, kind: Token, token_hash: &str) -> Result<Spent> {
    let reply = state
        .http
        .post(format!("{}/auth/v1/verify", state.config.supabase_url))
        .header("apikey", &state.config.supabase_anon_key)
        .json(&serde_json::json!({ "type": kind.gotrue_type(), "token_hash": token_hash }))
        .send()
        .await
        .context("reaching the account service")?;
    let status = reply.status();
    let body = reply.text().await.unwrap_or_default();
    if status.is_client_error() && verify_error_is_expired(&body) {
        return Ok(Spent::Expired);
    }
    if !status.is_success() {
        bail!("{}", complain(&body, &format!("the account service refused the token ({status})")));
    }
    let session: GoTrueSession = serde_json::from_str(&body).context("reading the session")?;
    match (session.user, session.access_token) {
        (Some(user), Some(token)) => Ok(Spent::Good(Admitted { user: user.id, gotrue: token })),
        _ => bail!("no session came back"),
    }
}

/// Set a person's password as the administrator, there being no old one.
pub async fn set_password(state: &AppState, user: Uuid, password: &str) -> Result<()> {
    let reply = state
        .http
        .put(format!("{}/auth/v1/admin/users/{user}", state.config.supabase_url))
        .header("apikey", &state.config.supabase_service_key)
        .bearer_auth(&state.config.supabase_service_key)
        .json(&serde_json::json!({ "password": password }))
        .send()
        .await
        .context("reaching the account service")?;
    let status = reply.status();
    if !status.is_success() {
        let body = reply.text().await.unwrap_or_default();
        bail!(
            "{}",
            complain(&body, &format!("the account service refused the password ({status})"))
        );
    }
    Ok(())
}

// --------------------------------------------------------------- sessions ---

fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

pub async fn open_session(
    state: &AppState,
    user: Uuid,
    gotrue: &str,
    agent: Option<&str>,
) -> Result<String> {
    let token = crate::admin::mint_token()?;
    let sealed = state.config.sealer.seal(gotrue)?;
    sqlx::query(
        "insert into sessions
            (token_hash, user_id, expires_at, user_agent, gotrue_secret, gotrue_nonce)
         values ($1, $2, $3, $4, $5, $6)",
    )
    .bind(hash(&token))
    .bind(user)
    .bind(Utc::now() + Duration::days(SESSION_DAYS))
    .bind(agent.map(|a| a.chars().take(200).collect::<String>()))
    .bind(&sealed.ciphertext)
    .bind(&sealed.nonce)
    .execute(&state.pool)
    .await
    .context("opening a session")?;
    Ok(token)
}

/// The Supabase token stored with a session, for acting as that user.
pub async fn gotrue_of(state: &AppState, token: &str) -> Option<String> {
    let row = sqlx::query(
        "select gotrue_secret, gotrue_nonce from sessions
          where token_hash = $1 and expires_at > now()",
    )
    .bind(hash(token))
    .fetch_optional(&state.pool)
    .await
    .ok()??;
    let secret: Option<Vec<u8>> = row.try_get("gotrue_secret").ok().flatten();
    let nonce: Option<Vec<u8>> = row.try_get("gotrue_nonce").ok().flatten();
    let sealed = crate::crypto::Sealed { ciphertext: secret?, nonce: nonce? };
    state.config.sealer.unseal(&sealed).ok()
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

/// Shut every session a person hath, as when they are banned.
pub async fn close_all_sessions(pool: &PgPool, user: Uuid) {
    let _ = sqlx::query("delete from sessions where user_id = $1").bind(user).execute(pool).await;
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

/// Carries a sign-in that has passed the password but not yet the key. Short
/// lived, and replaced by the real session the moment the key answers.
pub fn pending_cookie(token: String, secure: bool) -> Cookie<'static> {
    brief(PENDING, token, time::Duration::minutes(10), secure)
}

/// Where to go once signed in: the refused link a calendar's red notice
/// pointed at. Only its id is kept, so it can never send anyone elsewhere.
pub const RETURN_TO: &str = "sg_return";

pub fn return_cookie(account: uuid::Uuid, secure: bool) -> Cookie<'static> {
    brief(RETURN_TO, account.to_string(), time::Duration::minutes(15), secure)
}

pub fn return_gone(secure: bool) -> Cookie<'static> {
    brief(RETURN_TO, String::new(), time::Duration::ZERO, secure)
}

pub fn pending_gone(secure: bool) -> Cookie<'static> {
    brief(PENDING, String::new(), time::Duration::ZERO, secure)
}

/// A cookie that only carrieth a step from one page to the next: unreadable
/// by script, sent only to this site, and gone in minutes — or at once, with
/// a lifetime of nought.
fn brief(
    name: &'static str,
    value: String,
    lasts: time::Duration,
    secure: bool,
) -> Cookie<'static> {
    let mut cookie = Cookie::new(name, value);
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_path("/");
    cookie.set_secure(secure);
    cookie.set_max_age(lasts);
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

    #[test]
    fn an_address_is_trimmed_and_lowercased() {
        assert_eq!(normalise_address(" Max@Maxi.CO.at "), "max@maxi.co.at");
    }

    #[test]
    fn an_expired_token_is_told_from_a_broken_service() {
        assert!(verify_error_is_expired(
            r#"{"code":403,"error_code":"otp_expired","msg":"Email link is invalid or has expired"}"#
        ));
        assert!(!verify_error_is_expired(r#"{"code":500,"msg":"boom"}"#));
    }

    #[test]
    fn token_types() {
        assert_eq!(Token::Confirm.gotrue_type(), "magiclink");
        assert_eq!(Token::Reset.gotrue_type(), "recovery");
    }
}
