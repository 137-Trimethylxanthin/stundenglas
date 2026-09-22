//! Fetcheth every account's timetable on a turn of the glass, and layeth it
//! in the cache the feeds are rendered from.

use anyhow::Result;
use chrono::{Duration as Days, Local, NaiveDate};
use sha2::{Digest, Sha256};
use stundenglas_core::untis;

use crate::AppState;
use crate::db::Failure;

/// One WebUntis session lasteth half an hour, so each run logs in afresh
/// rather than holding sessions open for every account at once.
pub async fn run(state: AppState) {
    let mut ticker = tokio::time::interval(state.config.sync_every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        crate::auth::sweep(&state.pool).await;
        crate::mfa::sweep(&state).await;
        // A timetable nobody is refreshing is stale, and is still somebody's
        // whereabouts. Forget the lessons, keep the link.
        match crate::db::forget_stale(&state.pool, state.config.keep_days).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(count = n, "forgot stale timetables"),
            Err(err) => tracing::warn!("could not forget stale timetables: {err:#}"),
        }
        if let Err(err) = once(state.clone()).await {
            tracing::error!("sync round failed: {err:#}");
        }
        tell_the_stalled(&state).await;
    }
}

pub async fn once(state: AppState) -> Result<usize> {
    let accounts = crate::db::accounts_to_sync(&state.pool, &state.config.sealer).await?;
    if accounts.is_empty() {
        return Ok(0);
    }
    tracing::info!(count = accounts.len(), "refreshing");

    // One task per account, each holding only what it owns, with a permit to
    // keep the school's server from being hit by everyone at once. Each task
    // also fails on its own: one school being unreachable must not rob the
    // rest of their refresh.
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(state.config.sync_lanes));
    let mut tasks = tokio::task::JoinSet::new();
    for entry in accounts {
        let state = state.clone();
        let permits = permits.clone();
        tasks.spawn(async move {
            let _permit = permits.acquire_owned().await;
            let id = entry.account.id;
            match fetch_one(state.clone(), entry).await {
                Ok(count) => {
                    tracing::info!(account = %id, lessons = count, "refreshed");
                    true
                }
                Err(err) => {
                    // A refused password and an unreachable school want
                    // opposite answers: one must stop before the school locks
                    // the account, the other should be tried again shortly.
                    let kind = if untis::was_rejected(&err) {
                        tracing::warn!(account = %id, "login refused; waiting for a new password");
                        // Nobody is looking at the page when this happens, and
                        // nothing refreshes until they do something about it.
                        if let Some(owner) = crate::db::owner_of(&state.pool, id).await {
                            let _ = crate::post::tell(
                                &state,
                                owner,
                                crate::post::Notice::PasswordRefused,
                                Some(id),
                            )
                            .await;
                        }
                        Failure::Rejected
                    } else {
                        tracing::warn!(account = %id, "refresh failed: {err:#}");
                        Failure::Transient
                    };
                    let _ =
                        crate::db::store_failure(&state.pool, id, format!("{err:#}"), kind).await;
                    false
                }
            }
        });
    }

    let mut done = 0;
    while let Some(outcome) = tasks.join_next().await {
        if matches!(outcome, Ok(true)) {
            done += 1;
        }
    }
    Ok(done)
}

/// A link that has been failing for a day is worth a word. A refused password
/// has already had its own, and is left out here.
async fn tell_the_stalled(state: &AppState) {
    let stalled = match crate::db::stalled_since(&state.pool, 24).await {
        Ok(found) => found,
        Err(err) => {
            tracing::warn!("could not look for stalled links: {err:#}");
            return;
        }
    };
    for (account, owner) in stalled {
        let _ = crate::post::tell(state, owner, crate::post::Notice::Stalled, Some(account)).await;
    }
}

async fn fetch_one(state: AppState, entry: crate::db::AccountWithSecret) -> Result<usize> {
    let state = &state;
    let entry = &entry;
    let mut client = untis::Client::new(entry.credentials.clone())?;
    client.login().await?;

    crate::db::set_identity(
        &state.pool,
        entry.account.id,
        client.person_name.clone(),
        client.person_id,
    )
    .await?;

    let (from, to) = window(client.year);
    let mut lessons = client.fetch(from, to).await?;

    // An excused absence should leave the day empty, exactly as the CLI doth.
    for leave in client.fetch_absences(from, to).await.unwrap_or_default() {
        lessons.retain(|lesson| !leave.covers(lesson));
    }

    // Exams are a separate endpoint, and a school that keepeth none, or that
    // withholdeth them, must not cost the timetable its refresh.
    // Once per account per round, which is not noisy, and a tenant that
    // withholdeth exams for ever should not do so silently.
    let exams = client.fetch_exams(from, to).await.unwrap_or_else(|err| {
        tracing::warn!(account = %entry.account.id, "no exams read: {err:#}");
        Vec::new()
    });

    let homework = client.fetch_homework(from, to).await.unwrap_or_else(|err| {
        tracing::warn!(account = %entry.account.id, "no homework read: {err:#}");
        Vec::new()
    });

    // The school year's holidays change perhaps twice a year, but the call is
    // one request and keeps the cache honest without a second schedule.
    let holidays = client.fetch_holidays().await.unwrap_or_else(|err| {
        tracing::debug!(account = %entry.account.id, "no holidays read: {err:#}");
        Vec::new()
    });

    let got = crate::db::Fetched { lessons, exams, homework, holidays };
    let etag = fingerprint(&got);
    crate::db::store_sync(&state.pool, entry.account.id, &got, etag, (from, to)).await?;

    // Those who asked for it get the same timetable written into Google, so
    // they need not wait for Google to look at the subscribed link.
    match crate::google::push(state.clone(), entry.account.clone(), got.lessons.clone()).await {
        Ok(Some(tally)) => tracing::info!(
            account = %entry.account.id,
            inserted = tally.inserted, updated = tally.updated,
            deleted = tally.deleted, failed = tally.failed,
            "pushed to Google"
        ),
        Ok(None) => {}
        // A Google mishap must not lose the timetable we just stored.
        Err(err) => tracing::warn!(account = %entry.account.id, "Google push failed: {err:#}"),
    }
    Ok(got.lessons.len())
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
fn fingerprint(got: &crate::db::Fetched) -> String {
    let mut hasher = Sha256::new();
    for lesson in &got.lessons {
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
    for exam in &got.exams {
        hasher.update(exam.event_id().as_bytes());
        hasher.update([0]);
        hasher.update(exam.title().as_bytes());
        hasher.update([0]);
        hasher.update(exam.start.to_rfc3339().as_bytes());
        hasher.update(exam.end.to_rfc3339().as_bytes());
        hasher.update(*b"\n");
    }
    for piece in &got.homework {
        hasher.update(piece.event_id().as_bytes());
        hasher.update([0]);
        hasher.update(piece.title().as_bytes());
        hasher.update(piece.due.to_string().as_bytes());
        hasher.update(*b"\n");
    }
    for shut in &got.holidays {
        hasher.update(shut.event_id().as_bytes());
        hasher.update(shut.title().as_bytes());
        hasher.update(shut.start.to_string().as_bytes());
        hasher.update(shut.end.to_string().as_bytes());
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
