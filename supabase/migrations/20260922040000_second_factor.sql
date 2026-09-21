-- To manage a user's security keys we must speak to Supabase as that user, so
-- the session keeps their tokens -- sealed with the same key as everything
-- else, never in the clear.
alter table public.sessions
    add column gotrue_secret bytea,
    add column gotrue_nonce  bytea,
    add column key_version   integer not null default 1;

-- A password accepted but a key not yet presented. Short-lived on purpose: it
-- holds a half-finished sign-in and nothing should linger in that state.
create table public.pending_logins (
    token_hash     bytea primary key,
    user_id        uuid not null references auth.users (id) on delete cascade,
    factor_id      uuid not null,
    gotrue_secret  bytea not null,
    gotrue_nonce   bytea not null,
    key_version    integer not null default 1,
    created_at     timestamptz not null default now(),
    expires_at     timestamptz not null
);
create index pending_logins_expiry_idx on public.pending_logins (expires_at);

alter table public.pending_logins enable row level security;
revoke all on public.pending_logins from anon, authenticated;
grant all on public.pending_logins to service_role;
