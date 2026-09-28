//! The public face of the service: a secret URL any calendar may subscribe to.

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use stundenglas_core::ics;

use crate::AppState;

/// `GET /cal/<token>.ics`
///
/// Answereth from the cache alone; a subscriber never waiteth on WebUntis, and
/// a hundred calendars polling costeth the school nothing.
pub async fn serve(
    State(state): State<AppState>,
    Path(file): Path<String>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let who = caller(&headers, peer);
    if state.guard.is_barred(who) {
        return (StatusCode::TOO_MANY_REQUESTS, "too many misses; wait a while").into_response();
    }
    let Some(token) = file.strip_suffix(".ics") else {
        state.guard.note_miss(who);
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    if !token_is_plausible(token) {
        state.guard.note_miss(who);
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    let payload = match crate::db::feed_by_token(&state.pool, token).await {
        Ok(Some(found)) => found,
        Ok(None) => {
            state.guard.note_miss(who);
            return (StatusCode::NOT_FOUND, "no such calendar").into_response();
        }
        Err(err) => {
            tracing::error!("feed lookup failed: {err:#}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "try again shortly").into_response();
        }
    };

    // Nothing fetched yet. Answer with an empty but valid calendar rather than
    // an error, so a client that subscribed a moment ago settles quietly.
    let name = payload
        .feed
        .display_name
        .clone()
        .map_or_else(|| "Stundenplan".to_owned(), |who| format!("Stundenplan {who}"));

    // The stored etag follows the school's timetable alone, but what we render
    // follows this link's own settings too. Fold them in, or a reminder just
    // switched on answers 304 for ever and is never seen.
    let etag = etag_for(&payload, &name);
    let quoted = format!("\"{etag}\"");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|sent| sent.split(',').any(|part| part.trim() == quoted))
    {
        crate::db::note_served(&state.pool, token).await;
        return StatusCode::NOT_MODIFIED.into_response();
    }

    let feed = ics::Feed {
        name: &name,
        refresh_minutes: payload.feed.refresh_minutes.max(5) as u32,
        keep_cancelled: payload.feed.keep_cancelled,
        with_homework: payload.feed.with_homework,
        with_holidays: payload.feed.with_holidays,
        hide_subjects: &payload.feed.hide_subjects,
        remind_before: payload.feed.remind_before_minutes.and_then(|m| u32::try_from(m).ok()),
    };
    let body = feed.render(&payload.what, payload.fetched_at.unwrap_or_else(chrono::Utc::now));

    crate::db::note_served(&state.pool, token).await;

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8".to_owned()),
            (header::ETAG, quoted),
            (header::CACHE_CONTROL, "private, max-age=300".to_owned()),
            (header::CONTENT_DISPOSITION, "inline; filename=\"stundenplan.ics\"".to_owned()),
        ],
        body,
    )
        .into_response()
}

/// Who is asking. Behind a proxy the peer is the proxy itself, so its own
/// header is preferred where there is one — without this, one busy edge node
/// would be barred on everybody's behalf.
///
/// The headers are only as honest as whoever set them; they are trusted here
/// because the alternative, in front of Cloudflare, is to count the whole
/// world as one caller.
fn caller(headers: &HeaderMap, peer: std::net::SocketAddr) -> std::net::IpAddr {
    for name in ["cf-connecting-ip", "x-real-ip"] {
        if let Some(found) = headers.get(name).and_then(|v| v.to_str().ok())
            && let Ok(address) = found.trim().parse()
        {
            return address;
        }
    }
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|chain| chain.split(',').next())
        .and_then(|first| first.trim().parse().ok())
        .unwrap_or_else(|| peer.ip())
}

