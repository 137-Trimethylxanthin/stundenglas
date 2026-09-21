# stundenglas-server

The multi-user service. One WebUntis fetch per account per tick, cached in
Postgres, rendered as a private calendar feed each subscriber may poll freely.

## Why a feed first

A secret `.ics` URL works in Google Calendar, iOS, Outlook and Thunderbird
without any of them granting OAuth to anything. Google refreshes such a
subscription on its own schedule — often many hours — so users who want
minute-fresh updates in Google specifically can additionally link their
account and be pushed to. Everyone else needs nothing but the URL.

## Running it locally

```sh
supabase start                      # Postgres, auth, Studio
stundenglas-server generate-key     # keep this somewhere the database is not
```

Then, with `STUNDENGLAS_KEY`, `DATABASE_URL`, `SUPABASE_URL`,
`SUPABASE_ANON_KEY` and `SUPABASE_SERVICE_KEY` in the environment:

```sh
stundenglas-server add-account --user <uuid> --server <host> \
    --school <name> --username <login>     # password from UNTIS_PASS
stundenglas-server sync-now
stundenglas-server list --user <uuid>
stundenglas-server                         # serve, and refresh on a timer
```

## On keeping other people's passwords

WebUntis offers students no OAuth and no app token, so a service that syncs on
their behalf has no choice but to hold their school password. That is a real
responsibility, and the design answers it thus:

- the password is AES-256-GCM ciphertext in the database, and the key is read
  from the environment — a stolen dump alone openeth nothing
- the ciphertext columns are not readable through the API at all; the grant is
  revoked from `anon` and `authenticated` alike
- row-level security standeth behind that, so one user cannot see another's
  rows even were the grants wrong
- the browser never speaks to the database. It speaks only to this server

None of which removes the underlying fact: friends are trusting you with their
school logins. Tell them so plainly, and check what the school's rules say.
