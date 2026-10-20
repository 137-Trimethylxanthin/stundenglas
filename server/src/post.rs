//! Telling somebody something, once.
//!
//! The service can run without any of this: with no `SMTP_URL` configured
//! nothing is sent, and the pages say everything they said before. What email
//! adds is the case where nobody is looking at the page — a password the
//! school has refused, which stops the timetable until its owner acts.

use std::net::IpAddr;

use anyhow::{Context, Result};
use lettre::message::header::ContentType;
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::{AsyncTransport, Message, Tokio1Executor};
use sqlx::PgPool;
use uuid::Uuid;

use crate::AppState;
use crate::auth::Token;
use crate::words::Lang;

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

/// What the mail saith in the subject, bare; the sender prefixeth the name.
pub fn subject(kind: Token, lang: Lang) -> &'static str {
    match (kind, lang) {
        (Token::Confirm, Lang::De) => "Bestätige deine Adresse",
        (Token::Confirm, Lang::En) => "Confirm your address",
        (Token::Reset, Lang::De) => "Neues Passwort",
        (Token::Reset, Lang::En) => "A new password",
    }
}

/// The mail's words, with the link in them. Each saith that the link lasteth
/// an hour, and that ignoring the mail changeth nothing.
pub fn body(kind: Token, lang: Lang, link: &str) -> String {
    match (kind, lang) {
        (Token::Confirm, Lang::De) => format!(
            "Jemand, hoffentlich du, hat sich mit dieser Adresse bei stundenglas angemeldet.\n\n\
             Um fertig zu werden, öffne diesen Link und wähle dort dein Passwort \
             (er gilt eine Stunde):\n{link}\n\n\
             Warst du das nicht, ignoriere diese Nachricht. Ohne den Link passiert nichts.\n"
        ),
        (Token::Confirm, Lang::En) => format!(
            "Someone, hopefully you, signed up at stundenglas with this address.\n\n\
             To finish, open this link and choose your password there \
             (it is valid for one hour):\n{link}\n\n\
             If it was not you, ignore this message. Nothing happens without the link.\n"
        ),
        (Token::Reset, Lang::De) => format!(
            "Du (hoffentlich) hast darum gebeten, ein neues Passwort zu wählen.\n\n\
             Öffne den Link, um es zu wählen (er gilt eine Stunde):\n{link}\n\n\
             Hast du nicht darum gebeten, ignoriere diese Nachricht; dein Passwort \
             bleibt, wie es ist.\n"
        ),
        (Token::Reset, Lang::En) => format!(
            "You (hopefully) asked to choose a new password.\n\n\
             Open the link to choose it (it is valid for one hour):\n{link}\n\n\
             If you did not ask, ignore this message and your password stays as it is.\n"
        ),
    }
}

/// Where the mail sendeth them: the public address, the page, the token
/// percent-encoded so that nothing in it may be mistaken for the URL's own marks.
pub fn link_for(public_url: &str, kind: Token, token_hash: &str) -> String {
    let page = match kind {
        Token::Confirm => "confirm",
        Token::Reset => "reset",
    };
    let mut encoded = String::with_capacity(token_hash.len());
    for byte in token_hash.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("{}/{page}?t={encoded}", public_url.trim_end_matches('/'))
}

/// How much hath been sent lately, to this address, this network, and in all.
pub struct Counts {
    pub address_minute: i64,
    pub address_day: i64,
    pub ip_hour: i64,
    pub all_day: i64,
}

/// One mail a minute and five a day to an address, ten an hour to a network,
/// a hundred and fifty a day in all: the relay's goodwill is not endless.
pub fn within_limits(c: &Counts) -> bool {
    c.address_minute < 1 && c.address_day < 5 && c.ip_hour < 10 && c.all_day < 150
}

/// Whether a new account may be made: the same network and service-wide
/// limits as the mail it will be sent, for an account no mail can follow is
/// one that only the sweep will ever see. The address hath no count yet.
pub fn room_for_account(c: &Counts) -> bool {
    c.ip_hour < 10 && c.all_day < 150
}

