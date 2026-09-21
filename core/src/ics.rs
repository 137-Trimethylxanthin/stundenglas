//! Rendereth lessons as an iCalendar (RFC 5545) that any calendar may subscribe to.
//!
//! Times go out in UTC with a trailing `Z`, which spareth us a VTIMEZONE block
//! and leaveth no room for a client to guess the zone wrongly.

use chrono::{DateTime, Utc};

use crate::untis::{Lesson, Status};

const PRODID: &str = "-//stundenglas//WebUntis timetable//EN";

/// How a feed shall be rendered.
#[derive(Debug, Clone)]
pub struct Feed<'a> {
    /// Shown as the calendar's name in most clients.
    pub name: &'a str,
    /// How often a client is asked to look again.
    pub refresh_minutes: u32,
    /// Cancelled lessons kept as transparent entries, or left out entirely.
    pub keep_cancelled: bool,
}

impl Default for Feed<'_> {
    fn default() -> Self {
        Self { name: "Stundenplan", refresh_minutes: 60, keep_cancelled: true }
    }
}

impl Feed<'_> {
    pub fn render(&self, lessons: &[Lesson], stamp: DateTime<Utc>) -> String {
        let mut out = String::with_capacity(256 + lessons.len() * 320);

        line(&mut out, "BEGIN:VCALENDAR");
        line(&mut out, "VERSION:2.0");
        line(&mut out, &format!("PRODID:{PRODID}"));
        line(&mut out, "CALSCALE:GREGORIAN");
        line(&mut out, "METHOD:PUBLISH");
        line(&mut out, &format!("NAME:{}", escape(self.name)));
        line(&mut out, &format!("X-WR-CALNAME:{}", escape(self.name)));
        let ttl = format!("PT{}M", self.refresh_minutes.max(1));
        line(&mut out, &format!("REFRESH-INTERVAL;VALUE=DURATION:{ttl}"));
        line(&mut out, &format!("X-PUBLISHED-TTL:{ttl}"));

        for lesson in lessons {
            if lesson.cancelled() && !self.keep_cancelled {
                continue;
            }
            self.event(&mut out, lesson, stamp);
        }

        line(&mut out, "END:VCALENDAR");
        out
    }

    fn event(&self, out: &mut String, lesson: &Lesson, stamp: DateTime<Utc>) {
        line(out, "BEGIN:VEVENT");
        line(out, &format!("UID:{}@stundenglas", lesson.event_id()));
        line(out, &format!("DTSTAMP:{}", utc(stamp)));
        line(out, &format!("DTSTART:{}", utc(lesson.start.with_timezone(&Utc))));
        line(out, &format!("DTEND:{}", utc(lesson.end.with_timezone(&Utc))));
        line(out, &format!("SUMMARY:{}", escape(&lesson.title())));

        let where_at = lesson.rooms.join(", ");
        if !where_at.is_empty() {
            line(out, &format!("LOCATION:{}", escape(&where_at)));
        }
        let what = lesson.description();
        if !what.is_empty() {
            line(out, &format!("DESCRIPTION:{}", escape(&what)));
        }

        // STATUS:CANCELLED would be right by the letter of the standard, but
        // Google and others then hide the entry outright -- and seeing that the
        // hour is off is the whole point. Mark it free instead, and let the ❌
        // in the title carry the meaning.
        if lesson.cancelled() {
            line(out, "TRANSP:TRANSPARENT");
        } else {
            line(out, "TRANSP:OPAQUE");
        }
        line(out, "STATUS:CONFIRMED");

        let kind = if lesson.cancelled() {
            "CANCELLED"
        } else if lesson.is_event {
            "EVENT"
        } else if lesson.status == Status::Changed && lesson.affects_me() {
            "CHANGED"
        } else {
            "REGULAR"
        };
        line(out, &format!("CATEGORIES:{kind}"));
        line(out, "END:VEVENT");
    }
}

fn utc(when: DateTime<Utc>) -> String {
    when.format("%Y%m%dT%H%M%SZ").to_string()
}

/// RFC 5545 §3.3.11: backslash, semicolon and comma are escaped, and a newline
/// becometh a literal `\n`.
fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 8);
    for ch in raw.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(ch),
        }
    }
    out
}

