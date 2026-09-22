//! The pages. Server-rendered, no script, no framework: a school timetable
//! needeth none of it.

use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;
use chrono_tz::Tz;
use maud::{DOCTYPE, Markup, html};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::{self, CurrentUser};
use crate::db::{self, NewAccount};
use crate::words::{Lang, Words};
use crate::{AppState, people, schools};

// A short list of the zones a European school is likely to keep, with the rest
// reachable by typing. Better than a thousand-line dropdown.
const COMMON_ZONES: &[&str] = &[
    "Europe/Vienna",
    "Europe/Berlin",
    "Europe/Zurich",
    "Europe/Prague",
    "Europe/Budapest",
    "Europe/Ljubljana",
    "Europe/Rome",
    "Europe/Madrid",
    "Europe/Paris",
    "Europe/London",
    "Europe/Warsaw",
    "Europe/Bucharest",
    "UTC",
];

fn page(lang: Lang, title: &str, user: Option<CurrentUser>, body: Markup) -> Markup {
    let w = lang.words();
    html! {
        (DOCTYPE)
        html lang=(lang.code()) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                // The tab colour follows the page, so a pinned tab is not a
                // white slab beside a dark one.
                meta name="color-scheme" content="light dark";
                meta name="description" content="A WebUntis timetable as a calendar you can subscribe to.";
                title { (title) " — stundenglas" }
                style { (maud::PreEscaped(STYLE)) }
            }
            body {
                header {
                    a.brand href="/" { (MARK) "stundenglas" }
                    nav {
                        @if user.is_some() {
                            form method="post" action="/logout" {
                                button.link type="submit" { (w.sign_out) }
                            }
                        }
                    }
                }
                main { (body) }
                footer {
                    (w.footer) " · "
                    a href="/privacy" { (w.privacy_link) }
                    " · "
                    // Plain links: a language toggle that needs script is one
                    // that fails for whoever most needs it.
                    a href={ "/language/" (lang.other().code()) } { (lang.other().own_name()) }
                }
            }
        }
    }
}

/// An hourglass, drawn rather than fetched: one more request for one small
/// picture is one more thing to fail, and a service about a school timetable
/// should not reach out to a font CDN to draw its own name.
const MARK: maud::PreEscaped<&str> = maud::PreEscaped(
    r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
            stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
         <path d="M6 2h12M6 22h12"/>
         <path d="M7 2v4.5a5 5 0 0 0 2.5 4.3L12 12l-2.5 1.2A5 5 0 0 0 7 17.5V22"/>
         <path d="M17 2v4.5a5 5 0 0 1-2.5 4.3L12 12l2.5 1.2a5 5 0 0 1 2.5 4.3V22"/>
       </svg>"#,
);

const STYLE: &str = r#"
/* ---------------------------------------------------------------- tokens --
   One palette, stated twice. Light first, because that is what a school
   projector shows; dark follows the system, since a timetable is mostly
   looked at late.                                                          */
:root {
  color-scheme: light dark;

  --ink:        oklch(23% 0.02 260);
  --ink-soft:   oklch(48% 0.02 260);
  --ink-faint:  oklch(62% 0.02 260);
  --paper:      oklch(99% 0.004 260);
  --raised:     oklch(100% 0 0);
  --sunken:     oklch(97% 0.006 260);
  --edge:       oklch(90% 0.008 260);
  --edge-firm:  oklch(84% 0.012 260);

  --accent:     oklch(52% 0.19 275);
  --accent-ink: oklch(99% 0.01 275);
  --accent-wash: oklch(96% 0.03 275);

  --warn:       oklch(58% 0.14 75);
  --warn-wash:  oklch(96% 0.05 75);
  --bad:        oklch(53% 0.20 25);
  --bad-wash:   oklch(96% 0.04 25);
  --good:       oklch(52% 0.13 155);

  --lift: 0 1px 2px oklch(23% 0.02 260 / 0.05),
          0 4px 12px oklch(23% 0.02 260 / 0.04);
  --ring: 0 0 0 3px oklch(52% 0.19 275 / 0.35);

  --radius: 0.7rem;
  --radius-sm: 0.45rem;
  --gutter: clamp(1rem, 4vw, 2.5rem);
  --measure: 46rem;
}

@media (prefers-color-scheme: dark) {
  :root {
    --ink:        oklch(93% 0.012 260);
    --ink-soft:   oklch(74% 0.015 260);
    --ink-faint:  oklch(60% 0.015 260);
    --paper:      oklch(17% 0.015 265);
    --raised:     oklch(21% 0.016 265);
    --sunken:     oklch(19% 0.016 265);
    --edge:       oklch(28% 0.018 265);
    --edge-firm:  oklch(36% 0.02 265);

    --accent:     oklch(72% 0.15 275);
    --accent-ink: oklch(18% 0.03 275);
    --accent-wash: oklch(26% 0.05 275);

    --warn:       oklch(78% 0.13 75);
    --warn-wash:  oklch(28% 0.05 75);
    --bad:        oklch(70% 0.16 25);
    --bad-wash:   oklch(27% 0.06 25);
    --good:       oklch(72% 0.13 155);

    --lift: 0 1px 2px oklch(0% 0 0 / 0.3), 0 6px 16px oklch(0% 0 0 / 0.25);
  }
}

/* ------------------------------------------------------------- the frame -- */
* { box-sizing: border-box; }

body {
  margin: 0;
  background: var(--paper);
  color: var(--ink);
  font: 16px/1.6 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
  -webkit-text-size-adjust: 100%;
}

header {
  display: flex; justify-content: space-between; align-items: center; gap: 1rem;
  padding: 0.9rem var(--gutter);
  border-bottom: 1px solid var(--edge);
  background: var(--raised);
  position: sticky; top: 0; z-index: 5;
}
.brand {
  display: inline-flex; align-items: center; gap: 0.5rem;
  font-weight: 650; letter-spacing: -0.015em; text-decoration: none; color: inherit;
}
.brand svg { width: 1.15rem; height: 1.15rem; color: var(--accent); }
header nav { display: flex; align-items: center; gap: 0.9rem; font-size: 0.9rem; }

main { max-width: var(--measure); margin: 0 auto; padding: 2.25rem var(--gutter) 3rem; }
footer {
  max-width: var(--measure); margin: 0 auto;
  padding: 0 var(--gutter) 3rem;
  /* Soft rather than faint: at 0.85rem the faint tone falls under 4.5:1 on
     white, and a footer is still something people read. */
  color: var(--ink-soft); font-size: 0.85rem;
}
footer a { color: inherit; }

/* --------------------------------------------------------------- reading -- */
h1 { font-size: clamp(1.5rem, 1.2rem + 1.4vw, 2rem); line-height: 1.2;
     letter-spacing: -0.025em; margin: 0 0 0.4rem; font-weight: 680; }
h2 { font-size: 1.1rem; letter-spacing: -0.012em; margin: 2.75rem 0 0.85rem;
     font-weight: 640; }
h3 { margin: 0; font-size: 1rem; font-weight: 620; letter-spacing: -0.01em; }
p { margin: 0 0 0.9rem; }
p.lede { color: var(--ink-soft); font-size: 1.05rem; margin-top: 0; }
ul { padding-left: 1.15rem; }
li { margin-bottom: 0.35rem; }
a { color: var(--accent); text-underline-offset: 0.15em; }
strong { font-weight: 640; }
.meta { font-size: 0.85rem; color: var(--ink-soft); margin: 0; }
.meta + .meta { margin-top: 0.15rem; }
.note {
  border-left: 2px solid var(--edge-firm);
  padding: 0.1rem 0 0.1rem 0.9rem;
  color: var(--ink-soft); font-size: 0.92rem;
}
.bad { color: var(--bad); }
code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.88em; }

/* ---------------------------------------------------------------- fields -- */
input, select, button, textarea { font: inherit; color: inherit; }
label { display: block; margin: 0.9rem 0 0.3rem; font-size: 0.875rem;
        font-weight: 550; color: var(--ink-soft); }
input[type="text"], input[type="email"], input[type="password"], select {
  width: 100%; padding: 0.6rem 0.75rem;
  border: 1px solid var(--edge-firm); border-radius: var(--radius-sm);
  background: var(--paper);
  transition: border-color 0.15s, box-shadow 0.15s;
}
input:hover, select:hover { border-color: var(--ink-faint); }
input:focus-visible, select:focus-visible, button:focus-visible, a:focus-visible,
summary:focus-visible {
  outline: none; border-color: var(--accent); box-shadow: var(--ring);
}
input::placeholder { color: var(--ink-faint); }
label:has(input[type="checkbox"]) {
  display: flex; align-items: center; gap: 0.5rem;
  font-weight: 450; color: var(--ink); cursor: pointer;
}
input[type="checkbox"] { width: auto; accent-color: var(--accent); margin: 0; }
form.stack { margin-bottom: 1rem; }
.row { display: flex; gap: 0.85rem; flex-wrap: wrap; }
.row > * { flex: 1 1 13rem; min-width: 0; }

