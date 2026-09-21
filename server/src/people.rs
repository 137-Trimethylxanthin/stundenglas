//! Who may use the service. Signing up is open; being let in is not.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Profile {
    pub user_id: Uuid,
    pub email: String,
    pub approved: bool,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
}

fn profile_from(row: &sqlx::postgres::PgRow) -> Profile {
    Profile {
        user_id: row.get("user_id"),
        email: row.try_get::<Option<String>, _>("email").ok().flatten().unwrap_or_default(),
        approved: row.get("approved"),
        is_admin: row.get("is_admin"),
        created_at: row.get("created_at"),
    }
}

/// Called the moment an account is made. An address named in ADMIN_EMAILS is
/// admitted at once and may admit others; everyone else waits.
pub async fn on_signup(
    pool: &PgPool,
    user: Uuid,
    email: &str,
    admin_emails: &[String],
) -> Result<Profile> {
    let privileged = admin_emails.iter().any(|a| a.eq_ignore_ascii_case(email));
    let row = sqlx::query(
        "insert into profiles (user_id, email, approved, is_admin, approved_at)
         values ($1, $2, $3, $3, case when $3 then now() else null end)
         on conflict (user_id) do update
            set email = excluded.email,
                is_admin = profiles.is_admin or excluded.is_admin,
                approved = profiles.approved or excluded.approved
         returning user_id, email, approved, is_admin, created_at",
    )
    .bind(user)
    .bind(email)
    .bind(privileged)
    .fetch_one(pool)
    .await
    .context("recording the new account")?;
    Ok(profile_from(&row))
}

pub async fn profile(pool: &PgPool, user: Uuid) -> Option<Profile> {
    let row = sqlx::query(
        "select user_id, email, approved, is_admin, created_at from profiles where user_id = $1",
    )
    .bind(user)
    .fetch_optional(pool)
    .await
    .ok()??;
    Some(profile_from(&row))
}

/// Those waiting first, then the rest by when they arrived.
pub async fn everyone(pool: &PgPool) -> Result<Vec<Profile>> {
    let rows = sqlx::query(
        "select user_id, email, approved, is_admin, created_at from profiles
          order by approved, created_at",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(profile_from).collect())
}

pub async fn set_approved(pool: &PgPool, who: Uuid, by: Uuid, approved: bool) -> Result<()> {
    sqlx::query(
        "update profiles
            set approved = $2,
                approved_at = case when $2 then now() else null end,
                approved_by = case when $2 then $3 else null end
          where user_id = $1
            -- an admin may not be shut out, not even by another admin
            and not is_admin",
    )
    .bind(who)
    .bind(approved)
    .bind(by)
    .execute(pool)
    .await?;
    Ok(())
}
