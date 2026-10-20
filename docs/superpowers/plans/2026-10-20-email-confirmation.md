# Confirm by email — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sign-up confirms by an emailed link (the password is chosen on that link), a confirmed address is in without an administrator, an administrator can ban (stopping pages, feeds, WebUntis sync and Google), and a forgotten password can be reset by email.

**Architecture:** GoTrue keeps the accounts, but its own mails are never used. The server mints one-time tokens through GoTrue's admin `generate_link`, sends its own plain-text mail through the existing `post::Mailer`, and spends the tokens with GoTrue's `verify` on a `POST`. A new `profiles.banned_at` column carries bans. Every "is this person in" question goes through `people::is_in`, and SQL uses one shared fragment.

**Tech Stack:** Rust, axum, maud, sqlx/Postgres, lettre, self-hosted Supabase GoTrue v2.196.0.

**Spec:** `docs/superpowers/specs/2026-10-20-email-confirmation-design.md`

## Global Constraints

- Every link in a mail is `PUBLIC_URL` + `/confirm?t=<hashed_token>` or `/reset?t=<hashed_token>`. Never `API_EXTERNAL_URL`.
- Tokens are spent only on `POST`, never on `GET`.
- Passwords are at least ten characters, typed twice on `/confirm` and `/reset`.
- Rate limits (for the confirm and reset mails together):
  - 1 per address per minute;
  - 5 per address per day;
  - 10 per IP per hour;
  - 150 per day across the service.
  - Over a limit: the same neutral page, nothing sent, and a `warn` log when the 150 cap is hit.
- Neutral answers: sign-up, send-again and forgot give the same page whatever the address's state.
- A reset never opens a session. It closes every session of that user.
- A ban stops: the web pages, the feeds (answering exactly like an unknown token and counting a `guard` miss), the WebUntis sync, and Google pushes. Admins cannot be banned.
- No `SMTP_URL` means today's behaviour exactly: a password at sign-up, confirmation at once, the manual approve button, and no forgot or send-again links.
- Mail subjects start `stundenglas — `. Mails are plain text, in the `Lang` of the request that asked for them.
- An account unconfirmed for 7 days (`auth.users.email_confirmed_at is null`) is deleted by the sweep. This happens only when mail is configured.
- Code style follows the crate: archaic third-person in comments (`doth`, `-eth`) as in the surrounding modules, `anyhow::Result`, and maud for pages. Tests are module `#[cfg(test)]` unit tests. There are no database tests in this crate; database behaviour is checked live in Task 8.
- Migrations go in `supabase/migrations/` and are applied by hand in production: `docker exec -i supabase-db psql -U postgres -v ON_ERROR_STOP=1 < file` inside CT 142.

**Verified against the live GoTrue (2026-10-20):**
- `POST /auth/v1/admin/users {email, email_confirm:false}` with no password creates the user.
- `POST /auth/v1/admin/generate_link {type:"magiclink"|"recovery", email}` returns `hashed_token`. An unknown address gives 404 `user_not_found`.
- `POST /auth/v1/verify {type, token_hash}` (anon key) returns `{access_token, user, …}` and confirms the email. Using the same token again gives 403 `otp_expired`.
- `PUT /auth/v1/admin/users/{id} {password}` returns 200.

## Review Focus

1. **An address in a different case or with spaces.** `Max@Maxi.co.at ` must hit the same account and the same rate counts. Test: `normalise_address` in Task 3.
2. **The confirm link opened twice.** The second `POST` gets the "link used or expired" page offering send-again and sign-in, not a 500. Test: `verify_error_is_expired` in Task 3 maps the 403 `otp_expired` body.
3. **The two passwords differ, or one is under ten characters.** The page is re-shown with the token kept and nothing spent. Test: `password_pair_checks` in Task 5.
4. **A banned user's feed URL.** It answers byte-for-byte like an unknown token. Live check in Task 8, and the SQL fragment test in Task 2.
5. **Mail is down (the relay is unreachable).** Sign-up still shows the neutral page, the failure is logged, and nothing is counted against the rate. Test: `rate_counts_only_sent` in Task 4 (the pure decision), plus a live check in Task 8.

---

### Task 1: Bans in the schema and in `people`

