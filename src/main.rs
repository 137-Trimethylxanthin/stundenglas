//! Bringeth the WebUntis timetable into Google Calendar, and keepeth it so.

mod config;
mod gcal;
mod oauth;
mod untis;

use anyhow::{Context, Result};
use chrono::{Duration, Local, NaiveDate, NaiveTime, TimeZone};
use clap::Parser;
use std::path::PathBuf;

use config::Settings;
use untis::TZ;

#[derive(Parser, Debug)]
#[command(
    name = "stundenglas",
    about = "Keeps a personal WebUntis timetable in step with Google Calendar",
    version
)]
struct Args {
    /// First day to sync (YYYY-MM-DD).
    #[arg(long = "from", value_name = "DATE")]
    start: Option<NaiveDate>,

    /// Last day to sync, inclusive (YYYY-MM-DD).
    #[arg(long = "to", value_name = "DATE")]
    end: Option<NaiveDate>,

    /// Weeks ahead when no explicit range is given.
    #[arg(long, default_value_t = 4)]
    weeks: i64,

    /// Days of history to keep in view.
    #[arg(long, default_value_t = 7)]
    back: i64,

    /// Sync the whole current school year.
    #[arg(long)]
    all_year: bool,

    /// Name of the target calendar.
    #[arg(long, default_value = gcal::CALENDAR_NAME)]
    calendar: String,

    /// Show the plan and write nothing.
    #[arg(long)]
    dry_run: bool,

    /// Keep lessons that fall inside a leave of absence.
    #[arg(long)]
    keep_during_absence: bool,

    /// Requests in flight against Google at once.
    #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u16).range(1..=32))]
    lanes: u16,

    /// Directory holding .env, credentials.json and token.json.
    #[arg(long, default_value = ".")]
    dir: PathBuf,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(0) => std::process::ExitCode::SUCCESS,
        Ok(_) => std::process::ExitCode::FAILURE,
        Err(err) => {
            eprintln!("error: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<usize> {
    let args = Args::parse();

    let settings = Settings::load(&args.dir.join(".env"))?;
    let mut untis = untis::Client::new(settings)?;
    untis.login().await.context("logging in to WebUntis")?;

    let (from, to) = window(&args, untis.year);
    println!("WebUntis: {} (personId {})", untis.person_name, untis.person_id);
    println!("Window  : {from} → {to}");

    let mut lessons = untis.fetch(from, to).await.context("fetching the timetable")?;

    if !args.keep_during_absence {
        for leave in untis.fetch_absences(from, to).await.context("fetching absences")? {
            let before = lessons.len();
            lessons.retain(|lesson| !leave.covers(lesson));
            println!(
                "Absence : {} → {}  {}  ({} lessons hidden)",
                leave.start.format("%d.%m %H:%M"),
                leave.end.format("%d.%m %H:%M"),
                leave.label(),
                before - lessons.len()
            );
        }
    }

    let cancelled = lessons.iter().filter(|l| l.cancelled()).count();
    let changed =
        lessons.iter().filter(|l| l.status == untis::Status::Changed && l.affects_me()).count();
    let events = lessons.iter().filter(|l| l.is_event).count();
    println!(
        "Fetched : {} lessons ({cancelled} cancelled, {changed} changed, {events} events)",
        lessons.len()
    );

    let http = reqwest::Client::builder()
        .user_agent(concat!("stundenglas/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let authoriser = oauth::Authoriser::new(
        http.clone(),
        args.dir.join("credentials.json"),
        args.dir.join("token.json"),
    );
    let token = authoriser.access_token().await.context("authorising with Google")?;
    let calendar = gcal::Calendar::new(http, token);

    let calendar_id = calendar.find_or_create(&args.calendar).await?;
    println!("Calendar: {}  ({calendar_id})", args.calendar);

    let midnight = NaiveTime::from_hms_opt(0, 0, 0).expect("midnight existeth");
    let since = TZ
        .from_local_datetime(&from.and_time(midnight))
        .earliest()
        .context("start of window falleth in no valid hour")?;
    let until = TZ
        .from_local_datetime(&(to + Duration::days(1)).and_time(midnight))
        .earliest()
        .context("end of window falleth in no valid hour")?;

    let existing = calendar.existing(&calendar_id, since, until).await?;
    let plan = gcal::plan(&lessons, &existing);

    println!(
        "\nPlan    : +{} new  ~{} changed  -{} removed  ={} unchanged",
        plan.inserts.len(),
        plan.updates.len(),
        plan.deletes.len(),
        plan.unchanged
    );
    for event in &plan.inserts {
        println!("  + {}  {}", event.start.format("%Y-%m-%d %H:%M"), event.summary);
    }
    for event in &plan.updates {
        println!("  ~ {}  {}", event.start.format("%Y-%m-%d %H:%M"), event.summary);
    }
    for event in &plan.deletes {
        println!("  - {}  {}", event.start.chars().take(16).collect::<String>(), event.summary);
    }

    if args.dry_run {
        println!("\n(dry run — nothing was written)");
        return Ok(0);
    }
    if plan.is_empty() {
        println!("\nNothing to do.");
        return Ok(0);
    }

    let tally = calendar.apply(&calendar_id, &plan, args.lanes as usize).await?;
    println!(
        "\nDone    : {} inserted, {} updated, {} deleted, {} failed",
        tally.inserted, tally.updated, tally.deleted, tally.failed
    );
    Ok(tally.failed)
}

fn window(args: &Args, year: (NaiveDate, NaiveDate)) -> (NaiveDate, NaiveDate) {
    if args.all_year {
        return year;
    }
    if let (Some(start), Some(end)) = (args.start, args.end) {
        return (start, end);
    }
    let today = Local::now().date_naive();
    (today - Duration::days(args.back), today + Duration::weeks(args.weeks))
}
