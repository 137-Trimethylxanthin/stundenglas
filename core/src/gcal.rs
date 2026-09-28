//! Writeth only unto its own secondary calendar, and only unto its own events.

use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, SecondsFormat};
use chrono_tz::Tz;
use futures::stream::{self, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::untis::{Lesson, Status};

const API: &str = "https://www.googleapis.com/calendar/v3";
pub const CALENDAR_NAME: &str = "Schule (Untis)";
const MARKER: &str = "untis";
const ATTEMPTS: u32 = 4;

// Google's own palette, numbered one to eleven.
const COLOUR_CANCELLED: &str = "11";
const COLOUR_CHANGED: &str = "6";
const COLOUR_EVENT: &str = "5";
const COLOUR_EXAM: &str = "4";
const COLOUR_HOMEWORK: &str = "7";
const COLOUR_HOLIDAY: &str = "8";

#[derive(Debug, Clone)]
pub struct Desired {
    pub id: String,
    pub summary: String,
    pub start: crate::untis::Stamp,
    pub body: Value,
}

#[derive(Debug, Clone)]
pub struct Existing {
    pub id: String,
    pub summary: String,
    pub start: String,
    pub fingerprint: String,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub inserts: Vec<Desired>,
    pub updates: Vec<Desired>,
    pub deletes: Vec<Existing>,
    pub unchanged: usize,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.inserts.is_empty() && self.updates.is_empty() && self.deletes.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct Tally {
    pub inserted: usize,
    pub updated: usize,
    pub deleted: usize,
    pub failed: usize,
}

/// Cheap to clone: an HTTP client and a token. Cloning it per request lets
/// the concurrent writes below own what they need, rather than borrowing
/// `self` across a stream -- a borrow that makes the whole future's Send-ness
/// depend on a higher-ranked lifetime, and so unspawnable.
#[derive(Clone)]
pub struct Calendar {
    http: reqwest::Client,
    token: String,
}

/// The three public calls return boxed futures rather than being plain `async
/// fn`s. An `async fn` is generic over the lifetimes of its references, and a
/// caller that spawns the work cannot then be shown Send for *every* lifetime,
/// only for some -- the compiler's "not general enough". Boxing fixes the
/// lifetime here, where it is known, and the callers are free.
type Eventually<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

impl Calendar {
    pub fn new(http: reqwest::Client, token: String) -> Self {
        Self { http, token }
    }

    pub fn find_or_create<'a>(&'a self, name: &'a str, zone: Tz) -> Eventually<'a, String> {
        Box::pin(self.find_or_create_inner(name, zone))
    }

    async fn find_or_create_inner(&self, name: &str, zone: Tz) -> Result<String> {
        let mut page: Option<String> = None;
        loop {
            let mut query: Vec<(&str, String)> = vec![("maxResults", "250".to_owned())];
            if let Some(token) = &page {
                query.push(("pageToken", token.clone()));
            }
            let list: CalendarList =
                self.get(format!("{API}/users/me/calendarList"), &query).await?;
            if let Some(found) = list.items.iter().find(|c| c.summary.as_deref() == Some(name)) {
                return Ok(found.id.clone());
            }
            page = list.next_page_token;
            if page.is_none() {
                break;
            }
        }

        let made: CalendarRef = self
            .post(format!("{API}/calendars"), &json!({ "summary": name, "timeZone": zone.name() }))
            .await
            .context("creating the calendar")?;
        Ok(made.id)
    }

    pub fn existing<'a>(
        &'a self,
        calendar: &'a str,
        from: crate::untis::Stamp,
        to: crate::untis::Stamp,
    ) -> Eventually<'a, HashMap<String, Existing>> {
        Box::pin(self.existing_inner(calendar, from, to))
    }

    async fn existing_inner(
        &self,
        calendar: &str,
        from: crate::untis::Stamp,
        to: crate::untis::Stamp,
    ) -> Result<HashMap<String, Existing>> {
        let mut found = HashMap::new();
        let mut page: Option<String> = None;
        loop {
            let mut query: Vec<(&str, String)> = vec![
                ("timeMin", from.to_rfc3339_opts(SecondsFormat::Secs, true)),
                ("timeMax", to.to_rfc3339_opts(SecondsFormat::Secs, true)),
                ("privateExtendedProperty", format!("source={MARKER}")),
                ("singleEvents", "true".to_owned()),
                ("showDeleted", "false".to_owned()),
                ("maxResults", "2500".to_owned()),
            ];
            if let Some(token) = &page {
                query.push(("pageToken", token.clone()));
            }
            let listing: EventList =
                self.get(format!("{API}/calendars/{}/events", urlencode(calendar)), &query).await?;
            for item in listing.items {
                found.insert(
                    item.id.clone(),
                    Existing {
                        id: item.id,
                        summary: item.summary.unwrap_or_default(),
                        start: item.start.and_then(|s| s.date_time).unwrap_or_default(),
                        fingerprint: item
                            .extended_properties
                            .and_then(|e| e.private)
                            .and_then(|p| p.get("fp").and_then(Value::as_str).map(str::to_owned))
                            .unwrap_or_default(),
                    },
                );
            }
            page = listing.next_page_token;
            if page.is_none() {
                return Ok(found);
            }
        }
    }

    pub fn apply<'a>(
        &'a self,
        calendar: &'a str,
        plan: &'a Plan,
        lanes: usize,
    ) -> Eventually<'a, Tally> {
        Box::pin(self.apply_inner(calendar, plan, lanes))
    }

    async fn apply_inner(&self, calendar: &str, plan: &Plan, lanes: usize) -> Result<Tally> {
        let mut tally = Tally::default();
        // Owned once, up here, so the closures below capture no borrow of
        // `self` at all -- a borrowed capture makes the closure generic over
        // its lifetime and the whole future unspawnable.
        let me = self.clone();
        let cal = calendar.to_owned();

        // Owned items, not borrowed ones: an item of `&Desired` makes the
        // closure generic over that lifetime, which is the last thing keeping
        // this future from being spawnable.
        let work: Vec<(Desired, bool)> = plan
            .inserts
            .iter()
            .cloned()
            .map(|e| (e, true))
            .chain(plan.updates.iter().cloned().map(|e| (e, false)))
            .collect();

        let written = stream::iter(work)
            .map(move |(event, fresh)| {
                let me = me.clone();
                let calendar = cal.clone();
                async move {
                    let outcome = if fresh {
                        match me.insert(&calendar, &event).await {
                            // Already there, though beyond the window we looked upon.
                            Err(Conflict::Exists) => {
                                me.update(&calendar, &event).await.map(|()| false)
                            }
                            Err(Conflict::Other(err)) => Err(err),
                            Ok(()) => Ok(true),
                        }
                    } else {
                        me.update(&calendar, &event).await.map(|()| false)
                    };
                    (event.id.clone(), outcome)
                }
            })
            .buffer_unordered(lanes)
            .collect::<Vec<_>>()
            .await;

        for (id, outcome) in written {
            match outcome {
                Ok(true) => tally.inserted += 1,
                Ok(false) => tally.updated += 1,
                Err(err) => {
                    eprintln!("  ! {id}: {err:#}");
                    tally.failed += 1;
                }
            }
        }

        let me = self.clone();
        let cal = calendar.to_owned();
        let doomed: Vec<Existing> = plan.deletes.clone();
        let removed = stream::iter(doomed)
            .map(move |event| {
                let me = me.clone();
                let calendar = cal.clone();
                async move { (event.id.clone(), me.delete(&calendar, &event.id).await) }
            })
            .buffer_unordered(lanes)
            .collect::<Vec<_>>()
            .await;

        for (id, outcome) in removed {
            match outcome {
                Ok(()) => tally.deleted += 1,
                Err(err) => {
                    eprintln!("  ! {id}: {err:#}");
                    tally.failed += 1;
                }
            }
        }

        Ok(tally)
    }

    async fn insert(&self, calendar: &str, event: &Desired) -> Result<(), Conflict> {
        let url = format!("{API}/calendars/{}/events", urlencode(calendar));
        for attempt in 0..ATTEMPTS {
            let reply = self
                .http
                .post(&url)
                .bearer_auth(&self.token)
                .json(&event.body)
                .send()
                .await
                .map_err(|e| Conflict::Other(e.into()))?;
            let status = reply.status();
            if status.is_success() {
                return Ok(());
            }
            if status == reqwest::StatusCode::CONFLICT {
                return Err(Conflict::Exists);
            }
            let body = reply.text().await.unwrap_or_default();
            if worth_retrying(status, &body) && attempt + 1 < ATTEMPTS {
                pause(attempt).await;
                continue;
            }
            return Err(Conflict::Other(anyhow::anyhow!("{status}: {}", snippet(&body))));
        }
        unreachable!("the loop returneth upon its last turn")
    }

    async fn update(&self, calendar: &str, event: &Desired) -> Result<()> {
        let url =
            format!("{API}/calendars/{}/events/{}", urlencode(calendar), urlencode(&event.id));
        for attempt in 0..ATTEMPTS {
            let reply =
                self.http.put(&url).bearer_auth(&self.token).json(&event.body).send().await?;
            let status = reply.status();
            if status.is_success() {
                return Ok(());
            }
            let body = reply.text().await.unwrap_or_default();
            if worth_retrying(status, &body) && attempt + 1 < ATTEMPTS {
                pause(attempt).await;
                continue;
            }
            bail!("{status}: {}", snippet(&body));
        }
        unreachable!("the loop returneth upon its last turn")
    }

    async fn delete(&self, calendar: &str, id: &str) -> Result<()> {
        let url = format!("{API}/calendars/{}/events/{}", urlencode(calendar), urlencode(id));
        for attempt in 0..ATTEMPTS {
            let reply = self.http.delete(&url).bearer_auth(&self.token).send().await?;
            let status = reply.status();
            // Gone already is gone enough.
            if status.is_success()
                || status == reqwest::StatusCode::GONE
                || status == reqwest::StatusCode::NOT_FOUND
            {
                return Ok(());
            }
            let body = reply.text().await.unwrap_or_default();
            if worth_retrying(status, &body) && attempt + 1 < ATTEMPTS {
                pause(attempt).await;
                continue;
            }
            bail!("{status}: {}", snippet(&body));
        }
        unreachable!("the loop returneth upon its last turn")
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        url: String,
        query: &[(&str, String)],
    ) -> Result<T> {
        let reply = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .query(query)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = reply.status();
        let body = reply.text().await?;
        if !status.is_success() {
            bail!("{status} for {url}: {}", snippet(&body));
        }
        serde_json::from_str(&body).with_context(|| format!("decoding the reply of {url}"))
    }

    async fn post<T: serde::de::DeserializeOwned>(&self, url: String, body: &Value) -> Result<T> {
        let reply = self.http.post(&url).bearer_auth(&self.token).json(body).send().await?;
        let status = reply.status();
        let text = reply.text().await?;
        if !status.is_success() {
            bail!("{status} for {url}: {}", snippet(&text));
        }
        serde_json::from_str(&text).with_context(|| format!("decoding the reply of {url}"))
    }
}

