//! Readeth the personal timetable from the selfsame endpoint the web client useth.

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::{Europe::Vienna, Tz};
use serde::{Deserialize, Serialize};

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:155.0) Gecko/20100101 Firefox/155.0";

/// WebUntis yieldeth bare wall-clock strings with no offset at all, so the
/// school's own zone must be supplied; this is merely a sensible default for
/// Austrian schools. Each account chooseth its own.
pub const DEFAULT_TZ: Tz = Vienna;

/// Lessons carry a fixed offset rather than a named zone: the offset is
/// computed from the school's zone at that very instant, which is all that
/// rendering, comparing and pushing require -- and, unlike a named zone,
/// chrono can read one back from JSON.
pub type Stamp = DateTime<FixedOffset>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub ids: Vec<i64>,
    pub start: Stamp,
    pub end: Stamp,
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
    pub start: Stamp,
    pub end: Stamp,
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
    /// The zone the school keepeth its clocks in.
    pub timezone: Tz,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("server", &self.server)
            .field("school", &self.school)
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .field("timezone", &self.timezone.name())
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

/// A written exam, which a timetable doth not carry and a student careth
/// about more than any ordinary hour.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Exam {
    pub id: i64,
    pub start: Stamp,
    pub end: Stamp,
    /// The subject's short name, as the timetable spelleth it.
    pub subject: String,
    /// What the school called this one: "Schularbeit", "Test", and such.
    pub kind: String,
    /// A name the school gave this particular exam, if any.
    pub name: String,
    pub text: String,
    pub teachers: Vec<String>,
    pub rooms: Vec<String>,
}

impl Exam {
    /// `📝 M Schularbeit` — the marker first, so it is plain in a crowded day.
    pub fn title(&self) -> String {
        let mut out = String::from("📝 ");
        if !self.subject.is_empty() {
            out.push_str(&self.subject);
        }
        for extra in [&self.kind, &self.name] {
            if !extra.is_empty() && !out.contains(extra.as_str()) {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(extra);
            }
        }
        out.trim().to_owned()
    }

    pub fn description(&self) -> String {
        let mut lines = Vec::new();
        if !self.text.is_empty() {
            lines.push(self.text.clone());
        }
        if !self.teachers.is_empty() {
            lines.push(format!("Mit {}", self.teachers.join(", ")));
        }
        if !self.rooms.is_empty() {
            lines.push(format!("Raum {}", self.rooms.join(", ")));
        }
        lines.join("\n")
    }

    /// Stable across refreshes, and acceptable to Google as an event id.
    pub fn event_id(&self) -> String {
        format!("exam{}", self.id)
    }
}

/// A piece of homework, which belongeth to the day it is due rather than the
/// hour it was set.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Homework {
    pub id: i64,
    /// The day it is due. Homework hath no hour, so it is rendered whole-day.
    pub due: NaiveDate,
    pub subject: String,
    pub text: String,
    pub remark: String,
    pub done: bool,
}

impl Homework {
    /// `📚 M — Kapitel 4 lesen`
    pub fn title(&self) -> String {
        let what = if self.text.is_empty() { "Hausübung" } else { &self.text };
        if self.subject.is_empty() {
            format!("📚 {what}")
        } else {
            format!("📚 {} — {what}", self.subject)
        }
    }

    pub fn description(&self) -> String {
        let mut lines = Vec::new();
        if !self.remark.is_empty() {
            lines.push(self.remark.clone());
        }
        if self.done {
            lines.push("Als erledigt vermerkt.".to_owned());
        }
        lines.join("\n")
    }

    pub fn event_id(&self) -> String {
        format!("hw{}", self.id)
    }
}

/// Everything one refresh yields, kept together because it travels together:
/// into the cache, out to a calendar file, and up to Google.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Timetable {
    pub lessons: Vec<Lesson>,
    pub exams: Vec<Exam>,
    pub homework: Vec<Homework>,
    pub holidays: Vec<Holiday>,
}

