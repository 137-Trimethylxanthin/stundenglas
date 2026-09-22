//! All that toucheth Postgres. The browser never cometh here; only this
//! server doth, and it alone holdeth the key that openeth the sealed columns.

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Row};
use stundenglas_core::{Credentials, DEFAULT_TZ, Exam, Homework, Lesson};
use uuid::Uuid;

use crate::crypto::{Sealed, Sealer};

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
    pub timezone: Tz,
    /// The school refused this login. Nothing is tried again until the owner
    /// enters a new password.
    pub credentials_rejected: bool,
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
        // A zone the database accepted but chrono-tz doth not know is not
        // worth failing a sync over; fall back and carry on.
        timezone: row
            .try_get::<String, _>("timezone")
            .ok()
            .and_then(|name| name.parse().ok())
            .unwrap_or(DEFAULT_TZ),
        credentials_rejected: row.try_get("credentials_rejected").unwrap_or(false),
    }
}

/// What a user giveth when linking a school, whether by form or by command.
#[derive(Debug, Clone)]
pub struct NewAccount {
    pub user_id: Uuid,
    pub server: String,
    pub school: String,
    pub username: String,
    pub password: String,
    pub timezone: Tz,
}

pub async fn add_account(pool: &PgPool, sealer: &Sealer, new: &NewAccount) -> Result<UntisAccount> {
    let sealed = sealer.seal(&new.password)?;
    let row = sqlx::query(
        "insert into untis_accounts
            (user_id, server, school, username, secret, nonce, key_version, timezone)
         values ($1, $2, $3, $4, $5, $6, $7, $8)
         on conflict (user_id, server, school, username) do update
            set secret = excluded.secret,
                nonce = excluded.nonce,
                key_version = excluded.key_version,
                timezone = excluded.timezone,
                enabled = true
         returning id, server, school, username, display_name, enabled, timezone,
                   credentials_rejected",
    )
    .bind(new.user_id)
    .bind(&new.server)
    .bind(&new.school)
    .bind(&new.username)
    .bind(&sealed.ciphertext)
    .bind(&sealed.nonce)
    .bind(sealer.version())
    .bind(new.timezone.name())
    .fetch_one(pool)
    .await
    .context("storing the account")?;
    Ok(account_from(&row))
}