/* --------------------------------------------------------------- buttons -- */
button {
  display: inline-flex; align-items: center; justify-content: center; gap: 0.4rem;
  padding: 0.55rem 0.95rem;
  border: 1px solid var(--edge-firm); border-radius: var(--radius-sm);
  background: var(--raised);
  font-size: 0.925rem; font-weight: 550; line-height: 1.35;
  cursor: pointer;
  transition: background 0.15s, border-color 0.15s, transform 0.06s;
}
button:hover { background: var(--sunken); border-color: var(--ink-faint); }
button:active { transform: translateY(1px); }
/* One primary action per form, so the eye knows where to go. */
form.stack > p + button, button.primary {
  background: var(--accent); border-color: var(--accent); color: var(--accent-ink);
}
form.stack > p + button:hover, button.primary:hover {
  background: color-mix(in oklab, var(--accent) 88%, black); border-color: transparent;
}
button.link {
  border: 0; background: none; padding: 0; color: var(--ink-soft);
  text-decoration: underline; font-weight: 450;
}
button.link:hover { background: none; color: var(--ink); }
button.danger { border-color: color-mix(in oklab, var(--bad) 45%, transparent); color: var(--bad); }
button.danger:hover { background: var(--bad-wash); border-color: var(--bad); }
.actions { display: flex; gap: 0.5rem; flex-wrap: wrap; margin-top: 0.75rem; }
.actions form { margin: 0; }
.actions a { text-decoration: none; }

/* ----------------------------------------------------------------- cards -- */
.card {
  border: 1px solid var(--edge); border-radius: var(--radius);
  background: var(--raised); box-shadow: var(--lift);
  padding: 1.1rem 1.25rem; margin: 0.85rem 0;
}
.card-head {
  display: flex; align-items: baseline; justify-content: space-between;
  gap: 0.75rem; flex-wrap: wrap;
}

/* A word for how a link is faring, so the state is seen before it is read. */
.badge {
  display: inline-flex; align-items: center; gap: 0.35rem;
  padding: 0.15rem 0.55rem; border-radius: 999px;
  font-size: 0.78rem; font-weight: 600; letter-spacing: 0.01em;
  white-space: nowrap;
}
.badge::before { content: ""; width: 0.45rem; height: 0.45rem; border-radius: 50%;
                 background: currentColor; }
.badge.ok   { background: color-mix(in oklab, var(--good) 14%, transparent); color: var(--good); }
.badge.warn { background: var(--warn-wash); color: var(--warn); }
.badge.bad  { background: var(--bad-wash); color: var(--bad); }
.badge.idle { background: var(--sunken); color: var(--ink-soft); }

/* A refused password stops everything, so it is framed rather than mentioned. */
.notice {
  border: 1px solid color-mix(in oklab, var(--bad) 40%, transparent);
  background: var(--bad-wash);
  border-radius: var(--radius-sm);
  padding: 0.9rem 1rem; margin: 0.9rem 0;
}
.notice p:last-of-type { margin-bottom: 0; }
.notice label { color: inherit; }

/* ------------------------------------------------------------ the feed -- */
.feedbox {
  background: var(--sunken); border: 1px solid var(--edge);
  border-radius: var(--radius-sm); padding: 0.7rem 0.8rem; margin: 0.85rem 0 0;
}
code.feed {
  display: block; word-break: break-all; font-size: 0.8rem; line-height: 1.5;
  color: var(--ink-soft);
}
.feedbox .actions { margin-top: 0.65rem; }

details { margin: 0.6rem 0; }
details summary {
  cursor: pointer; font-size: 0.9rem; font-weight: 550; color: var(--ink-soft);
  padding: 0.3rem 0; list-style: none; display: flex; align-items: center; gap: 0.4rem;
}
details summary::-webkit-details-marker { display: none; }
details summary::before {
  content: "›"; display: inline-block; transition: transform 0.15s;
  font-size: 1.1em; line-height: 1;
}
details[open] summary::before { transform: rotate(90deg); }
details summary:hover { color: var(--ink); }
details > *:not(summary) { margin-left: 1.05rem; }

.qrbox {
  background: #fff; padding: 0.7rem; border-radius: var(--radius-sm);
  width: max-content; margin-top: 0.5rem; box-shadow: var(--lift);
}
.qrbox svg { display: block; width: 190px; height: 190px; }

.subjects { display: flex; gap: 0.4rem; flex-wrap: wrap; margin: 0.4rem 0 0.2rem; }
.pill {
  display: inline-flex; align-items: center; gap: 0.35rem; margin: 0;
  border: 1px solid var(--edge-firm); border-radius: 999px;
  padding: 0.22rem 0.7rem 0.22rem 0.55rem;
  font-size: 0.85rem; font-weight: 450; color: var(--ink); cursor: pointer;
  transition: background 0.12s, border-color 0.12s;
}
.pill:hover { border-color: var(--ink-faint); background: var(--sunken); }
.pill:has(input:checked) {
  background: var(--accent-wash); border-color: color-mix(in oklab, var(--accent) 50%, transparent);
  color: color-mix(in oklab, var(--accent) 75%, var(--ink));
}

/* ------------------------------------------------------------ the front -- */
.hero { margin-bottom: 2rem; }
.hero h1 { max-width: 20ch; }
.points { list-style: none; padding: 0; margin: 1.25rem 0 0; display: grid;
          gap: 0.55rem; font-size: 0.95rem; color: var(--ink-soft); }
.points li { display: flex; gap: 0.6rem; align-items: flex-start; margin: 0; }
.points li::before { content: "✓"; color: var(--accent); font-weight: 700; }

@media (prefers-reduced-motion: reduce) {
  * { transition: none !important; animation: none !important; }
}
"#;

// ----------------------------------------------------------------- landing ---

#[derive(Deserialize, Default)]
pub struct Prefill {
    #[serde(default)]
    server: String,
    #[serde(default)]
    school: String,
}

pub async fn index(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Query(chosen): Query<Prefill>,
) -> Response {
    let lang = Lang::of(&jar, &headers);
    let w = lang.words();
    match current(&state, &jar).await {
        Some(user) => dashboard_with(state, user, None, chosen, lang).await,
        None => page(lang, w.sign_in, None, front(w)).into_response(),
    }
}

// --------------------------------------------------------------- dashboard ---

/// The feed address as a QR, drawn inline. Nothing is fetched and nothing
/// leaves the page — a secret URL has no business being sent to an image
/// service to be made scannable.
fn qr_svg(url: &str) -> Option<maud::PreEscaped<String>> {
    use qrcode::render::svg;
    use qrcode::{EcLevel, QrCode};

    let code = QrCode::with_error_correction_level(url, EcLevel::M).ok()?;
    let drawn = code
        .render()
        .min_dimensions(190, 190)
        // Fixed colours: a QR wants light behind dark whatever the page
        // around it is doing.
        .dark_color(svg::Color("#111111"))
        .light_color(svg::Color("#ffffff"))
        .quiet_zone(true)
        .build();

    // The renderer writes a standalone document, prolog and all. Inside a page
    // that prolog is not markup but litter, so it is cut away.
    let from = drawn.find("<svg")?;
    Some(maud::PreEscaped(drawn[from..].to_owned()))
}

/// The page a stranger sees. Apart so that it may be looked at without a
/// database behind it, which is how it came to be looked at at all.
fn front(w: &'static Words) -> Markup {
    html! {
                .hero {
                    h1 { (w.welcome) }
                    p.lede { (w.lede) }
                    ul.points {
                        li { (w.point_cancelled) }
                        li { (w.point_exams) }
                        li { (w.point_nothing) }
                    }
                }
                div.row {
                    form.stack.card method="post" action="/signup" {
                        h2 { (w.create_account) }
                        label for="su-email" { (w.email) }
                        input #su-email type="email" name="email" required autocomplete="email";
                        label for="su-pw" { (w.password_ten) }
                        input #su-pw type="password" name="password" required
                              autocomplete="new-password" minlength="10";
                        p {} button type="submit" { (w.sign_up) }
                    }
                    form.stack.card method="post" action="/login" {
                        h2 { (w.sign_in) }
                        label for="li-email" { (w.email) }
                        input #li-email type="email" name="email" required autocomplete="email";
                        label for="li-pw" { (w.password) }
                        input #li-pw type="password" name="password" required
                              autocomplete="current-password";
                        p {} button type="submit" { (w.sign_in) }
                    }
                }
                p.note { (w.front_warning) " " a href="/privacy" { (w.privacy_link) } "." }
    }
}

/// How a link is faring, in the one word a badge has room for.
enum Standing {
    Well,
    Failing,
    Refused,
    Waiting,
}

fn standing(account: &db::UntisAccount, status: Option<&db::SyncStatus>) -> Standing {
    if account.credentials_rejected {
        return Standing::Refused;
    }
    match status {
        Some(state) if state.last_error.is_some() => Standing::Failing,
        Some(state) if state.last_ok_at.is_some() => Standing::Well,
        _ => Standing::Waiting,
    }
}

