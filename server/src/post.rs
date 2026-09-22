//! Telling somebody something, once.
//!
//! The service can run without any of this: with no `SMTP_URL` configured
//! nothing is sent, and the pages say everything they said before. What email
//! adds is the case where nobody is looking at the page — a password the
//! school has refused, which stops the timetable until its owner acts.

use anyhow::{Context, Result};
use lettre::message::header::ContentType;
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::{AsyncTransport, Message, Tokio1Executor};
use sqlx::PgPool;
use uuid::Uuid;

use crate::AppState;

/// What a notice is about, and how often it may be repeated.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// An administrator let them in.
    Admitted,
    /// The school refused the stored password; nothing syncs until it changes.
    PasswordRefused,
    /// A school link has been failing for a day or more.
    Stalled,
}

impl Notice {
    fn kind(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::PasswordRefused => "password-refused",
            Self::Stalled => "stalled",
        }
    }

    /// How long before the same thing may be said again. A refused password is
    /// said once and not repeated until it is fixed and refused afresh; a
    /// stalled link is worth a reminder after a week, not a day.
    fn again_after_days(self) -> i64 {
        match self {
            Self::Admitted => 3650,
            Self::PasswordRefused => 30,
            Self::Stalled => 7,
        }
    }

    fn subject(self) -> &'static str {
        match self {
            Self::Admitted => "You are in",
            Self::PasswordRefused => "Your school password no longer works",
            Self::Stalled => "Your timetable has stopped refreshing",
        }
    }

    fn body(self, where_at: &str, about: &str) -> String {
        match self {
            Self::Admitted => format!(
                "An administrator has let you in.\n\n\
                 Add your school and copy the calendar link:\n{where_at}\n"
            ),
            Self::PasswordRefused => format!(
                "WebUntis refused the password stored for {about}, so your timetable \
                 has stopped refreshing.\n\n\
                 Nothing more will be tried until you enter it again — repeating a \
                 refused password is how a WebUntis account gets locked.\n\n\
                 Enter it here:\n{where_at}\n\n\
                 Your calendar link itself is unchanged, and whatever it last held is \
                 still there.\n"
            ),
            Self::Stalled => format!(
                "Your timetable for {about} has not refreshed for a day or more.\n\n\
                 This is usually the school's server being unreachable, and it mends \
                 itself. If it does not, the page says what went wrong:\n{where_at}\n"
            ),
        }
    }
}

/// Send unless this was already said. True if something went out.
///
/// The remembering happens whether or not there is anywhere to send: a service
/// that later gains an SMTP server should not then announce a month of old
/// news all at once.
pub async fn tell(state: &AppState, who: Uuid, what: Notice, about: Option<Uuid>) -> Result<bool> {
    let about = about.unwrap_or(Uuid::nil());
    if !claim(&state.pool, who, what, about).await? {
        return Ok(false);
    }

    let Some(mailer) = &state.mail else { return Ok(false) };
    let Some(address) = crate::people::profile(&state.pool, who).await.map(|p| p.email) else {
        return Ok(false);
    };
    if address.is_empty() {
        return Ok(false);
    }

    let what_about = match about {
        id if id.is_nil() => "your account".to_owned(),
        id => crate::db::name_of(&state.pool, id)
            .await
            .unwrap_or(None)
            .unwrap_or_else(|| "your school".to_owned()),
    };

    let message = Message::builder()
        .from(mailer.from.parse().context("MAIL_FROM is not an address")?)
        .to(address.parse().context("that account has no usable address")?)
        .subject(format!("stundenglas — {}", what.subject()))
        .header(ContentType::TEXT_PLAIN)
        .body(what.body(&state.config.public_url, &what_about))
        .context("composing the message")?;

    mailer.transport.send(message).await.context("sending the message")?;
    Ok(true)
}

/// Write the notice down first. If the row is already there and recent enough,
/// this says so and nothing is sent.
async fn claim(pool: &PgPool, who: Uuid, what: Notice, about: Uuid) -> Result<bool> {
    let done = sqlx::query(
        "insert into notices (user_id, kind, about) values ($1, $2, $3)
         on conflict (user_id, kind, about) do update set sent_at = now()
          where notices.sent_at < now() - make_interval(days => $4::int)",
    )
    .bind(who)
    .bind(what.kind())
    .bind(about)
    .bind(what.again_after_days() as i32)
    .execute(pool)
    .await
    .context("noting what was said")?;
    Ok(done.rows_affected() > 0)
}

/// Forget that something was said, so it may be said again when it recurs.
pub async fn forget(pool: &PgPool, who: Uuid, what: Notice, about: Uuid) {
    let _ = sqlx::query("delete from notices where user_id = $1 and kind = $2 and about = $3")
        .bind(who)
        .bind(what.kind())
        .bind(about)
        .execute(pool)
        .await;
}

/// The configured way out. Absent, and nothing is sent.
pub struct Mailer {
    pub transport: AsyncSmtpTransport<Tokio1Executor>,
    pub from: String,
}

impl Mailer {
    /// From `SMTP_URL`, as lettre spells it — `smtps://user:pass@host:465`.
    /// Absent, this is None and the service says so once at startup.
    pub fn from_env() -> Result<Option<Self>> {
        let Ok(url) = std::env::var("SMTP_URL") else { return Ok(None) };
        if url.is_empty() {
            return Ok(None);
        }
        let from = std::env::var("MAIL_FROM")
            .context("SMTP_URL is set but MAIL_FROM is not; give it an address to send from")?;

        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(&url)
            .context("SMTP_URL is not a usable smtp:// or smtps:// address")?
            .build();
        Ok(Some(Self { transport, from }))
    }
}

#[cfg(test)]
mod tests {
    use super::Notice;

    #[test]
    fn every_notice_says_where_to_go_and_what_it_is_about() {
        for what in [Notice::Admitted, Notice::PasswordRefused, Notice::Stalled] {
            let body = what.body("https://example.test", "BG Beispiel (anna)");
            assert!(
                body.contains("https://example.test"),
                "a notice with no way back is a dead end"
            );
            assert!(!what.subject().is_empty());
            assert!(what.again_after_days() > 0);
        }
    }

    #[test]
    fn a_refused_password_says_why_nothing_is_being_retried() {
        let body = Notice::PasswordRefused.body("https://example.test", "BG Beispiel (anna)");
        assert!(body.contains("BG Beispiel (anna)"), "which of their schools it is");
        assert!(body.contains("locked"), "the reason we stopped rather than kept trying");
    }

    #[test]
    fn the_kinds_are_distinct_or_one_would_silence_another() {
        let kinds =
            [Notice::Admitted.kind(), Notice::PasswordRefused.kind(), Notice::Stalled.kind()];
        let mut seen = kinds.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), kinds.len());
    }
}