**Files:**
- Create: `supabase/migrations/20261020000000_bans_and_mail.sql`
- Modify: `server/src/people.rs`
- Modify: `server/src/auth.rs` (add `close_all_sessions`)
- Modify: `server/src/web.rs:521,1520,1564` (the three `p.approved` checks)

**Interfaces:**
- Produces:
  - `Profile.banned_at: Option<DateTime<Utc>>`
  - `people::is_in(p: &Profile) -> bool`
  - `people::set_banned(pool: &PgPool, who: Uuid, banned: bool) -> Result<()>`. It does nothing to admins.
  - `people::admit(pool: &PgPool, who: Uuid) -> Result<()>`. It sets `approved = true, approved_at = now()` and leaves `banned_at` alone.
  - `auth::close_all_sessions(pool: &PgPool, user: Uuid)`
  - `people::ADMITTED_SQL: &str = "p.approved and p.banned_at is null"`, for a join aliased `p`.

- [ ] **Step 1: Write the migration.**
  - `alter table public.profiles add column banned_at timestamptz;`
  - `create table public.mail_sends (id bigserial primary key, address text not null, ip inet, kind text not null, sent_at timestamptz not null default now());`
  - Indexes on `(address, sent_at)`, `(ip, sent_at)` and `(sent_at)`.
  - RLS enabled, `revoke all … from anon, authenticated`, and `grant all … to service_role`, mirroring `20260922050000_approval.sql`.
- [ ] **Step 2: Write the failing test** `is_in_needs_approval_and_no_ban` in `people.rs`. It asserts:
  - approved with no ban → `true`;
  - approved and banned → `false`;
  - not approved → `false`.
- [ ] **Step 3: Run** `cargo test -p stundenglas-server people::` and expect a compile failure (no `is_in`).
- [ ] **Step 4: Implement the Produces items.** `profile_from` reads `banned_at`, and every `select` in `people.rs` adds it. `set_banned(true)` also calls `auth::close_all_sessions`. Replace the three web checks with `people::is_in`.
- [ ] **Step 5: Run** `cargo test -p stundenglas-server && cargo clippy --workspace --all-targets`. All pass, no warnings.
- [ ] **Step 6: Commit** — "Keep a ban apart from being let in".

### Task 2: A ban stops feeds, sync and Google

**Files:**
- Modify: `server/src/db.rs`:
  - `accounts_to_sync` (~142)
  - `refused_to_remind` (~355)
  - `feed_by_token` (~389)
  - any other query that hands a user's account to `sync` or `google::push`. Grep for `from untis_accounts a`.

**Interfaces:**
- Consumes: `people::ADMITTED_SQL`.

- [ ] **Step 1: Write the failing test** `ban_gate_is_in_every_outward_query` in `db.rs`. Keep each of the three SQL strings as a `const` and assert that each one contains `people::ADMITTED_SQL` and `join profiles p on p.user_id = a.user_id`.
- [ ] **Step 2: Run it, and see it fail.**
- [ ] **Step 3: Implement.** Hoist the three query strings to `const`s built with `concat!`, or with a `format!` once in a `LazyLock`, and add the join and `and {ADMITTED_SQL}`. `feed::serve` needs no change: a `None` from `feed_by_token` already gives the unknown-token 404 and the `guard` miss.
- [ ] **Step 4: Run** the tests and clippy, and see them pass.
- [ ] **Step 5: Commit** — "Let a ban reach the calendars, the school and Google".

### Task 3: Talking to GoTrue for tokens

**Files:**
- Modify: `server/src/auth.rs`
- Modify: `server/src/db.rs` (`auth_user_by_email`)