async fn dashboard(
    state: AppState,
    user: CurrentUser,
    problem: Option<String>,
    lang: Lang,
) -> Response {
    dashboard_with(state, user, problem, Prefill::default(), lang).await
}

async fn dashboard_with(
    state: AppState,
    user: CurrentUser,
    problem: Option<String>,
    chosen: Prefill,
    lang: Lang,
) -> Response {
    let w = lang.words();
    let me = people::profile(&state.pool, user.id).await;
    let admin = me.as_ref().is_some_and(|p| p.is_admin);
    if !me.as_ref().is_some_and(|p| p.approved) {
        return waiting_room(admin, lang);
    }
    let accounts = db::accounts_of(&state.pool, user.id).await.unwrap_or_default();

    let mut cards = Vec::new();
    for account in &accounts {
        let feeds = db::feeds_of(&state.pool, account.id).await.unwrap_or_default();
        let state_of = db::sync_status(&state.pool, account.id).await.unwrap_or(None);
        let google = crate::google::has_link(&state.pool, account.id).await;
        let subjects = db::subjects_of(&state.pool, account.id).await.unwrap_or_default();
        cards.push((account.clone(), feeds, state_of, google, subjects));
    }

    let has_google = state.config.google.is_some();
    // Filled in when they came back from the school search, empty otherwise.
    let prefill = (schools::tidy_server(&chosen.server), chosen.school.trim().to_owned());
    page(
        lang,
        w.your_timetables,
        Some(user),
        html! {
            h1 { (w.your_timetables) }
            p.lede { (w.one_link_each) }
            p.meta {
                a href="/security" { (w.security_keys) }
                @if admin { " · " a href="/admin" { (w.who_may_join) } }
            }
            @if has_google {
                p.note { (w.google_note) }
            }

            @if let Some(why) = &problem {
                p.bad { (why) }
            }

            @for (account, feeds, status, google, subjects) in &cards {
                .card {
                    .card-head {
                        h3 {
                            (account.display_name.clone()
                                .unwrap_or_else(|| account.username.clone()))
                        }
                        // The state before the detail: a glance should be
                        // enough to know whether anything wants doing.
                        @match standing(account, status.as_ref()) {
                            Standing::Refused => span.badge.bad { (w.state_refused) }
                            Standing::Failing => span.badge.warn { (w.state_failing) }
                            Standing::Waiting => span.badge.idle { (w.state_waiting) }
                            Standing::Well => span.badge.ok { (w.state_well) }
                        }
                    }
                    p.meta {
                        (account.school) " · " (account.server) " · " (account.timezone.name())
                    }
                    @if account.credentials_rejected {
                        .bad.notice {
                            p {
                                strong { (w.refused_title) }
                                (w.refused_body)
                            }
                            form.stack method="post" action={ "/links/" (account.id) "/password" } {
                                label for={ "pw-" (account.id) } { (w.untis_password) }
                                input id={ "pw-" (account.id) } type="password" name="password"
                                      required autocomplete="off";
                                p {} button type="submit" { (w.try_this_one) }
                            }
                        }
                    }
                    p.meta {
                        @match status {
                            Some(s) if s.last_error.is_some() => {
                                span.bad { (w.last_refresh_failed)
                                    (s.last_error.clone().unwrap_or_default()) }
                            }
                            Some(s) => {
                                (s.lesson_count) " " (w.lessons)
                                @if s.exam_count > 0 { ", " (s.exam_count) " " (w.exams) }
                                @if s.homework_count > 0 {
                                    ", " (s.homework_count) " " (w.homework_count)
                                }
                                @if let Some(when) = s.last_ok_at {
                                    ", " (w.refreshed) " " (when.format("%d %b %H:%M UTC").to_string())
                                }
                            }
                            None => { (w.waiting_first) }
                        }
                    }
                    @for feed in feeds {
                        .feedbox {
                            @if let Some(name) = &feed.label {
                                p.meta { (name) }
                            }
                            code.feed { (state.config.feed_url(&feed.token)) }
                            .actions {
                                a href=(state.config.webcal_url(&feed.token)) {
                                    button.primary type="button" { (w.subscribe_here) }
                                }
                            }
                        @if let Some(qr) = qr_svg(&state.config.feed_url(&feed.token)) {
                            details {
                                summary { (w.show_qr) }
                                p.meta { (w.qr_warning) }
                                .qrbox { (qr) }
                            }
                        }
                        details {
                            summary { (w.link_settings) }
                            form.stack method="post"
                                 action={ "/feeds/" (feed.id) "/settings" } {
                                label { (w.what_to_call_it) }
                                input type="text" name="label" maxlength="60"
                                      placeholder="Phone, laptop, …"
                                      value=(feed.label.clone().unwrap_or_default());

                                label { (w.ask_every) }
                                select name="refresh_minutes" {
                                    @for choice in [15_i32, 30, 60, 180, 360, 720, 1440] {
                                        option value=(choice)
                                               selected[choice == feed.refresh_minutes] {
                                            (refresh_label(choice))
                                        }
                                    }
                                }
                                p.meta { (w.refresh_note) }

                                label { (w.remind_before) }
                                select name="remind_before_minutes" {
                                    option value="" selected[feed.remind_before_minutes.is_none()] {
                                        (w.never)
                                    }
                                    @for choice in [30_i32, 60, 180, 720, 1440, 2880] {
                                        option value=(choice)
                                               selected[Some(choice) == feed.remind_before_minutes] {
                                            (remind_label(choice))
                                        }
                                    }
                                }
                                p.meta { (w.only_exams_ring) }

                                label {
                                    input type="checkbox" name="keep_cancelled" value="1"
                                          checked[feed.keep_cancelled];
                                    (w.keep_cancelled)
                                }
                                label {
                                    input type="checkbox" name="with_homework" value="1"
                                          checked[feed.with_homework];
                                    (w.carry_homework)
                                }
                                label {
                                    input type="checkbox" name="with_holidays" value="1"
                                          checked[feed.with_holidays];
                                    (w.mark_holidays)
                                }
                                @if !subjects.is_empty() {
                                    label { (w.leave_out) }
                                    p.meta { (w.leave_out_note) }
                                    .subjects {
                                        @for subject in subjects {
                                            label.pill {
                                                input type="checkbox" name="hide" value=(subject)
                                                      checked[feed.hide_subjects.iter()
                                                          .any(|h| h.eq_ignore_ascii_case(subject))];
                                                " " (subject)
                                            }
                                        }
                                    }
                                }
                                p {} button type="submit" { (w.save) }
                            }
                            .actions {
                                form method="post" action={ "/feeds/" (feed.id) "/rotate" }
                                     onsubmit={ "return confirm('" (w.new_address_warning) "')" } {
                                    button type="submit" { (w.new_address) }
                                }
                                form method="post" action={ "/feeds/" (feed.id) "/delete" }
                                     onsubmit={ "return confirm('" (w.delete_link_warning) "')" } {
                                    button.danger type="submit" { (w.delete_link) }
                                }
                            }
                        }
                        }
                    }
                    @if *google {
                        p.meta { (w.pushed_to_google) }
                    }
                    .actions {
                        form method="post" action={ "/links/" (account.id) "/feeds" } {
                            button type="submit" { (w.new_link) }
                        }
                        @if has_google {
                            @if *google {
                                form method="post" action={ "/links/" (account.id) "/google/delete" } {
                                    button type="submit" { (w.disconnect_google) }
                                }
                            } @else {
                                a href={ "/links/" (account.id) "/google" } {
                                    button type="button" { (w.push_to_google) }
                                }
                            }
                        }
                        form method="post" action={ "/links/" (account.id) "/delete" } {
                            button type="submit" { (w.remove_school) }
                        }
                    }
                }
            }

            @if cards.is_empty() {
                p.note { (w.nothing_linked) }
            }

            h2 { (w.your_data) }
            p.note { (w.your_data_note) }
            .actions {
                a href="/account/export.json" { button type="button" { (w.download_my_data) } }
                form method="post" action="/account/delete"
                     onsubmit={ "return confirm('" (w.delete_account_warning) "')" } {
                    button.danger type="submit" { (w.delete_my_account) }
                }
            }

            h2 { (w.link_a_school) }
            form.stack method="post" action="/links" {
                label for="server" { (w.webuntis_server) }
                input #server type="text" name="server" required placeholder="example.webuntis.com"
                      list="known-servers" value=(prefill.0);
                label for="school" { (w.school_login_name) }
                input #school type="text" name="school" required placeholder="example-school"
                      value=(prefill.1);
                p.meta {
                    a href="/schools" { (w.find_by_name) }
                    (w.or_from_url)
                    code { "https://<server>/WebUntis/?school=<name>" } "."
                }
                .row {
                    div {
                        label for="username" { (w.untis_username) }
                        input #username type="text" name="username" required autocomplete="off";
                    }
                    div {
                        label for="password" { (w.untis_password) }
                        input #password type="password" name="password" required autocomplete="off";
                    }
                }
                label for="timezone" { (w.school_timezone) }
                select #timezone name="timezone" {
                    @for zone in COMMON_ZONES {
                        option value=(zone) selected[*zone == "Europe/Vienna"] { (zone) }
                    }
                }
                p {} button type="submit" { (w.link_school) }
            }
        },
    )
    .into_response()
}