pub async fn accounts_of(pool: &PgPool, user_id: Uuid) -> Result<Vec<UntisAccount>> {
    let rows = sqlx::query(
        "select id, server, school, username, display_name, enabled, timezone,
                credentials_rejected
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
        "select a.id, a.server, a.school, a.username, a.display_name, a.enabled,
                a.timezone, a.credentials_rejected, a.secret, a.nonce
           from untis_accounts a
           left join sync_state s on s.untis_account_id = a.id
          where a.enabled
            and not a.credentials_rejected
            and (s.next_attempt_at is null or s.next_attempt_at <= now())
          order by a.created_at",
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
                    timezone: account.timezone,
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
    display_name: String,
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
    pub id: Uuid,
    pub token: String,
    pub keep_cancelled: bool,
    pub refresh_minutes: i32,
    /// Minutes before an exam to ring. None, and nothing rings.
    pub remind_before_minutes: Option<i32>,
    pub with_homework: bool,
    pub label: Option<String>,
    pub display_name: Option<String>,
}

/// What a request for `/cal/<token>.ics` resolveth to.
pub struct FeedPayload {
    pub feed: Feed,
    pub lessons: Vec<Lesson>,
    pub exams: Vec<Exam>,
    pub homework: Vec<Homework>,
    pub etag: Option<String>,
    pub fetched_at: Option<DateTime<Utc>>,
}

fn feed_from(row: &PgRow, display_name: Option<String>) -> Feed {
    Feed {
        id: row.get("id"),
        token: row.get("token"),
        keep_cancelled: row.get("keep_cancelled"),
        refresh_minutes: row.get("refresh_minutes"),
        remind_before_minutes: row.try_get("remind_before_minutes").ok().flatten(),
        with_homework: row.try_get("with_homework").unwrap_or(true),
        label: row.try_get("label").ok().flatten(),
        display_name,
    }
}

pub async fn create_feed(pool: &PgPool, account: Uuid, token: &str) -> Result<Feed> {
    let row = sqlx::query(
        "insert into feeds (untis_account_id, token) values ($1, $2)
         returning id, token, untis_account_id, keep_cancelled, refresh_minutes,
                   remind_before_minutes, with_homework, label",
    )
    .bind(account)
    .bind(token)
    .fetch_one(pool)
    .await
    .context("creating the feed")?;
    Ok(feed_from(&row, None))
}

/// What a feed's owner may change about it. Bounded here as well as in the
/// schema: a form is only as honest as the hand that sendeth it.
pub struct FeedSettings {
    pub keep_cancelled: bool,
    pub refresh_minutes: i32,
    pub remind_before_minutes: Option<i32>,
    pub with_homework: bool,
    pub label: Option<String>,
}

/// Updating by owner as well as by id, so another user's feed simply matcheth
/// nothing and there is no separate check to forget.
pub async fn update_feed(
    pool: &PgPool,
    user_id: Uuid,
    feed: Uuid,
    want: &FeedSettings,
) -> Result<bool> {
    let done = sqlx::query(
        "update feeds f
            set keep_cancelled = $3,
                refresh_minutes = $4,
                remind_before_minutes = $5,
                with_homework = $6,
                label = $7
           from untis_accounts a
          where f.id = $1 and f.untis_account_id = a.id and a.user_id = $2",
    )
    .bind(feed)
    .bind(user_id)
    .bind(want.keep_cancelled)
    .bind(want.refresh_minutes.clamp(5, 1440))
    .bind(want.remind_before_minutes.map(|m| m.clamp(5, 10_080)))
    .bind(want.with_homework)
    .bind(want.label.as_deref().filter(|l| !l.is_empty()))
    .execute(pool)
    .await
    .context("changing the feed")?;
    Ok(done.rows_affected() > 0)
}

pub async fn feeds_of(pool: &PgPool, account: Uuid) -> Result<Vec<Feed>> {
    let rows = sqlx::query(
        "select id, token, untis_account_id, keep_cancelled, refresh_minutes,
                remind_before_minutes, with_homework, label
           from feeds where untis_account_id = $1 order by created_at",
    )
    .bind(account)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(|row| feed_from(row, None)).collect())
}

