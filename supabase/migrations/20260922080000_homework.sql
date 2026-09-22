-- Homework belongeth to the day it is due, not the hour it was set, and it
-- cometh from an endpoint of its own.
alter table public.sync_state
    add column homework       jsonb   not null default '[]'::jsonb,
    add column homework_count integer not null default 0;

-- Whole-day entries add up. Whoever wanteth their hours uncluttered may say so
-- per link rather than for the whole account.
alter table public.feeds
    add column with_homework boolean not null default true;
