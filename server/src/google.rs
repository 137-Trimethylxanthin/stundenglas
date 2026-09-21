//! For those who want Google to know at once rather than whenever it feels
//! like looking. A subscribed URL is refreshed on Google's own schedule, often
//! many hours; events written through the API appear as soon as we write them.

use anyhow::{Context, Result, bail};
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;
use chrono::{Duration, Utc};
use serde::Deserialize;
use sqlx::{PgPool, Row};
use std::future::Future;
use std::pin::Pin;
use stundenglas_core::{Lesson, gcal};
use uuid::Uuid;

use crate::crypto::{KEY_VERSION, Sealed};
use crate::{AppState, db};

const AUTH: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/calendar";

/// Where consent returneth to. Google matcheth this exactly, so it is built
/// from the public URL and nothing else.
pub fn redirect_uri(state: &AppState) -> String {
    format!("{}/google/callback", state.config.public_url)
}

// ------------------------------------------------------------- the dance ---

pub async fn begin(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(account): Path<Uuid>,
) -> Response {
    let Some(user) = crate::web::signed_in(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    let Some(client) = state.config.google.clone() else {
        return crate::web::say(
            "Not offered here",
            "This server has no Google client configured, so linking a Google \
             Calendar is not on offer. The calendar link works regardless.",
        );
    };
    if !db::owns(&state.pool, user.id, account).await.unwrap_or(false) {
        return Redirect::to("/").into_response();
    }

    let token = match crate::admin::mint_token() {
        Ok(t) => t,
        Err(err) => {
            tracing::error!("no randomness: {err:#}");
            return Redirect::to("/").into_response();
        }
    };
    if let Err(err) = remember_state(&state.pool, &token, user.id, account).await {
        tracing::error!("could not remember the consent state: {err:#}");
        return Redirect::to("/").into_response();
    }

    let url = format!(
        "{AUTH}?response_type=code&client_id={}&redirect_uri={}&scope={}\
         &state={}&access_type=offline&prompt=consent&include_granted_scopes=true",
        urlencoding(&client.id),
        urlencoding(&redirect_uri(&state)),
        urlencoding(SCOPE),
        urlencoding(&token),
    );
    Redirect::to(&url).into_response()
}

#[derive(Deserialize)]
pub struct Returned {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

pub async fn callback(State(app): State<AppState>, Query(back): Query<Returned>) -> Response {
    if let Some(err) = back.error {
        return crate::web::say("Google said no", &format!("Consent was refused: {err}"));
    }
    let (Some(code), Some(state)) = (back.code, back.state) else {
        return crate::web::say("Something is missing", "That return from Google made no sense.");
    };

    // The state is spent on use, so a replayed link buys nothing.
    let Ok(Some((user, account))) = spend_state(&app.pool, &state).await else {
        return crate::web::say(
            "That link is stale",
            "Start the connection again from your timetables.",
        );
    };

    match finish(&app, &code, account).await {
        Ok(()) => {
            let app2 = app.clone();
            tokio::spawn(async move {
                if let Err(err) = crate::sync::once(app2).await {
                    tracing::warn!("first push failed: {err:#}");
                }
            });
            let _ = user;
            Redirect::to("/").into_response()
        }
        Err(err) => {
            tracing::warn!("google link failed: {err:#}");
            crate::web::say("That did not work", &format!("{err}"))
        }
    }
}

async fn finish(state: &AppState, code: &str, account: Uuid) -> Result<()> {
    let client = state.config.google.clone().context("no Google client configured")?;
    let reply = state
        .http
        .post(TOKEN)
        .form(&[
            ("code", code),
            ("client_id", client.id.as_str()),
            ("client_secret", client.secret.as_str()),
            ("redirect_uri", redirect_uri(state).as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .context("exchanging the code with Google")?;

    let ok = reply.status().is_success();
    let body = reply.text().await.unwrap_or_default();
    if !ok {
        bail!("Google refused the exchange: {}", body.chars().take(200).collect::<String>());
    }

    #[derive(Deserialize)]
    struct Granted {
        #[serde(default)]
        refresh_token: Option<String>,
    }
    let granted: Granted = serde_json::from_str(&body).context("reading Google's reply")?;
    let refresh = granted.refresh_token.context(
        "Google returned no refresh token. Revoke this app at \
         myaccount.google.com/permissions and connect again.",
    )?;

    let sealed = state.config.sealer.seal(&refresh)?;
    put_link(&state.pool, account, &sealed).await
}

pub async fn unlink(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(account): Path<Uuid>,
) -> Response {
    if let Some(user) = crate::web::signed_in(&state, &jar).await
        && db::owns(&state.pool, user.id, account).await.unwrap_or(false)
    {
        let _ = sqlx::query("delete from google_links where untis_account_id = $1")
            .bind(account)
            .execute(&state.pool)
            .await;
    }
    Redirect::to("/").into_response()
}

// ----------------------------------------------------------------- store ---

async fn remember_state(pool: &PgPool, token: &str, user: Uuid, account: Uuid) -> Result<()> {
    sqlx::query(
        "insert into oauth_states (state, user_id, untis_account_id, expires_at)
         values ($1, $2, $3, $4)",
    )
    .bind(token)
    .bind(user)
    .bind(account)
    .bind(Utc::now() + Duration::minutes(15))
    .execute(pool)
    .await?;
    Ok(())
}

async fn spend_state(pool: &PgPool, token: &str) -> Result<Option<(Uuid, Uuid)>> {
    let row = sqlx::query(
        "delete from oauth_states where state = $1 and expires_at > now()
         returning user_id, untis_account_id",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| (r.get("user_id"), r.get("untis_account_id"))))
}

async fn put_link(pool: &PgPool, account: Uuid, sealed: &Sealed) -> Result<()> {
    sqlx::query(
        "insert into google_links (untis_account_id, refresh_secret, refresh_nonce, key_version)
         values ($1, $2, $3, $4)
         on conflict (untis_account_id) do update
            set refresh_secret = excluded.refresh_secret,
                refresh_nonce = excluded.refresh_nonce,
                key_version = excluded.key_version",
    )
    .bind(account)
    .bind(&sealed.ciphertext)
    .bind(&sealed.nonce)
    .bind(KEY_VERSION)
    .execute(pool)
    .await
    .context("storing the Google link")?;
    Ok(())
}

pub struct Link {
    pub refresh: String,
    pub calendar_id: Option<String>,
    pub calendar_name: String,
}

pub async fn link_of(state: &AppState, account: Uuid) -> Result<Option<Link>> {
    let Some(row) = sqlx::query(
        "select refresh_secret, refresh_nonce, calendar_id, calendar_name
           from google_links where untis_account_id = $1",
    )
    .bind(account)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Ok(None);
    };
    let sealed = Sealed { ciphertext: row.get("refresh_secret"), nonce: row.get("refresh_nonce") };
    Ok(Some(Link {
        refresh: state.config.sealer.unseal(&sealed)?,
        calendar_id: row.try_get("calendar_id").ok().flatten(),
        calendar_name: row.get("calendar_name"),
    }))
}

/// Store a refresh token obtained elsewhere -- migrating from the CLI, or
/// seeding a test -- without walking the consent flow.
pub async fn adopt_refresh_token(
    state: &AppState,
    account: Uuid,
    refresh: &str,
    calendar_name: &str,
) -> Result<()> {
    let sealed = state.config.sealer.seal(refresh)?;
    put_link(&state.pool, account, &sealed).await?;
    sqlx::query("update google_links set calendar_name = $2 where untis_account_id = $1")
        .bind(account)
        .bind(calendar_name)
        .execute(&state.pool)
        .await?;
    Ok(())
}

pub async fn has_link(pool: &PgPool, account: Uuid) -> bool {
    sqlx::query("select 1 from google_links where untis_account_id = $1")
        .bind(account)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .is_some()
}

// ------------------------------------------------------------------ push ---

async fn access_token(state: &AppState, refresh: &str) -> Result<String> {
    let client = state.config.google.clone().context("no Google client configured")?;
    let reply = state
        .http
        .post(TOKEN)
        .form(&[
            ("client_id", client.id.as_str()),
            ("client_secret", client.secret.as_str()),
            ("refresh_token", refresh),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .context("refreshing the Google token")?;
    let ok = reply.status().is_success();
    let body = reply.text().await.unwrap_or_default();
    if !ok {
        bail!("Google refused the refresh: {}", body.chars().take(200).collect::<String>());
    }
    #[derive(Deserialize)]
    struct Granted {
        access_token: String,
    }
    Ok(serde_json::from_str::<Granted>(&body).context("reading the refreshed token")?.access_token)
}

/// Write this account's lessons into its linked Google calendar.
///
/// Owned parameters and a boxed future on purpose. An `async fn` taking
/// `&[Lesson]` and `&AppState` is generic over those lifetimes, and the caller
/// -- a spawned per-account task -- cannot then be shown Send for *every*
/// lifetime, only for some. With nothing borrowed at this boundary there is
/// nothing to generalise over.
pub fn push(
    state: AppState,
    account: db::UntisAccount,
    lessons: Vec<Lesson>,
) -> Pin<Box<dyn Future<Output = Result<Option<gcal::Tally>>> + Send>> {
    Box::pin(async move {
        let Some(link) = link_of(&state, account.id).await? else {
            return Ok(None);
        };
        let token = access_token(&state, &link.refresh).await?;
        let calendar = gcal::Calendar::new(state.http.clone(), token);

        let id = match link.calendar_id {
            Some(id) => id,
            None => {
                let made = calendar.find_or_create(&link.calendar_name, account.timezone).await?;
                sqlx::query("update google_links set calendar_id = $2 where untis_account_id = $1")
                    .bind(account.id)
                    .bind(&made)
                    .execute(&state.pool)
                    .await?;
                made
            }
        };

        // Reconcile only within the span we actually hold, so nothing outside
        // it is touched -- the same rule the command-line tool follows.
        let Some(first) = lessons.iter().map(|l| l.start).min() else {
            return Ok(None);
        };
        let last = lessons.iter().map(|l| l.end).max().unwrap_or(first);

        let existing = calendar.existing(&id, first, last).await?;
        let plan = gcal::plan(&lessons, &existing, account.timezone);
        let tally = calendar.apply(&id, &plan, 6).await?;

        sqlx::query("update google_links set last_push_at = now() where untis_account_id = $1")
            .bind(account.id)
            .execute(&state.pool)
            .await?;
        Ok(Some(tally))
    })
}

fn urlencoding(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::urlencoding;

    #[test]
    fn the_redirect_uri_survives_being_a_query_value() {
        assert_eq!(
            urlencoding("https://a.test/google/callback"),
            "https%3A%2F%2Fa.test%2Fgoogle%2Fcallback"
        );
    }

    #[test]
    fn the_scope_url_is_escaped_whole() {
        assert_eq!(
            urlencoding("https://www.googleapis.com/auth/calendar"),
            "https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcalendar"
        );
    }

    #[test]
    fn a_state_token_passes_through_unharmed() {
        // tokens are url-safe base64 already; nothing should be mangled
        let token = "abcXYZ012-_";
        assert_eq!(urlencoding(token), token);
    }
}
