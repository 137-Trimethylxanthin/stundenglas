-- The browser speaketh only to our server, so it carries our cookie and never
-- a Supabase token. Only the hash of the cookie is kept: a stolen dump then
-- yieldeth no usable session.
create table public.sessions (
    token_hash  bytea primary key,
    user_id     uuid not null references auth.users (id) on delete cascade,
    created_at  timestamptz not null default now(),
    expires_at  timestamptz not null,
    user_agent  text
);
create index sessions_user_idx on public.sessions (user_id);
create index sessions_expiry_idx on public.sessions (expires_at);

alter table public.sessions enable row level security;
revoke all on public.sessions from anon, authenticated;
grant all on public.sessions to service_role;