enum Conflict {
    Exists,
    Other(anyhow::Error),
}

pub fn desired_of(lesson: &Lesson, zone: Tz) -> Desired {
    let colour = if lesson.cancelled() {
        Some(COLOUR_CANCELLED)
    } else if lesson.is_event {
        Some(COLOUR_EVENT)
    } else if lesson.status == Status::Changed && lesson.affects_me() {
        Some(COLOUR_CHANGED)
    } else {
        None
    };

    let summary = lesson.title();
    let description = lesson.description();
    let location = lesson.rooms.join(", ");
    let start = lesson.start.to_rfc3339_opts(SecondsFormat::Secs, false);
    let end = lesson.end.to_rfc3339_opts(SecondsFormat::Secs, false);
    // A lesson that falleth away should not devour thy free hours.
    let transparency = if lesson.cancelled() { "transparent" } else { "opaque" };

    let mut body = json!({
        "id": lesson.event_id(),
        "summary": summary,
        "description": description,
        "location": location,
        "start": { "dateTime": start, "timeZone": zone.name() },
        "end": { "dateTime": end, "timeZone": zone.name() },
        "transparency": transparency,
        "reminders": { "useDefault": false, "overrides": [] },
        "extendedProperties": { "private": { "source": MARKER } },
    });
    if let Some(colour) = colour {
        body["colorId"] = json!(colour);
    }

    let fingerprint = fingerprint_of(&[
        &summary,
        &description,
        &location,
        &start,
        &end,
        colour.unwrap_or_default(),
        transparency,
    ]);
    body["extendedProperties"]["private"]["fp"] = json!(fingerprint);

    Desired { id: lesson.event_id(), summary, start: lesson.start, body }
}

