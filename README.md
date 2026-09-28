# stundenglas — a WebUntis timetable in the calendar you already use

Reads a [WebUntis](https://webuntis.com) timetable and publishes it as a private
calendar link, so it turns up in Google Calendar, iOS, Outlook or Thunderbird
beside everything else in your life.

Many schools leave WebUntis' calendar-sharing switched off, which removes the
subscribe button and leaves students retyping their timetable by hand. Those
flags only hide the buttons — the data is still yours to read.

Run it as a small **service** your friends can sign up to, or as a **command**
on your own machine with no database at all.

| Your timetables | The same, in the dark | What a stranger sees |
| --- | --- | --- |
| [![The dashboard: one card per school, each with its calendar link, its state and its settings](docs/dashboard-light.webp)](docs/dashboard-light.webp) | [![The same dashboard in the dark theme](docs/dashboard-dark.webp)](docs/dashboard-dark.webp) | [![The sign-in page: what it does, and what it asks for](docs/front.webp)](docs/front.webp) |

---

## What you get

A secret URL like `https://your.host/cal/<token>.ics`. Any calendar can
subscribe to it. Nothing to install, nothing to grant, no app to keep open.

It is deliberately more careful than the export WebUntis itself would give you:

| | |
|---|---|
| **Cancelled lessons** | kept as *transparent* entries rather than vanishing, so you can see the free hour and it does not block your availability. WebUntis' own ICS silently drops them |
| **Special events** | named properly; the official export leaves their title empty |
| **Substitutions** | tells *"X is covering for Y"* apart from *"Y is away and nobody is covering"* |
| **Leave of absence** | lessons inside an approved absence are dropped, so an excused day shows empty. Overlap-based, so a half-day leave clears only its own hours |
| **Shared lessons** | a lesson shared between classes is marked changed whenever any class joins or leaves — only a change of teacher or room is flagged |
| **Exams** | from the endpoint the timetable does not carry, marked 📝, and the only thing here that may ring: a reminder per link, a day before or an hour |
| **Homework** | on the day it is due, whole-day and transparent, because it occupies a day without blocking an hour |
| **Holidays** | so an empty week reads as a holiday rather than as a week nobody fetched |
| **Several schools** | one account may link many, each with its own timezone |

Markers: ❌ cancelled · ⚠️ changed · 📌 event · 📝 exam · 📚 homework · 🌴 holiday.

Every link carries its own settings — what to call it, how often calendars are
asked to look again, whether cancelled hours stay, whether exams ring, and
which subjects to leave out entirely. Beside the address there is a `webcal://`
link, which subscribes in one tap on iOS and macOS, and a QR code for pointing
a phone at.

[![Settings for one link: what to call it, how often to look, when to ring, and which subjects to leave out](docs/settings.webp)](docs/settings.webp)

The interface is English and German, following the browser's `Accept-Language`
unless the reader says otherwise.

### On Google, specifically

Google refreshes a subscribed URL on its own schedule — often many hours, and
not adjustable. iOS lets you pick fifteen minutes. If you want minute-fresh
updates in Google itself, connect your account and events are written through
the API the moment we see a change, into a secondary calendar of its own. It
carries what the link carries — lessons, exams, homework and holidays — and
touches only events it made.

That is optional, and the server offers it only when a Google OAuth **web**
client is configured (`GOOGLE_CLIENT_ID` and `GOOGLE_CLIENT_SECRET`, with
`<PUBLIC_URL>/google/callback` as an authorised redirect URI). Without one,
everything else still works.

---

## Run the service

### On Proxmox, in one command

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/137-Trimethylxanthin/stundenglas/main/scripts/proxmox-lxc.sh)"
```

As root, on the Proxmox host. It builds an unprivileged Debian container,
compiles the server inside it, generates the encryption key, writes
`/etc/stundenglas/server.env` and enables the service — stopped, until you fill
in the database details it cannot guess. It also asks where `cloudflared` runs,
because that decides whether the server should listen on loopback or on the
bridge.

### By hand

```sh
supabase start                              # Postgres, auth, Studio
cargo run -p stundenglas-server -- generate-key
```

Put that key somewhere the database is not, then:

```sh
export STUNDENGLAS_KEY=…           DATABASE_URL=postgresql://…
export SUPABASE_URL=…              SUPABASE_ANON_KEY=…
export SUPABASE_SERVICE_KEY=…      PUBLIC_URL=https://your.host
# optional, for pushing into Google Calendar rather than being polled:
export GOOGLE_CLIENT_ID=…          GOOGLE_CLIENT_SECRET=…
# optional, to tell people when something needs them:
export SMTP_URL=smtps://user:pass@smtp.example.test:465
export MAIL_FROM="stundenglas <noreply@example.test>"
# optional: how long a timetable nobody refreshes is kept (default 180 days)
export KEEP_DAYS=180
cargo run -p stundenglas-server
```

Friends sign up, add their school — by name, from WebUntis' own directory, so
nobody has to dig a server host out of a URL — and copy the link. The server
refreshes every account on a timer and serves each feed from cache, so a
hundred subscribers polling cost the school nothing.

`systemd/stundenglas-server.service` is the unit the installer uses, if you are
placing it yourself.

### Who gets in

Signing up is open; being let in is not. A new account waits until an
administrator approves it, and until then can do nothing at all — not even add
a school. Name the administrators by address:

```sh
export ADMIN_EMAILS=you@example.test,someone@example.test
```

Those addresses are admitted the moment they sign up, and see a page listing
everyone waiting, and another showing how every link is faring: who is failing,
who is waiting on a password, whose calendar nobody has ever fetched.

### Security keys

A key may be registered as a **second factor**: after the password, not instead
of it. Supabase's account service offers no passwordless sign-in over its API
yet, so that is as far as it goes for now.

Two things about the relying party, which is derived from `PUBLIC_URL`:

- it must be a hostname, never an IP — WebAuthn refuses `127.0.0.1`, though
  `localhost` is allowed for development
- a key is bound to the host it was registered under. Changing `PUBLIC_URL`
  later invalidates every key already registered, and the same host must appear
  in `supabase/config.toml` under `[auth.webauthn]`

### Operator commands

| | |
|---|---|
| `generate-key` | a fresh `STUNDENGLAS_KEY`, before there is a database |
| `add-account` | attach a WebUntis login to a user and print its calendar URL |
| `list` | a user's schools and their links |
| `link-google` | attach a refresh token obtained elsewhere |
| `sync-now` | refresh every account once and exit |
| `rotate-key` | re-seal every stored secret under a new key |

See `server/README.md` for the details.

---

## Run the command

No database, no accounts, one timetable:

```sh
cargo build --release
install -Dm755 target/release/stundenglas ~/.local/bin/stundenglas

