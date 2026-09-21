-- A school keepeth its own clocks, and WebUntis sendeth wall-clock times with
-- no offset at all. One user may link several schools, each in its own zone,
-- so the zone belongeth to the link and not to the server.
alter table public.untis_accounts
    add column timezone text not null default 'Europe/Vienna';

-- Postgres knoweth the zone table; refuse nonsense at the door.
alter table public.untis_accounts
    add constraint untis_accounts_timezone_known
    check (now() at time zone timezone is not null);

grant select (timezone) on public.untis_accounts to anon, authenticated;
