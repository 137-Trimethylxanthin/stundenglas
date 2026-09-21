//! All that toucheth Postgres. The browser never cometh here; only this
//! server doth, and it alone holdeth the key that openeth the sealed columns.

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Row};
use stundenglas_core::{Credentials, Lesson};
use uuid::Uuid;

use crate::crypto::{KEY_VERSION, Sealed, Sealer};

pub async fn connect(url: &str) -> Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(url)
        .await
        .context("connecting to Postgres")
}

#[derive(Debug, Clone)]
pub struct UntisAccount {
    pub id: Uuid,
    pub server: String,
    pub school: String,
    pub username: String,
    pub display_name: Option<String>,
    pub enabled: bool,
}

/// An account together with what is needed to log in as it.
pub struct AccountWithSecret {
    pub account: UntisAccount,
    pub credentials: Credentials,
}

fn account_from(row: &PgRow) -> UntisAccount {
    UntisAccount {
        id: row.get("id"),
        server: row.get("server"),
        school: row.get("school"),
        username: row.get("username"),
        display_name: row.try_get("display_name").ok().flatten(),
        enabled: row.get("enabled"),
    }
}

pub async fn add_account(
    pool: &PgPool,
    sealer: &Sealer,
    user_id: Uuid,
    server: &str,
    school: &str,
    username: &str,
    password: &str,
) -> Result<UntisAccount> {
    let sealed = sealer.seal(password)?;
    let row = sqlx::query(
        "insert into untis_accounts
            (user_id, server, school, username, secret, nonce, key_version)
         values ($1, $2, $3, $4, $5, $6, $7)
         on conflict (user_id, server, school, username) do update
            set secret = excluded.secret,
                nonce = excluded.nonce,
                key_version = excluded.key_version,
                enabled = true
         returning id, user_id, server, school, username, display_name, enabled",
    )
    .bind(user_id)
    .bind(server)
    .bind(school)
    .bind(username)
    .bind(&sealed.ciphertext)
    .bind(&sealed.nonce)
    .bind(KEY_VERSION)
    .fetch_one(pool)
    .await
    .context("storing the account")?;
    Ok(account_from(&row))
}

pub async fn accounts_of(pool: &PgPool, user_id: Uuid) -> Result<Vec<UntisAccount>> {
    let rows = sqlx::query(
        "select id, user_id, server, school, username, display_name, enabled
           from untis_accounts where user_id = $1 order by created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(account_from).collect())
}

/// Every account the scheduler should refresh, with its password unsealed.
pub async fn accounts_to_sync(pool: &PgPool, sealer: &Sealer) -> Result<Vec<AccountWithSecret>> {
    let rows = sqlx::query(
        "select id, user_id, server, school, username, display_name, enabled, secret, nonce
           from untis_accounts where enabled order by created_at",
    )
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let account = account_from(row);
        let sealed = Sealed { ciphertext: row.get("secret"), nonce: row.get("nonce") };
        match sealer.unseal(&sealed) {
            Ok(password) => out.push(AccountWithSecret {
                credentials: Credentials {
                    server: account.server.clone(),
                    school: account.school.clone(),
                    user: account.username.clone(),
                    password,
                },
                account,
            }),
            // A row sealed under a key we no longer hold. Say so once and
            // leave it be; better than failing the whole run.
            Err(err) => tracing::error!(account = %account.id, "cannot unseal: {err:#}"),
        }
    }
    Ok(out)
}

