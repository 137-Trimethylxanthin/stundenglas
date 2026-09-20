# Notes on the WebUntis API

What a logged-in student can actually read out of a WebUntis instance, and why
this program reads the endpoint it does. Mapped against **WebUntis 2027.2.1**;
placeholders below (`<server>`, `<school>`, `<personId>`, …) stand for whatever
your own tenant uses.

Everything here was found by watching the official web client's own traffic. No
endpoint is undocumented in the sense of being hidden — they are simply not
published.

---

## 1. Authentication

A plain form login, no browser and no SSO required, provided the tenant still
offers the password form (`hideWuLogin: false`; many also offer Microsoft or an
OIDC provider alongside):

```
POST /WebUntis/j_spring_security_check
     school=<school>&j_username=…&j_password=…&token=

  → 302 Location: /WebUntis/index.do      success
  → 302 back to the login page            bad credentials
  → JSON {"state":"TOKEN_REQUIRED"}       TOTP is enabled on the account
```

This sets `JSESSIONID`, which idles out after 1800 s.

### The bearer token, and a misleading 404

Every `/WebUntis/api/rest/view/v*` path answers **404 when called with only the
session cookie** — not 401, not 403. It reads exactly like "no such endpoint",
and it is easy to conclude the tenant does not have the modern API at all.

It does. Fetch a token and attach it:

```
GET /WebUntis/api/token/new          → an ~840-char JWT, cookie-authenticated
Authorization: Bearer <that token>
```

Verified side by side on one tenant:

| Path | cookie only | + Bearer |
|---|---|---|
| `/api/rest/view/v1/app/data` | 404 | **200** |
| `/api/rest/view/v1/schoolyears` | 404 | **200** |
| `/api/rest/view/v1/messages/status` | 404 | **200** |
| `/api/rest/view/v2/trigger/startup` | 404 | **200** |
| `/api/rest/view/v1/dashboard/cards/status` | 404 | **200** |

Identity and the school-year range then come from
`GET /api/rest/view/v1/app/data` → `user.person.id`,
`currentSchoolYear.dateRange`. The same document carries a `permissions[]` list
— the authoritative statement of what the account may read, e.g. `TT_VIEW:R`,
`EXAMINATION:R`, `HOMEWORK:R`, `PERFORMANCEASSESSMENT:R`, `OFFICEHOURS:R`.

## 2. The timetable

```
GET /WebUntis/api/rest/view/v1/timetable/entries
    ?start=<YYYY-MM-DD>&end=<YYYY-MM-DD>&format=3
    &resourceType=STUDENT&resources=<personId>
    &periodTypes=&timetableType=MY_TIMETABLE
```

This is what the real *my timetable* page calls. It will serve a **whole school
year in one request** — on the tenant measured, ~530 KB and under two seconds
for 375 entries — so there is no need to walk week by week.

One entry:

```jsonc
{
  "ids": [5856529, 5856532],              // consecutive periods, already merged
  "duration": { "start": "2026-09-21T08:00", "end": "2026-09-21T09:40" },
  "type":   "NORMAL_TEACHING_PERIOD",     // or EVENT
  "status": "REGULAR",                    // or CHANGED, CANCELLED
  "position1": [ { "current": { "type": "TEACHER", "shortName": "…" },
                   "removed": null } ],
  "position2": [ { "current": { "type": "SUBJECT", "shortName": "…" } } ],
  "position3": [ { "current": { "type": "ROOM",    "shortName": "…" } } ],
  "position4": [ { "current": { "type": "CLASS",   "shortName": "…" } } ],
  "lessonText": "", "lessonInfo": "", "substitutionText": "", "notesAll": ""
}
```

Things worth knowing, each of which cost a bug:

- **`position2` is not always a `SUBJECT`.** For an `EVENT` it holds an element
  of type **`INFO`** carrying the event's name. Read the slot by type, not by
  position, or events end up nameless.
- **`removed` is the half that matters on a change.** A teacher who is away
  with no cover appears as `current.shortName == "?"` plus the real name under
  `removed`. That is how you tell *"X is covering for Y"* from *"Y is away and
  nobody is covering"*.
- **`status: "CHANGED"` is noisier than it looks.** A lesson shared between
  several classes is marked changed whenever any one of them joins or leaves,
  which for some subjects is every single week. Only treat a change of teacher
  or room as something the student feels.
- `duration` timestamps are **naive local wall-clock** — no offset. The zone is
  the school's; the tenant's own iCal export names it explicitly (see below).

## 3. Absences