/// An exam, which is a timed event like a lesson but louder.
pub fn desired_of_exam(exam: &crate::Exam, zone: Tz) -> Desired {
    let summary = exam.title();
    let description = exam.description();
    let location = exam.rooms.join(", ");
    let start = exam.start.to_rfc3339_opts(SecondsFormat::Secs, false);
    let end = exam.end.to_rfc3339_opts(SecondsFormat::Secs, false);

    let mut body = json!({
        "id": exam.event_id(),
        "summary": summary,
        "description": description,
        "location": location,
        "start": { "dateTime": start, "timeZone": zone.name() },
        "end": { "dateTime": end, "timeZone": zone.name() },
        "transparency": "opaque",
        "colorId": COLOUR_EXAM,
        "reminders": { "useDefault": false, "overrides": [] },
        "extendedProperties": { "private": { "source": MARKER } },
    });
    let fingerprint =
        fingerprint_of(&[&summary, &description, &location, &start, &end, COLOUR_EXAM]);
    body["extendedProperties"]["private"]["fp"] = json!(fingerprint);
    Desired { id: exam.event_id(), summary, start: exam.start, body }
}

/// Homework and holidays occupy days rather than hours. Google wanteth
/// `date` for those, and refuseth a body that offereth both `date` and
/// `dateTime`; the end is the day *after* the last, as in iCalendar.
fn desired_all_day(
    id: String,
    summary: String,
    description: String,
    from: NaiveDate,
    until: NaiveDate,
    colour: &str,
    zone: Tz,
) -> Option<Desired> {
    let first = from.format("%Y-%m-%d").to_string();
    let after = (until + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();

    let mut body = json!({
        "id": id,
        "summary": summary,
        "description": description,
        "start": { "date": first },
        "end": { "date": after },
        // A day marked is not a day occupied: neither should block an hour.
        "transparency": "transparent",
        "colorId": colour,
        "reminders": { "useDefault": false, "overrides": [] },
        "extendedProperties": { "private": { "source": MARKER } },
    });
    let fingerprint = fingerprint_of(&[&summary, &description, &first, &after, colour]);
    body["extendedProperties"]["private"]["fp"] = json!(fingerprint);

    // Only a sort key; what Google is shown is the date above.
    let start = crate::untis::midnight_in(from, zone)?;
    Some(Desired { id, summary, start, body })
}

pub fn desired_of_homework(piece: &crate::Homework, zone: Tz) -> Option<Desired> {
    desired_all_day(
        piece.event_id(),
        piece.title(),
        piece.description(),
        piece.due,
        piece.due,
        COLOUR_HOMEWORK,
        zone,
    )
}

pub fn desired_of_holiday(shut: &crate::Holiday, zone: Tz) -> Option<Desired> {
    desired_all_day(
        shut.event_id(),
        shut.title(),
        String::new(),
        shut.start,
        shut.end,
        COLOUR_HOLIDAY,
        zone,
    )
}

/// What the calendar should hold, against what it holds.
///
/// Everything the timetable carrieth goes up, not the lessons alone: whoever
/// connected Google did so to see the same calendar their link shows, and an
/// exam missing from it is the worst way to learn of the difference.
pub fn plan(want: &crate::Timetable, existing: &HashMap<String, Existing>, zone: Tz) -> Plan {
    let mut plan = Plan::default();
    let mut wanted: HashMap<String, ()> = HashMap::with_capacity(want.lessons.len());

    let everything = want
        .lessons
        .iter()
        .map(|lesson| desired_of(lesson, zone))
        .chain(want.exams.iter().map(|exam| desired_of_exam(exam, zone)))
        .chain(want.homework.iter().filter_map(|piece| desired_of_homework(piece, zone)))
        .chain(want.holidays.iter().filter_map(|shut| desired_of_holiday(shut, zone)));

    for desired in everything {
        wanted.insert(desired.id.clone(), ());
        match existing.get(&desired.id) {
            None => plan.inserts.push(desired),
            Some(there) if there.fingerprint != fingerprint_in(&desired) => {
                plan.updates.push(desired)
            }
            Some(_) => plan.unchanged += 1,
        }
    }

    plan.deletes = existing.values().filter(|e| !wanted.contains_key(&e.id)).cloned().collect();
    plan.inserts.sort_by_key(|e| e.start);
    plan.updates.sort_by_key(|e| e.start);
    plan.deletes.sort_by(|a, b| a.start.cmp(&b.start));
    plan
}

fn fingerprint_in(desired: &Desired) -> String {
    desired.body["extendedProperties"]["private"]["fp"].as_str().unwrap_or_default().to_owned()
}

fn fingerprint_of(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for (n, part) in parts.iter().enumerate() {
        if n > 0 {
            hasher.update([0_u8]);
        }
        hasher.update(part.as_bytes());
    }
    hasher.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Google answereth a flood with 429, 5xx, or a 403 that nameth a rate limit.
fn worth_retrying(status: reqwest::StatusCode, body: &str) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
        || (status == reqwest::StatusCode::FORBIDDEN
            && (body.contains("rateLimitExceeded") || body.contains("userRateLimitExceeded")))
}

/// Half a second, then double, with a little jitter lest the lanes march in step.
async fn pause(attempt: u32) {
    let base = 500_u64 << attempt.min(4);
    let jitter = u64::from(std::process::id() % 251) + (attempt as u64 * 37);
    tokio::time::sleep(Duration::from_millis(base + jitter)).await;
}

fn urlencode(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn snippet(body: &str) -> String {
    body.chars().take(200).collect()
}

// ---------------------------------------------------------------- the wire --

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CalendarList {
    #[serde(default)]
    items: Vec<CalendarRef>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct CalendarRef {
    id: String,
    #[serde(default)]
    summary: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventList {
    #[serde(default)]
    items: Vec<EventRef>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventRef {
    id: String,
    summary: Option<String>,
    start: Option<Stamp>,
    extended_properties: Option<Extended>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stamp {
    date_time: Option<String>,
}

#[derive(Deserialize)]
struct Extended {
    private: Option<serde_json::Map<String, Value>>,
}

#[cfg(test)]
mod tests {
    use crate::{Exam, Holiday, Homework, Timetable};
    use chrono::TimeZone;

    fn a_day() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 9, 25).unwrap()
    }

    fn an_exam() -> Exam {
        let at = |h: u32| {
            crate::DEFAULT_TZ
                .from_local_datetime(&a_day().and_hms_opt(h, 0, 0).unwrap())
                .earliest()
                .unwrap()
                .fixed_offset()
        };
        Exam {
            id: 42,
            start: at(8),
            end: at(10),
            subject: "M".into(),
            kind: "Schularbeit".into(),
            name: String::new(),
            text: String::new(),
            teachers: vec![],
            rooms: vec!["A1".into()],
        }
    }

    fn some_homework() -> Homework {
        Homework {
            id: 5,
            due: a_day(),
            subject: "D".into(),
            text: "Kapitel 4".into(),
            remark: String::new(),
            done: false,
        }
    }

    #[test]
    fn a_whole_day_entry_offers_google_a_date_and_never_a_time() {
        for desired in [
            desired_of_homework(&some_homework(), crate::DEFAULT_TZ).unwrap(),
            desired_of_holiday(
                &Holiday { id: 3, name: "Herbstferien".into(), start: a_day(), end: a_day() },
                crate::DEFAULT_TZ,
            )
            .unwrap(),
        ] {
            let body = serde_json::to_string(&desired.body).unwrap();
            assert!(body.contains(r#""start":{"date":"2026-09-25"}"#), "{body}");
            // The end is the day after the last, and a body may not offer both
            // shapes: Google refuses one that does.
            assert!(body.contains(r#""end":{"date":"2026-09-26"}"#), "{body}");
            assert!(!body.contains("dateTime"), "a whole day has no time: {body}");
            assert_eq!(desired.body["transparency"], "transparent");
        }
    }

    #[test]
    fn the_same_work_on_another_day_is_another_fingerprint() {
        let monday = desired_of_homework(&some_homework(), crate::DEFAULT_TZ).unwrap();
        let mut moved = some_homework();
        moved.due = a_day().succ_opt().unwrap();
        let tuesday = desired_of_homework(&moved, crate::DEFAULT_TZ).unwrap();
        assert_ne!(
            fingerprint_in(&monday),
            fingerprint_in(&tuesday),
            "work that moved a day must be updated, not left where it was"
        );
    }

    #[test]
    fn everything_the_timetable_holds_is_pushed_and_nothing_of_it_deleted() {
        let want = Timetable {
            lessons: vec![],
            exams: vec![an_exam()],
            homework: vec![some_homework()],
            holidays: vec![Holiday {
                id: 3,
                name: "Herbstferien".into(),
                start: a_day(),
                end: a_day(),
            }],
        };
        let first_run = plan(&want, &HashMap::new(), crate::DEFAULT_TZ);
        assert_eq!(first_run.inserts.len(), 3, "the exam, the homework and the holiday");
        assert!(first_run.deletes.is_empty());

        // And a second run against what the first wrote changes nothing.
        let existing: HashMap<String, Existing> = first_run
            .inserts
            .iter()
            .map(|d| {
                (
                    d.id.clone(),
                    Existing {
                        id: d.id.clone(),
                        summary: d.summary.clone(),
                        start: String::new(),
                        fingerprint: fingerprint_in(d),
                    },
                )
            })
            .collect();
        let again = plan(&want, &existing, crate::DEFAULT_TZ);
        assert!(again.is_empty(), "a settled calendar should be left alone");
        assert_eq!(again.unchanged, 3);
    }

    #[test]
    fn the_span_covers_the_whole_day_entries_too() {
        // A holiday beyond the last lesson must widen the window, or it would
        // be written once and never reconciled again.
        let want = Timetable {
            holidays: vec![Holiday {
                id: 1,
                name: "Sommer".into(),
                start: chrono::NaiveDate::from_ymd_opt(2027, 7, 5).unwrap(),
                end: chrono::NaiveDate::from_ymd_opt(2027, 9, 1).unwrap(),
            }],
            ..Default::default()
        };
        let (first, last) = want.span(crate::DEFAULT_TZ).expect("a span");
        assert_eq!(first.format("%Y-%m-%d").to_string(), "2027-07-05");
        assert_eq!(last.format("%Y-%m-%d").to_string(), "2027-09-02", "the day after the last");

        assert!(Timetable::default().span(crate::DEFAULT_TZ).is_none(), "nothing spans nothing");
    }

    use super::*;

    #[test]
    fn calendar_ids_are_escaped() {
        assert_eq!(urlencode("abc@group.calendar.google.com"), "abc%40group.calendar.google.com");
        assert_eq!(urlencode("u5856529p5856532"), "u5856529p5856532");
    }

    #[test]
    fn fingerprints_follow_the_content() {
        let a = fingerprint_of(&["AM", "body", "134", "s", "e", "", "opaque"]);
        let b = fingerprint_of(&["AM", "body", "229", "s", "e", "", "opaque"]);
        assert_ne!(a, b);
        assert_eq!(a.len(), 16);
        assert_eq!(a, fingerprint_of(&["AM", "body", "134", "s", "e", "", "opaque"]));
    }

    #[test]
    fn throttling_is_told_from_real_refusal() {
        use reqwest::StatusCode;
        assert!(worth_retrying(StatusCode::TOO_MANY_REQUESTS, ""));
        assert!(worth_retrying(StatusCode::INTERNAL_SERVER_ERROR, ""));
        assert!(worth_retrying(StatusCode::SERVICE_UNAVAILABLE, ""));
        assert!(worth_retrying(StatusCode::FORBIDDEN, r#"{"reason":"rateLimitExceeded"}"#));
        assert!(!worth_retrying(StatusCode::FORBIDDEN, r#"{"reason":"insufficientPermissions"}"#));
        assert!(!worth_retrying(StatusCode::NOT_FOUND, ""));
        assert!(!worth_retrying(StatusCode::CONFLICT, ""));
    }

    #[test]
    fn fields_cannot_bleed_into_one_another() {
        assert_ne!(fingerprint_of(&["ab", "c"]), fingerprint_of(&["a", "bc"]));
    }
}