pub async fn set_identity(
    pool: &PgPool,
    account: Uuid,
    display_name: &str,
    person_id: i64,
) -> Result<()> {
    sqlx::query("update untis_accounts set display_name = $2, person_id = $3 where id = $1")
        .bind(account)
        .bind(display_name)
        .bind(person_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ------------------------------------------------------------------ feeds ---

#[derive(Debug, Clone)]
pub struct Feed {
    pub token: String,
    pub keep_cancelled: bool,
    pub refresh_minutes: i32,
    pub display_name: Option<String>,
}

/// What a request for `/cal/<token>.ics` resolveth to.
pub struct FeedPayload {
    pub feed: Feed,
    pub lessons: Vec<Lesson>,
    pub etag: Option<String>,
    pub fetched_at: Option<DateTime<Utc>>,
}

pub async fn create_feed(pool: &PgPool, account: Uuid, token: &str) -> Result<Feed> {
    let row = sqlx::query(
        "insert into feeds (untis_account_id, token) values ($1, $2)
         returning token, untis_account_id, keep_cancelled, refresh_minutes",
    )
    .bind(account)
    .bind(token)
    .fetch_one(pool)
    .await
    .context("creating the feed")?;
    Ok(Feed {
        token: row.get("token"),
        keep_cancelled: row.get("keep_cancelled"),
        refresh_minutes: row.get("refresh_minutes"),
        display_name: None,
    })
}

pub async fn feeds_of(pool: &PgPool, account: Uuid) -> Result<Vec<Feed>> {
    let rows = sqlx::query(
        "select token, untis_account_id, keep_cancelled, refresh_minutes
           from feeds where untis_account_id = $1 order by created_at",
    )
    .bind(account)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| Feed {
            token: r.get("token"),
            keep_cancelled: r.get("keep_cancelled"),
            refresh_minutes: r.get("refresh_minutes"),
            display_name: None,
        })
        .collect())
}

pub async fn feed_by_token(pool: &PgPool, token: &str) -> Result<Option<FeedPayload>> {
    let Some(row) = sqlx::query(
        "select f.token, f.untis_account_id, f.keep_cancelled, f.refresh_minutes,
                a.display_name, s.lessons, s.etag, s.last_ok_at
           from feeds f
           join untis_accounts a on a.id = f.untis_account_id
           left join sync_state s on s.untis_account_id = f.untis_account_id
          where f.token = $1 and a.enabled",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };

    let raw: Option<serde_json::Value> = row.try_get("lessons").ok().flatten();
    let lessons: Vec<Lesson> = match raw {
        Some(value) => serde_json::from_value(value).unwrap_or_default(),
        None => Vec::new(),
    };

    Ok(Some(FeedPayload {
        feed: Feed {
            token: row.get("token"),
            keep_cancelled: row.get("keep_cancelled"),
            refresh_minutes: row.get("refresh_minutes"),
            display_name: row.try_get("display_name").ok().flatten(),
        },
        lessons,
        etag: row.try_get("etag").ok().flatten(),
        fetched_at: row.try_get("last_ok_at").ok().flatten(),
    }))
}

/// Counting a serve must never fail a serve, so this is fire-and-forget.
pub async fn note_served(pool: &PgPool, token: &str) {
    let _ = sqlx::query(
        "update feeds set last_served_at = now(), serve_count = serve_count + 1
          where token = $1",
    )
    .bind(token)
    .execute(pool)
    .await;
}

// ------------------------------------------------------------- sync state ---

pub async fn store_sync(
    pool: &PgPool,
    account: Uuid,
    lessons: &[Lesson],
    etag: &str,
    window: (NaiveDate, NaiveDate),
) -> Result<()> {
    let payload = serde_json::to_value(lessons)?;
    sqlx::query(
        "insert into sync_state
            (untis_account_id, lessons, lesson_count, etag, window_start, window_end,
             last_ok_at, last_error, last_error_at, consecutive_fails)
         values ($1, $2, $3, $4, $5, $6, now(), null, null, 0)
         on conflict (untis_account_id) do update
            set lessons = excluded.lessons,
                lesson_count = excluded.lesson_count,
                etag = excluded.etag,
                window_start = excluded.window_start,
                window_end = excluded.window_end,
                last_ok_at = now(),
                last_error = null,
                last_error_at = null,
                consecutive_fails = 0",
    )
    .bind(account)
    .bind(payload)
    .bind(lessons.len() as i32)
    .bind(etag)
    .bind(window.0)
    .bind(window.1)
    .execute(pool)
    .await
    .context("storing the timetable")?;
    Ok(())
}

pub async fn store_failure(pool: &PgPool, account: Uuid, why: &str) -> Result<()> {
    sqlx::query(
        "insert into sync_state (untis_account_id, last_error, last_error_at, consecutive_fails)
         values ($1, $2, now(), 1)
         on conflict (untis_account_id) do update
            set last_error = excluded.last_error,
                last_error_at = now(),
                consecutive_fails = sync_state.consecutive_fails + 1",
    )
    .bind(account)
    .bind(why.chars().take(500).collect::<String>())
    .execute(pool)
    .await?;
    Ok(())
}