pub async fn feed_by_token(pool: &PgPool, token: &str) -> Result<Option<FeedPayload>> {
    let Some(row) = sqlx::query(
        "select f.id, f.token, f.untis_account_id, f.keep_cancelled, f.refresh_minutes,
                f.remind_before_minutes, f.with_homework, f.label,
                a.display_name, s.lessons, s.exams, s.homework, s.etag, s.last_ok_at
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

    // A cache written by an older build, or by a hand, must not take the feed
    // down with it: an unreadable column reads as nothing.
    let lessons: Vec<Lesson> = row
        .try_get::<Option<serde_json::Value>, _>("lessons")
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let exams: Vec<Exam> = row
        .try_get::<Option<serde_json::Value>, _>("exams")
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();

    let homework: Vec<Homework> = row
        .try_get::<Option<serde_json::Value>, _>("homework")
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();

    Ok(Some(FeedPayload {
        feed: feed_from(&row, row.try_get("display_name").ok().flatten()),
        lessons,
        exams,
        homework,
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
    exams: &[Exam],
    homework: &[Homework],
    etag: String,
    window: (NaiveDate, NaiveDate),
) -> Result<()> {
    let payload = serde_json::to_value(lessons)?;
    let sat = serde_json::to_value(exams)?;
    let set = serde_json::to_value(homework)?;
    sqlx::query(
        "insert into sync_state
            (untis_account_id, lessons, lesson_count, exams, exam_count,
             homework, homework_count, etag, window_start, window_end,
             last_ok_at, last_error, last_error_at, consecutive_fails)
         values ($1, $2, $3, $7, $8, $9, $10, $4, $5, $6, now(), null, null, 0)
         on conflict (untis_account_id) do update
            set lessons = excluded.lessons,
                lesson_count = excluded.lesson_count,
                exams = excluded.exams,
                exam_count = excluded.exam_count,
                homework = excluded.homework,
                homework_count = excluded.homework_count,
                etag = excluded.etag,
                window_start = excluded.window_start,
                window_end = excluded.window_end,
                last_ok_at = now(),
                last_error = null,
                last_error_at = null,
                consecutive_fails = 0,
                next_attempt_at = null",
    )
    .bind(account)
    .bind(payload)
    .bind(lessons.len() as i32)
    .bind(etag)
    .bind(window.0)
    .bind(window.1)
    .bind(sat)
    .bind(exams.len() as i32)
    .bind(set)
    .bind(homework.len() as i32)
    .execute(pool)
    .await
    .context("storing the timetable")?;
    Ok(())
}

/// What went wrong, and therefore how soon we may try again.
pub enum Failure {
    /// The school refused the login. Trying again is how an account gets
    /// locked, so it is not tried again at all.
    Rejected,
    /// The school was unreachable, or answered something we could not read.
    /// Worth another go, but not immediately.
    Transient,
}

pub async fn store_failure(pool: &PgPool, account: Uuid, why: String, kind: Failure) -> Result<()> {
    let why = why.chars().take(500).collect::<String>();

    // Five minutes, then ten, twenty, and so on to a ceiling of six hours. A
    // school that is down for a morning is asked a handful of times, not
    // ninety.
    sqlx::query(
        "insert into sync_state
            (untis_account_id, last_error, last_error_at, consecutive_fails, next_attempt_at)
         values ($1, $2, now(), 1, now() + interval '5 minutes')
         on conflict (untis_account_id) do update
            set last_error = excluded.last_error,
                last_error_at = now(),
                consecutive_fails = sync_state.consecutive_fails + 1,
                next_attempt_at = now() + least(
                    interval '6 hours',
                    interval '5 minutes' * power(2, least(sync_state.consecutive_fails, 7)))",
    )
    .bind(account)
    .bind(&why)
    .execute(pool)
    .await?;

    if matches!(kind, Failure::Rejected) {
        sqlx::query(
            "update untis_accounts
                set credentials_rejected = true, rejected_at = now()
              where id = $1 and not credentials_rejected",
        )
        .bind(account)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// A new password for a link the school refused. Re-sealing and clearing the
/// refusal are one act: either the owner is back in, or nothing changed.
pub async fn replace_password(
    pool: &PgPool,
    sealer: &Sealer,
    user_id: Uuid,
    account: Uuid,
    password: &str,
) -> Result<bool> {
    let sealed = sealer.seal(password)?;
    let done = sqlx::query(
        "update untis_accounts
            set secret = $3, nonce = $4, key_version = $5,
                credentials_rejected = false, rejected_at = null
          where id = $1 and user_id = $2",
    )
    .bind(account)
    .bind(user_id)
    .bind(&sealed.ciphertext)
    .bind(&sealed.nonce)
    .bind(sealer.version())
    .execute(pool)
    .await
    .context("storing the new password")?;

    if done.rows_affected() > 0 {
        // Let it be tried at once rather than after the backoff it earned.
        sqlx::query("update sync_state set next_attempt_at = null where untis_account_id = $1")
            .bind(account)
            .execute(pool)
            .await?;
    }
    Ok(done.rows_affected() > 0)
}

/// A timetable nobody has refreshed in a long while is stale and is still
/// somebody's whereabouts. Drop the cached lessons, keep the link itself, so
/// the owner finds their account as they left it and merely empty.
pub async fn forget_stale(pool: &PgPool, days: i64) -> Result<u64> {
    let done = sqlx::query(
        "update sync_state
            set lessons = '[]'::jsonb, lesson_count = 0,
                exams = '[]'::jsonb, exam_count = 0,
                homework = '[]'::jsonb, homework_count = 0, etag = null
          where (lesson_count > 0 or exam_count > 0 or homework_count > 0)
            and coalesce(last_ok_at, updated_at) < now() - make_interval(days => $1::int)",
    )
    .bind(days as i32)
    .execute(pool)
    .await
    .context("forgetting stale timetables")?;
    Ok(done.rows_affected())
}

/// Whether this user owneth that school link. Asked before anything is changed.
pub async fn owns(pool: &PgPool, user_id: Uuid, account: Uuid) -> Result<bool> {
    let found = sqlx::query("select 1 from untis_accounts where id = $1 and user_id = $2")
        .bind(account)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(found.is_some())
}

/// Deleting by owner as well as id: another user's id then matcheth nothing,
/// and there is no separate check to forget.
pub async fn delete_account(pool: &PgPool, user_id: Uuid, account: Uuid) -> Result<u64> {
    let done = sqlx::query("delete from untis_accounts where id = $1 and user_id = $2")
        .bind(account)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected())
}

#[derive(Debug, Clone)]
pub struct SyncStatus {
    pub lesson_count: i32,
    pub exam_count: i32,
    pub homework_count: i32,
    pub last_ok_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

pub async fn sync_status(pool: &PgPool, account: Uuid) -> Result<Option<SyncStatus>> {
    let row = sqlx::query(
        "select lesson_count, exam_count, homework_count, last_ok_at, last_error
           from sync_state where untis_account_id = $1",
    )
    .bind(account)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| SyncStatus {
        lesson_count: r.get("lesson_count"),
        exam_count: r.try_get("exam_count").unwrap_or(0),
        homework_count: r.try_get("homework_count").unwrap_or(0),
        last_ok_at: r.try_get("last_ok_at").ok().flatten(),
        last_error: r.try_get("last_error").ok().flatten(),
    }))
}

/// Everything this service holdeth about one person, save the two things it
/// must not hand back: the sealed password, which is theirs already, and the
/// Google refresh token, which is Google's to reissue.
pub async fn export_for(pool: &PgPool, user_id: Uuid) -> Result<serde_json::Value> {
    let mut schools = Vec::new();
    for account in accounts_of(pool, user_id).await? {
        let feeds = feeds_of(pool, account.id).await?;
        let lessons = sqlx::query(
            "select lessons, lesson_count, window_start, window_end, last_ok_at
               from sync_state where untis_account_id = $1",
        )
        .bind(account.id)
        .fetch_optional(pool)
        .await?;

        schools.push(serde_json::json!({
            "server": account.server,
            "school": account.school,
            "username": account.username,
            "display_name": account.display_name,
            "timezone": account.timezone.name(),
            "enabled": account.enabled,
            "credentials_rejected": account.credentials_rejected,
            "feeds": feeds.iter().map(|feed| serde_json::json!({
                "token": feed.token,
                "keep_cancelled": feed.keep_cancelled,
                "refresh_minutes": feed.refresh_minutes,
            })).collect::<Vec<_>>(),
            "timetable": lessons.as_ref().map(|row| serde_json::json!({
                "lesson_count": row.try_get::<i32, _>("lesson_count").unwrap_or_default(),
                "window_start": row.try_get::<Option<NaiveDate>, _>("window_start").ok().flatten(),
                "window_end": row.try_get::<Option<NaiveDate>, _>("window_end").ok().flatten(),
                "refreshed_at": row.try_get::<Option<DateTime<Utc>>, _>("last_ok_at").ok().flatten(),
                "lessons": row.try_get::<serde_json::Value, _>("lessons")
                    .unwrap_or(serde_json::Value::Null),
            })),
        }));
    }

    Ok(serde_json::json!({
        "exported_at": Utc::now(),
        "note": "Your WebUntis password is not here: it is sealed, and it is yours already. \
                 Nor is any Google token, which Google alone can reissue.",
        "schools": schools,
    }))
}

// ------------------------------------------------------------- rotation ---

/// One row's sealed secret, and where it liveth.
struct Rotatable {
    id: Uuid,
    sealed: Sealed,
}

/// Re-seal everything the retiring key holdeth under the new one.
///
/// Row by row, each in its own statement: a rotation interrupted half way
/// leaveth every row at one version or the other, never in between, and
/// running it again finisheth the job rather than starting it afresh.
pub async fn rotate_key(pool: &PgPool, old: &Sealer, new: &Sealer) -> Result<(usize, usize)> {
    let mut done = 0;
    let mut stuck = 0;

    // The school passwords.
    let rows = sqlx::query(
        "select id, secret, nonce from untis_accounts where key_version <> $1 order by created_at",
    )
    .bind(new.version())
    .fetch_all(pool)
    .await
    .context("listing the accounts to rotate")?;

    for row in &rows {
        let one = Rotatable {
            id: row.get("id"),
            sealed: Sealed { ciphertext: row.get("secret"), nonce: row.get("nonce") },
        };
        match old.unseal(&one.sealed).and_then(|plain| new.seal(&plain)) {
            Ok(sealed) => {
                sqlx::query(
                    "update untis_accounts set secret = $2, nonce = $3, key_version = $4
                      where id = $1",
                )
                .bind(one.id)
                .bind(&sealed.ciphertext)
                .bind(&sealed.nonce)
                .bind(new.version())
                .execute(pool)
                .await
                .context("re-sealing an account")?;
                done += 1;
            }
            // A row the retiring key cannot open is left exactly as it was,
            // and named, rather than quietly lost.
            Err(err) => {
                tracing::error!(account = %one.id, "cannot re-seal: {err:#}");
                stuck += 1;
            }
        }
    }

    // The Google refresh tokens, sealed the same way.
    let rows = sqlx::query(
        "select untis_account_id, refresh_secret, refresh_nonce from google_links
          where key_version <> $1",
    )
    .bind(new.version())
    .fetch_all(pool)
    .await
    .context("listing the Google links to rotate")?;

    for row in &rows {
        let id: Uuid = row.get("untis_account_id");
        let sealed =
            Sealed { ciphertext: row.get("refresh_secret"), nonce: row.get("refresh_nonce") };
        match old.unseal(&sealed).and_then(|plain| new.seal(&plain)) {
            Ok(fresh) => {
                sqlx::query(
                    "update google_links
                        set refresh_secret = $2, refresh_nonce = $3, key_version = $4
                      where untis_account_id = $1",
                )
                .bind(id)
                .bind(&fresh.ciphertext)
                .bind(&fresh.nonce)
                .bind(new.version())
                .execute(pool)
                .await
                .context("re-sealing a Google link")?;
                done += 1;
            }
            Err(err) => {
                tracing::error!(account = %id, "cannot re-seal the Google link: {err:#}");
                stuck += 1;
            }
        }
    }
    Ok((done, stuck))
}

/// Deleting by owner as well as by id, as everywhere else here.
pub async fn delete_feed(pool: &PgPool, user_id: Uuid, feed: Uuid) -> Result<u64> {
    let done = sqlx::query(
        "delete from feeds f using untis_accounts a
          where f.id = $1 and f.untis_account_id = a.id and a.user_id = $2",
    )
    .bind(feed)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

/// A fresh address for a link, keeping everything else about it. The old
/// address stops working the moment this returneth.
pub async fn rotate_token(pool: &PgPool, user_id: Uuid, feed: Uuid, token: &str) -> Result<bool> {
    let done = sqlx::query(
        "update feeds f set token = $3
           from untis_accounts a
          where f.id = $1 and f.untis_account_id = a.id and a.user_id = $2",
    )
    .bind(feed)
    .bind(user_id)
    .bind(token)
    .execute(pool)
    .await
    .context("rotating the address")?;
    Ok(done.rows_affected() > 0)
}