impl Timetable {
    pub fn is_empty(&self) -> bool {
        self.lessons.is_empty()
            && self.exams.is_empty()
            && self.homework.is_empty()
            && self.holidays.is_empty()
    }

    /// The span everything in here occupies, in the school's own zone. A
    /// whole-day entry counts from its midnight to the midnight after, which
    /// is how iCalendar and Google both read one.
    pub fn span(&self, zone: Tz) -> Option<(Stamp, Stamp)> {
        let midnight = |day: NaiveDate, plus: i64| {
            localise((day + Duration::days(plus)).and_hms_opt(0, 0, 0)?, zone).ok()
        };

        let mut first: Option<Stamp> = None;
        let mut last: Option<Stamp> = None;
        let mut widen = |from: Stamp, to: Stamp| {
            first = Some(first.map_or(from, |had| had.min(from)));
            last = Some(last.map_or(to, |had| had.max(to)));
        };

        for lesson in &self.lessons {
            widen(lesson.start, lesson.end);
        }
        for exam in &self.exams {
            widen(exam.start, exam.end);
        }
        for piece in &self.homework {
            if let (Some(from), Some(to)) = (midnight(piece.due, 0), midnight(piece.due, 1)) {
                widen(from, to);
            }
        }
        for shut in &self.holidays {
            if let (Some(from), Some(to)) = (midnight(shut.start, 0), midnight(shut.end, 1)) {
                widen(from, to);
            }
        }
        Some((first?, last?))
    }
}

/// A stretch of days the school is shut. Whole days, and no hours.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Holiday {
    pub id: i64,
    pub name: String,
    /// Inclusive, both ends, as the school giveth them.
    pub start: NaiveDate,
    pub end: NaiveDate,
}

impl Holiday {
    pub fn title(&self) -> String {
        format!("🌴 {}", self.name)
    }

    pub fn event_id(&self) -> String {
        format!("hol{}", self.id)
    }
}

/// A school as the public directory nameth it. `server` and `login_name` are
/// exactly the two things a student would otherwise have to dig out of a URL.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SchoolHit {
    pub display_name: String,
    pub login_name: String,
    pub server: String,
    pub address: String,
}

/// Search WebUntis' own directory of schools. Needeth no account: it is the
/// same call the login page maketh while one typeth.
pub async fn find_schools(http: &reqwest::Client, query: &str) -> Result<Vec<SchoolHit>> {
    let query = query.trim();
    if query.chars().count() < 3 {
        return Ok(Vec::new());
    }

    let reply = http
        .post("https://mobile.webuntis.com/ms/schoolquery2")
        .json(&serde_json::json!({
            "id": "stundenglas",
            "method": "searchSchool",
            "params": [{ "search": query }],
            "jsonrpc": "2.0",
        }))
        .send()
        .await
        .context("asking WebUntis for its list of schools")?;

    let status = reply.status();
    let body = reply.text().await?;
    if !status.is_success() {
        bail!("{status} searching for schools");
    }

    let found: SchoolReply = serde_json::from_str(&body).context("decoding the schools")?;
    Ok(found
        .result
        .map(|result| result.schools)
        .unwrap_or_default()
        .into_iter()
        .filter(|school| !school.server.is_empty() && !school.login_name.is_empty())
        .map(|school| SchoolHit {
            display_name: school.display_name,
            login_name: school.login_name,
            // The directory gives the bare host, which is what we want.
            server: school.server,
            address: school.address,
        })
        .collect())
}

#[derive(Deserialize)]
struct SchoolReply {
    result: Option<SchoolResult>,
}