fn refresh_label(minutes: i32) -> String {
    match minutes {
        m if m < 60 => format!("{m} minutes"),
        60 => "hour".to_owned(),
        m if m < 1440 => format!("{} hours", m / 60),
        _ => "day".to_owned(),
    }
}

fn remind_label(minutes: i32) -> String {
    match minutes {
        m if m < 60 => format!("{m} minutes before"),
        60 => "an hour before".to_owned(),
        m if m < 1440 => format!("{} hours before", m / 60),
        1440 => "the day before".to_owned(),
        m => format!("{} days before", m / 1440),
    }
}

/// `POST /feeds/{id}/settings`
pub async fn feed_settings(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(feed): Path<Uuid>,
    headers: HeaderMap,
    Form(form): Form<FeedForm>,
) -> Response {
    let lang = Lang::of(&jar, &headers);
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };

    let want = db::FeedSettings {
        // An unchecked box sendeth nothing at all, which is how HTML saith no.
        keep_cancelled: form.keep_cancelled.is_some(),
        refresh_minutes: form.refresh_minutes.unwrap_or(60),
        remind_before_minutes: form.remind_before_minutes.filter(|m| *m > 0),
        with_homework: form.with_homework.is_some(),
        with_holidays: form.with_holidays.is_some(),
        // Each unticked box sends nothing, so what arrives is exactly the set
        // to leave out.
        hide_subjects: form.hide.unwrap_or_default(),
        label: form.label.map(|l| l.trim().chars().take(60).collect()),
    };

    match db::update_feed(&state.pool, user.id, feed, &want).await {
        Ok(true) => Redirect::to("/").into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such link").into_response(),
        Err(err) => dashboard(state, user, Some(format!("{err}")), lang).await,
    }
}

#[derive(Deserialize)]
pub struct Search {
    #[serde(default)]
    q: String,
}

/// A plain page for telling the user something went one way or another.
pub fn say(lang: Lang, title: &str, body: &str) -> Response {
    page(
        lang,
        title,
        None,
        html! {
            h1 { (title) }
            p.lede { (body) }
            p { a href="/" { "←" } }
        },
    )
    .into_response()
}

/// `GET /language/{code}`
///
/// Remembered in a cookie of its own. A preference, not a secret: nothing
/// turneth on it, and a stranger setting it can do no more than read German.
pub async fn set_language(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> impl axum::response::IntoResponse {
    let secure = state.config.public_url.starts_with("https");
    let chosen = crate::words::Lang::from_code(&code);
    (jar.add(crate::words::cookie_for(chosen, secure)), Redirect::to("/"))
}

/// `GET /schools?q=`
///
/// WebUntis' own directory, so nobody need dig a server name and a login name
/// out of a URL. Choosing one returneth to the form with both filled in.
pub async fn schools_page(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Query(search): Query<Search>,
) -> Response {
    let lang = Lang::of(&jar, &headers);
    let w = lang.words();
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    let query = search.q.trim().to_owned();
    let found = if query.chars().count() >= 3 {
        stundenglas_core::find_schools(&state.http, &query).await.unwrap_or_else(|err| {
            tracing::warn!("school search failed: {err:#}");
            Vec::new()
        })
    } else {
        Vec::new()
    };

    page(
        lang,
        w.find_your_school,
        Some(user),
        html! {
            h1 { (w.find_your_school) }
            p.lede { (w.find_lede) }
            form.stack method="get" action="/schools" {
                label for="q" { (w.school_or_town) }
                input #q type="text" name="q" value=(query) required
                      placeholder="BG Beispiel, or Wien";
                p {} button type="submit" { (w.search) }
            }

            @if query.chars().count() >= 3 && found.is_empty() {
                p.note { (w.nothing_found) }
            }
            @for school in &found {
                .card {
                    h3 { (school.display_name) }
                    @if !school.address.is_empty() { p.meta { (school.address) } }
                    p.meta { (school.login_name) " · " (school.server) }
                    .actions {
                        a href={ "/?server=" (urlencode(&school.server))
                                 "&school=" (urlencode(&school.login_name)) } {
                            button type="button" { (w.use_this_one) }
                        }
                    }
                }
            }
            @if !query.is_empty() && query.chars().count() < 3 {
                p.note { (w.three_letters) }
            }
        },
    )
    .into_response()
}

/// Enough escaping for a query value: everything but the unreserved set goes
/// out as a percent triple.
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

/// `POST /feeds/{id}/rotate`
///
/// A new address for a link whose old one got out, keeping its settings. The
/// old address stops working at once, so whatever subscribed to it must be
/// pointed at the new one.
pub async fn rotate_feed(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(feed): Path<Uuid>,
) -> Response {
    let lang = Lang::of(&jar, &headers);
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    let Ok(token) = crate::admin::mint_token() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "could not draw a new address").into_response();
    };
    match db::rotate_token(&state.pool, user.id, feed, &token).await {
        Ok(true) => Redirect::to("/").into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such link").into_response(),
        Err(err) => dashboard(state, user, Some(format!("{err}")), lang).await,
    }
}

/// `POST /feeds/{id}/delete`
pub async fn drop_feed(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(feed): Path<Uuid>,
) -> Response {
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    let _ = db::delete_feed(&state.pool, user.id, feed).await;
    Redirect::to("/").into_response()
}

/// `GET /privacy`
///
/// The README saith to tell people plainly what they are handing over. This
/// is where the service itself saith it, to the person doing the handing.
pub async fn privacy(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
) -> Response {
    let lang = Lang::of(&jar, &headers);
    let user = current(&state, &jar).await;
    let admins = if state.config.admin_emails.is_empty() {
        "whoever runs this instance".to_owned()
    } else {
        state.config.admin_emails.join(", ")
    };
    let keep_days = state.config.keep_days;
    page(
        lang,
        lang.words().privacy_link,
        user,
        html! {
            (privacy_text(lang, &admins, keep_days))
            p { a href="/" { "←" } }
        },
    )
    .into_response()
}

