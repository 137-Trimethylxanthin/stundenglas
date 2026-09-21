# stundenglas

Reads a [WebUntis](https://webuntis.com) timetable and publishes it as a private
calendar link, so it appears in whatever calendar you already use.

Many schools leave WebUntis' calendar-sharing feature switched off, which
removes the subscribe button and leaves students retyping their timetable by
hand. Those flags only hide the buttons — the data is still yours to read.

Run it as a small **service** your friends can sign up to, or as a **command**
on your own machine with no database at all.

---

## What you get

A secret URL like `https://your.host/cal/<token>.ics`. Google Calendar, iOS,
macOS, Outlook and Thunderbird can all subscribe to it. Nothing to install,
nothing to grant.

It is deliberately more careful than the export WebUntis itself would give you:

| | |
|---|---|
| **Cancelled lessons** | kept as *transparent* entries rather than vanishing, so you can see the free hour and it does not block your availability. WebUntis' own ICS silently drops them |
| **Special events** | named properly; the official export leaves their title empty |
| **Substitutions** | tells *"X is covering for Y"* apart from *"Y is away and nobody is covering"* |
| **Leave of absence** | lessons inside an approved absence are dropped, so an excused day shows empty. Overlap-based, so a half-day leave clears only its own hours |
| **Shared lessons** | a lesson shared between classes is marked changed whenever any class joins or leaves — only a change of teacher or room is flagged |
| **Several schools** | one account may link many, each with its own timezone |

Markers: ❌ cancelled · ⚠️ changed · 📌 event.

### On Google, specifically

Google refreshes a subscribed URL on its own schedule — often many hours, and
not adjustable. iOS lets you pick fifteen minutes. If you want minute-fresh
updates in Google itself, the CLI writes events through the API instead, and
the service will grow the same as an option.

---

## Run the service

```sh
supabase start                              # Postgres, auth, Studio
cargo run -p stundenglas-server -- generate-key
```

Put that key somewhere the database is not, then:

```sh
export STUNDENGLAS_KEY=…           DATABASE_URL=postgresql://…
export SUPABASE_URL=…              SUPABASE_ANON_KEY=…
export SUPABASE_SERVICE_KEY=…      PUBLIC_URL=https://your.host
cargo run -p stundenglas-server
```

Friends sign up, add their school with its timezone, and copy the link. The
server refreshes every account on a timer and serves each feed from cache, so a
hundred subscribers polling cost the school nothing.

There are also operator commands — `add-account`, `list`, `sync-now` — for
doing it without the web pages. See `server/README.md`.

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

None of which changes the underlying fact. If you run this for friends, tell
them plainly what they are handing over, and check what your school's rules
say about it.

---

## Layout

| | |
|---|---|
| `core/` | WebUntis client, iCalendar rendering, Google Calendar syncing |
| `cli/` | the headless binary: one account, files on disk, no database |
| `server/` | the service: accounts, scheduler, feeds |
| `supabase/` | schema migrations |

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

`SCOUT.md` documents the WebUntis API as observed, including the traps: a
missing bearer token that answers `404` rather than `401`, the filter default
that hides approved absences, and the `showICal` flags behind "our school
didn't enable calendar sharing".

## Name

A *Stundenglas* is an hourglass, and a *Stundenplan* is a timetable. It keeps
the hours.

## Licence

MIT.