```
GET /WebUntis/api/classreg/absences/students
    ?studentId=<personId>&startDate=<YYYYMMDD>&endDate=<YYYYMMDD>
    &excuseStatusId=-1&includeTodaysAbsence=true
```

Classic endpoint: cookie auth, no bearer, compact dates.

`excuseStatusId=-1` means *all*. The Abwesenheiten page in the web client
defaults to **`-3`, which is unexcused only** — so an approved leave is
invisible there, and that is precisely the case worth acting on. Observed
values: `-1`, `-2` and `1` return everything; `-3`, `0`, `2`, `3` returned
nothing for an excused record.

A multi-day absence is one record: `startTime` applies on `startDate`,
`endTime` on `endDate`, and the days between are whole.

## 4. The iCal export

Undocumented but live, and it needs only a session:

```
GET /WebUntis/Ical.do?elemType=5&elemId=<personId>
    &rpt_sd=2026-09-14&rpt_ed=2027-07-11
```

Takes `YYYY-MM-DD` or `YYYYMMDD` (not `DD.MM.YYYY`). `elemType=1&elemId=<klasseId>`
gives a whole class instead. UIDs are `lessonId-periodId[-periodId…]`.

### "Our school did not enable calendar sharing"

That complaint usually means this, from
`GET /api/rest/view/v1/timetable/entries/settings?format=3&resourceType=STUDENT`:

```json
"showICal": false,
"showICalExport": false
```

Those flags only **hide the subscribe and export buttons in the web UI**. They
are not enforced on the server: `Ical.do` answers normally for an authenticated
session, and `timetable/externalCalendar?myTimetable=true` returns `[]` rather
than a permission error. What is actually missing is the *public, tokenised*
URL the UI would otherwise hand out — which is why a program that holds a
session and pushes events is the way round it.

### Why this program does not use the ICS

Compared over one week against `entries?format=3` (33 raw periods):

| | `Ical.do` | `entries` format=3 |
|---|---|---|
| consecutive periods merged | yes | yes (`ids`) |
| timestamps | ISO, with `TZID` | ISO, naive local |
| cancelled lessons | **dropped silently** | kept, `status: CANCELLED` |
| events named | **`SUMMARY` empty** | named in the `INFO` slot |
| replaced teacher | **lost** | in `removed` |
| long names | no | inline |

The counts reconcile exactly: 23 modern entries = 21 ICS events + the 2
cancellations the ICS drops. A cancelled lesson vanishing from the feed is
indistinguishable from one that never existed, which makes "your first period
is cancelled, come in later" impossible to show.

## 5. Other endpoints that work

Classic, cookie-authenticated, dates as `YYYYMMDD`:

| | |
|---|---|
| `GET /api/timegrid` | the bell schedule |
| `GET /api/homeworks/lessons?startDate=&endDate=` | homework |
| `GET /api/exams?studentId=&klasseId=-1&startDate=&endDate=` | exams (needs `klasseId`) |
| `GET /api/classreg/grade/grading/list?studentId=&schoolyearId=` | grades |
| `GET /api/classreg/classregevents?studentId=&startDate=&endDate=` | class-register entries |
| `GET /api/classreg/classservices?startDate=&endDate=` | class services |
| `GET /api/profile/general` | the account's own profile |

Modern, bearer-authenticated: `rest/view/v1/messages`,
`rest/view/v1/exams?start=&end=&withDeleted=false`, `timetable/grid`,
`timetable/filter`, `timetable/entries/settings`.

There is also the older mobile JSON-RPC API at
`POST /WebUntis/jsonrpc.do?school=<school>` (`authenticate`, `getTimetable`,
`getSubjects`, `getRooms`, `getKlassen`, `getHolidays`, `getTimegridUnits`).
It resolves names inline but cannot distinguish a substitution from a normal
lesson — both carry an empty `code`, where only `format=3` separates them.

## 6. Things that do not work

- **Anonymous access** is usually off (`no right for anonymous user`), so every
  call needs a login.
- **The mobile app key** may be unavailable (`appCredentials: null`,
  `publicAppAccessAllowed: false`), leaving no password-less token — credentials
  must be stored.
- `Ical.do` **without a session is 403**, so a calendar application cannot
  subscribe to the URL directly.
- `classreg/homework/{meta,list}` answered 403 under the modern API even with
  the homework permission granted; the classic endpoint works.

## 7. A note on method

Blind probing produced two wrong conclusions: that the modern REST API was
absent (it was the missing bearer, disguised as 404) and that several classic
endpoints did not exist (they wanted different parameters — `klasseId=-1` for
exams, a different path for grades). Both were settled in minutes by opening
the real web client and reading the requests it makes.