/// The privacy notice, written out whole in each tongue rather than assembled
/// from fragments. It is the one page where the wording matters most and where
/// a sentence stitched together from four labels would read like one.
fn privacy_text(lang: Lang, admins: &str, keep_days: i64) -> Markup {
    match lang {
        Lang::En => html! {
            h1 { "What is kept, and why" }
            p.lede {
                "This service signs in to your school's WebUntis as you, reads your timetable, "
                "and publishes it at a secret address your calendar can subscribe to."
            }

            h2 { "Your WebUntis password" }
            p {
                "WebUntis offers students no OAuth and no application token, so anything that "
                "reads your timetable on your behalf must hold your school password. There is "
                "no way around it, and it is the thing worth understanding before you sign up."
            }
            ul {
                li { "It is stored as AES-256-GCM ciphertext, never in the clear." }
                li {
                    "The key lives in this server's environment, not in the database, so a "
                    "stolen copy of the database alone opens nothing."
                }
                li {
                    "Those columns are not readable through the database's own API at all: the "
                    "grant is revoked from every role but this server's."
                }
                li { "Your browser never speaks to the database. It speaks only to this server." }
                li {
                    "A password the school refuses is not tried again, so this service cannot "
                    "get your school account locked."
                }
            }
            p.note {
                "None of which changes the underlying fact. If you would rather not hand over a "
                "school password, do not sign up — and it is worth knowing what your school's "
                "own rules say about it."
            }

            h2 { "What else is kept" }
            ul {
                li { "Your email address, so you can sign in and be recognised." }
                li { "The schools you link: the server, the school name and your username." }
                li {
                    "Your timetable as last read — lessons, teachers, rooms and times, and your "
                    "exams, homework and holidays — cached so that a calendar refreshing the "
                    "link never waits on the school."
                }
                li {
                    "Each calendar link, when it was last fetched and how often. That is how a "
                    "link that has stopped working can be told from one nobody uses."
                }
                li {
                    "If you connected Google Calendar, a token allowing writes to the one "
                    "calendar this service made. It writes nothing else and reads nothing."
                }
            }
            p {
                "A timetable that has not been refreshed in " (keep_days) " days is forgotten, "
                "and the link left empty."
            }

            h2 { "Who can see it" }
            p {
                "The administrators of this instance — " (admins) " — can see who has an "
                "account and whether their sync is working. They cannot read your password: it "
                "is sealed, and nothing in the interface unseals it."
            }
            p {
                "Anyone holding a calendar link can read that timetable. That is what makes it "
                "work in a calendar without a login, and why the address is long and secret. "
                "Share it as you would a password, and draw a new one if it gets out."
            }

            h2 { "Getting your data, and getting rid of it" }
            p {
                "Signed in, you can download everything held about you as one file, and you can "
                "delete the account outright. Deletion takes the schools, the calendar links, "
                "the stored passwords and the cached timetables with it, at once. There is no "
                "copy kept elsewhere, though ordinary database backups may hold one for a short "
                "while before they in turn expire."
            }
        },
        Lang::De => html! {
            h1 { "Was gespeichert wird, und warum" }
            p.lede {
                "Dieser Dienst meldet sich in deinem Namen bei WebUntis an, liest deinen "
                "Stundenplan und veröffentlicht ihn unter einer geheimen Adresse, die dein "
                "Kalender abonnieren kann."
            }

            h2 { "Dein WebUntis-Passwort" }
            p {
                "WebUntis bietet Schülerinnen und Schülern weder OAuth noch ein App-Token. Alles, "
                "was den Stundenplan in deinem Namen liest, muss daher dein Schulpasswort "
                "aufbewahren. Daran führt kein Weg vorbei, und das ist das Wesentliche, bevor du "
                "dich registrierst."
            }
            ul {
                li { "Es wird als AES-256-GCM-Chiffrat gespeichert, nie im Klartext." }
                li {
                    "Der Schlüssel liegt in der Umgebung dieses Servers, nicht in der Datenbank. "
                    "Eine gestohlene Kopie der Datenbank allein öffnet also nichts."
                }
                li {
                    "Diese Spalten sind über die API der Datenbank überhaupt nicht lesbar: die "
                    "Berechtigung ist allen Rollen außer der dieses Servers entzogen."
                }
                li {
                    "Dein Browser spricht nie mit der Datenbank, sondern ausschließlich mit "
                    "diesem Server."
                }
                li {
                    "Ein Passwort, das die Schule ablehnt, wird nicht erneut versucht. Dieser "
                    "Dienst kann dein Schulkonto also nicht sperren lassen."
                }
            }
            p.note {
                "Nichts davon ändert etwas an der Tatsache selbst. Wenn du dein Schulpasswort "
                "lieber nicht aus der Hand gibst, registriere dich nicht — und es lohnt sich zu "
                "wissen, was die Hausordnung deiner Schule dazu sagt."
            }

            h2 { "Was sonst gespeichert wird" }
            ul {
                li { "Deine E-Mail-Adresse, damit du dich anmelden kannst." }
                li {
                    "Die Schulen, die du verknüpfst: Server, Schulkürzel und dein Benutzername."
                }
                li {
                    "Deinen zuletzt gelesenen Stundenplan — Stunden, Lehrkräfte, Räume und "
                    "Zeiten, dazu Schularbeiten, Hausübungen und Ferien — zwischengespeichert, "
                    "damit ein Kalender beim Aktualisieren nie auf die Schule warten muss."
                }
                li {
                    "Zu jedem Kalender-Link, wann er zuletzt und wie oft abgerufen wurde. Nur so "
                    "lässt sich ein defekter Link von einem ungenutzten unterscheiden."
                }
                li {
                    "Falls du Google Calendar verbunden hast, ein Token, das Schreibzugriff auf "
                    "genau den einen Kalender erlaubt, den dieser Dienst angelegt hat. Er "
                    "schreibt sonst nichts und liest nichts."
                }
            }
            p {
                "Ein Stundenplan, der seit " (keep_days) " Tagen nicht aktualisiert wurde, wird "
                "vergessen; der Link bleibt dann leer."
            }

            h2 { "Wer es sehen kann" }
            p {
                "Die Administration dieser Instanz — " (admins) " — sieht, wer ein Konto hat und "
                "ob dessen Abgleich funktioniert. Dein Passwort kann sie nicht lesen: es ist "
                "versiegelt, und nichts in der Oberfläche entsiegelt es."
            }
            p {
                "Wer einen Kalender-Link hat, kann diesen Stundenplan lesen. Genau deshalb "
                "funktioniert er im Kalender ohne Anmeldung, und genau deshalb ist die Adresse "
                "lang und geheim. Behandle sie wie ein Passwort, und erzeuge eine neue, wenn sie "
                "in falsche Hände gerät."
            }

            h2 { "Deine Daten mitnehmen oder loswerden" }
            p {
                "Angemeldet kannst du alles, was über dich gespeichert ist, als eine Datei "
                "herunterladen und das Konto vollständig löschen. Die Löschung nimmt die "
                "Schulen, die Kalender-Links, die gespeicherten Passwörter und die "
                "zwischengespeicherten Stundenpläne sofort mit. Es wird keine Kopie anderswo "
                "aufbewahrt; gewöhnliche Datenbank-Sicherungen können allerdings für kurze Zeit "
                "eine enthalten, bis auch sie ablaufen."
            }
        },
    }
}

/// `GET /account/export.json`
pub async fn export(State(state): State<AppState>, jar: CookieJar) -> Response {
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    match db::export_for(&state.pool, user.id).await {
        Ok(payload) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/json; charset=utf-8".to_owned()),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"stundenglas.json\"".to_owned(),
                ),
            ],
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".into()),
        )
            .into_response(),
        Err(err) => {
            tracing::error!("export failed: {err:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, "could not gather your data").into_response()
        }
    }
}

/// `POST /account/delete`
///
/// Deleting the user cascadeth through every table that hangeth off it, so
/// this one call is the whole erasure.
pub async fn delete_account(
    State(state): State<AppState>,
    jar: CookieJar,
) -> impl axum::response::IntoResponse {
    let Some(user) = current(&state, &jar).await else {
        return (jar, Redirect::to("/"));
    };
    if let Err(err) = auth::delete_user(&state, user.id).await {
        tracing::error!(user = %user.id, "could not delete the account: {err:#}");
        return (jar, Redirect::to("/?trouble=delete"));
    }
    tracing::info!(user = %user.id, "account deleted at its owner's asking");

    // The session row went with the user; the cookie must go too.
    let secure = state.config.public_url.starts_with("https");
    (jar.add(auth::cookie_gone(secure)), Redirect::to("/"))
}

#[derive(Deserialize)]
pub struct FeedForm {
    label: Option<String>,
    refresh_minutes: Option<i32>,
    /// Empty when "never" is chosen, which serde readeth as None.
    #[serde(default, deserialize_with = "empty_as_none")]
    remind_before_minutes: Option<i32>,
    keep_cancelled: Option<String>,
    with_homework: Option<String>,
    with_holidays: Option<String>,
    /// One entry per ticked subject; axum gathers repeated fields into a Vec.
    #[serde(default)]
    hide: Option<Vec<String>>,
}

/// A select whose "never" option hath an empty value sendeth `""`, which is
/// neither a number nor absent. Read it as absent.
fn empty_as_none<'de, D>(given: D) -> Result<Option<i32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(given)?;
    Ok(raw.filter(|text| !text.trim().is_empty()).and_then(|text| text.trim().parse().ok()))
}

#[derive(Deserialize)]
pub struct NewPassword {
    password: String,
}

/// `POST /links/{id}/password`
///
/// The way back from a refused login: a new password, and the refusal lifted
/// so the scheduler will try once more.
pub async fn new_password(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(account): Path<Uuid>,
    headers: HeaderMap,
    Form(form): Form<NewPassword>,
) -> Response {
    let lang = Lang::of(&jar, &headers);
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    if form.password.is_empty() {
        return dashboard(state, user, Some("a password is wanted".into()), lang).await;
    }

    match db::replace_password(&state.pool, &state.config.sealer, user.id, account, &form.password)
        .await
    {
        // Owned by someone else, or gone: say nothing either way.
        Ok(false) => (StatusCode::NOT_FOUND, "no such school").into_response(),
        Ok(true) => {
            // Said once already; should it be refused again, it is worth
            // saying again.
            crate::post::forget(
                &state.pool,
                user.id,
                crate::post::Notice::PasswordRefused,
                account,
            )
            .await;
            // Try it at once, so they learn straight away whether it took.
            let soon = state.clone();
            tokio::spawn(async move {
                if let Err(err) = crate::sync::once(soon).await {
                    tracing::warn!("refresh after a new password failed: {err:#}");
                }
            });
            Redirect::to("/").into_response()
        }
        Err(err) => dashboard(state, user, Some(format!("{err}")), lang).await,
    }
}

// ------------------------------------------------------------------ forms ---

#[derive(Deserialize)]
pub struct Login {
    email: String,
    password: String,
}

pub async fn sign_up(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<Login>,
) -> Response {
    let email = form.email.trim().to_ascii_lowercase();
    match auth::sign_up(&state, &email, &form.password).await {
        Ok(who) => {
            if let Err(err) =
                people::on_signup(&state.pool, who.user, &email, &state.config.admin_emails).await
            {
                tracing::error!("could not record the account: {err:#}");
            }
            begin(&state, jar, &headers, who).await
        }
        Err(err) => refuse(&state, &format!("{err}"), Lang::of(&jar, &headers)).await,
    }
}

