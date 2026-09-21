//! Fetcheth every account's timetable on a turn of the glass, and layeth it
//! in the cache the feeds are rendered from.

use anyhow::Result;
use chrono::{Duration as Days, Local, NaiveDate};
use futures::stream::{self, StreamExt};
use sha2::{Digest, Sha256};
use stundenglas_core::{Lesson, untis};

use crate::AppState;

/// One WebUntis session lasteth half an hour, so each run logs in afresh
/// rather than holding sessions open for every account at once.
pub async fn run(state: AppState) {
    let mut ticker = tokio::time::interval(state.config.sync_every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        if let Err(err) = once(&state).await {
            tracing::error!("sync round failed: {err:#}");
        }
    }
}

pub async fn once(state: &AppState) -> Result<usize> {
    let accounts = crate::db::accounts_to_sync(&state.pool, &state.config.sealer).await?;
    if accounts.is_empty() {
        return Ok(0);
    }
    tracing::info!(count = accounts.len(), "refreshing");

    let done = stream::iter(accounts)
        .map(|entry| async move {
            let id = entry.account.id;
            match fetch_one(state, &entry).await {
                Ok(count) => {
                    tracing::info!(account = %id, lessons = count, "refreshed");
                    true
                }
                Err(err) => {
                    tracing::warn!(account = %id, "refresh failed: {err:#}");
                    let _ = crate::db::store_failure(&state.pool, id, &format!("{err:#}")).await;
                    false
                }
            }
        })
        .buffer_unordered(state.config.sync_lanes)
        .filter(|ok| futures::future::ready(*ok))
        .count()
        .await;

    Ok(done)
}

async fn fetch_one(state: &AppState, entry: &crate::db::AccountWithSecret) -> Result<usize> {
    let mut client = untis::Client::new(entry.credentials.clone())?;
    client.login().await?;

    crate::db::set_identity(&state.pool, entry.account.id, &client.person_name, client.person_id)
        .await?;

    let (from, to) = window(client.year);
    let mut lessons = client.fetch(from, to).await?;

    // An excused absence should leave the day empty, exactly as the CLI doth.
    for leave in client.fetch_absences(from, to).await.unwrap_or_default() {
        lessons.retain(|lesson| !leave.covers(lesson));
    }

    let etag = fingerprint(&lessons);
    crate::db::store_sync(&state.pool, entry.account.id, &lessons, &etag, (from, to)).await?;
    Ok(lessons.len())
}

/// A week behind, and as far ahead as the school year reacheth.
fn window(year: (NaiveDate, NaiveDate)) -> (NaiveDate, NaiveDate) {
    let today = Local::now().date_naive();
    let from = (today - Days::days(7)).max(year.0);
    let to = year.1.max(today + Days::days(28));
    (from, to)
}

/// Changeth only when something a subscriber would notice changeth, so an
/// unchanged timetable answereth 304 and costeth nothing.
fn fingerprint(lessons: &[Lesson]) -> String {
    let mut hasher = Sha256::new();
    for lesson in lessons {
        hasher.update(lesson.event_id().as_bytes());
        hasher.update([0]);
        hasher.update(lesson.title().as_bytes());
        hasher.update([0]);
        hasher.update(lesson.description().as_bytes());
        hasher.update([0]);
        hasher.update(lesson.start.to_rfc3339().as_bytes());
        hasher.update(lesson.end.to_rfc3339().as_bytes());
        hasher.update(lesson.rooms.join(",").as_bytes());
        hasher.update(*b"\n");
    }
    hasher.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_never_starts_before_the_school_year() {
        let year = (Local::now().date_naive(), Local::now().date_naive() + Days::days(200));
        let (from, _) = window(year);
        assert!(from >= year.0, "we must not ask for days the year does not have");
    }

    #[test]
    fn the_window_reaches_the_end_of_the_year() {
        let today = Local::now().date_naive();
        let year = (today - Days::days(30), today + Days::days(200));
        let (from, to) = window(year);
        assert_eq!(from, today - Days::days(7));
        assert_eq!(to, year.1);
    }

    #[test]
    fn a_year_already_over_still_yields_a_forward_window() {
        let today = Local::now().date_naive();
        let year = (today - Days::days(400), today - Days::days(30));
        let (_, to) = window(year);
        assert!(to > today, "an ended year must not produce an empty window");
    }
}