/// May one more account be made for this caller? Read only: the mail that
/// followeth taketh its own place, under the lock, as any other.
pub async fn may_create_account(pool: &PgPool, ip: Option<IpAddr>) -> Result<bool> {
    let seen = counts(pool, "", ip.map(|ip| bucket(ip).to_string())).await?;
    Ok(room_for_account(&seen))
}

/// What hath been sent in the last day, to this address and this network.
async fn counts<'e, E: sqlx::PgExecutor<'e>>(
    ex: E,
    email: &str,
    ip: Option<String>,
) -> Result<Counts> {
    let (address_minute, address_day, ip_hour, all_day): (i64, i64, i64, i64) = sqlx::query_as(
        "select
            count(*) filter (where address = $1 and sent_at > now() - interval '1 minute'),
            count(*) filter (where address = $1),
            count(*) filter (where $2::inet is not null and ip = $2::inet
                               and sent_at > now() - interval '1 hour'),
            count(*)
           from mail_sends
          where sent_at > now() - interval '1 day'",
    )
    .bind(email)
    .bind(&ip)
    .fetch_one(ex)
    .await
    .context("counting recent mail")?;
    Ok(Counts { address_minute, address_day, ip_hour, all_day })
}

fn mail_kind(kind: Token) -> &'static str {
    match kind {
        Token::Confirm => "confirm",
        Token::Reset => "reset",
    }
}

/// One network, one bucket: an IPv6 visitor commandeth a whole /64, so the
/// low half is dropped; an IPv4 address stayeth as it is.
fn bucket(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => IpAddr::V6((u128::from(v6) & (u128::MAX << 64)).into()),
    }
}

/// The one lock that maketh counting and reserving a single act.
const MAIL_LOCK: i64 = 0x5347_4d41_494c;

/// Count what hath been sent and, if the limits allow, write down this mail
/// beforehand, all under one lock so that no two requests count alike.
/// None when a limit is reached.
async fn reserve(
    pool: &PgPool,
    kind: Token,
    email: &str,
    ip: Option<IpAddr>,
) -> Result<Option<i64>> {
    let ip = ip.map(|ip| bucket(ip).to_string());
    let mut tx = pool.begin().await.context("beginning the count")?;
    sqlx::query("select pg_advisory_xact_lock($1)")
        .bind(MAIL_LOCK)
        .execute(&mut *tx)
        .await
        .context("taking the mail lock")?;
    let seen = counts(&mut *tx, email, ip.clone()).await?;
    if !within_limits(&seen) {
        if seen.all_day >= 150 {
            tracing::warn!(sent_today = seen.all_day, "the daily mail limit is reached");
        }
        return Ok(None);
    }
    let (id,): (i64,) = sqlx::query_as(
        "insert into mail_sends (address, ip, kind) values ($1, $2::inet, $3) returning id",
    )
    .bind(email)
    .bind(&ip)
    .bind(mail_kind(kind))
    .fetch_one(&mut *tx)
    .await
    .context("reserving the mail")?;
    tx.commit().await.context("committing the reservation")?;
    Ok(Some(id))
}

/// Give back a reservation whose mail never left. Best effort.
async fn release(pool: &PgPool, id: i64) {
    if let Err(err) =
        sqlx::query("delete from mail_sends where id = $1").bind(id).execute(pool).await
    {
        tracing::warn!(id, "could not release an unsent mail's place: {err:#}");
    }
}

/// Mint a token for this address and mail the link. False when nothing went:
/// no mailer, a limit reached, or no such account (which the caller must not
/// let the visitor tell apart from success).
pub async fn send_token_mail(
    state: &AppState,
    kind: Token,
    email: &str,
    ip: Option<IpAddr>,
    lang: Lang,
) -> Result<bool> {
    let Some(mailer) = &state.mail else { return Ok(false) };
    let email = crate::auth::normalise_address(email);
    let email = email.as_str();

    // The place is taken before anything is sent, so that requests which come
    // together cannot all pass the count; it is given back if the mail fails.
    let Some(id) = reserve(&state.pool, kind, email, ip).await? else { return Ok(false) };

    match mint_and_send(state, mailer, kind, email, lang).await {
        Ok(true) => Ok(true),
        Ok(false) => {
            release(&state.pool, id).await;
            Ok(false)
        }
        Err(err) => {
            release(&state.pool, id).await;
            Err(err)
        }
    }
}