/// RFC 5545 §3.1: no line may exceed 75 octets, and a folded line continueth
/// after CRLF and a single space. Folding counteth octets, never characters,
/// but may not cleave one character in two.
fn line(out: &mut String, content: &str) {
    const LIMIT: usize = 73;
    let mut used = 0;
    for ch in content.chars() {
        let width = ch.len_utf8();
        if used + width > LIMIT {
            out.push_str("\r\n ");
            used = 1;
        }
        out.push(ch);
        used += width;
    }
    out.push_str("\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::untis::{DEFAULT_TZ, Lesson, Status};
    use chrono::{NaiveDate, TimeZone};

    pub(super) fn sample_lesson() -> Lesson {
        let mut l = lesson(Status::Changed, "MAT");
        l.teachers_removed = vec!["ABC".to_owned()];
        l.lesson_info = "Gruppe 1".to_owned();
        l.notes = "Ümläüte, Kommas; und \\ Schrägstriche".to_owned();
        l
    }

    fn lesson(status: Status, subject: &str) -> Lesson {
        let at = |h: u32| {
            DEFAULT_TZ
                .from_local_datetime(
                    &NaiveDate::from_ymd_opt(2026, 9, 21).unwrap().and_hms_opt(h, 0, 0).unwrap(),
                )
                .earliest()
                .unwrap()
                .fixed_offset()
        };
        Lesson {
            ids: vec![5_856_529, 5_856_532],
            start: at(8),
            end: at(10),
            status,
            is_event: false,
            subjects: vec![subject.to_owned()],
            subject_long: "Mathematik".to_owned(),
            info: vec![],
            teachers: vec!["ABC".to_owned()],
            rooms: vec!["R1".to_owned()],
            classes: vec![],
            teachers_removed: vec![],
            rooms_removed: vec![],
            lesson_info: String::new(),
            lesson_text: String::new(),
            substitution_text: String::new(),
            notes: String::new(),
        }
    }

    fn render(lessons: &[Lesson]) -> String {
        Feed::default().render(lessons, Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap())
    }

    #[test]
    fn wraps_every_event_in_a_calendar() {
        let out = render(&[lesson(Status::Regular, "MAT")]);
        assert!(out.starts_with("BEGIN:VCALENDAR\r\n"));
        assert!(out.ends_with("END:VCALENDAR\r\n"));
        assert_eq!(out.matches("BEGIN:VEVENT").count(), 1);
        assert_eq!(out.matches("END:VEVENT").count(), 1);
    }

    #[test]
    fn every_line_ends_crlf_and_fits_in_75_octets() {
        let mut long = lesson(Status::Regular, "MAT");
        long.lesson_text = "ä".repeat(200); // multi-byte, to catch octet mistakes
        let out = render(&[long]);
        assert!(!out.contains("\n\r"), "line endings must be CRLF");
        for raw in out.split("\r\n") {
            assert!(raw.len() <= 75, "line of {} octets: {raw:?}", raw.len());
        }
        // folding must not have cloven a character in two
        assert!(out.is_char_boundary(out.len()));
        assert_eq!(out.matches('ä').count(), 200);
    }

    #[test]
    fn times_go_out_in_utc() {
        // 08:00 Vienna in September is 06:00Z
        let out = render(&[lesson(Status::Regular, "MAT")]);
        assert!(out.contains("DTSTART:20260921T060000Z"), "{out}");
        assert!(out.contains("DTEND:20260921T080000Z"));
        // and in winter the offset differs
        assert!(!out.contains("DTSTART:20260921T070000Z"));
    }

    #[test]
    fn uids_are_stable_and_unique() {
        let out = render(&[lesson(Status::Regular, "MAT")]);
        assert!(out.contains("UID:u5856529p5856532@stundenglas"));
    }

    #[test]
    fn cancelled_stays_visible_but_free() {
        let out = render(&[lesson(Status::Cancelled, "MAT")]);
        assert!(out.contains("TRANSP:TRANSPARENT"));
        assert!(out.contains("CATEGORIES:CANCELLED"));
        // never STATUS:CANCELLED -- clients would hide it
        assert!(!out.contains("STATUS:CANCELLED"));
        assert!(out.contains("SUMMARY:❌ MAT"));
    }

    #[test]
    fn cancelled_can_be_left_out_entirely() {
        let feed = Feed { keep_cancelled: false, ..Feed::default() };
        let out = feed.render(
            &[lesson(Status::Cancelled, "MAT")],
            Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap(),
        );
        assert_eq!(out.matches("BEGIN:VEVENT").count(), 0);
    }

    #[test]
    fn specials_are_escaped() {
        assert_eq!(escape("a,b;c\\d\ne"), "a\\,b\\;c\\\\d\\ne");
        let mut tricky = lesson(Status::Regular, "MAT");
        tricky.rooms = vec!["R1, R2".to_owned()];
        let out = render(&[tricky]);
        assert!(out.contains("LOCATION:R1\\, R2"));
    }

    #[test]
    fn a_refresh_hint_is_offered() {
        let feed = Feed { refresh_minutes: 15, ..Feed::default() };
        let out = feed.render(&[], Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap());
        assert!(out.contains("REFRESH-INTERVAL;VALUE=DURATION:PT15M"));
        assert!(out.contains("X-PUBLISHED-TTL:PT15M"));
    }
}

#[cfg(test)]
mod round_trip {
    use crate::untis::Lesson;

    /// The cache in Postgres holdeth lessons as JSON; what goeth in must come
    /// out the same, or a feed would drift from what was fetched.
    #[test]
    fn lessons_survive_a_turn_through_json() {
        let before = super::tests::sample_lesson();
        let text = serde_json::to_string(&before).unwrap();
        let after: Lesson = serde_json::from_str(&text).unwrap();
        assert_eq!(before.ids, after.ids);
        assert_eq!(before.start, after.start, "the instant must not shift");
        assert_eq!(before.end, after.end);
        assert_eq!(before.title(), after.title());
        assert_eq!(before.description(), after.description());
        assert_eq!(before.event_id(), after.event_id());
        // and the rendering is byte-identical
        let stamp = chrono::Utc::now();
        let feed = super::Feed::default();
        assert_eq!(feed.render(&[before], stamp), feed.render(&[after], stamp));
    }
}
