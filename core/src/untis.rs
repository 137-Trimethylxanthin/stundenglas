//! Readeth the personal timetable from the selfsame endpoint the web client useth.

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::{Europe::Vienna, Tz};
use serde::Deserialize;

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:155.0) Gecko/20100101 Firefox/155.0";

/// WebUntis yieldeth bare wall-clock strings; the school's own iCal export
/// stampeth every hour TZID=Europe/Vienna, so herein lieth the zone.
pub const TZ: Tz = Vienna;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Regular,
    Changed,
    Cancelled,
}

impl Status {
    fn parse(raw: &str) -> Self {
        match raw {
            "CANCELLED" => Self::Cancelled,
            "CHANGED" => Self::Changed,
            _ => Self::Regular,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Lesson {
    pub ids: Vec<i64>,
    pub start: DateTime<Tz>,
    pub end: DateTime<Tz>,
    pub status: Status,
    pub is_event: bool,
    pub subjects: Vec<String>,
    pub subject_long: String,
    /// An EVENT nameth itself in position2 as `INFO`, where a lesson hath `SUBJECT`.
    pub info: Vec<String>,
    pub teachers: Vec<String>,
    pub rooms: Vec<String>,
    pub classes: Vec<String>,
    pub teachers_removed: Vec<String>,
    pub rooms_removed: Vec<String>,
    pub lesson_info: String,
    pub lesson_text: String,
    pub substitution_text: String,
    pub notes: String,
}

impl Lesson {
    pub fn cancelled(&self) -> bool {
        self.status == Status::Cancelled
    }

    /// Google brooketh only `^[a-v0-9]{5,1024}$`; both `u` and `p` lie within.
    pub fn event_id(&self) -> String {
        let mut id = String::with_capacity(1 + self.ids.len() * 9);
        id.push('u');
        for (n, period) in self.ids.iter().enumerate() {
            if n > 0 {
                id.push('p');
            }
            id.push_str(itoa(*period).as_str());
        }
        id
    }

    /// A shared lesson is marked CHANGED whensoever any other class cometh or
    /// goeth; only a change of teacher or room toucheth this pupil.
    pub fn affects_me(&self) -> bool {
        !self.teachers_removed.is_empty() || !self.rooms_removed.is_empty()
    }

    pub fn title(&self) -> String {
        let base = if !self.subjects.is_empty() {
            self.subjects.join("/")
        } else if !self.info.is_empty() {
            self.info.join(" / ")
        } else if !self.substitution_text.is_empty() {
            self.substitution_text.clone()
        } else {
            "Termin".to_owned()
        };
        if self.cancelled() {
            format!("❌ {base}")
        } else if self.is_event {
            format!("📌 {base}")
        } else if self.status == Status::Changed && self.affects_me() {
            format!("⚠️ {base}")
        } else {
            base
        }
    }

    pub fn description(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        if self.cancelled() {
            lines.push("Diese Stunde entfällt.".to_owned());
        }
        if !self.subject_long.is_empty() && !self.subjects.contains(&self.subject_long) {
            lines.push(self.subject_long.clone());
        }
        if !self.teachers.is_empty() {
            lines.push(format!("Lehrkraft: {}", self.teachers.join(", ")));
        }
        if !self.teachers_removed.is_empty() {
            let who = self.teachers_removed.join(", ");
            // A teacher away with none assigned cometh as `?` plus the old name.
            lines.push(if self.teachers.is_empty() {
                format!("{who} fehlt — keine Vertretung eingetragen")
            } else {
                format!("Vertretung für {who}")
            });
        }
        if !self.rooms_removed.is_empty() {
            lines.push(format!("Raumänderung, vorher: {}", self.rooms_removed.join(", ")));
        }
        if !self.classes.is_empty() {
            lines.push(format!("Klassen: {}", self.classes.join(", ")));
        }
        for extra in [&self.lesson_info, &self.substitution_text, &self.lesson_text, &self.notes] {
            if !extra.is_empty() {
                lines.push(extra.clone());
            }
        }
        lines.push(format!(
            "[untis {}]",
            self.ids.iter().map(|i| itoa(*i)).collect::<Vec<_>>().join(",")
        ));
        lines.join("\n")
    }
}

#[derive(Debug, Clone)]
pub struct Absence {
    pub start: DateTime<Tz>,
    pub end: DateTime<Tz>,
    pub reason: String,
    pub text: String,
}

impl Absence {
    pub fn covers(&self, lesson: &Lesson) -> bool {
        lesson.start < self.end && lesson.end > self.start
    }

    pub fn label(&self) -> String {
        let parts: Vec<&str> = [self.reason.as_str(), self.text.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() { "Abwesenheit".to_owned() } else { parts.join(" – ") }
    }
}

/// What is needed to speak to one WebUntis account.
#[derive(Clone)]
pub struct Credentials {
    pub server: String,
    pub school: String,
    pub user: String,
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("server", &self.server)
            .field("school", &self.school)
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl Credentials {
    pub fn base_url(&self) -> String {
        format!("https://{}", self.server)
    }
}

pub struct Client {
    http: reqwest::Client,
    settings: Credentials,
    base: String,
    bearer: Option<String>,
    pub person_id: i64,
    pub person_name: String,
    pub year: (NaiveDate, NaiveDate),
}

impl Client {
    pub fn new(settings: Credentials) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .cookie_store(true)
            // The login answereth 302; we must read its Location ourselves.
            .redirect(reqwest::redirect::Policy::none())
            .gzip(true)
            .build()
            .context("building the HTTP client")?;
        let base = settings.base_url();
        Ok(Self {
            http,
            settings,
            base,
            bearer: None,
            person_id: 0,
            person_name: String::new(),
            year: (NaiveDate::default(), NaiveDate::default()),
        })
    }

    pub async fn login(&mut self) -> Result<()> {
        self.http
            .get(format!("{}/WebUntis/?school={}", self.base, self.settings.school))
            .send()
            .await
            .context("reaching WebUntis")?;

        let reply = self
            .http
            .post(format!("{}/WebUntis/j_spring_security_check", self.base))
            .header("X-Requested-With", "XMLHttpRequest")
            .form(&[
                ("school", self.settings.school.as_str()),
                ("j_username", self.settings.user.as_str()),
                ("j_password", self.settings.password.as_str()),
                ("token", ""),
            ])
            .send()
            .await
            .context("posting the login form")?;

        let status = reply.status();
        let location = reply
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let body = reply.text().await.unwrap_or_default();

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body)
            && let Some(state) = json.get("state").and_then(|s| s.as_str())
            && state != "SUCCESS"
        {
            bail!("login rejected: {state}");
        }
        if status.is_redirection() && !location.contains("index.do") {
            bail!("login failed (redirected to {location})");
        }

        let token = self
            .http
            .get(format!("{}/WebUntis/api/token/new", self.base))
            .send()
            .await
            .context("requesting the bearer token")?
            .text()
            .await?;
        let token = token.trim().to_owned();
        if token.len() < 40 {
            bail!("no usable bearer token was returned; are the credentials right?");
        }
        self.bearer = Some(token);

        let data: AppData = self.rest("/WebUntis/api/rest/view/v1/app/data", &[]).await?;
        self.person_id = data.user.person.id;
        self.person_name = data.user.person.display_name;
        self.year = (
            NaiveDate::parse_from_str(&data.current_school_year.date_range.start, "%Y-%m-%d")?,
            NaiveDate::parse_from_str(&data.current_school_year.date_range.end, "%Y-%m-%d")?,
        );
        Ok(())
    }

    /// Every `rest/view` path feigneth 404 until the bearer be given.
    async fn rest<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let bearer = self.bearer.as_ref().ok_or_else(|| anyhow!("not logged in"))?;
        let reply = self
            .http
            .get(format!("{}{path}", self.base))
            .bearer_auth(bearer)
            .query(query)
            .send()
            .await
            .with_context(|| format!("GET {path}"))?;
        let status = reply.status();
        let body = reply.text().await?;
        if !status.is_success() {
            let hint = if status == reqwest::StatusCode::NOT_FOUND {
                " (a 404 here usually meaneth the bearer token was refused)"
            } else {
                ""
            };
            bail!("{status} for {path}{hint}: {}", body.chars().take(200).collect::<String>());
        }
        serde_json::from_str(&body).with_context(|| format!("decoding the reply of {path}"))
    }

    pub async fn fetch(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<Lesson>> {
        let entries: Entries = self
            .rest(
                "/WebUntis/api/rest/view/v1/timetable/entries",
                &[
                    ("start", from.to_string()),
                    ("end", to.to_string()),
                    ("format", "3".to_owned()),
                    ("resourceType", "STUDENT".to_owned()),
                    ("resources", self.person_id.to_string()),
                    ("periodTypes", String::new()),
                    ("timetableType", "MY_TIMETABLE".to_owned()),
                ],
            )
            .await?;

        if !entries.errors.is_empty() {
            bail!("WebUntis reported errors: {}", serde_json::to_string(&entries.errors)?);
        }

        let mut lessons = Vec::new();
        for day in entries.days {
            for entry in day.grid_entries.unwrap_or_default() {
                lessons.push(entry.into_lesson()?);
            }
        }
        lessons.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.title().cmp(&b.title())));
        Ok(lessons)
    }

    /// `excuseStatusId=-1` meaneth *all*; the web page defaulteth to `-3`,
    /// which showeth only the unexcused and so hideth an approved leave.
    pub async fn fetch_absences(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<Absence>> {
        let reply = self
            .http
            .get(format!("{}/WebUntis/api/classreg/absences/students", self.base))
            .query(&[
                ("studentId", self.person_id.to_string()),
                ("startDate", from.format("%Y%m%d").to_string()),
                ("endDate", to.format("%Y%m%d").to_string()),
                ("excuseStatusId", "-1".to_owned()),
                ("includeTodaysAbsence", "true".to_owned()),
            ])
            .send()
            .await
            .context("fetching absences")?;
        let status = reply.status();
        let body = reply.text().await?;
        if !status.is_success() {
            bail!("{status} fetching absences: {}", body.chars().take(200).collect::<String>());
        }

        let envelope: AbsenceEnvelope = serde_json::from_str(&body).context("decoding absences")?;
        envelope
            .data
            .absences
            .unwrap_or_default()
            .into_iter()
            .map(|raw| {
                Ok(Absence {
                    start: stamp(raw.start_date, raw.start_time)?,
                    end: stamp(raw.end_date, if raw.end_time == 0 { 2359 } else { raw.end_time })?,
                    reason: raw.reason.unwrap_or_default(),
                    text: raw.text.unwrap_or_default(),
                })
            })
            .collect()
    }
}

fn itoa(value: i64) -> String {
    value.to_string()
}

/// Untis keepeth dates as 20260914 and hours as 800 or 2155.
fn stamp(date: i64, time: i64) -> Result<DateTime<Tz>> {
    let date = NaiveDate::from_ymd_opt(
        (date / 10_000) as i32,
        ((date / 100) % 100) as u32,
        (date % 100) as u32,
    )
    .ok_or_else(|| anyhow!("nonsensical date {date}"))?;
    let naive = date
        .and_hms_opt((time / 100) as u32, (time % 100) as u32, 0)
        .ok_or_else(|| anyhow!("nonsensical time {time}"))?;
    localise(naive)
}

fn localise(naive: NaiveDateTime) -> Result<DateTime<Tz>> {
    TZ.from_local_datetime(&naive)
        .earliest()
        .ok_or_else(|| anyhow!("{naive} falleth in no valid hour of Europe/Vienna"))
}

fn parse_stamp(raw: &str) -> Result<DateTime<Tz>> {
    let naive = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M"))
        .with_context(|| format!("unreadable timestamp {raw}"))?;
    localise(naive)
}

// ---------------------------------------------------------------- the wire --

#[derive(Deserialize)]
struct Entries {
    #[serde(default)]
    days: Vec<Day>,
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Day {
    grid_entries: Option<Vec<GridEntry>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GridEntry {
    #[serde(default)]
    ids: Vec<i64>,
    duration: Span,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    status: String,
    position1: Option<Vec<Slot>>,
    position2: Option<Vec<Slot>>,
    position3: Option<Vec<Slot>>,
    position4: Option<Vec<Slot>>,
    position5: Option<Vec<Slot>>,
    position6: Option<Vec<Slot>>,
    position7: Option<Vec<Slot>>,
    lesson_text: Option<String>,
    lesson_info: Option<String>,
    substitution_text: Option<String>,
    notes_all: Option<String>,
}

#[derive(Deserialize)]
struct Span {
    start: String,
    end: String,
}

#[derive(Deserialize)]
struct Slot {
    current: Option<Element>,
    removed: Option<Element>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Element {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    short_name: String,
    #[serde(default)]
    long_name: String,
}

impl GridEntry {
    fn slots(&self) -> impl Iterator<Item = &Slot> {
        [
            &self.position1,
            &self.position2,
            &self.position3,
            &self.position4,
            &self.position5,
            &self.position6,
            &self.position7,
        ]
        .into_iter()
        .flatten()
        .flatten()
    }

    fn present(&self, kind: &str) -> Vec<String> {
        self.slots()
            .filter_map(|s| s.current.as_ref())
            .filter(|e| e.kind == kind && !e.short_name.is_empty() && e.short_name != "?")
            .map(|e| e.short_name.clone())
            .collect()
    }

    fn vanished(&self, kind: &str) -> Vec<String> {
        self.slots()
            .filter_map(|s| s.removed.as_ref())
            .filter(|e| e.kind == kind && !e.short_name.is_empty())
            .map(|e| e.short_name.clone())
            .collect()
    }

    fn into_lesson(self) -> Result<Lesson> {
        let subject_long = self
            .slots()
            .filter_map(|s| s.current.as_ref())
            .find(|e| e.kind == "SUBJECT")
            .map(|e| e.long_name.clone())
            .unwrap_or_default();

        Ok(Lesson {
            start: parse_stamp(&self.duration.start)?,
            end: parse_stamp(&self.duration.end)?,
            status: Status::parse(&self.status),
            is_event: self.kind == "EVENT",
            subjects: self.present("SUBJECT"),
            subject_long,
            info: self.present("INFO"),
            teachers: self.present("TEACHER"),
            rooms: self.present("ROOM"),
            classes: self.present("CLASS"),
            teachers_removed: self.vanished("TEACHER"),
            rooms_removed: self.vanished("ROOM"),
            lesson_info: self.lesson_info.clone().unwrap_or_default(),
            lesson_text: self.lesson_text.clone().unwrap_or_default(),
            substitution_text: self.substitution_text.clone().unwrap_or_default(),
            notes: self.notes_all.clone().unwrap_or_default(),
            ids: self.ids.clone(),
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppData {
    user: UserBlock,
    current_school_year: SchoolYear,
}

#[derive(Deserialize)]
struct UserBlock {
    person: Person,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Person {
    id: i64,
    display_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchoolYear {
    date_range: DateRange,
}

#[derive(Deserialize)]
struct DateRange {
    start: String,
    end: String,
}

#[derive(Deserialize)]
struct AbsenceEnvelope {
    data: AbsenceData,
}

#[derive(Deserialize)]
struct AbsenceData {
    absences: Option<Vec<AbsenceRaw>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AbsenceRaw {
    start_date: i64,
    end_date: i64,
    #[serde(default)]
    start_time: i64,
    #[serde(default)]
    end_time: i64,
    reason: Option<String>,
    text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lesson(ids: Vec<i64>, status: Status, day: u32, from: u32, to: u32) -> Lesson {
        let at = |hour: u32| {
            localise(
                NaiveDate::from_ymd_opt(2026, 9, day).unwrap().and_hms_opt(hour, 0, 0).unwrap(),
            )
            .unwrap()
        };
        Lesson {
            ids,
            start: at(from),
            end: at(to),
            status,
            is_event: false,
            subjects: vec!["MAT".to_owned()],
            subject_long: "Mathematik".to_owned(),
            info: vec![],
            teachers: vec![],
            rooms: vec![],
            classes: vec![],
            teachers_removed: vec![],
            rooms_removed: vec![],
            lesson_info: String::new(),
            lesson_text: String::new(),
            substitution_text: String::new(),
            notes: String::new(),
        }
    }

    #[test]
    fn event_ids_please_google() {
        let valid = |id: &str| {
            id.len() >= 5 && id.chars().all(|c| c.is_ascii_digit() || ('a'..='v').contains(&c))
        };
        assert!(valid(&lesson(vec![5_494_087], Status::Regular, 21, 8, 9).event_id()));
        assert!(valid(&lesson(vec![5_856_529, 5_856_532], Status::Regular, 21, 8, 9).event_id()));
        assert_eq!(
            lesson(vec![5_856_529, 5_856_532], Status::Regular, 21, 8, 9).event_id(),
            "u5856529p5856532"
        );
    }

    #[test]
    fn markers_mean_something() {
        assert!(lesson(vec![1], Status::Cancelled, 21, 8, 9).title().starts_with('❌'));

        let mut roster = lesson(vec![1], Status::Changed, 21, 8, 9);
        roster.classes = vec!["2B".to_owned()];
        assert_eq!(roster.title(), "MAT", "roster churn must not be flagged");

        let mut swapped = lesson(vec![1], Status::Changed, 21, 8, 9);
        swapped.teachers_removed = vec!["ABC".to_owned()];
        assert!(swapped.title().starts_with('⚠'));

        let mut happening = lesson(vec![1], Status::Changed, 21, 8, 9);
        happening.is_event = true;
        happening.subjects.clear();
        happening.info = vec!["Projekttag".to_owned()];
        assert_eq!(happening.title(), "📌 Projekttag");

        let mut nameless = lesson(vec![1], Status::Changed, 21, 8, 9);
        nameless.is_event = true;
        nameless.subjects.clear();
        assert_eq!(nameless.title(), "📌 Termin", "a nameless event still needeth a name");
    }

    #[test]
    fn absences_swallow_the_hours_they_cover() {
        let leave = Absence {
            start: stamp(20_260_914, 800).unwrap(),
            end: stamp(20_260_917, 2155).unwrap(),
            reason: "Freistellung".to_owned(),
            text: "Exkursion".to_owned(),
        };
        assert!(leave.covers(&lesson(vec![1], Status::Regular, 14, 8, 9)));
        assert!(leave.covers(&lesson(vec![1], Status::Regular, 16, 8, 9)));
        assert!(leave.covers(&lesson(vec![1], Status::Regular, 17, 15, 16)));
        assert!(!leave.covers(&lesson(vec![1], Status::Regular, 18, 8, 9)));
        assert!(!leave.covers(&lesson(vec![1], Status::Regular, 11, 8, 9)));

        let half = Absence {
            start: stamp(20_260_921, 1200).unwrap(),
            end: stamp(20_260_921, 1800).unwrap(),
            reason: String::new(),
            text: String::new(),
        };
        assert!(half.covers(&lesson(vec![1], Status::Regular, 21, 13, 14)));
        assert!(!half.covers(&lesson(vec![1], Status::Regular, 21, 8, 9)));
    }

    #[test]
    fn timestamps_land_in_vienna() {
        let noon = parse_stamp("2026-09-21T08:00").unwrap();
        assert_eq!(noon.format("%Y-%m-%d %H:%M %:z").to_string(), "2026-09-21 08:00 +02:00");
        let winter = parse_stamp("2026-12-01T08:00:00").unwrap();
        assert_eq!(winter.format("%:z").to_string(), "+01:00");
    }

    #[test]
    fn descriptions_tell_absence_from_substitution() {
        let mut nobody = lesson(vec![1], Status::Changed, 21, 8, 9);
        nobody.teachers_removed = vec!["ABC".to_owned()];
        assert!(nobody.description().contains("ABC fehlt — keine Vertretung eingetragen"));

        let mut covered = lesson(vec![1], Status::Changed, 21, 8, 9);
        covered.teachers_removed = vec!["ABC".to_owned()];
        covered.teachers = vec!["XYZ".to_owned()];
        assert!(covered.description().contains("Vertretung für ABC"));
    }
}
