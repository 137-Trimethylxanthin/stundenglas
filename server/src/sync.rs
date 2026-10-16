//! Fetcheth every account's timetable on a turn of the glass, and layeth it
//! in the cache the feeds are rendered from.

use anyhow::Result;
use chrono::{Duration as Days, Local, NaiveDate};
use sha2::{Digest, Sha256};
use stundenglas_core::{Timetable, untis};

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
        remind_the_refused(&state).await;
    }
}

/// A refused link is never synced, so nothing would otherwise reach its
/// Google calendar: the notice goeth up there with the timetable last
/// fetched, and only that — a push of the notice alone would empty the
/// calendar of everything else.
async fn remind_the_refused(state: &AppState) {
    let refused = match crate::db::refused_to_remind(&state.pool).await {
        Ok(found) => found,
        Err(err) => {
            tracing::warn!("could not look for refused links: {err:#}");
            return;
        }
    };
    for (account, mut cached, window) in refused {
        cached.relogin = Some(crate::relogin::notice(
            &state.config.public_url,
            account.id,
            account.timezone,
            chrono::Utc::now(),
        ));
        let id = account.id;
        match crate::google::push(state.clone(), account, cached, window).await {
            Ok(_) => tracing::info!(account = %id, "relogin notice pushed to Google"),
            Err(err) => tracing::warn!(account = %id, "relogin notice push failed: {err:#}"),
        }
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
            let streak = entry.no_token_streak;
            match fetch_one(state.clone(), entry).await {
                Ok(count) => {
                    tracing::info!(account = %id, lessons = count, "refreshed");
                    true
                }
                Err(err) => {
                    // A refused password and an unreachable school want
                    // opposite answers: one must stop before the school locks
                    // the account, the other should be tried again shortly.
                    let kind = judge(&err, streak);
                    let why = match kind {
                        Failure::Rejected if !untis::was_rejected(&err) => format!(
                            "{err:#} — {NO_TOKEN_LIMIT} times in a row, so taken for a refused \
                             password"
                        ),
                        _ => format!("{err:#}"),
                    };
                    if matches!(kind, Failure::Rejected) {
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
                    } else {
                        tracing::warn!(account = %id, "refresh failed: {err:#}");
                    }
                    let _ = crate::db::store_failure(&state.pool, id, why, kind).await;
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

/// How many logins in a row may yield no token before it is taken for a
/// refused password. With the backoff that is about an hour and a quarter:
/// long enough to outlast a school's hiccup, short enough to stop well before
/// WebUntis would lock the account.
const NO_TOKEN_LIMIT: i32 = 4;

/// What a failed refresh is, given how many tokenless logins came before it.
///
/// A refused password and an unreachable school want opposite answers: one
/// must stop before the school locks the account, the other should be tried
/// again shortly. A login that yieldeth nothing, again and again, is the
/// former in disguise.
fn judge(err: &anyhow::Error, streak_before: i32) -> Failure {
    if untis::was_rejected(err) {
        Failure::Rejected
    } else if untis::got_no_token(err) {
        if streak_before + 1 >= NO_TOKEN_LIMIT { Failure::Rejected } else { Failure::Tokenless }
    } else {
        Failure::Transient
    }
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

    let got = Timetable { lessons, exams, homework, holidays, relogin: None };
    let etag = fingerprint(&got);
    crate::db::store_sync(&state.pool, entry.account.id, &got, etag, (from, to)).await?;

    // Those who asked for it get the same timetable written into Google, so
    // they need not wait for Google to look at the subscribed link.
    match crate::google::push(state.clone(), entry.account.clone(), got.clone(), (from, to)).await {
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
fn fingerprint(got: &Timetable) -> String {
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
        // A room or a note changed is rendered too, and must not answer 304.
        hasher.update(exam.description().as_bytes());
        hasher.update([0]);
        hasher.update(exam.rooms.join(",").as_bytes());
        hasher.update([0]);
        hasher.update(exam.start.to_rfc3339().as_bytes());
        hasher.update(exam.end.to_rfc3339().as_bytes());
        hasher.update(*b"\n");
    }
    for piece in &got.homework {
        hasher.update(piece.event_id().as_bytes());
        hasher.update([0]);
        hasher.update(piece.title().as_bytes());
        hasher.update([0]);
        hasher.update(piece.description().as_bytes());
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
    fn a_login_that_keeps_yielding_nothing_is_in_the_end_a_refusal() {
        let tokenless = || anyhow::Error::new(untis::NoToken("no usable bearer token".into()));
        assert!(matches!(judge(&tokenless(), 0), Failure::Tokenless), "once is a hiccup");
        assert!(matches!(judge(&tokenless(), NO_TOKEN_LIMIT - 2), Failure::Tokenless));
        assert!(
            matches!(judge(&tokenless(), NO_TOKEN_LIMIT - 1), Failure::Rejected),
            "the last allowed is taken for a refusal"
        );
        let outage = anyhow::anyhow!("connection timed out").context("reaching WebUntis");
        assert!(
            matches!(judge(&outage, NO_TOKEN_LIMIT + 5), Failure::Transient),
            "an unreachable school is never a refused password"
        );
        let refused = anyhow::Error::new(untis::Rejected("sent back to the login page".into()));
        assert!(matches!(judge(&refused, 0), Failure::Rejected));
    }

    #[test]
    fn a_room_changed_for_an_exam_is_a_changed_feed() {
        let exam = |room: &str| stundenglas_core::Exam {
            id: 7,
            start: chrono::Utc::now().fixed_offset(),
            end: chrono::Utc::now().fixed_offset(),
            subject: "M".into(),
            kind: "SA".into(),
            name: String::new(),
            text: String::new(),
            teachers: vec![],
            rooms: vec![room.into()],
        };
        let before = Timetable { exams: vec![exam("A1")], ..Default::default() };
        let after = Timetable { exams: vec![exam("B2")], ..Default::default() };
        assert_ne!(fingerprint(&before), fingerprint(&after));
    }

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