pub async fn sign_in(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<Login>,
) -> Response {
    let lang = Lang::of(&jar, &headers);
    let email = form.email.trim().to_ascii_lowercase();
    match auth::sign_in(&state, &email, &form.password).await {
        Ok(who) => {
            // An account made before profiles existed still needs a row.
            let _ =
                people::on_signup(&state.pool, who.user, &email, &state.config.admin_emails).await;
            // A security key, if one is enrolled, before any session is opened.
            if let Some(factor) = crate::mfa::verified_factor(&state, &who.gotrue).await {
                return match crate::mfa::park(&state, who.user, factor.id, &who.gotrue).await {
                    Ok(token) => {
                        let jar = jar.add(auth::pending_cookie(
                            token,
                            state.config.public_url.starts_with("https"),
                        ));
                        (jar, key_prompt(&factor.friendly_name, lang)).into_response()
                    }
                    Err(err) => {
                        tracing::error!("could not park the sign-in: {err:#}");
                        refuse(&state, "could not sign you in just now", lang).await
                    }
                };
            }
            begin(&state, jar, &headers, who).await
        }
        Err(err) => refuse(&state, &format!("{err}"), Lang::of(&jar, &headers)).await,
    }
}

async fn begin(
    state: &AppState,
    jar: CookieJar,
    headers: &HeaderMap,
    who: auth::Admitted,
) -> Response {
    let lang = Lang::of(&jar, headers);
    let agent = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok());
    match auth::open_session(state, who.user, &who.gotrue, agent).await {
        Ok(token) => {
            let jar =
                jar.add(auth::cookie_for(token, state.config.public_url.starts_with("https")));
            (jar, Redirect::to("/")).into_response()
        }
        Err(err) => {
            tracing::error!("could not open a session: {err:#}");
            refuse(state, "could not sign you in just now", lang).await
        }
    }
}

async fn refuse(_state: &AppState, why: &str, lang: Lang) -> Response {
    (
        StatusCode::BAD_REQUEST,
        page(
            lang,
            "Sorry",
            None,
            html! {
                h1 { "That did not work" }
                p.bad { (why) }
                p { a href="/" { "Try again" } }
            },
        ),
    )
        .into_response()
}

pub async fn sign_out(State(state): State<AppState>, jar: CookieJar) -> Response {
    if let Some(cookie) = jar.get(auth::COOKIE) {
        auth::close_session(&state.pool, cookie.value()).await;
    }
    let jar = jar.add(auth::cookie_gone(state.config.public_url.starts_with("https")));
    (jar, Redirect::to("/")).into_response()
}

#[derive(Deserialize)]
pub struct LinkForm {
    server: String,
    school: String,
    username: String,
    password: String,
    timezone: String,
}

pub async fn add_link(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<LinkForm>,
) -> Response {
    let lang = Lang::of(&jar, &headers);
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };

    if !people::profile(&state.pool, user.id).await.is_some_and(|p| p.approved) {
        return Redirect::to("/").into_response();
    }
    let Ok(timezone) = form.timezone.parse::<Tz>() else {
        return dashboard(state, user, Some("that is not a timezone I know".into()), lang).await;
    };
    let server = schools::tidy_server(&form.server);
    if server.is_empty() || form.school.trim().is_empty() {
        return dashboard(state, user, Some("a server and a school name are wanted".into()), lang)
            .await;
    }

    let new = NewAccount {
        user_id: user.id,
        server,
        school: form.school.trim().to_owned(),
        username: form.username.trim().to_owned(),
        password: form.password.clone(),
        timezone,
    };

    match crate::admin::add_account(&state, &new).await {
        Ok(_) => {
            // Fetch at once, so the link is not empty when they click it.
            let state2 = state.clone();
            tokio::spawn(async move {
                if let Err(err) = crate::sync::once(state2).await {
                    tracing::warn!("first refresh failed: {err:#}");
                }
            });
            Redirect::to("/").into_response()
        }
        Err(err) => dashboard(state, user, Some(format!("{err}")), lang).await,
    }
}

pub async fn new_feed(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(account): Path<Uuid>,
) -> Response {
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    if !people::profile(&state.pool, user.id).await.is_some_and(|p| p.approved)
        || !db::owns(&state.pool, user.id, account).await.unwrap_or(false)
    {
        return (StatusCode::NOT_FOUND, "no such school").into_response();
    }
    match crate::admin::mint_token() {
        Ok(token) => {
            let _ = db::create_feed(&state.pool, account, &token).await;
        }
        Err(err) => tracing::error!("could not mint a token: {err:#}"),
    }
    Redirect::to("/").into_response()
}

pub async fn drop_link(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(account): Path<Uuid>,
) -> Response {
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    // Deleting by (id, owner) means another user's id simply matches nothing.
    let _ = db::delete_account(&state.pool, user.id, account).await;
    Redirect::to("/").into_response()
}

/// Who is asking, if anyone. Shared with the Google routes.
pub async fn signed_in(state: &AppState, jar: &CookieJar) -> Option<CurrentUser> {
    current(state, jar).await
}

async fn current(state: &AppState, jar: &CookieJar) -> Option<CurrentUser> {
    let cookie = jar.get(auth::COOKIE)?;
    auth::user_of(&state.pool, cookie.value()).await
}

// ------------------------------------------------------------ the gateway ---

fn waiting_room(admin: bool, lang: Lang) -> Response {
    let w = lang.words();
    page(
        lang,
        w.waiting_title,
        Some(CurrentUser { id: Uuid::nil() }),
        html! {
            h1 { (w.waiting_title) }
            p.lede { (w.waiting_body) }
            @if admin {
                p { a href="/admin" { "You are an administrator — review the list" } }
            }
        },
    )
    .into_response()
}