**Interfaces:**
- Produces:
  - `pub fn normalise_address(raw: &str) -> String`. It trims and lowercases.
  - `pub enum Token { Confirm, Reset }`, with `fn gotrue_type(self) -> &'static str` returning `"magiclink"` or `"recovery"`.
  - `pub async fn create_unconfirmed(state: &AppState, email: &str) -> Result<Uuid>`. Admin create with `email_confirm: false` and no password.
  - `pub async fn mint(state: &AppState, kind: Token, email: &str) -> Result<Option<String>>`. It returns `hashed_token`, or `None` on 404 `user_not_found`.
  - `pub enum Spent { Good(Admitted), Expired }`
  - `pub async fn spend(state: &AppState, kind: Token, token_hash: &str) -> Result<Spent>`. `Expired` covers any 4xx whose `error_code` is `otp_expired`, or whose `msg` contains "invalid or has expired".
  - `pub async fn set_password(state: &AppState, user: Uuid, password: &str) -> Result<()>`. This is the admin `PUT`.
  - `db::auth_user_by_email(pool, email) -> Result<Option<(Uuid, bool /*confirmed*/)>>`, reading `auth.users`.
  - `fn verify_error_is_expired(body: &str) -> bool`. This is the pure helper `spend` uses.

- [ ] **Step 1: Write the failing tests:**
  - `normalise_address`: `" Max@Maxi.CO.at "` → `"max@maxi.co.at"`.
  - `verify_error_is_expired`:
    - true for `{"code":403,"error_code":"otp_expired","msg":"Email link is invalid or has expired"}`;
    - false for `{"code":500,"msg":"boom"}`.
  - `token_types`: `Token::Confirm.gotrue_type() == "magiclink"` and `Token::Reset.gotrue_type() == "recovery"`.
- [ ] **Step 2: Run them, and see them fail.**
- [ ] **Step 3: Implement.** Do it with the service key, the way `delete_user` does, and anon for `/verify` as `post()` does. `spend` builds `Admitted { user: user.id, gotrue: access_token }`.
- [ ] **Step 4: Run** the tests and clippy, and see them pass.
- [ ] **Step 5: Commit** — "Ask the account service for tokens and spend them".

### Task 4: The two mails and their rate

**Files:**
- Modify: `server/src/post.rs`

**Interfaces:**
- Consumes: `auth::Token`, `crate::words::Lang`.
- Produces:
  - `pub fn subject(kind: Token, lang: Lang) -> &'static str`
  - `pub fn body(kind: Token, lang: Lang, link: &str) -> String`
  - `pub fn link_for(public_url: &str, kind: Token, token_hash: &str) -> String`. It gives `{public_url}/confirm?t=…` or `/reset?t=…`, with the token URL-encoded and no double slash.
  - `pub struct Counts { pub address_minute: i64, pub address_day: i64, pub ip_hour: i64, pub all_day: i64 }`
  - `pub fn within_limits(c: &Counts) -> bool`
  - `pub async fn send_token_mail(state: &AppState, kind: Token, email: &str, ip: Option<IpAddr>, lang: Lang) -> Result<bool>`. It:
    - reads `Counts` from `mail_sends`;
    - returns `Ok(false)` over a limit, with a `warn` when `all_day >= 150`;
    - calls `auth::mint`, and returns `Ok(false)` on `None`;
    - sends;
    - inserts into `mail_sends` **only after** a successful send;
    - returns `Ok(true)`.
  - Returns `Ok(false)` when `state.mail` is `None`.

- [ ] **Step 1: Write the failing tests:**
  - `bodies_carry_the_link_in_both_tongues`: for each `Token` × `Lang`, the body contains the link and the subject is non-empty. The German confirm body contains "eine Stunde", and the English one "one hour".
  - `link_for_joins_cleanly`: `link_for("https://untis.maxi.co.at/", Token::Reset, "a b")` == `"https://untis.maxi.co.at/reset?t=a%20b"`.
  - `within_limits_edges`:
    - `{0,0,0,0}` → true;
    - `address_minute: 1` → false;
    - `address_day: 5` → false;
    - `ip_hour: 10` → false;
    - `all_day: 150` → false;
    - `{0,4,9,149}` → true.
  - `rate_counts_only_sent`: a doc-tested contract is not possible without a DB, so assert in code order instead. `send_token_mail`'s insert comes after `transport.send`, stated in a comment beside the call. Review checks it; the live check is in Task 8.
- [ ] **Step 2: Run them, and see them fail.**
- [ ] **Step 3: Implement.** The copy is the spec's draft wording, with "one hour" and the "if it wasn't you" line. Leave the subject with no `stundenglas — ` prefix in `subject()`; `send_token_mail` adds it, as `tell` does.
- [ ] **Step 4: Run** the tests and clippy, and see them pass.
- [ ] **Step 5: Commit** — "Write the confirm and reset mails, and count them".

