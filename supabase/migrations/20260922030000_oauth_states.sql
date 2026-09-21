-- A consent round-trip must come back to the same browser and the same link,
-- so the state we send to Google is remembered here and spent on return.
create table public.oauth_states (
    state             text primary key,
    user_id           uuid not null references auth.users (id) on delete cascade,
    untis_account_id  uuid not null references public.untis_accounts (id) on delete cascade,
    created_at        timestamptz not null default now(),
    expires_at        timestamptz not null
);
create index oauth_states_expiry_idx on public.oauth_states (expires_at);

alter table public.oauth_states enable row level security;
revoke all on public.oauth_states from anon, authenticated;
grant all on public.oauth_states to service_role;
