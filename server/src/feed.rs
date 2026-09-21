//! The public face of the service: a secret URL any calendar may subscribe to.

use axum::extract::{Path, State};
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
    headers: HeaderMap,
) -> Response {
    let Some(token) = file.strip_suffix(".ics") else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    if !token_is_plausible(token) {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    let payload = match crate::db::feed_by_token(&state.pool, token).await {
        Ok(Some(found)) => found,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such calendar").into_response(),
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

    let etag = payload.etag.clone().unwrap_or_else(|| "empty".to_owned());
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
    };
    let body = feed.render(&payload.lessons, payload.fetched_at.unwrap_or_else(chrono::Utc::now));

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