### Task 5: Pages: sign-up, confirm, send again, forgot, reset

**Files:**
- Modify: `server/src/web.rs`:
  - `front`
  - `sign_up`
  - `sign_in`
  - new handlers beside them
  - `waiting_room`, reworded for a ban
- Modify: `server/src/words.rs` (new fields, both languages)
- Modify: `server/src/main.rs` (routes)
- Modify: `server/src/feed.rs` (make `caller` `pub(crate)`)

**Interfaces:**
- Consumes: Tasks 1, 3 and 4.
- Produces these routes:
  - `GET /confirm` and `POST /confirm`
  - `POST /confirm/again`
  - `GET /forgot` and `POST /forgot`
  - `GET /reset` and `POST /reset`
- Also produces:
  - `fn front(w: &'static Words, mail: bool) -> Markup`
  - `fn password_pair(a: &str, b: &str) -> Result<(), &'static str>`
  - `async fn admit(state: &AppState, jar: CookieJar, headers: &HeaderMap, who: auth::Admitted) -> Response`. This is the security-key check plus `begin`, moved out of `sign_in` unchanged, so `/confirm` uses the same path.

- [ ] **Step 1: Write the failing tests** (in `web.rs` tests; maud renders to a `String`):
  - `front_without_mail_is_todays_front`: `front(w, false)` contains `name="password"` inside the `/signup` form and no `/forgot`.
  - `front_with_mail_asks_only_for_the_address`: `front(w, true)` has no `su-pw` and contains `href="/forgot"` and `/confirm/again`.
  - `password_pair_checks`:
    - `("short","short")` → Err;
    - `("long-enough-1","long-enough-2")` → Err;
    - `("long-enough-1","long-enough-1")` → Ok.
- [ ] **Step 2: Run them, and see them fail.**
- [ ] **Step 3: Implement the handlers.**
  - **`sign_up` with mail:**
    - `normalise_address`, then `db::auth_user_by_email`;
    - none → `create_unconfirmed` plus `people::on_signup`;
    - unconfirmed → nothing extra;
    - then `send_token_mail(Confirm)` unless confirmed;
    - always the "check your inbox" page.
  - **`sign_up` without mail:** today's code.
  - **`GET /confirm` and `GET /reset`:** a form with hidden `t`, two password fields and a button. Nothing is spent.
  - **`POST /confirm`:**
    - `password_pair` fails → re-show the form with `t`;
    - `spend(Confirm)`:
      - `Expired` → a page offering the send-again form and "sign in";
      - `Good(who)` → `set_password`, then `people::admit`, then `admit(…, who)`.
  - **`POST /confirm/again` and `POST /forgot`:** `send_token_mail` (`Confirm`, or `Reset`), then the neutral page. `/confirm/again` sends only for an existing **unconfirmed** user, checked with `auth_user_by_email`.
  - **`POST /reset`:**
    - `password_pair`;
    - `spend(Reset)`:
      - `Good(who)` → `set_password`, then `close_all_sessions(who.user)`, then redirect to `/?changed=1`. There is **no** session; the front page shows "Password changed. Sign in with the new one." when `changed=1`;
      - `Expired` → a page offering `/forgot`.
  - **Signed in and not `is_in`:** `waiting_room` shows the "no access" wording when `banned_at` is set. With mail on, a confirmed, not-approved, not-banned profile is `admit`ted on sign-in, before the gate. That's the spec's "let in on next sign-in".
  - **IP** comes from `feed::caller(&headers, peer)`. The handlers take `ConnectInfo<SocketAddr>`, as `feed::serve` does.
  - **`words.rs`:** every new string gets both languages.
- [ ] **Step 4: Run** `cargo test --workspace && cargo clippy --workspace --all-targets && cargo fmt --check`, and see them pass.
- [ ] **Step 5: Commit** — "Sign up by address, choose the password on the link, and reset it by mail".

### Task 6: The admin page bans

**Files:**
- Modify: `server/src/web.rs` (`admin_page`, `admin_decide`)

