//! The pages. Server-rendered, no script, no framework: a school timetable
//! needeth none of it.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;
use chrono_tz::Tz;
use maud::{DOCTYPE, Markup, html};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::{self, CurrentUser};
use crate::db::{self, NewAccount};
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

fn page(title: &str, user: Option<CurrentUser>, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " — stundenglas" }
                style { (maud::PreEscaped(STYLE)) }
            }
            body {
                header {
                    a.brand href="/" { "stundenglas" }
                    nav {
                        @if user.is_some() {
                            form method="post" action="/logout" {
                                button.link type="submit" { "sign out" }
                            }
                        }
                    }
                }
                main { (body) }
                footer { "Your timetable, in your own calendar." }
            }
        }
    }
}

const STYLE: &str = r#"
:root { color-scheme: light dark; --edge: color-mix(in oklab, currentColor 18%, transparent); }
* { box-sizing: border-box; }
body { margin: 0; font: 16px/1.55 system-ui, sans-serif; }
header { display: flex; justify-content: space-between; align-items: center;
         padding: 1rem clamp(1rem, 4vw, 3rem); border-bottom: 1px solid var(--edge); }
.brand { font-weight: 650; letter-spacing: -0.01em; text-decoration: none; color: inherit; }
main { max-width: 46rem; margin: 0 auto; padding: 2rem clamp(1rem, 4vw, 3rem) 4rem; }
footer { max-width: 46rem; margin: 0 auto; padding: 0 clamp(1rem, 4vw, 3rem) 3rem;
         opacity: .6; font-size: .875rem; }
h1 { font-size: 1.6rem; letter-spacing: -0.02em; margin: 0 0 .25rem; }
h2 { font-size: 1.05rem; margin: 2.5rem 0 .75rem; }
p.lede { opacity: .75; margin-top: 0; }
label { display: block; margin: .85rem 0 .25rem; font-size: .875rem; opacity: .8; }
input, select, button { font: inherit; }
input, select { width: 100%; padding: .55rem .7rem; border: 1px solid var(--edge);
                border-radius: .4rem; background: transparent; color: inherit; }
button { padding: .55rem 1rem; border: 1px solid var(--edge); border-radius: .4rem;
         background: color-mix(in oklab, currentColor 8%, transparent); color: inherit;
         cursor: pointer; }
button.link { border: 0; background: none; padding: 0; text-decoration: underline; }
form.stack { margin-bottom: 1rem; }
.row { display: flex; gap: .75rem; flex-wrap: wrap; }
.row > * { flex: 1 1 12rem; }
.card { border: 1px solid var(--edge); border-radius: .6rem; padding: 1rem 1.15rem; margin: .75rem 0; }
.card h3 { margin: 0 0 .2rem; font-size: 1rem; }
.meta { font-size: .85rem; opacity: .7; margin: 0; }
code.feed { display: block; word-break: break-all; font-size: .8rem; padding: .5rem .6rem;
            border: 1px solid var(--edge); border-radius: .4rem; margin: .6rem 0 .4rem; }
.note { border-left: 3px solid var(--edge); padding: .4rem 0 .4rem .9rem; opacity: .8;
        font-size: .9rem; }
