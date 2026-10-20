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
    pub banned_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// The test of whether one is in, for a query that joins profiles as `p`.
pub const ADMITTED_SQL: &str = "p.approved and p.banned_at is null";

/// One is in when approved and not banned; a ban outweigheth an approval.
pub fn is_in(p: &Profile) -> bool {
    p.approved && p.banned_at.is_none()
}

fn profile_from(row: &sqlx::postgres::PgRow) -> Profile {
    Profile {
        user_id: row.get("user_id"),
        email: row.try_get::<Option<String>, _>("email").ok().flatten().unwrap_or_default(),
        approved: row.get("approved"),
        is_admin: row.get("is_admin"),
        banned_at: row.get("banned_at"),
        created_at: row.get("created_at"),
    }
}

/// Called the moment an account is made, and at every sign-in. An address
/// named in ADMIN_EMAILS is admitted at once and may admit others; everyone
/// else waits. An address once banned is banned again from the first moment,
/// though the account that earned the ban be long deleted.
pub async fn on_signup(
    pool: &PgPool,
    user: Uuid,
    email: &str,
    admin_emails: &[String],
) -> Result<Profile> {
    let privileged = admin_emails.iter().any(|a| a.eq_ignore_ascii_case(email));
    let row = sqlx::query(
        "insert into profiles (user_id, email, approved, is_admin, approved_at, banned_at)
         values ($1, $2, $3, $3, case when $3 then now() else null end,
                 case when $3 then null
                      else (select b.banned_at from banned_addresses b
                             where b.address = lower(btrim($2)))
                 end)
         on conflict (user_id) do update
            set email = excluded.email,
                is_admin = profiles.is_admin or excluded.is_admin,
                approved = profiles.approved or excluded.approved,
                banned_at = coalesce(
                    profiles.banned_at,
                    case when profiles.is_admin then null else excluded.banned_at end)
         returning user_id, email, approved, is_admin, banned_at, created_at",
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
        "select user_id, email, approved, is_admin, banned_at, created_at from profiles where user_id = $1",
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
        "select user_id, email, approved, is_admin, banned_at, created_at from profiles
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

/// Let one in, and leave any ban as it lay.
pub async fn admit(pool: &PgPool, who: Uuid) -> Result<()> {
    sqlx::query("update profiles set approved = true, approved_at = now() where user_id = $1")
        .bind(who)
        .execute(pool)
        .await?;
    Ok(())
}

/// Bar one, or lift the bar. A ban also shutteth every open session, and is
/// written against the address too, that deleting the account and making it
/// anew may not shake it off; lifting it forgetteth the address. An admin may
/// not be barred, and this doth nothing to one.
pub async fn set_banned(pool: &PgPool, who: Uuid, banned: bool) -> Result<()> {
    let touched: i64 = sqlx::query_scalar(
        "with done as (
             update profiles
                set banned_at = case when $2 then now() else null end
              where user_id = $1 and not is_admin
             returning email, banned_at
         ), marked as (
             insert into banned_addresses (address, banned_at)
             select lower(btrim(email)), banned_at from done
              where $2 and coalesce(btrim(email), '') <> ''
             on conflict (address) do update set banned_at = excluded.banned_at
             returning 1
         ), cleared as (
             delete from banned_addresses b
              using done
              where not $2 and b.address = lower(btrim(done.email))
             returning 1
         )
         select count(*) from done",
    )
    .bind(who)
    .bind(banned)
    .fetch_one(pool)
    .await
    .context("recording the ban")?;
    if banned
        && touched > 0
        && let Err(err) = crate::auth::close_all_sessions(pool, who).await
    {
        // The ban is recorded, and every request asketh afresh whether one is
        // in; a session left open is shut at its next knock.
        tracing::warn!(user = %who, "banned, but the sessions were not closed: {err:#}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(approved: bool, banned: bool) -> Profile {
        Profile {
            user_id: Uuid::nil(),
            email: String::new(),
            approved,
            is_admin: false,
            banned_at: banned.then(Utc::now),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn is_in_needs_approval_and_no_ban() {
        assert!(is_in(&profile(true, false)));
        assert!(!is_in(&profile(true, true)));
        assert!(!is_in(&profile(false, false)));
    }
}