mkdir -p ~/.config/stundenglas && chmod 700 ~/.config/stundenglas
cp .env.example ~/.config/stundenglas/.env && chmod 600 ~/.config/stundenglas/.env
```

Fill in `UNTIS_SERVER`, `UNTIS_SCHOOL`, `UNTIS_USER`, `UNTIS_PASS` and
`UNTIS_TZ`. Find the first two by searching your school at
<https://webuntis.com>: it sends you to `https://<server>/WebUntis/?school=<name>`.

```sh
stundenglas --dir ~/.config/stundenglas --dry-run          # the plan, no writes
stundenglas --dir ~/.config/stundenglas --all-year         # into Google Calendar
stundenglas --dir ~/.config/stundenglas --all-year --ics plan.ics   # just a file
```

Writes only to a secondary calendar it creates, and touches only events it
made. Reruns are idempotent. `systemd/` holds timer units.

---

## How it keeps up

```
WebUntis ──login──▶ scheduler ──▶ Postgres (sealed password, cached timetable)
                        │                            │
                        │                            ├──▶ /cal/<token>.ics
                        └──▶ Google Calendar API     └──▶ the pages
```

Every half hour the scheduler wakes, takes each account that is due, logs in
once, and stores the timetable it finds. Feeds are then served from that cache,
never from WebUntis, so a subscriber never waits on the school and the school
never sees the subscribers.

Three details do most of the work:

- **An ETag per feed.** It covers the timetable *and* the link's own settings,
  so an unchanged feed answers `304` and costs nothing — and a setting you just
  changed is not hidden behind a `304` until the school next moves a lesson.
- **Backoff.** A password the school refuses is not offered again: WebUntis
  locks an account that is tried too often, and a locked school account is not
  something this can give back. The link is disabled until its owner enters a
  new one. A school that is merely unreachable waits five minutes, then ten,
  then twenty, to a ceiling of six hours.
- **Retention.** A timetable nobody has refreshed in `KEEP_DAYS` is forgotten,
  being stale and still somebody's whereabouts.

[![A card whose password the school refused, asking for a new one](docs/refused.webp)](docs/refused.webp)

---

## Keeping other people's passwords

WebUntis offers students no OAuth and no app token, so anything that syncs on
your behalf must hold your school password. That is the central risk of the
service, and it is answered as follows:

- the password is **AES-256-GCM ciphertext**; the key is read from the
  environment, never stored beside the data it protects
- those columns are **not readable through the API at all** — the grant is
  revoked from `anon` and `authenticated` alike
- **row-level security** sits behind that, so one user cannot reach another's
  rows even were the grants wrong
- the **browser never speaks to the database**, only to this server, and it
  carries a session cookie of which only a hash is kept
- `rotate-key` re-seals everything under a new key, row by row, so a leaked key
  does not mean asking everyone to type their password again

None of which changes the underlying fact. If you run this for friends, tell
them plainly what they are handing over, and check what your school's rules say
about it. The service says it too, at `/privacy`, beside a button to download
everything held about an account and one to delete it outright — deletion
cascades from `auth.users`, so the sealed passwords go with it.

---

## Layout

| | |
|---|---|
| `core/` | WebUntis client, iCalendar rendering, Google Calendar syncing |
| `cli/` | the headless binary: one account, files on disk, no database |
| `server/` | the service: accounts, scheduler, feeds, pages |
| `supabase/` | schema migrations |
| `scripts/` | the Proxmox installer |
| `systemd/` | units for the service and for the command |
| `docs/` | the screenshots above |

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p stundenglas-server preview   # writes the pages to target/preview
```

That last one renders the interface to HTML in both languages, so a change to
the stylesheet can be looked at rather than imagined.

`SCOUT.md` documents the WebUntis API as observed, including the traps: a
missing bearer token that answers `404` rather than `401`, the filter default
that hides approved absences, the `klasseId=-1` without which exams return
nothing, and the `showICal` flags behind "our school didn't enable calendar
sharing".

## Name

A *Stundenglas* is an hourglass, and a *Stundenplan* is a timetable. It keeps
the hours.

## Licence

MIT.
