//! Writeth only unto its own secondary calendar, and only unto its own events.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, SecondsFormat};
use chrono_tz::Tz;
use futures::stream::{self, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::Duration;

use crate::untis::{Lesson, Status};

const API: &str = "https://www.googleapis.com/calendar/v3";
pub const CALENDAR_NAME: &str = "Schule (Untis)";
const TIME_ZONE: &str = "Europe/Vienna";
const MARKER: &str = "untis";
const ATTEMPTS: u32 = 4;

// Google's own palette, numbered one to eleven.
const COLOUR_CANCELLED: &str = "11";
const COLOUR_CHANGED: &str = "6";
const COLOUR_EVENT: &str = "5";

#[derive(Debug, Clone)]
pub struct Desired {
    pub id: String,
    pub summary: String,
    pub start: DateTime<Tz>,
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

pub struct Calendar {
    http: reqwest::Client,
    token: String,
}

impl Calendar {
    pub fn new(http: reqwest::Client, token: String) -> Self {
        Self { http, token }
    }

    pub async fn find_or_create(&self, name: &str) -> Result<String> {
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
            .post(format!("{API}/calendars"), &json!({ "summary": name, "timeZone": TIME_ZONE }))
            .await
            .context("creating the calendar")?;
        Ok(made.id)
    }

    pub async fn existing(
        &self,
        calendar: &str,
        from: DateTime<Tz>,
        to: DateTime<Tz>,
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

    pub async fn apply(&self, calendar: &str, plan: &Plan, lanes: usize) -> Result<Tally> {
        let mut tally = Tally::default();

        let written = stream::iter(
            plan.inserts.iter().map(|e| (e, true)).chain(plan.updates.iter().map(|e| (e, false))),
        )
        .map(|(event, fresh)| async move {
            let outcome = if fresh {
                match self.insert(calendar, event).await {
                    // Already there, though beyond the window we looked upon.
                    Err(Conflict::Exists) => self.update(calendar, event).await.map(|()| false),
                    Err(Conflict::Other(err)) => Err(err),
                    Ok(()) => Ok(true),
                }
            } else {
                self.update(calendar, event).await.map(|()| false)
            };
            (event.id.clone(), outcome)
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

        let removed = stream::iter(plan.deletes.iter())
            .map(|event| async move { (event.id.clone(), self.delete(calendar, &event.id).await) })
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

pub fn desired_of(lesson: &Lesson) -> Desired {
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
        "start": { "dateTime": start, "timeZone": TIME_ZONE },
        "end": { "dateTime": end, "timeZone": TIME_ZONE },
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

pub fn plan(lessons: &[Lesson], existing: &HashMap<String, Existing>) -> Plan {
    let mut plan = Plan::default();
    let mut wanted: HashMap<String, ()> = HashMap::with_capacity(lessons.len());

    for lesson in lessons {
        let desired = desired_of(lesson);
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