async fn mint_and_send(
    state: &AppState,
    mailer: &Mailer,
    kind: Token,
    email: &str,
    lang: Lang,
) -> Result<bool> {
    let Some(hash) = crate::auth::mint(state, kind, email).await? else { return Ok(false) };
    let link = link_for(&state.config.public_url, kind, &hash);

    let message = Message::builder()
        .from(mailer.from.parse().context("MAIL_FROM is not an address")?)
        .to(email.parse().context("that address is not usable")?)
        .subject(format!("stundenglas — {}", subject(kind, lang)))
        .header(ContentType::TEXT_PLAIN)
        .body(body(kind, lang, &link))
        .context("composing the message")?;

    mailer.transport.send(message).await.context("sending the message")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{Counts, Notice, body, bucket, link_for, room_for_account, subject, within_limits};
    use crate::auth::Token;
    use crate::words::Lang;

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

    #[test]
    fn bodies_carry_the_link_in_both_tongues() {
        let link = "https://example.test/confirm?t=abc";
        for kind in [Token::Confirm, Token::Reset] {
            for lang in [Lang::De, Lang::En] {
                assert!(body(kind, lang, link).contains(link));
                assert!(!subject(kind, lang).is_empty());
            }
        }
        assert!(body(Token::Confirm, Lang::De, "x").contains("eine Stunde"));
        assert!(body(Token::Confirm, Lang::En, "x").contains("one hour"));
    }

    #[test]
    fn link_for_joins_cleanly() {
        assert_eq!(
            link_for("https://untis.maxi.co.at/", Token::Reset, "a b"),
            "https://untis.maxi.co.at/reset?t=a%20b"
        );
        assert_eq!(
            link_for("https://x.test", Token::Confirm, "a+b/c=d-e_f"),
            "https://x.test/confirm?t=a%2Bb%2Fc%3Dd-e_f"
        );
    }

    #[test]
    fn within_limits_edges() {
        let c = |address_minute, address_day, ip_hour, all_day| Counts {
            address_minute,
            address_day,
            ip_hour,
            all_day,
        };
        assert!(within_limits(&c(0, 0, 0, 0)));
        assert!(!within_limits(&c(1, 0, 0, 0)));
        assert!(!within_limits(&c(0, 5, 0, 0)));
        assert!(!within_limits(&c(0, 0, 10, 0)));
        assert!(!within_limits(&c(0, 0, 0, 150)));
        assert!(within_limits(&c(0, 4, 9, 149)));
    }

    #[test]
    fn a_new_account_heeds_the_network_and_the_service_but_not_the_address() {
        let c = |address_minute, address_day, ip_hour, all_day| Counts {
            address_minute,
            address_day,
            ip_hour,
            all_day,
        };
        assert!(room_for_account(&c(0, 0, 9, 149)));
        assert!(!room_for_account(&c(0, 0, 10, 0)), "a network over its hour maketh none");
        assert!(!room_for_account(&c(0, 0, 0, 150)), "nor doth a service over its day");
        assert!(room_for_account(&c(1, 5, 0, 0)), "a new address hath no count of its own");
    }

    #[test]
    fn an_ipv6_network_is_one_bucket_and_ipv4_hosts_are_not() {
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        assert_eq!(bucket(ip("2001:db8:1:2:3:4:5:6")), bucket(ip("2001:db8:1:2:ffff::1")));
        assert_ne!(bucket(ip("2001:db8:1:2::1")), bucket(ip("2001:db8:1:3::1")));
        assert_ne!(bucket(ip("192.0.2.1")), bucket(ip("192.0.2.2")));
        assert_eq!(bucket(ip("2001:db8:1:2:3:4:5:6")), ip("2001:db8:1:2::"));
    }
}