.bad { color: #b3261e; }
.actions { display: flex; gap: .5rem; flex-wrap: wrap; margin-top: .5rem; }
.actions form { margin: 0; }
"#;

// ----------------------------------------------------------------- landing ---

pub async fn index(State(state): State<AppState>, jar: CookieJar) -> Response {
    match current(&state, &jar).await {
        Some(user) => dashboard(state, user, None).await,
        None => page(
            "Sign in",
            None,
            html! {
                h1 { "Your school timetable, in the calendar you already use" }
                p.lede {
                    "stundenglas reads your WebUntis timetable and publishes it as a private "
                    "calendar link. Google Calendar, iOS, Outlook and Thunderbird can all "
                    "subscribe to it. Cancelled lessons stay visible so you can see the free hour."
                }
                div.row {
                    form.stack method="post" action="/signup" {
                        h2 { "Create an account" }
                        label for="su-email" { "Email" }
                        input #su-email type="email" name="email" required autocomplete="email";
                        label for="su-pw" { "Password (ten characters or more)" }
                        input #su-pw type="password" name="password" required
                              autocomplete="new-password" minlength="10";
                        p {} button type="submit" { "Sign up" }
                    }
                    form.stack method="post" action="/login" {
                        h2 { "Sign in" }
                        label for="li-email" { "Email" }
                        input #li-email type="email" name="email" required autocomplete="email";
                        label for="li-pw" { "Password" }
                        input #li-pw type="password" name="password" required
                              autocomplete="current-password";
                        p {} button type="submit" { "Sign in" }
                    }
                }
                p.note {
                    "WebUntis gives students no way to grant access without a password, so this "
                    "service has to store your school password to read your timetable for you. "
                    "It is encrypted, and the key is kept apart from the database — but you "
                    "should know that before you hand it over, and check what your school's "
                    "rules say."
                }
            },
        )
        .into_response(),
    }
}

// --------------------------------------------------------------- dashboard ---

async fn dashboard(state: AppState, user: CurrentUser, problem: Option<String>) -> Response {
    let me = people::profile(&state.pool, user.id).await;
    let admin = me.as_ref().is_some_and(|p| p.is_admin);
    if !me.as_ref().is_some_and(|p| p.approved) {
        return waiting_room(admin);
    }
    let accounts = db::accounts_of(&state.pool, user.id).await.unwrap_or_default();

    let mut cards = Vec::new();
    for account in &accounts {
        let feeds = db::feeds_of(&state.pool, account.id).await.unwrap_or_default();
        let state_of = db::sync_status(&state.pool, account.id).await.unwrap_or(None);
        let google = crate::google::has_link(&state.pool, account.id).await;
        cards.push((account.clone(), feeds, state_of, google));
    }

    let has_google = state.config.google.is_some();
    page(
        "Your timetables",
        Some(user),
        html! {
            h1 { "Your timetables" }
            p.lede { "One link per school. Each keeps its own clock." }
            p.meta {
                a href="/security" { "Security keys" }
                @if admin { " · " a href="/admin" { "Who may join" } }
            }
            @if has_google {
                p.note {
                    "A subscribed link is refreshed on your calendar's own schedule — Google "
                    "often takes hours. Connect Google to have changes written the moment we "
                    "see them."
                }
            }

            @if let Some(why) = &problem {
                p.bad { (why) }
            }

            @for (account, feeds, status, google) in &cards {
                .card {
                    h3 { (account.display_name.clone().unwrap_or_else(|| account.username.clone())) }
                    p.meta {
                        (account.school) " · " (account.server) " · " (account.timezone.name())
                    }
                    p.meta {
                        @match status {
                            Some(s) if s.last_error.is_some() => {
                                span.bad { "last refresh failed: "
                                    (s.last_error.clone().unwrap_or_default()) }
                            }
                            Some(s) => {
                                (s.lesson_count) " lessons"
                                @if let Some(when) = s.last_ok_at {
                                    ", refreshed " (when.format("%d %b %H:%M UTC").to_string())
                                }
                            }
                            None => { "waiting for the first refresh" }
                        }
                    }
                    @for feed in feeds {
                        code.feed { (state.config.feed_url(&feed.token)) }
                    }
                    @if *google {
                        p.meta { "Pushed into Google Calendar as well as the link above." }
                    }
                    .actions {
                        form method="post" action={ "/links/" (account.id) "/feeds" } {
                            button type="submit" { "New link" }
                        }
                        @if has_google {
                            @if *google {
                                form method="post" action={ "/links/" (account.id) "/google/delete" } {
                                    button type="submit" { "Disconnect Google" }
                                }
                            } @else {
                                a href={ "/links/" (account.id) "/google" } {
                                    button type="button" { "Push to Google Calendar" }
                                }
                            }
                        }
                        form method="post" action={ "/links/" (account.id) "/delete" } {
                            button type="submit" { "Remove school" }
                        }
                    }
                }
            }

            @if cards.is_empty() {
                p.note { "No school linked yet. Add one below and a calendar link appears." }
            }

            h2 { "Link a school" }
            form.stack method="post" action="/links" {
                label for="server" { "WebUntis server" }
                input #server type="text" name="server" required placeholder="example.webuntis.com"
                      list="known-servers";
                label for="school" { "School login name" }
                input #school type="text" name="school" required placeholder="example-school";
                p.meta {
                    "Search your school at webuntis.com; it sends you to "
                    code { "https://<server>/WebUntis/?school=<name>" } "."
                }
                .row {
                    div {
                        label for="username" { "WebUntis username" }
                        input #username type="text" name="username" required autocomplete="off";
                    }
                    div {
                        label for="password" { "WebUntis password" }
                        input #password type="password" name="password" required autocomplete="off";
                    }
                }
                label for="timezone" { "The school's timezone" }
                select #timezone name="timezone" {
                    @for zone in COMMON_ZONES {
                        option value=(zone) selected[*zone == "Europe/Vienna"] { (zone) }
                    }
                }
                p {} button type="submit" { "Link school" }
            }
        },
    )
    .into_response()
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
        Err(err) => refuse(&state, &format!("{err}")).await,
    }
}

