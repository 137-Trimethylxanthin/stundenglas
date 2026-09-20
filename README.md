# stundenglas

Keeps a personal [WebUntis](https://webuntis.com) timetable in step with Google
Calendar.

Many schools leave WebUntis' calendar-sharing feature switched off, which
removes the subscribe button and leaves you retyping your timetable by hand.
Those flags only hide the buttons, though — the data is still yours to read.
`stundenglas` logs in as you, reads the timetable the same way the web client
does, and mirrors it into a calendar of its own.

It is deliberately more careful than an iCal feed would be:

| | |
|---|---|
| **Cancelled lessons** | kept as red, *transparent* events instead of vanishing, so you can see the free hour and it does not block your availability |
| **Special events** | named properly; WebUntis' own ICS export leaves their title empty |
| **Substitutions** | tells *"X is covering for Y"* apart from *"Y is away and nobody is covering"* |
| **Leave of absence** | lessons overlapping an approved absence are dropped, so an excused day shows empty. Overlap-based, so a half-day leave clears only its own hours |
| **Shared lessons** | a lesson shared between classes is flagged as changed whenever any class joins or leaves — only a teacher or room change is marked |

Markers: ❌ cancelled · ⚠️ changed · 📌 event.

## Install

```sh
cargo build --release
install -Dm755 target/release/stundenglas ~/.local/bin/stundenglas
```

## Configure

```sh
mkdir -p ~/.config/stundenglas && chmod 700 ~/.config/stundenglas
cp .env.example ~/.config/stundenglas/.env
chmod 600 ~/.config/stundenglas/.env
```

Fill in `UNTIS_SERVER`, `UNTIS_SCHOOL`, `UNTIS_USER` and `UNTIS_PASS`. Find the
first two by searching your school at <https://webuntis.com>: it redirects to
`https://<server>/WebUntis/?school=<school>`.

Then give it access to a Google Calendar:

1. [Google Cloud console](https://console.cloud.google.com) → new project →
   enable the **Google Calendar API**
2. Credentials → **Create OAuth client ID** → type **Desktop app** → download
   the JSON to `~/.config/stundenglas/credentials.json`
3. Run it once. With no `token.json` it prints a consent URL, waits on a
   loopback port, and stores the grant

> While the OAuth consent screen is in **Testing**, Google expires refresh
> tokens after **7 days**. Publish the app (Audience → *Publish app*) so it
> keeps working unattended. It stays unverified, which only means a warning
> screen when you first consent.

## Use

```sh
stundenglas --dir ~/.config/stundenglas --dry-run   # show the plan, write nothing
stundenglas --dir ~/.config/stundenglas             # 7 days back, 4 weeks ahead
stundenglas --dir ~/.config/stundenglas --all-year
stundenglas --dir ~/.config/stundenglas --from 2026-09-21 --to 2026-10-16
```

Writes only to a secondary calendar — **"Schule (Untis)"** by default,
`--calendar` to change it — which it creates itself, and only ever touches
events it made (`extendedProperties.private.source = untis`). Your primary
calendar is never opened. Reruns are idempotent: unchanged lessons are left
alone, and a lesson that disappears from WebUntis is removed.

Reconciliation happens strictly *inside* the window you ask for, so narrowing
the range will not delete what now falls outside it.

## Run it on a timer

```sh
cp systemd/*.service systemd/*.timer ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now stundenglas.timer stundenglas-fullyear.timer
loginctl enable-linger "$USER"     # so it runs while you are logged out
```

Half-hourly for the coming four weeks, and once a night for the whole school
year. Authorise interactively at least once first — it refuses to sit waiting
for consent when there is no terminal, rather than hanging a `oneshot` unit
forever.

## How it works

Reads `rest/view/v1/timetable/entries?format=3`, which needs **both** the
session cookie and a bearer token from `api/token/new` — without the bearer
every path answers `404`, not `401`. That endpoint merges consecutive periods
itself, keeps cancellations, names events and reports the replaced teacher, and
serves a whole school year in one request.

`SCOUT.md` documents the API in detail, including the traps.

| | |
|---|---|
| `src/untis.rs` | WebUntis client, lessons and absences |
| `src/gcal.rs` | Google Calendar: diffing, writing, backoff |
| `src/oauth.rs` | installed-app flow and token refresh |
| `src/config.rs` | configuration |
| `src/main.rs` | command line |

```sh
cargo test       # offline: id shapes, markers, absence overlap, DST, fingerprints
cargo clippy --all-targets -- -D warnings
```

## Name

A *Stundenglas* is an hourglass, and a *Stundenplan* is a timetable. It keeps
the hours.

## Licence

MIT.
