//! Rendereth lessons as an iCalendar (RFC 5545) that any calendar may subscribe to.
//!
//! Times go out in UTC with a trailing `Z`, which spareth us a VTIMEZONE block
//! and leaveth no room for a client to guess the zone wrongly.

use chrono::{DateTime, Utc};

use crate::untis::{Exam, Holiday, Homework, Lesson, Status};

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
    /// Subject short names to leave out entirely. Empty leaveth all in.
    pub hide_subjects: &'a [String],
    /// Whether the school's holidays are marked.
    pub with_holidays: bool,
    /// Whether homework is carried at all. Whole-day entries add up, and not
    /// everyone wanteth them in the same calendar as their hours.
    pub with_homework: bool,
    /// How long before an exam to ring, if at all. Ordinary lessons never
    /// ring: a calendar that alarms forty times a week is one nobody keeps.
    pub remind_before: Option<u32>,
}

impl Default for Feed<'_> {
    fn default() -> Self {
        Self {
            name: "Stundenplan",
            refresh_minutes: 60,
            keep_cancelled: true,
            hide_subjects: &[],
            with_holidays: true,
            with_homework: true,
            remind_before: None,
        }
    }
}

impl Feed<'_> {
    pub fn render(
        &self,
        lessons: &[Lesson],
        exams: &[Exam],
        homework: &[Homework],
        holidays: &[Holiday],
        stamp: DateTime<Utc>,
    ) -> String {
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
            if self.hidden(&lesson.subjects) {
                continue;
            }
            self.event(&mut out, lesson, stamp);
        }
        for exam in exams {
            if self.hidden(std::slice::from_ref(&exam.subject)) {
                continue;
            }
            self.exam(&mut out, exam, stamp);
        }
        if self.with_holidays {
            for shut in holidays {
                Self::holiday(&mut out, shut, stamp);
            }
        }
        if self.with_homework {
            for piece in homework {
                if self.hidden(std::slice::from_ref(&piece.subject)) {
                    continue;
                }
                Self::homework(&mut out, piece, stamp);
            }
        }

        line(&mut out, "END:VCALENDAR");
        out
    }

    /// Whether any of these subjects is one this feed leaveth out. Compared
    /// without regard to case, since a student typeth "rk" for "RK".
    fn hidden(&self, subjects: &[String]) -> bool {
        !self.hide_subjects.is_empty()
            && subjects.iter().any(|subject| {
                self.hide_subjects.iter().any(|hidden| hidden.eq_ignore_ascii_case(subject))
            })
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

    /// An exam standeth apart from the lesson it displaceth: it is the thing
    /// a student would set an alarm for, and the only thing here that ringeth.
    fn exam(&self, out: &mut String, exam: &Exam, stamp: DateTime<Utc>) {
        line(out, "BEGIN:VEVENT");
        line(out, &format!("UID:{}@stundenglas", exam.event_id()));
        line(out, &format!("DTSTAMP:{}", utc(stamp)));
        line(out, &format!("DTSTART:{}", utc(exam.start.with_timezone(&Utc))));
        line(out, &format!("DTEND:{}", utc(exam.end.with_timezone(&Utc))));
        line(out, &format!("SUMMARY:{}", escape(&exam.title())));

        let where_at = exam.rooms.join(", ");
        if !where_at.is_empty() {
            line(out, &format!("LOCATION:{}", escape(&where_at)));
        }
        let what = exam.description();
        if !what.is_empty() {
            line(out, &format!("DESCRIPTION:{}", escape(&what)));
        }
        line(out, "TRANSP:OPAQUE");
        line(out, "STATUS:CONFIRMED");
        line(out, "CATEGORIES:EXAM");

        if let Some(minutes) = self.remind_before {
            line(out, "BEGIN:VALARM");
            line(out, "ACTION:DISPLAY");
            line(out, &format!("DESCRIPTION:{}", escape(&exam.title())));
            // Whole days where they divide evenly: a client showing "1 day
            // before" readeth better than one showing "1440 minutes before".
            if minutes % 1440 == 0 && minutes > 0 {
                line(out, &format!("TRIGGER:-P{}D", minutes / 1440));
            } else {
                line(out, &format!("TRIGGER:-PT{minutes}M"));
            }
            line(out, "END:VALARM");
        }
        line(out, "END:VEVENT");
    }

    /// A holiday spanneth whole days and blocketh nothing: it is there to
    /// explain an empty week, not to occupy it.
    fn holiday(out: &mut String, shut: &Holiday, stamp: DateTime<Utc>) {
        line(out, "BEGIN:VEVENT");
        line(out, &format!("UID:{}@stundenglas", shut.event_id()));
        line(out, &format!("DTSTAMP:{}", utc(stamp)));
        line(out, &format!("DTSTART;VALUE=DATE:{}", shut.start.format("%Y%m%d")));
        // The school giveth both ends inclusive; iCalendar wanteth the day
        // after the last.
        line(
            out,
            &format!(
                "DTEND;VALUE=DATE:{}",
                (shut.end + chrono::Duration::days(1)).format("%Y%m%d")
            ),
        );
        line(out, &format!("SUMMARY:{}", escape(&shut.title())));
        line(out, "TRANSP:TRANSPARENT");
        line(out, "STATUS:CONFIRMED");
        line(out, "CATEGORIES:HOLIDAY");
        line(out, "END:VEVENT");
    }

    /// Homework hath a day but no hour, so it goeth out as a whole-day entry
    /// on the day it is due — transparent, because it blocketh no time.
    ///
    /// `DTEND` for a whole-day entry is the morning *after*: the standard
    /// readeth it as exclusive, and a client given the same day showeth
    /// nothing at all.
    fn homework(out: &mut String, piece: &Homework, stamp: DateTime<Utc>) {
        line(out, "BEGIN:VEVENT");
        line(out, &format!("UID:{}@stundenglas", piece.event_id()));
        line(out, &format!("DTSTAMP:{}", utc(stamp)));
        line(out, &format!("DTSTART;VALUE=DATE:{}", piece.due.format("%Y%m%d")));
        line(
            out,
            &format!(
                "DTEND;VALUE=DATE:{}",
                (piece.due + chrono::Duration::days(1)).format("%Y%m%d")
            ),
        );
        line(out, &format!("SUMMARY:{}", escape(&piece.title())));
        let what = piece.description();
        if !what.is_empty() {
            line(out, &format!("DESCRIPTION:{}", escape(&what)));
        }
        line(out, "TRANSP:TRANSPARENT");
        line(out, "STATUS:CONFIRMED");
        line(out, "CATEGORIES:HOMEWORK");
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

    fn homework() -> crate::Homework {
        crate::Homework {
            id: 5,
            due: NaiveDate::from_ymd_opt(2026, 9, 25).unwrap(),
            subject: "D".to_owned(),
            text: "Kapitel 4 lesen".to_owned(),
            remark: String::new(),
            done: false,
        }
    }

    fn exam() -> crate::Exam {
        let at = |h: u32| {
            DEFAULT_TZ
                .from_local_datetime(
                    &NaiveDate::from_ymd_opt(2026, 9, 21).unwrap().and_hms_opt(h, 0, 0).unwrap(),
                )
                .earliest()
                .unwrap()
                .fixed_offset()
        };
        crate::Exam {
            id: 42,
            start: at(8),
            end: at(10),
            subject: "M".to_owned(),
            kind: "Schularbeit".to_owned(),
            name: String::new(),
            text: "Kapitel 1-4".to_owned(),
            teachers: vec!["ABC".to_owned()],
            rooms: vec!["A1".to_owned()],
        }
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
        Feed::default().render(
            lessons,
            &[],
            &[],
            &[],
            Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap(),
        )
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
            &[],
            &[],
            &[],
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
        let out =
            feed.render(&[], &[], &[], &[], Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap());
        assert!(out.contains("REFRESH-INTERVAL;VALUE=DURATION:PT15M"));
        assert!(out.contains("X-PUBLISHED-TTL:PT15M"));
    }

    #[test]
    fn an_exam_is_its_own_entry_and_saith_so() {
        let out = Feed::default().render(
            &[],
            &[exam()],
            &[],
            &[],
            Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap(),
        );
        assert!(out.contains("CATEGORIES:EXAM"), "a calendar should be able to colour them");
        assert!(out.contains("SUMMARY:📝 M Schularbeit"));
        assert!(out.contains("UID:exam42@stundenglas"), "stable across refreshes");
        assert!(out.contains("LOCATION:A1"));
    }

    #[test]
    fn nothing_ringeth_unless_asked() {
        let out = Feed::default().render(
            &[lesson(Status::Regular, "MAT")],
            &[exam()],
            &[],
            &[],
            Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap(),
        );
        assert!(!out.contains("BEGIN:VALARM"), "an unasked-for alarm is an unkept calendar");
    }

    #[test]
    fn an_alarm_rings_for_the_exam_and_not_the_lesson() {
        let feed = Feed { remind_before: Some(30), ..Feed::default() };
        let out = feed.render(
            &[lesson(Status::Regular, "MAT")],
            &[exam()],
            &[],
            &[],
            Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap(),
        );
        assert_eq!(out.matches("BEGIN:VALARM").count(), 1, "only the exam rings");
        assert!(out.contains("TRIGGER:-PT30M"));
    }

    #[test]
    fn a_whole_day_is_said_as_a_day() {
        let feed = Feed { remind_before: Some(1440), ..Feed::default() };
        let out = feed.render(
            &[],
            &[exam()],
            &[],
            &[],
            Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap(),
        );
        assert!(out.contains("TRIGGER:-P1D"), "clients read this better than -PT1440M");
    }

    #[test]
    fn homework_lands_whole_day_on_the_day_it_is_due() {
        let out = Feed::default().render(
            &[],
            &[],
            &[homework()],
            &[],
            Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap(),
        );
        assert!(out.contains("DTSTART;VALUE=DATE:20260925"));
        // Exclusive, per RFC 5545: the same day would show nothing at all.
        assert!(out.contains("DTEND;VALUE=DATE:20260926"));
        assert!(out.contains("SUMMARY:📚 D — Kapitel 4 lesen"));
        assert!(out.contains("TRANSP:TRANSPARENT"), "homework blocks no time");
        assert!(!out.contains("BEGIN:VALARM"), "only exams ring");
    }

    #[test]
    fn a_subject_left_out_takes_its_exams_and_homework_with_it() {
        // The fixtures: lessons in D and M, an exam in M, homework in D.
        let hide_d = vec!["d".to_owned()];
        let feed = Feed { hide_subjects: &hide_d, ..Feed::default() };
        let out = feed.render(
            &[lesson(Status::Regular, "D"), lesson(Status::Regular, "M")],
            &[exam()],
            &[homework()],
            &[],
            Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap(),
        );
        assert!(!out.contains("SUMMARY:📚"), "the homework in D goes with its subject");
        assert!(out.contains("SUMMARY:📝"), "the exam in M stays");
        assert_eq!(out.matches("BEGIN:VEVENT").count(), 2, "one lesson, one exam");

        // And the other way about, which catches a filter applied to only one
        // of the three kinds.
        let hide_m = vec!["M".to_owned()];
        let feed = Feed { hide_subjects: &hide_m, ..Feed::default() };
        let out = feed.render(
            &[lesson(Status::Regular, "D"), lesson(Status::Regular, "M")],
            &[exam()],
            &[homework()],
            &[],
            Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap(),
        );
        assert!(!out.contains("SUMMARY:📝"), "the exam in M goes with its subject");
        assert!(out.contains("SUMMARY:📚"), "the homework in D stays");
        assert_eq!(out.matches("BEGIN:VEVENT").count(), 2, "one lesson, one homework");
    }

    #[test]
    fn a_holiday_spans_its_days_and_blocks_none_of_them() {
        let shut = crate::Holiday {
            id: 3,
            name: "Herbstferien".to_owned(),
            start: NaiveDate::from_ymd_opt(2026, 10, 26).unwrap(),
            end: NaiveDate::from_ymd_opt(2026, 10, 30).unwrap(),
        };
        let out = Feed::default().render(
            &[],
            &[],
            &[],
            &[shut],
            Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap(),
        );
        assert!(out.contains("DTSTART;VALUE=DATE:20261026"));
        // The school gives both ends inclusive; iCalendar wants the day after.
        assert!(out.contains("DTEND;VALUE=DATE:20261031"));
        assert!(out.contains("SUMMARY:🌴 Herbstferien"));
        assert!(
            out.contains("TRANSP:TRANSPARENT"),
            "a holiday explains a week, it does not fill it"
        );
    }

    #[test]
    fn leaving_nothing_out_leaves_everything_in() {
        let feed = Feed::default();
        let out = feed.render(
            &[lesson(Status::Regular, "D")],
            &[exam()],
            &[homework()],
            &[],
            Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap(),
        );
        assert_eq!(out.matches("BEGIN:VEVENT").count(), 3);
    }

    #[test]
    fn a_link_may_refuse_homework_without_refusing_the_rest() {
        let feed = Feed { with_homework: false, ..Feed::default() };
        let out = feed.render(
            &[lesson(Status::Regular, "MAT")],
            &[exam()],
            &[homework()],
            &[],
            Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap(),
        );
        assert!(!out.contains("CATEGORIES:HOMEWORK"));
        assert!(out.contains("CATEGORIES:EXAM"), "the rest is untouched");
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
        assert_eq!(
            feed.render(&[before], &[], &[], &[], stamp),
            feed.render(&[after], &[], &[], &[], stamp)
        );
    }
}