/// `GET /admin/health`
///
/// What an administrator would otherwise go to the database for: who is
/// failing, who has never been subscribed to, and who is merely waiting out a
/// backoff.
pub async fn health_page(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
) -> Response {
    let lang = Lang::of(&jar, &headers);
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    if !people::profile(&state.pool, user.id).await.is_some_and(|p| p.is_admin) {
        return (StatusCode::NOT_FOUND, "no such page").into_response();
    }
    let rows = db::health(&state.pool).await.unwrap_or_default();
    let stuck = rows.iter().filter(|r| r.credentials_rejected).count();
    let failing = rows.iter().filter(|r| r.consecutive_fails > 0).count();
    let idle = rows.iter().filter(|r| r.last_served_at.is_none()).count();
    let now = chrono::Utc::now();

    page(
        lang,
        "How it fares",
        Some(user),
        html! {
            h1 { "How it fares" }
            p.lede {
                (rows.len()) " school link" (plural(rows.len())) " · "
                (stuck) " waiting on a password · "
                (failing) " failing · "
                (idle) " never subscribed to"
            }
            p.meta { a href="/admin" { "Who may join" } }

            @for row in &rows {
                .card {
                    h3 { (row.school) " · " (row.username) }
                    p.meta {
                        (row.email) " · "
                        // The id the operator commands take, so an
                        // administrator need not go to the database for it.
                        code { (row.account) }
                    }
                    @if row.credentials_rejected {
                        p.bad {
                            "The school refused this login. Nothing is tried until its owner "
                            "enters the password again."
                        }
                    } @else if let Some(why) = &row.last_error {
                        p.bad {
                            (row.consecutive_fails) " failure" (plural(row.consecutive_fails as usize))
                            " in a row: " (why.chars().take(160).collect::<String>())
                        }
                        @if let Some(when) = row.next_attempt_at {
                            @if when > now {
                                p.meta {
                                    "Waiting until " (when.format("%d %b %H:%M UTC").to_string())
                                }
                            }
                        }
                    }
                    p.meta {
                        (row.lesson_count) " lessons"
                        @if row.exam_count > 0 { ", " (row.exam_count) " exams" }
                        " · " (row.feeds) " link" (plural(row.feeds as usize))
                        @if !row.enabled { " · disabled" }
                    }
                    p.meta {
                        @match row.last_ok_at {
                            Some(when) => {
                                "Last refreshed " (when.format("%d %b %H:%M UTC").to_string())
                            }
                            None => { "Never refreshed" }
                        }
                        " · "
                        @match row.last_served_at {
                            Some(when) => {
                                "last fetched " (when.format("%d %b %H:%M UTC").to_string())
                            }
                            None => { "never fetched by any calendar" }
                        }
                    }
                }
            }
            @if rows.is_empty() {
                p.note { "No school is linked yet." }
            }
        },
    )
    .into_response()
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

pub async fn admin_page(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
) -> Response {
    let lang = Lang::of(&jar, &headers);
    let w = lang.words();
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    if !people::profile(&state.pool, user.id).await.is_some_and(|p| p.is_admin) {
        return (StatusCode::NOT_FOUND, "no such page").into_response();
    }
    let folk = people::everyone(&state.pool).await.unwrap_or_default();
    let waiting = folk.iter().filter(|p| !p.approved).count();

    page(
        lang,
        "Who may join",
        Some(user),
        html! {
            h1 { "Who may join" }
            p.meta { a href="/admin/health" { (lang.words().how_it_fares) } }
            p.lede {
                @if waiting == 0 { "Nobody is waiting." }
                @else if waiting == 1 { "One person is waiting." }
                @else { (waiting) " people are waiting." }
            }
            @for person in &folk {
                .card {
                    h3 { (person.email) }
                    p.meta {
                        @if person.is_admin { "administrator · " }
                        @if person.approved { "approved" } @else { "waiting since " }
                        @if !person.approved { (person.created_at.format("%d %b %H:%M").to_string()) }
                    }
                    @if !person.is_admin {
                        .actions {
                            @if person.approved {
                                form method="post" action={ "/admin/" (person.user_id) "/revoke" } {
                                    button type="submit" { "Revoke" }
                                }
                            } @else {
                                form method="post" action={ "/admin/" (person.user_id) "/approve" } {
                                    button type="submit" { "Let them in" }
                                }
                            }
                        }
                    }
                }
            }
            p { a href="/" { (w.back_to_timetables) } }
        },
    )
    .into_response()
}

pub async fn admin_decide(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((who, what)): Path<(Uuid, String)>,
) -> Response {
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    if !people::profile(&state.pool, user.id).await.is_some_and(|p| p.is_admin) {
        return (StatusCode::NOT_FOUND, "no such page").into_response();
    }
    let admitting = what == "approve";
    let _ = people::set_approved(&state.pool, who, user.id, admitting).await;
    if admitting {
        // They signed up and were told to wait; this is the end of the waiting.
        let _ = crate::post::tell(&state, who, crate::post::Notice::Admitted, None).await;
    }
    Redirect::to("/admin").into_response()
}

// ------------------------------------------------------- the security key ---

/// WebAuthn lives in the browser; there is no doing the ceremony server-side.
/// This is the whole of the script: fetch options from us, hand them to the
/// platform, post the answer back. Nothing is stored in the page.
const CEREMONY_JS: &str = r#"
const b64u = {
  dec: s => Uint8Array.from(atob(s.replace(/-/g,'+').replace(/_/g,'/')), c => c.charCodeAt(0)),
  enc: b => btoa(String.fromCharCode(...new Uint8Array(b)))
            .replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,''),
};
function reviveCreate(o) {
  o.challenge = b64u.dec(o.challenge);
  o.user.id = b64u.dec(o.user.id);
  (o.excludeCredentials||[]).forEach(c => c.id = b64u.dec(c.id));
  return o;
}
function reviveGet(o) {
  o.challenge = b64u.dec(o.challenge);
  (o.allowCredentials||[]).forEach(c => c.id = b64u.dec(c.id));
  return o;
}
function packAttestation(c) {
  return { id: c.id, rawId: b64u.enc(c.rawId), type: c.type,
    response: { clientDataJSON: b64u.enc(c.response.clientDataJSON),
                attestationObject: b64u.enc(c.response.attestationObject) } };
}
function packAssertion(c) {
  return { id: c.id, rawId: b64u.enc(c.rawId), type: c.type,
    response: { clientDataJSON: b64u.enc(c.response.clientDataJSON),
                authenticatorData: b64u.enc(c.response.authenticatorData),
                signature: b64u.enc(c.response.signature),
                userHandle: c.response.userHandle ? b64u.enc(c.response.userHandle) : null } };
}
async function post(url, body) {
  const r = await fetch(url, { method: 'POST', headers: {'Content-Type':'application/json'},
                               body: JSON.stringify(body || {}) });
  const text = await r.text();
  if (!r.ok) throw new Error(text || r.statusText);
  return text ? JSON.parse(text) : {};
}
function complain(err) {
  const box = document.getElementById('problem');
  if (box) { box.textContent = String(err && err.message || err); box.hidden = false; }
}
async function enrolKey() {
  try {
    const name = (document.getElementById('keyname') || {}).value || 'Security key';
    const started = await post('/security/enrol/start', { name });
    const cred = await navigator.credentials.create(
      { publicKey: reviveCreate(started.options.publicKey || started.options) });
    await post('/security/enrol/finish',
      { factor_id: started.factor_id, challenge_id: started.challenge_id,
        credential: packAttestation(cred) });
    location.href = '/security';
  } catch (e) { complain(e); }
}
async function useKey() {
  try {
    const started = await post('/mfa/challenge', {});
    const cred = await navigator.credentials.get(
      { publicKey: reviveGet(started.options.publicKey || started.options) });
    await post('/mfa/verify',
      { challenge_id: started.challenge_id, credential: packAssertion(cred) });
    location.href = '/';
  } catch (e) { complain(e); }
}
"#;

fn key_prompt(name: &str, lang: Lang) -> Markup {
    page(
        lang,
        "Your security key",
        None,
        html! {
            h1 { "Present your security key" }
            p.lede {
                "Your password was accepted. " (name) " is registered on this account, so it is "
                "wanted as well."
            }
            p.bad #problem hidden {}
            p { button type="button" onclick="useKey()" { "Use my key" } }
            p.meta { a href="/" { "Cancel" } }
            script { (maud::PreEscaped(CEREMONY_JS)) }
        },
    )
}

pub async fn security_page(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
) -> Response {
    let lang = Lang::of(&jar, &headers);
    let w = lang.words();
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    let Some(token) = session_token(&jar) else { return Redirect::to("/").into_response() };
    let bearer = auth::gotrue_of(&state, &token).await.unwrap_or_default();
    let keys = crate::mfa::factors(&state, &bearer).await.unwrap_or_default();

    page(
        lang,
        lang.words().security_keys,
        Some(user),
        html! {
            h1 { (w.security_keys) }
            p.lede {
                (w.key_lede)
            }
            p.bad #problem hidden {}
            @for key in &keys {
                .card {
                    h3 { (key.friendly_name) }
                    p.meta { @if key.verified { (w.registered) } @else { (w.never_finished) } }
                    .actions {
                        form method="post" action={ "/security/" (key.id) "/forget" } {
                            button type="submit" { (w.forget) }
                        }
                    }
                }
            }
            @if keys.is_empty() { p.note { (w.no_key_yet) } }
            h2 { (w.register_a_key) }
            label for="keyname" { (w.a_name_youll_know) }
            input #keyname type="text" value=(w.security_key);
            p {} button type="button" onclick="enrolKey()" { (w.register_this_one) }
            p.meta { a href="/" { (w.back_to_timetables) } }
            script { (maud::PreEscaped(CEREMONY_JS)) }
        },
    )
    .into_response()
}

#[derive(Deserialize)]
pub struct EnrolStart {
    #[serde(default)]
    name: Option<String>,
}

pub async fn enrol_start(
    State(state): State<AppState>,
    jar: CookieJar,
    axum::Json(body): axum::Json<EnrolStart>,
) -> Response {
    let Some(_user) = current(&state, &jar).await else {
        return (StatusCode::UNAUTHORIZED, "sign in first").into_response();
    };
    let Some(token) = session_token(&jar) else {
        return (StatusCode::UNAUTHORIZED, "sign in first").into_response();
    };
    let bearer = auth::gotrue_of(&state, &token).await.unwrap_or_default();
    let name = body.name.unwrap_or_else(|| "Security key".to_owned());

    match crate::mfa::enrol(&state, &bearer, &name).await {
        Ok(factor) => match crate::mfa::challenge(&state, &bearer, factor).await {
            Ok(c) => axum::Json(c).into_response(),
            Err(err) => (StatusCode::BAD_REQUEST, format!("{err}")).into_response(),
        },
        Err(err) => (StatusCode::BAD_REQUEST, format!("{err}")).into_response(),
    }
}

#[derive(Deserialize)]
pub struct Finish {
    factor_id: Uuid,
    challenge_id: Uuid,
    credential: serde_json::Value,
}

pub async fn enrol_finish(
    State(state): State<AppState>,
    jar: CookieJar,
    axum::Json(body): axum::Json<Finish>,
) -> Response {
    let Some(token) = session_token(&jar) else {
        return (StatusCode::UNAUTHORIZED, "sign in first").into_response();
    };
    let bearer = auth::gotrue_of(&state, &token).await.unwrap_or_default();
    match crate::mfa::verify(
        &state,
        &bearer,
        body.factor_id,
        body.challenge_id,
        crate::mfa::Ceremonial::Create,
        body.credential,
    )
    .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, format!("{err}")).into_response(),
    }
}

pub async fn forget_key(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(factor): Path<Uuid>,
) -> Response {
    if let Some(token) = session_token(&jar) {
        let bearer = auth::gotrue_of(&state, &token).await.unwrap_or_default();
        if let Err(err) = crate::mfa::forget(&state, &bearer, factor).await {
            tracing::warn!("could not remove the key: {err:#}");
        }
    }
    Redirect::to("/security").into_response()
}

