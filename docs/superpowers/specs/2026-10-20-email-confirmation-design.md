# Confirm by email instead of waiting for an administrator

## Why

Today anyone may sign up, but nobody is let in until an administrator presses
"Let them in" on `/admin`. The owner wants out of that loop: whoever proves
they own the address they signed up with is in, exactly as if they had been
approved. What is left for the administrator is shutting somebody out.

Alongside it: a forgotten password can be reset by email. Today there is no
way back from one at all.

Mail now works. Everything leaves through the Postfix relay in CT 144
(`SMTP_URL=smtp://10.10.1.144:25`, sending as `stundenglas <noreply@maxi.co.at>`)
and Brevo, with SPF, DKIM and DMARC passing.

## Goals

- Signing up asks only for an address and sends a mail with one link. The
  link opens a page where the person chooses their password; that confirms
  the address, lets them in and signs them in, landing on their timetables.
- Until the link is followed the account cannot be used. Signing in says to
  look in the inbox, and offers to send the mail again.
- Administrators have no approving to do. `/admin` lists people and lets an
  administrator ban someone and undo a ban.
- "Forgot your password?" on the front page sends a mail whose link leads to
  a page for choosing a new one.
- Both mails in German or English, by the language the page was read in when
  the mail was asked for.

## Non-goals

- Changing the address of an existing account.
- Signing in by a link alone, without a password.
- An HTML mail. The existing notices are plain text and these are too.
- Stopping someone who was banned from coming back under another address.
  Getting in is open by design now; a ban is answered with another ban.

## Anti-goals: wrong even if it works

- **A reset that slips past a security key.** If an account has a key enrolled
  (`/security`), the key must still be asked for when signing in after a reset.
  An inbox alone must never be enough to get into such an account.
- **Telling a stranger who is signed up.** Sign-up, "send it again" and "forgot
  your password" give the same answer whether or not the address has an
  account, and whatever state it is in.
- **Links that point at Supabase.** `API_EXTERNAL_URL` is
  `http://127.0.0.1:8000` and unreachable from outside. Every link in a mail
  points at `PUBLIC_URL` (`https://untis.maxi.co.at`).