pub async fn sign_in(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<Login>,
) -> Response {
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
                        (jar, key_prompt(&factor.friendly_name)).into_response()
                    }
                    Err(err) => {
                        tracing::error!("could not park the sign-in: {err:#}");
                        refuse(&state, "could not sign you in just now").await
                    }
                };
            }
            begin(&state, jar, &headers, who).await
        }
        Err(err) => refuse(&state, &format!("{err}")).await,
    }
}

async fn begin(
    state: &AppState,
    jar: CookieJar,
    headers: &HeaderMap,
    who: auth::Admitted,
) -> Response {
    let agent = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok());
    match auth::open_session(state, who.user, &who.gotrue, agent).await {
        Ok(token) => {
            let jar =
                jar.add(auth::cookie_for(token, state.config.public_url.starts_with("https")));
            (jar, Redirect::to("/")).into_response()
        }
        Err(err) => {
            tracing::error!("could not open a session: {err:#}");
            refuse(state, "could not sign you in just now").await
        }
    }
}

async fn refuse(_state: &AppState, why: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        page(
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
    Form(form): Form<LinkForm>,
) -> Response {
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };

    if !people::profile(&state.pool, user.id).await.is_some_and(|p| p.approved) {
        return Redirect::to("/").into_response();
    }
    let Ok(timezone) = form.timezone.parse::<Tz>() else {
        return dashboard(state, user, Some("that is not a timezone I know".into())).await;
    };
    let server = schools::tidy_server(&form.server);
    if server.is_empty() || form.school.trim().is_empty() {
        return dashboard(state, user, Some("a server and a school name are wanted".into())).await;
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
        Err(err) => dashboard(state, user, Some(format!("{err}"))).await,
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

/// A plain page for telling the user something went one way or another.
pub fn say(title: &str, body: &str) -> Response {
    page(
        title,
        None,
        html! {
            h1 { (title) }
            p { (body) }
            p { a href="/" { "Back to your timetables" } }
        },
    )
    .into_response()
}

async fn current(state: &AppState, jar: &CookieJar) -> Option<CurrentUser> {
    let cookie = jar.get(auth::COOKIE)?;
    auth::user_of(&state.pool, cookie.value()).await
}

// ------------------------------------------------------------ the gateway ---

fn waiting_room(admin: bool) -> Response {
    page(
        "Waiting to be let in",
        Some(CurrentUser { id: Uuid::nil() }),
        html! {
            h1 { "Your account is waiting" }
            p.lede {
                "Anyone may sign up here, but an administrator lets people in one at a time. "
                "Yours is on the list. Come back once you hear that it has been approved."
            }
            @if admin {
                p { a href="/admin" { "You are an administrator — review the list" } }
            }
        },
    )
    .into_response()
}

pub async fn admin_page(State(state): State<AppState>, jar: CookieJar) -> Response {
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    if !people::profile(&state.pool, user.id).await.is_some_and(|p| p.is_admin) {
        return (StatusCode::NOT_FOUND, "no such page").into_response();
    }
    let folk = people::everyone(&state.pool).await.unwrap_or_default();
    let waiting = folk.iter().filter(|p| !p.approved).count();

    page(
        "Who may join",
        Some(user),
        html! {
            h1 { "Who may join" }
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
            p { a href="/" { "Back to your timetables" } }
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
    let _ = people::set_approved(&state.pool, who, user.id, what == "approve").await;
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

fn key_prompt(name: &str) -> Markup {
    page(
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

pub async fn security_page(State(state): State<AppState>, jar: CookieJar) -> Response {
    let Some(user) = current(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    let Some(token) = session_token(&jar) else { return Redirect::to("/").into_response() };
    let bearer = auth::gotrue_of(&state, &token).await.unwrap_or_default();
    let keys = crate::mfa::factors(&state, &bearer).await.unwrap_or_default();

    page(
        "Security keys",
        Some(user),
        html! {
            h1 { "Security keys" }
            p.lede {
                "A key is asked for after your password. It is a second factor, not a "
                "replacement: this account service offers no passwordless sign-in yet."
            }
            p.bad #problem hidden {}
            @for key in &keys {
                .card {
                    h3 { (key.friendly_name) }
                    p.meta { @if key.verified { "registered" } @else { "never finished" } }
                    .actions {
                        form method="post" action={ "/security/" (key.id) "/forget" } {
                            button type="submit" { "Remove" }
                        }
                    }
                }
            }
            @if keys.is_empty() { p.note { "No key registered yet." } }
            h2 { "Register a key" }
            label for="keyname" { "A name you will recognise" }
            input #keyname type="text" value="Security key";
            p {} button type="button" onclick="enrolKey()" { "Register this device or key" }
            p.meta { a href="/" { "Back to your timetables" } }
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
