-- One row per thing we have told somebody, so a scheduler that runs every
-- half hour does not tell them every half hour.
--
-- `about` is the school link a notice concerns, since a user with two schools
-- should hear about each. A notice about the account itself uses the nil uuid:
-- a primary key admits no nulls, and a partial index for the sake of one row
-- shape is not worth the reading.
create table public.notices (
    user_id uuid        not null references auth.users (id) on delete cascade,
    kind    text        not null,
    about   uuid        not null default '00000000-0000-0000-0000-000000000000',
    sent_at timestamptz not null default now(),
    primary key (user_id, kind, about)
);

alter table public.notices enable row level security;

-- Nobody reaches this through the API; the server alone writes it.
revoke all on public.notices from anon, authenticated;

comment on table public.notices is
    'What has already been said to whom, so nothing is said twice.';