/// The timetable's fingerprint and the link's settings together. Either one
/// changing must change the answer.
fn etag_for(payload: &crate::db::FeedPayload, name: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(payload.etag.as_deref().unwrap_or("empty").as_bytes());
    hasher.update([0]);
    hasher.update(name.as_bytes());
    hasher.update([0]);
    hasher.update([u8::from(payload.feed.keep_cancelled)]);
    hasher.update([u8::from(payload.feed.with_homework)]);
    hasher.update([u8::from(payload.feed.with_holidays)]);
    for subject in &payload.feed.hide_subjects {
        hasher.update(subject.as_bytes());
        hasher.update([0]);
    }
    hasher.update(payload.feed.refresh_minutes.to_le_bytes());
    hasher.update(payload.feed.remind_before_minutes.unwrap_or(-1).to_le_bytes());
    hasher.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Cheap gate before touching the database, so a flood of nonsense URLs
/// costeth us no queries.
pub(crate) fn token_is_plausible(token: &str) -> bool {
    (16..=128).contains(&token.len())
        && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::token_is_plausible;

    #[test]
    fn plausible_tokens_pass() {
        assert!(token_is_plausible("abcdefghijklmnop"));
        assert!(token_is_plausible("A-Za-z0-9_-0123456789abcdef"));
    }

    #[test]
    fn nonsense_is_turned_away_before_the_database() {
        assert!(!token_is_plausible(""), "empty");
        assert!(!token_is_plausible("short"), "too short to be a secret");
        assert!(!token_is_plausible(&"x".repeat(129)), "absurdly long");
        assert!(!token_is_plausible("has spaces here!"), "spaces");
        assert!(!token_is_plausible("../../etc/passwd"), "traversal");
        assert!(!token_is_plausible("abcdefghijklmnop';--"), "sql-ish");
    }
}

#[cfg(test)]
mod etag_tests {
    use crate::db::{Feed, FeedPayload};
    use uuid::Uuid;

    fn payload(keep_cancelled: bool, remind: Option<i32>) -> FeedPayload {
        FeedPayload {
            feed: Feed {
                id: Uuid::nil(),
                token: "tok".into(),
                keep_cancelled,
                refresh_minutes: 60,
                remind_before_minutes: remind,
                with_homework: true,
                with_holidays: true,
                hide_subjects: vec![],
                label: None,
                display_name: None,
            },
            what: Default::default(),
            etag: Some("abcd1234".into()),
            fetched_at: None,
        }
    }

    #[test]
    fn a_changed_setting_changes_the_answer() {
        let quiet = super::etag_for(&payload(true, None), "Stundenplan");
        let ringing = super::etag_for(&payload(true, Some(1440)), "Stundenplan");
        assert_ne!(quiet, ringing, "switching a reminder on must not answer 304 for ever");

        let without = super::etag_for(&payload(false, None), "Stundenplan");
        assert_ne!(quiet, without, "dropping cancelled hours changes what is rendered");

        let renamed = super::etag_for(&payload(true, None), "Stundenplan Anna");
        assert_ne!(quiet, renamed, "the calendar's name is in the body too");
    }

    #[test]
    fn an_unchanged_feed_keeps_its_fingerprint() {
        assert_eq!(
            super::etag_for(&payload(true, Some(60)), "Stundenplan"),
            super::etag_for(&payload(true, Some(60)), "Stundenplan"),
            "an unchanged feed must still cost a subscriber nothing"
        );
    }
}

#[cfg(test)]
mod caller_tests {
    use axum::http::HeaderMap;

    fn peer() -> std::net::SocketAddr {
        "10.0.0.5:9000".parse().unwrap()
    }

    #[test]
    fn without_a_proxy_the_peer_is_the_caller() {
        assert_eq!(super::caller(&HeaderMap::new(), peer()).to_string(), "10.0.0.5");
    }

    #[test]
    fn behind_cloudflare_the_header_wins() {
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", "203.0.113.9".parse().unwrap());
        assert_eq!(
            super::caller(&headers, peer()).to_string(),
            "203.0.113.9",
            "else one edge node would be barred on everybody's behalf"
        );
    }

    #[test]
    fn a_forwarded_chain_yields_its_first_hop() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.9, 70.41.3.18".parse().unwrap());
        assert_eq!(super::caller(&headers, peer()).to_string(), "203.0.113.9");
    }

    #[test]
    fn nonsense_in_a_header_falls_back_to_the_peer() {
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", "not-an-address".parse().unwrap());
        assert_eq!(super::caller(&headers, peer()).to_string(), "10.0.0.5");
    }
}
