-- A password the school has already refused must not be offered again every
-- half hour. WebUntis locks an account that is tried too often, and a locked
-- school account is not something this service can give back.
alter table public.untis_accounts
    add column credentials_rejected boolean not null default false,
    add column rejected_at           timestamptz;

-- When a school that was merely unreachable may be asked again. Null means
-- now: a fresh account, or one that last succeeded.
alter table public.sync_state
    add column next_attempt_at timestamptz;

create index sync_state_next_attempt_idx on public.sync_state (next_attempt_at);

comment on column public.untis_accounts.credentials_rejected is
    'The school refused this login. Cleared only when the owner enters a new password.';
comment on column public.sync_state.next_attempt_at is
    'Set by store_failure after a transport failure, doubling each time to a ceiling.';