- **An account claimed before its owner.** Nobody but the owner of the inbox
  may ever know the password of an account under that address. That is why
  the password is chosen after the link, not at sign-up (owner's decision).
- **A banned person let back in by a mail.** Confirming, resetting, or signing
  up again must not undo a ban.
- **A mail cannon.** These pages send mail to addresses a stranger types in.
  They must not be usable to flood someone's inbox or burn through Brevo's 300
  mails a day.

## How it works (decided)

### The mails are ours, not Supabase's

GoTrue's own confirmation and recovery mails are not used. They would link to
`API_EXTERNAL_URL`, come in one language, and be templated where this code
cannot see them. Instead:

- The server asks GoTrue, using the service key, for a one-time token for the
  address. That is the admin `generate_link` endpoint, which returns a
  `hashed_token` and sends nothing.
- It sends its own mail through the existing `post::Mailer`. The link is
  `PUBLIC_URL/confirm?t=<hashed_token>` or `PUBLIC_URL/reset?t=<hashed_token>`.
- When the link is followed, the server hands the token to GoTrue's `verify`
  endpoint (`token_hash` with `type` = `signup` or `recovery`).

Links are good for an hour and work once. That is GoTrue's default
(`otp_expiry`), and it is kept.

### Confirming (owner's decision: the password is chosen on confirm)

- The sign-up form asks only for the address.
- `POST /signup` makes sure a GoTrue user exists for the address, created
  unconfirmed and **without a password**, through the admin API. It records
  the profile, sends the confirmation mail, and never opens a session. The
  page answers "Check your inbox".
- Signing up with an address that already has an account gives the same
  answer.
  - If that account is unconfirmed, a fresh mail goes out (rate rules below).
  - If it is confirmed, nothing is sent.
  - Since no password exists until confirmation, a stranger who signs up with
    someone else's address learns nothing and can do nothing.
- `GET /confirm?t=…` shows a page with a password field (typed twice, at least
  ten characters) and a button. It does **not** use the token on `GET`, so a
  mail scanner that opens links cannot spend it. (My call, after review.)
- `POST /confirm` verifies the token with GoTrue. On success it:
  1. sets the chosen password through the admin API,
  2. marks the profile admitted (unless banned),
  3. signs in with the new password through the normal path (`begin`), and
  4. redirects to `/`.
- If the token is bad (expired, used or unknown), the page says so plainly.
  It offers both "send a new link" and "sign in", since the address may well
  be confirmed already.
- Supabase is set to `ENABLE_EMAIL_AUTOCONFIRM=false`. An unconfirmed account
  has no password, so it cannot sign in at all. `POST /login` answers it like
  any wrong password, and the front page offers "Didn't get the mail? Send it
  again" (`POST /confirm/again`), which gives the same neutral answer as
  sign-up.
- Addresses in `ADMIN_EMAILS` confirm like everyone else, and are
  administrators once confirmed.

### Banning

- A new column, `profiles.banned_at timestamptz null`, records a ban. It is
  kept apart from `approved` so that confirming cannot undo it.
- Being let in becomes `approved and banned_at is null`. Every check that reads
  `approved` today goes through one function that says this.
- On `/admin`, each person shows one of "not confirmed yet", "in" or "banned
  since …", with a **Ban** or **Undo ban** button. Administrators cannot be
  banned, as they cannot be revoked now.
- A banned person who signs in sees the old waiting-room page, reworded: they
  no longer have access, and nothing more.
- Banning closes that person's sessions at once.
- **A ban stops everything (owner's decision):**
  - the web pages;
  - their calendar links, which answer exactly as a link that never existed
    does, including counting as a miss for `guard`;
  - the WebUntis sync, so the server stops signing in to the school on their
    behalf;
  - the Google pushes.

  Undoing the ban resumes all of it as it was: the same links and the same
  stored credentials. Today `approved` is only checked in `web.rs`; the feed
  route (`feed::serve`), the sync scheduler (`sync.rs`) and `google.rs` must
  gain the check.

### Resetting a password

- The front page sign-in form gets "Forgot your password?". It leads to a page
  asking only for the address. Submitting always answers: if that address has
  an account, a mail is on its way.
- `GET /reset?t=…` shows a form for the new password, typed twice, at least
  ten characters like sign-up. The token travels in a hidden field and is
  **not** verified on `GET`, so a mail scanner that opens links cannot use it
  up.
- `POST /reset` verifies the token. If it is good, the server sets the new
  password through the admin API, closes **every** session that user has, and
  sends them to the front page with "Password changed. Sign in with the new
  one." No session is opened by the reset itself. Signing in afterwards goes
  through the normal path, which asks for the security key when one is
  enrolled.
- An unconfirmed account that resets its password has proved it owns the
  address, so it is confirmed at the same time.
- A banned account may reset, and still lands on the "no access" page after
  signing in.

### Rate rules

- At most one mail per address per minute, and five per address per day,
  across both kinds of mail.
- At most ten of these mails per sending IP per hour.
- At most 150 of these mails a day across the whole service, which leaves the
  other half of Brevo's 300 for the existing notices. Hitting the cap is
  logged as a warning. (My call, after review.)
- Kept in the database, so a restart forgets nothing. The `notices` table is
  the precedent.
- Over the limit, the page gives the same neutral answer and sends nothing.

### The mails

Plain text, the subject prefixed `stundenglas —` like the existing notices.
They say what happened, give the link, say it lasts an hour, and say what to
do if it wasn't you. Draft wording (the builder may polish it, but the
contents stay):

- **Confirm (de):** "Bestätige deine Adresse" / "Jemand, hoffentlich du, hat
  sich mit dieser Adresse bei stundenglas angemeldet. Um die Anmeldung
  abzuschließen, öffne diesen Link (eine Stunde gültig): … Warst du das
  nicht, ignoriere diese Nachricht; ohne den Link passiert nichts."
- **Confirm (en):** "Confirm your address", with the same content.
- **Reset (de/en):** "Neues Passwort" / "A new password", with the same
  structure. If you didn't ask, ignore it; your password stays as it is.

Reading `Accept-Language` or the language cookie when the mail is asked for
is enough to pick the language. It is not stored per account.

### Without mail

With no `SMTP_URL`, the service keeps today's behaviour: the sign-up form
keeps its password field, sign-up confirms at once, and administrators approve
people by hand. "Forgot your password?" and "send it again" are not shown. The approve button stays for
that case only. A self-hosted copy without a mail server must keep working.
The startup log already says when there is no SMTP; it should also say that
sign-ups will wait for an administrator.

### Housekeeping

- An account still unconfirmed after seven days is deleted (in the existing
  sweep), so a mistyped or stolen address is freed again. "Unconfirmed" means
  GoTrue's `email_confirmed_at` is null. The sweep only runs when mail is
  configured, so no-mail installs, where people wait for approval, are never
  touched.
- With mail on, an account that is confirmed but not approved and not banned
  is let in on its next sign-in. Examples are someone approved by nobody
  before mail was configured, or someone revoked under the old rules. The
  live site has none. (My call, after review.)
- The `Admitted` notice is no longer sent when someone confirms; the
  confirmation is their welcome. It is still sent when an administrator
  approves someone by hand in the no-mail case.

## Rolling out

1. Deploy the code with Supabase still set to autoconfirm. Existing accounts
   (two, both confirmed and approved) are unaffected. In this window
   sign-up still takes no password and sends the mail, so step 2 should
   follow at once.
2. Set `ENABLE_EMAIL_AUTOCONFIRM=false` in `/opt/supabase/.env` and recreate
   the auth container.
3. Check end to end on the live site:
   - Sign up with a mail-tester address and a real one, choose a password on
     confirm, sign out, and sign in.
   - Ask for a reset, use it, and check that an enrolled security key is
     still asked for.
   - Ban and undo the ban, checking that the calendar link stops and comes
     back.

## Left to the builder

- The exact GoTrue v2.196.0 calls: creating an unconfirmed user with no
  password, which `generate_link` type gives a token that confirms it (likely
  `invite` or `magiclink`) and works again for a repeat sign-up, and what
  `verify` returns for each type. Check
  them against that version, not from memory.
- Where the rate counts live: a new table or the `notices` table.
- Wording on the pages, following the tone of `words.rs`.
- Tests: unit tests for the pure parts (mail bodies in both languages, links,
  rate decisions, the admitted-and-not-banned rule), and the live check above.
- README: the sign-up section and `ADMIN_EMAILS` text change to match.