pub async fn mfa_challenge(State(state): State<AppState>, jar: CookieJar) -> Response {
    let Some(pending) = jar.get(auth::PENDING).map(|c| c.value().to_owned()) else {
        return (StatusCode::UNAUTHORIZED, "start again").into_response();
    };
    let Some(parked) = crate::mfa::peek(&state, &pending).await else {
        return (StatusCode::UNAUTHORIZED, "that sign-in has expired").into_response();
    };
    match crate::mfa::challenge(&state, &parked.gotrue, parked.factor).await {
        Ok(c) => axum::Json(c).into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, format!("{err}")).into_response(),
    }
}

#[derive(Deserialize)]
pub struct Answer {
    challenge_id: Uuid,
    credential: serde_json::Value,
}

pub async fn mfa_verify(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Answer>,
) -> Response {
    let Some(pending) = jar.get(auth::PENDING).map(|c| c.value().to_owned()) else {
        return (StatusCode::UNAUTHORIZED, "start again").into_response();
    };
    let Some(parked) = crate::mfa::peek(&state, &pending).await else {
        return (StatusCode::UNAUTHORIZED, "that sign-in has expired").into_response();
    };

    match crate::mfa::verify(
        &state,
        &parked.gotrue,
        parked.factor,
        body.challenge_id,
        crate::mfa::Ceremonial::Request,
        body.credential,
    )
    .await
    {
        Ok(raised) => {
            crate::mfa::unpark(&state, &pending).await;
            let agent = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok());
            let secure = state.config.public_url.starts_with("https");
            match auth::open_session(&state, parked.user, &raised, agent).await {
                Ok(token) => {
                    let jar =
                        jar.add(auth::cookie_for(token, secure)).add(auth::pending_gone(secure));
                    (jar, StatusCode::OK).into_response()
                }
                Err(err) => {
                    tracing::error!("could not open a session: {err:#}");
                    (StatusCode::INTERNAL_SERVER_ERROR, "try again").into_response()
                }
            }
        }
        Err(err) => (StatusCode::BAD_REQUEST, format!("{err}")).into_response(),
    }
}

fn session_token(jar: &CookieJar) -> Option<String> {
    jar.get(auth::COOKIE).map(|c| c.value().to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_query_value_survives_being_one() {
        // Neither appears in a school name often, but a space in a login name
        // would otherwise cut the link in half.
        assert_eq!(super::urlencode("bg beispiel"), "bg%20beispiel");
        assert_eq!(super::urlencode("a&b=c"), "a%26b%3Dc");
        assert_eq!(super::urlencode("neilo.webuntis.com"), "neilo.webuntis.com");
    }

    #[test]
    fn a_feed_address_becomes_a_scannable_square() {
        let drawn = super::qr_svg("https://example.test/cal/abcdefghijklmnop.ics")
            .expect("a URL of ordinary length must fit in a QR");
        assert!(drawn.0.starts_with("<svg"), "an XML prolog is litter inside a page");
        assert!(!drawn.0.contains("<?xml"), "the prolog must be cut away, not merely skipped");
        assert!(drawn.0.len() > 500, "suspiciously small for a QR of that address");
    }
}

#[cfg(test)]
mod page_tests {
    use crate::words::Lang;

    #[test]
    fn the_shell_speaks_whichever_tongue_was_asked_for() {
        let english =
            super::page(Lang::En, "Title", None, maud::html! { p { "body" } }).into_string();
        assert!(english.contains("<html lang=\"en\""), "the tag a screen reader reads");
        assert!(english.contains("Your timetable, in your own calendar."));
        assert!(english.contains("Deutsch"), "and the toggle offers the other one");

        let german =
            super::page(Lang::De, "Titel", None, maud::html! { p { "body" } }).into_string();
        assert!(german.contains("<html lang=\"de\""));
        assert!(german.contains("Dein Stundenplan, in deinem eigenen Kalender."));
        assert!(german.contains("English"));
        assert!(german.contains("/language/en"), "and a way back");
    }

    #[test]
    fn the_privacy_notice_exists_whole_in_both() {
        for (lang, expected) in
            [(Lang::En, "AES-256-GCM ciphertext"), (Lang::De, "AES-256-GCM-Chiffrat")]
        {
            let text = super::privacy_text(lang, "you@example.test", 180).into_string();
            assert!(text.contains(expected), "the part that matters most, in both tongues");
            assert!(text.contains("180"), "and the retention it actually runs with");
            assert!(text.contains("you@example.test"), "and who to ask");
        }
    }
}

/// Renders the pages to `target/preview/` so a person may look at them.
///
/// A stylesheet cannot be judged by reading it, and the pages that carry most
/// of the design need neither a database nor a school to be drawn. Run it with
/// `cargo test -p stundenglas-server preview` and open what it names.
#[cfg(test)]
mod preview {
    use super::*;
    use crate::words::Lang;

    fn feed_settings_demo(w: &'static Words) -> Markup {
        html! {
            details {
                summary { (w.link_settings) }
                form.stack {
                    label { (w.what_to_call_it) }
                    input type="text" value="Handy";
                    label { (w.ask_every) }
                    select { option { "30 minutes" } }
                    p.meta { (w.refresh_note) }
                    label { (w.remind_before) }
                    select { option { (w.never) } option selected { "the day before" } }
                    p.meta { (w.only_exams_ring) }
                    label { input type="checkbox" checked; (w.keep_cancelled) }
                    label { input type="checkbox" checked; (w.carry_homework) }
                    label { input type="checkbox"; (w.mark_holidays) }
                    label { (w.leave_out) }
                    p.meta { (w.leave_out_note) }
                    .subjects {
                        @for (subject, left_out) in
                            [("D", false), ("E", false), ("M", false), ("RK", true), ("BSP", false)]
                        {
                            label.pill {
                                input type="checkbox" checked[left_out];
                                " " (subject)
                            }
                        }
                    }
                    p {} button.primary type="submit" { (w.save) }
                }
            }
        }
    }

    /// A dashboard as it looks with something to show: one link faring well,
    /// one whose password the school has refused.
    fn dashboard_demo(w: &'static Words) -> Markup {
        let url = "https://stundenglas.example.test/cal/7Qb3xY_kLm2pR9sTvW4eZa8nFgH1jK5c.ics";
        html! {
            h1 { (w.your_timetables) }
            p.lede { (w.one_link_each) }
            p.meta { a href="/security" { (w.security_keys) } " · " a href="/admin" { (w.who_may_join) } }

            .card {
                .card-head {
                    h3 { "Anna Beispiel" }
                    span.badge.ok { (w.state_well) }
                }
                p.meta { "bg-beispiel · neilo.webuntis.com · Europe/Vienna" }
                p.meta { "312 " (w.lessons) ", 4 " (w.exams) ", 6 " (w.homework_count)
                         ", " (w.refreshed) " 22 Sep 08:30 UTC" }
                .feedbox {
                    p.meta { "Handy" }
                    code.feed { (url) }
                    .actions { button.primary type="button" { (w.subscribe_here) } }
                    details {
                        summary { (w.show_qr) }
                        p.meta { (w.qr_warning) }
                        @if let Some(qr) = qr_svg(url) { .qrbox { (qr) } }
                    }
                    (feed_settings_demo(w))
                    .actions {
                        button type="submit" { (w.new_address) }
                        button.danger type="submit" { (w.delete_link) }
                    }
                }
                .actions {
                    button type="submit" { (w.new_link) }
                    button type="button" { (w.push_to_google) }
                    button.danger type="submit" { (w.remove_school) }
                }
            }

            .card {
                .card-head {
                    h3 { "Max Beispiel" }
                    span.badge.bad { (w.state_refused) }
                }
                p.meta { "bg-beispiel · neilo.webuntis.com · Europe/Vienna" }
                .notice {
                    p { strong { (w.refused_title) } (w.refused_body) }
                    form.stack {
                        label { (w.untis_password) }
                        input type="password";
                        p {} button.primary type="submit" { (w.try_this_one) }
                    }
                }
            }

            h2 { (w.your_data) }
            p.note { (w.your_data_note) }
            .actions {
                button type="button" { (w.download_my_data) }
                button.danger type="submit" { (w.delete_my_account) }
            }
        }
    }

    #[test]
    fn draw_the_pages_for_the_eye() {
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/preview");
        std::fs::create_dir_all(&out).expect("somewhere to put them");

        for lang in [Lang::En, Lang::De] {
            let w = lang.words();
            let code = lang.code();
            let pages: [(&str, Markup); 3] = [
                ("front", front(w)),
                ("dashboard", dashboard_demo(w)),
                ("privacy", privacy_text(lang, "you@example.test", 180)),
            ];
            for (name, body) in pages {
                let who = (name != "front").then_some(CurrentUser { id: uuid::Uuid::nil() });
                let html = page(lang, name, who, body).into_string();
                assert!(html.contains("<html lang="), "a page should be a page");
                std::fs::write(out.join(format!("{name}.{code}.html")), html).expect("writing");
            }
        }
        println!("\\npreview written to {}\\n", out.canonicalize().unwrap_or(out).display());
    }
}
