//! The parts that know nothing of how they are driven: reading a timetable,
//! rendering it as a calendar, and pushing it into Google.

pub mod gcal;
pub mod ics;
pub mod untis;

pub use untis::{Absence, Credentials, Lesson, Status, TZ};