#[derive(Deserialize)]
struct SchoolResult {
    #[serde(default)]
    schools: Vec<SchoolRaw>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchoolRaw {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    login_name: String,
    #[serde(default)]
    server: String,
    #[serde(default)]
    address: String,
}

/// The school said no: a wrong password, or a login that wanteth a code we
/// cannot give it. Worth a type of its own because the answer to it is the
/// opposite of the answer to an outage — stop, rather than try again.
#[derive(Debug)]
pub struct Rejected(pub String);

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for Rejected {}

/// Whether anywhere in this failure's chain the school refused the login.
pub fn was_rejected(err: &anyhow::Error) -> bool {
    err.chain().any(|link| link.is::<Rejected>())
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
            return Err(Rejected(format!("the school refused the login: {state}")).into());
        }
        if status.is_redirection() && !location.contains("index.do") {
            // Sent back to the login page: the username or password is wrong.
            return Err(Rejected(format!(
                "the school refused the login (it sent us back to {location})"
            ))
            .into());
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

    pub fn timezone(&self) -> Tz {
        self.settings.timezone
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
                lessons.push(entry.into_lesson(self.settings.timezone)?);
            }
        }
        lessons.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.title().cmp(&b.title())));
        Ok(lessons)
    }

    /// `excuseStatusId=-1` meaneth *all*; the web page defaulteth to `-3`,
    /// which showeth only the unexcused and so hideth an approved leave.
    /// Holidays, from the old mobile JSON-RPC API — the only place that
    /// offereth them. It keepeth a session of its own, obtained by its own
    /// `authenticate` call; the cookie jar carrieth it from there.
    ///
    /// Everything else here speaketh the newer APIs, so this is the one place
    /// the old one is wanted, and a school that refuseth it simply hath no
    /// holidays in its calendar.
    pub async fn fetch_holidays(&self) -> Result<Vec<Holiday>> {
        let rpc = format!("{}/WebUntis/jsonrpc.do?school={}", self.base, self.settings.school);

        let hello: RpcReply<RpcSession> = self
            .http
            .post(&rpc)
            .json(&serde_json::json!({
                "id": "stundenglas",
                "method": "authenticate",
                "params": {
                    "user": self.settings.user,
                    "password": self.settings.password,
                    "client": "stundenglas",
                },
                "jsonrpc": "2.0",
            }))
            .send()
            .await
            .context("authenticating against the JSON-RPC API")?
            .json()
            .await
            .context("decoding the JSON-RPC greeting")?;

        if hello.result.as_ref().is_none_or(|session| session.session_id.is_empty()) {
            bail!("the JSON-RPC API would not open a session");
        }

        let holidays: RpcReply<Vec<HolidayRaw>> = self
            .http
            .post(&rpc)
            .json(&serde_json::json!({
                "id": "stundenglas",
                "method": "getHolidays",
                "params": {},
                "jsonrpc": "2.0",
            }))
            .send()
            .await
            .context("fetching holidays")?
            .json()
            .await
            .context("decoding the holidays")?;

        Ok(holidays
            .result
            .unwrap_or_default()
            .into_iter()
            .filter_map(|raw| {
                let start = compact_date(raw.start_date)?;
                let end = compact_date(raw.end_date)?;
                (end >= start).then_some(Holiday {
                    id: raw.id,
                    name: if raw.long_name.is_empty() { raw.name } else { raw.long_name },
                    start,
                    end,
                })
            })
            .collect())
    }

    /// Homework, from the classic endpoint. The subject liveth in a separate
    /// `lessons` array keyed by lesson id, so the two are joined here rather
    /// than leaving every entry nameless.
    ///
    /// As with exams, a school that keepeth none, or withholdeth them, is not
    /// worth failing a refresh over.
    pub async fn fetch_homework(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<Homework>> {
        let reply = self
            .http
            .get(format!("{}/WebUntis/api/homeworks/lessons", self.base))
            .query(&[
                ("startDate", from.format("%Y%m%d").to_string()),
                ("endDate", to.format("%Y%m%d").to_string()),
            ])
            .send()
            .await
            .context("fetching homework")?;

        let status = reply.status();
        let body = reply.text().await?;
        if !status.is_success() {
            bail!("{status} fetching homework: {}", body.chars().take(200).collect::<String>());
        }

        let envelope: HomeworkEnvelope =
            serde_json::from_str(&body).context("decoding homework")?;
        Ok(envelope.into_homework())
    }

    /// Exams, from the classic endpoint: cookie-authenticated, compact dates,
    /// and `klasseId=-1` without which it answereth nothing. Both are traps
    /// SCOUT.md recordeth.
    ///
    /// Two shapes have been seen in the wild — a compact `examDate` with
    /// `startTime`, and ISO `startDateTime` — so both are read, and an exam
    /// that maketh sense in neither is passed over rather than failing the
    /// refresh for the rest.
    pub async fn fetch_exams(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<Exam>> {
        let reply = self
            .http
            .get(format!("{}/WebUntis/api/exams", self.base))
            .query(&[
                ("studentId", self.person_id.to_string()),
                // Not optional: without it the endpoint returns nothing at all.
                ("klasseId", "-1".to_owned()),
                ("startDate", from.format("%Y%m%d").to_string()),
                ("endDate", to.format("%Y%m%d").to_string()),
            ])
            .send()
            .await
            .context("fetching exams")?;

        let status = reply.status();
        let body = reply.text().await?;
        if !status.is_success() {
            bail!("{status} fetching exams: {}", body.chars().take(200).collect::<String>());
        }

        let envelope: ExamEnvelope = serde_json::from_str(&body).context("decoding exams")?;
        let zone = self.settings.timezone;
        Ok(envelope
            .exams()
            .into_iter()
            .filter_map(|raw| raw.into_exam(zone))
            .filter(|exam| exam.end > exam.start)
            .collect())
    }

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
                    start: stamp(raw.start_date, raw.start_time, self.settings.timezone)?,
                    end: stamp(
                        raw.end_date,
                        if raw.end_time == 0 { 2359 } else { raw.end_time },
                        self.settings.timezone,
                    )?,
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
#[derive(Deserialize)]
struct RpcReply<T> {
    result: Option<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcSession {
    #[serde(default)]
    session_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HolidayRaw {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    long_name: String,
    #[serde(default)]
    start_date: i64,
    #[serde(default)]
    end_date: i64,
}

#[derive(Deserialize)]
struct HomeworkEnvelope {
    #[serde(default)]
    data: Option<HomeworkData>,
    #[serde(default)]
    homeworks: Option<Vec<HomeworkRaw>>,
}

#[derive(Deserialize)]
struct HomeworkData {
    #[serde(default)]
    homeworks: Option<Vec<HomeworkRaw>>,
    /// Lesson id to subject. Without it every entry would be nameless.
    #[serde(default)]
    lessons: Option<Vec<HomeworkLesson>>,
}

#[derive(Deserialize)]
struct HomeworkLesson {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    subject: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HomeworkRaw {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    lesson_id: i64,
    #[serde(default)]
    due_date: Option<i64>,
    #[serde(default)]
    date: Option<i64>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    remark: String,
    #[serde(default)]
    completed: bool,
}

impl HomeworkEnvelope {
    fn into_homework(self) -> Vec<Homework> {
        let (raw, lessons) = match self.data {
            Some(data) => (data.homeworks.unwrap_or_default(), data.lessons.unwrap_or_default()),
            None => (self.homeworks.unwrap_or_default(), Vec::new()),
        };
        raw.into_iter()
            .filter_map(|one| {
                // The due date is the point of it; the date it was set is a
                // poor substitute but better than dropping the entry.
                let day = compact_date(one.due_date.or(one.date)?)?;
                Some(Homework {
                    id: one.id,
                    due: day,
                    subject: lessons
                        .iter()
                        .find(|lesson| lesson.id == one.lesson_id)
                        .map(|lesson| lesson.subject.clone())
                        .unwrap_or_default(),
                    text: one.text,
                    remark: one.remark,
                    done: one.completed,
                })
            })
            .collect()
    }
}

/// `20260921` as the classic endpoints spell a date.
fn compact_date(raw: i64) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt((raw / 10_000) as i32, ((raw / 100) % 100) as u32, (raw % 100) as u32)
}

/// `{"data": {"exams": []}}` is what one tenant answered; a bare `{"exams": []}`
/// is what another did. Neither is worth a failed refresh, so both are read.
#[derive(Deserialize)]
struct ExamEnvelope {
    #[serde(default)]
    data: Option<ExamData>,
    #[serde(default)]
    exams: Option<Vec<ExamRaw>>,
}

#[derive(Deserialize)]
struct ExamData {
    #[serde(default)]
    exams: Option<Vec<ExamRaw>>,
}

impl ExamEnvelope {
    fn exams(self) -> Vec<ExamRaw> {
        self.data.and_then(|data| data.exams).or(self.exams).unwrap_or_default()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExamRaw {
    #[serde(default)]
    id: i64,
    // The compact shape.
    #[serde(default)]
    exam_date: Option<i64>,
    #[serde(default)]
    start_time: Option<i64>,
    #[serde(default)]
    end_time: Option<i64>,
    // The ISO shape.
    #[serde(default)]
    start_date_time: Option<String>,
    #[serde(default)]
    end_date_time: Option<String>,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    exam_type: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    teachers: Vec<String>,
    #[serde(default)]
    rooms: Vec<String>,
}

impl ExamRaw {
    fn into_exam(self, zone: Tz) -> Option<Exam> {
        let (start, end) = match (&self.start_date_time, &self.end_date_time) {
            (Some(from), Some(to)) => (parse_stamp(from, zone).ok()?, parse_stamp(to, zone).ok()?),
            _ => {
                let date = self.exam_date?;
                (stamp(date, self.start_time?, zone).ok()?, stamp(date, self.end_time?, zone).ok()?)
            }
        };
        Some(Exam {
            id: self.id,
            start,
            end,
            subject: self.subject,
            kind: self.exam_type,
            name: self.name,
            text: self.text,
            teachers: self.teachers,
            rooms: self.rooms,
        })
    }
}

fn stamp(date: i64, time: i64, zone: Tz) -> Result<Stamp> {
    let date = NaiveDate::from_ymd_opt(
        (date / 10_000) as i32,
        ((date / 100) % 100) as u32,
        (date % 100) as u32,
    )
    .ok_or_else(|| anyhow!("nonsensical date {date}"))?;
    let naive = date
        .and_hms_opt((time / 100) as u32, (time % 100) as u32, 0)
        .ok_or_else(|| anyhow!("nonsensical time {time}"))?;
    localise(naive, zone)
}

/// Midnight at the start of a day, in the school's own zone.
pub fn midnight_in(day: NaiveDate, zone: Tz) -> Option<Stamp> {
    localise(day.and_hms_opt(0, 0, 0)?, zone).ok()
}

/// A wall-clock time in a named zone becometh an instant with a fixed offset.
/// The hour that daylight saving skippeth existeth in no zone; take the next
/// valid one rather than fail a whole week's timetable for it.
fn localise(naive: NaiveDateTime, zone: Tz) -> Result<Stamp> {
    let local = zone
        .from_local_datetime(&naive)
        .earliest()
        .ok_or_else(|| anyhow!("{naive} falleth in no valid hour of {}", zone.name()))?;
    Ok(local.fixed_offset())
}

fn parse_stamp(raw: &str, zone: Tz) -> Result<Stamp> {
    let naive = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M"))
        .with_context(|| format!("unreadable timestamp {raw}"))?;
    localise(naive, zone)
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

    fn into_lesson(self, zone: Tz) -> Result<Lesson> {
        let subject_long = self
            .slots()
            .filter_map(|s| s.current.as_ref())
            .find(|e| e.kind == "SUBJECT")
            .map(|e| e.long_name.clone())
            .unwrap_or_default();

        Ok(Lesson {
            start: parse_stamp(&self.duration.start, zone)?,
            end: parse_stamp(&self.duration.end, zone)?,
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
    #[test]
    fn the_directory_is_read_into_what_the_form_wants() {
        let body = r#"{"result":{"schools":[
            {"server":"neilo.webuntis.com","displayName":"BG Beispiel",
             "loginName":"bg-beispiel","address":"Wien, Beispielgasse 1"},
            {"server":"","displayName":"Nameless","loginName":"","address":""}]}}"#;
        let found: super::SchoolReply = serde_json::from_str(body).unwrap();
        let schools: Vec<_> = found
            .result
            .map(|r| r.schools)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| !s.server.is_empty() && !s.login_name.is_empty())
            .collect();
        assert_eq!(schools.len(), 1, "an entry with no server is of no use to anyone");
        assert_eq!(schools[0].server, "neilo.webuntis.com");
        assert_eq!(schools[0].login_name, "bg-beispiel");
    }

    #[test]
    fn homework_takes_its_subject_from_the_lesson_it_belongs_to() {
        let body = r#"{"data":{"homeworks":[{"id":5,"lessonId":99,"dueDate":20260925,
            "text":"Kapitel 4 lesen","remark":"","completed":false}],
            "lessons":[{"id":99,"subject":"D"}]}}"#;
        let envelope: super::HomeworkEnvelope = serde_json::from_str(body).unwrap();
        let work = envelope.into_homework();
        assert_eq!(work.len(), 1);
        assert_eq!(work[0].title(), "📚 D — Kapitel 4 lesen");
        assert_eq!(work[0].due.to_string(), "2026-09-25");
    }

    #[test]
    fn homework_without_a_matching_lesson_is_still_shown() {
        let body = r#"{"data":{"homeworks":[{"id":6,"lessonId":1,"dueDate":20260925,
            "text":"Vokabeln"}],"lessons":[]}}"#;
        let work = serde_json::from_str::<super::HomeworkEnvelope>(body).unwrap().into_homework();
        assert_eq!(work.len(), 1, "a nameless subject is no reason to drop the work");
        assert_eq!(work[0].title(), "📚 Vokabeln");
    }

    #[test]
    fn homework_falls_back_to_the_day_it_was_set() {
        let body = r#"{"data":{"homeworks":[{"id":7,"lessonId":1,"date":20260921}]}}"#;
        let work = serde_json::from_str::<super::HomeworkEnvelope>(body).unwrap().into_homework();
        assert_eq!(work[0].due.to_string(), "2026-09-21");
        assert_eq!(work[0].title(), "📚 Hausübung", "no text, but still a marker");
    }

    #[test]
    fn exams_are_read_in_the_compact_shape() {
        let body = r#"{"data":{"exams":[{"id":42,"examDate":20260921,"startTime":800,
            "endTime":940,"subject":"M","examType":"Schularbeit","name":"2. SA",
            "text":"Kapitel 1-4","teachers":["MUS"],"rooms":["A1"]}]}}"#;
        let envelope: super::ExamEnvelope = serde_json::from_str(body).unwrap();
        let exams: Vec<_> = envelope
            .exams()
            .into_iter()
            .filter_map(|raw| raw.into_exam(super::DEFAULT_TZ))
            .collect();
        assert_eq!(exams.len(), 1);
        assert_eq!(exams[0].start.format("%H:%M").to_string(), "08:00");
        assert_eq!(exams[0].end.format("%H:%M").to_string(), "09:40");
        assert_eq!(exams[0].title(), "📝 M Schularbeit 2. SA");
        assert!(exams[0].description().contains("Kapitel 1-4"));
    }

    #[test]
    fn exams_are_read_in_the_iso_shape_too() {
        let body = r#"{"exams":[{"id":7,"startDateTime":"2026-09-21T08:00",
            "endDateTime":"2026-09-21T09:40","subject":"E","examType":"Test"}]}"#;
        let envelope: super::ExamEnvelope = serde_json::from_str(body).unwrap();
        let exams: Vec<_> = envelope
            .exams()
            .into_iter()
            .filter_map(|raw| raw.into_exam(super::DEFAULT_TZ))
            .collect();
        assert_eq!(exams.len(), 1, "a tenant answering without the data wrapper is still read");
        assert_eq!(exams[0].title(), "📝 E Test");
    }

    #[test]
    fn an_exam_without_a_usable_time_is_passed_over_not_fatal() {
        let body = r#"{"data":{"exams":[{"id":1,"subject":"M"},
            {"id":2,"examDate":20260921,"startTime":800,"endTime":940,"subject":"D"}]}}"#;
        let envelope: super::ExamEnvelope = serde_json::from_str(body).unwrap();
        let exams: Vec<_> = envelope
            .exams()
            .into_iter()
            .filter_map(|raw| raw.into_exam(super::DEFAULT_TZ))
            .collect();
        assert_eq!(exams.len(), 1, "one unreadable exam must not cost the others");
        assert_eq!(exams[0].id, 2);
    }

    use super::{Rejected, was_rejected};

    #[test]
    fn a_refusal_is_recognised_through_the_context_it_gathers() {
        let err = anyhow::Error::new(Rejected("sent back to the login page".into()))
            .context("logging in")
            .context("refreshing the timetable");
        assert!(was_rejected(&err), "the scheduler must still see the refusal under its context");
    }

    #[test]
    fn an_outage_is_not_mistaken_for_a_refusal() {
        let err = anyhow::anyhow!("connection timed out").context("reaching WebUntis");
        assert!(!was_rejected(&err), "a school that is merely down must be tried again");
    }

    use super::*;

    fn lesson(ids: Vec<i64>, status: Status, day: u32, from: u32, to: u32) -> Lesson {
        let at = |hour: u32| {
            localise(
                NaiveDate::from_ymd_opt(2026, 9, day).unwrap().and_hms_opt(hour, 0, 0).unwrap(),
                DEFAULT_TZ,
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
            start: stamp(20_260_914, 800, DEFAULT_TZ).unwrap(),
            end: stamp(20_260_917, 2155, DEFAULT_TZ).unwrap(),
            reason: "Freistellung".to_owned(),
            text: "Exkursion".to_owned(),
        };
        assert!(leave.covers(&lesson(vec![1], Status::Regular, 14, 8, 9)));
        assert!(leave.covers(&lesson(vec![1], Status::Regular, 16, 8, 9)));
        assert!(leave.covers(&lesson(vec![1], Status::Regular, 17, 15, 16)));
        assert!(!leave.covers(&lesson(vec![1], Status::Regular, 18, 8, 9)));
        assert!(!leave.covers(&lesson(vec![1], Status::Regular, 11, 8, 9)));

        let half = Absence {
            start: stamp(20_260_921, 1200, DEFAULT_TZ).unwrap(),
            end: stamp(20_260_921, 1800, DEFAULT_TZ).unwrap(),
            reason: String::new(),
            text: String::new(),
        };
        assert!(half.covers(&lesson(vec![1], Status::Regular, 21, 13, 14)));
        assert!(!half.covers(&lesson(vec![1], Status::Regular, 21, 8, 9)));
    }

    #[test]
    fn a_school_elsewhere_keeps_its_own_clock() {
        use chrono_tz::{America::New_York, Asia::Tokyo};
        let wall = NaiveDate::from_ymd_opt(2026, 9, 21).unwrap().and_hms_opt(8, 0, 0).unwrap();
        // the same wall-clock hour is a different instant in every school
        let vienna = localise(wall, DEFAULT_TZ).unwrap();
        let tokyo = localise(wall, Tokyo).unwrap();
        let york = localise(wall, New_York).unwrap();
        assert_eq!(vienna.format("%:z").to_string(), "+02:00");
        assert_eq!(tokyo.format("%:z").to_string(), "+09:00");
        assert_eq!(york.format("%:z").to_string(), "-04:00");
        assert_ne!(vienna.to_utc(), tokyo.to_utc());
        assert_ne!(vienna.to_utc(), york.to_utc());
        // and each still readeth as eight in the morning where the school is
        for when in [vienna, tokyo, york] {
            assert_eq!(when.format("%H:%M").to_string(), "08:00");
        }
    }

    #[test]
    fn timestamps_land_in_vienna() {
        let noon = parse_stamp("2026-09-21T08:00", DEFAULT_TZ).unwrap();
        assert_eq!(noon.format("%Y-%m-%d %H:%M %:z").to_string(), "2026-09-21 08:00 +02:00");
        let winter = parse_stamp("2026-12-01T08:00:00", DEFAULT_TZ).unwrap();
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
