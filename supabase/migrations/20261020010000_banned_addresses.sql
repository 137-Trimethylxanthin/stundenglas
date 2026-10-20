-- A ban outliveth the account: should the banned delete it, or the sweep forget
-- it, the address is still remembered, and a new account under it is barred.
create table public.banned_addresses (
    address    text primary key,
    banned_at  timestamptz not null default now()
);

alter table public.banned_addresses enable row level security;
revoke all on public.banned_addresses from anon, authenticated;
grant all on public.banned_addresses to service_role;
