-- Accounts themselves live in auth.users, managed by Supabase.
-- Everything here hangs off that, and is readable only by its owner.

create extension if not exists pgcrypto;

-- ---------------------------------------------------------------- untis ---
-- One row per WebUntis login a user has added. The password is never stored
-- in the clear: `secret` is AES-GCM ciphertext and the key lives in the
-- server's environment, never in this database. A dump alone is not enough.
create table public.untis_accounts (
    id            uuid primary key default gen_random_uuid(),
    user_id       uuid not null references auth.users (id) on delete cascade,
    server        text not null,
    school        text not null,
    username      text not null,
    secret        bytea not null,
    nonce         bytea not null,
    key_version   integer not null default 1,
    display_name  text,
    person_id     bigint,
    enabled       boolean not null default true,
    created_at    timestamptz not null default now(),
    updated_at    timestamptz not null default now(),
    unique (user_id, server, school, username)
);
create index untis_accounts_user_idx on public.untis_accounts (user_id);
create index untis_accounts_due_idx on public.untis_accounts (enabled) where enabled;

-- ----------------------------------------------------------------- feeds ---
-- The secret URL a calendar subscribes to. Held in the clear on purpose: it is
-- a share link the owner must be able to copy again, and it exposes only a
-- timetable. Rotation is a delete and re-create.
create table public.feeds (
    id                uuid primary key default gen_random_uuid(),
    untis_account_id  uuid not null references public.untis_accounts (id) on delete cascade,
    token             text not null unique,
    keep_cancelled    boolean not null default true,
    refresh_minutes   integer not null default 60 check (refresh_minutes between 5 and 1440),
    label             text,
    last_served_at    timestamptz,
    serve_count       bigint not null default 0,
    created_at        timestamptz not null default now()
);
create index feeds_account_idx on public.feeds (untis_account_id);

-- ------------------------------------------------------------ sync state ---
-- The normalised timetable, cached so a feed request never waits on WebUntis.
create table public.sync_state (
    untis_account_id  uuid primary key references public.untis_accounts (id) on delete cascade,
    lessons           jsonb not null default '[]'::jsonb,
    lesson_count      integer not null default 0,
    etag              text,
    window_start      date,
    window_end        date,
    last_ok_at        timestamptz,
    last_error        text,
    last_error_at     timestamptz,
    consecutive_fails integer not null default 0,
    updated_at        timestamptz not null default now()
);

-- --------------------------------------------------------- google push ---
-- Optional: only for users who want minute-fresh updates in Google rather
-- than whatever refresh interval Google feels like using for a subscription.
create table public.google_links (
    untis_account_id  uuid primary key references public.untis_accounts (id) on delete cascade,
    refresh_secret    bytea not null,
    refresh_nonce     bytea not null,
    key_version       integer not null default 1,
    calendar_id       text,
    calendar_name     text not null default 'Schule (Untis)',
    last_push_at      timestamptz,
    created_at        timestamptz not null default now()
);

-- ------------------------------------------------------------- touching ---
create or replace function public.touch_updated_at() returns trigger
language plpgsql as $$
begin
    new.updated_at = now();
    return new;
end;
$$;

create trigger untis_accounts_touch before update on public.untis_accounts
    for each row execute function public.touch_updated_at();
create trigger sync_state_touch before update on public.sync_state
    for each row execute function public.touch_updated_at();

-- ------------------------------------------------------------------ rls ---
-- The server works through the service role and bypasses all of this; these
-- policies guard the case of a user reaching PostgREST with their own key.
alter table public.untis_accounts enable row level security;
alter table public.feeds          enable row level security;
alter table public.sync_state     enable row level security;
alter table public.google_links   enable row level security;

create policy own_untis_accounts on public.untis_accounts
    for all using (user_id = (select auth.uid())) with check (user_id = (select auth.uid()));

create policy own_feeds on public.feeds
    for all using (exists (select 1 from public.untis_accounts a
                            where a.id = untis_account_id and a.user_id = (select auth.uid())))
    with check (exists (select 1 from public.untis_accounts a
                            where a.id = untis_account_id and a.user_id = (select auth.uid())));

create policy own_sync_state on public.sync_state
    for select using (exists (select 1 from public.untis_accounts a
                            where a.id = untis_account_id and a.user_id = (select auth.uid())));

create policy own_google_links on public.google_links
    for all using (exists (select 1 from public.untis_accounts a
                            where a.id = untis_account_id and a.user_id = (select auth.uid())))
    with check (exists (select 1 from public.untis_accounts a
                            where a.id = untis_account_id and a.user_id = (select auth.uid())));

-- The secret columns are never meant to leave the server. Even the owner has
-- no business reading their own ciphertext through the API.
revoke select (secret, nonce) on public.untis_accounts from anon, authenticated;
revoke select (refresh_secret, refresh_nonce) on public.google_links from anon, authenticated;