**Interfaces:**
- Consumes: `people::set_banned`, `people::is_in`, `db::auth_user_by_email`, or a batch variant `db::confirmed_users(pool) -> Result<HashSet<Uuid>>`. Add it if the page needs it for everyone.

- [ ] **Step 1: Write the failing test** `person_state_words`, for a pure `fn standing_of(p: &Profile, confirmed: bool, mail: bool) -> &'static str`:
  - unconfirmed → "not confirmed yet";
  - in → "in";
  - banned → "banned since …";
  - not approved without mail → "waiting since …".

  Format the date with the existing `"%d %b %H:%M"`.
- [ ] **Step 2: Run it, and see it fail.**
- [ ] **Step 3: Implement.**
  - The actions `ban` and `unban` call `set_banned`.
  - `approve` and `revoke` remain only for `state.mail.is_none()`, with `Notice::Admitted` sent only on `approve`.
  - With mail, the page shows a **Ban** or **Undo ban** button, and none for admins.
  - The lede counts banned people instead of waiting ones when mail is on.
- [ ] **Step 4: Run** the tests and clippy, and see them pass.
- [ ] **Step 5: Commit** — "Let an administrator ban, and nothing more".

### Task 7: Sweep, startup line, README

**Files:**
- Modify: `server/src/sync.rs` (`run`)
- Modify: `server/src/db.rs` (`unconfirmed_older_than`)
- Modify: `server/src/main.rs:186-188`
- Modify: `README.md` (the sign-up and `ADMIN_EMAILS` text, roughly lines 100–130)

**Interfaces:**
- Consumes: `auth::delete_user`.
- Produces: `db::unconfirmed_older_than(pool, days: i64) -> Result<Vec<Uuid>>`, which reads `auth.users` where `email_confirmed_at is null and created_at < now() - days`.

- [ ] **Step 1: Implement.**
  - In `run`, when `state.mail.is_some()`, delete `unconfirmed_older_than(7)` through `auth::delete_user`, logging the count.
  - Prune `mail_sends` older than 2 days in the same place.
  - The startup line with no `SMTP_URL` adds "; sign-ups wait for an administrator".
  - In the README, describe confirm-by-mail, ban, reset, and the no-mail fallback, in the README's own voice.
- [ ] **Step 2: Run** `cargo test --workspace && cargo clippy --workspace --all-targets`, and see them pass.
- [ ] **Step 3: Commit** — "Forget unconfirmed sign-ups after a week, and say so in the README".

### Task 8: Roll out on CT 142 and check it live

No code. Each step is checked on the live site.

- [ ] **Step 1: Deploy the code and migration.**
  - `git push`.
  - In CT 142: `cd /opt/stundenglas && git pull --ff-only`.
  - Build with `CARGO_HOME=/opt/cargo RUSTUP_HOME=/opt/rustup PATH=/opt/cargo/bin:$PATH cargo build --release -p stundenglas-server`.
  - Apply the migration with `docker exec -i supabase-db psql -U postgres -v ON_ERROR_STOP=1 < supabase/migrations/20261020000000_bans_and_mail.sql`.
  - `install` the binary and `systemctl restart stundenglas-server`.
  - Expected: `active`, and `/` gives 200.
- [ ] **Step 2: Turn off autoconfirm.**
  - In `/opt/supabase/.env` set `ENABLE_EMAIL_AUTOCONFIRM=false`, after a backup, then run `docker compose up -d auth`.
  - Expected: `supabase-auth` is healthy.
- [ ] **Step 3: Sign up with a mail-tester address.**
  - Expected:
    - the mail arrives, with SPF, DKIM and DMARC passing;
    - the link opens the password form;
    - submitting signs you in;
    - a second submit gets the "used or expired" page.
- [ ] **Step 4: Ask for a reset, and use it.**
  - Expected: you are signed out everywhere, and the new password works.
  - If the test account enrols a security key, it is asked for after the reset.
- [ ] **Step 5: Ban the test account on `/admin`.**
  - Expected: its feed URL answers exactly like a made-up token, and the next sync round skips it.
  - Undo the ban. Expected: the feed answers again.
- [ ] **Step 6: Check the rate.** Six sign-ups to one address within a day; the sixth sends nothing.
- [ ] **Step 7: Delete the test accounts.**
