-- Holidays come from the old JSON-RPC API, which is the only place that
-- offers them, and they are whole days rather than hours.
alter table public.sync_state
    add column holidays      jsonb   not null default '[]'::jsonb,
    add column holiday_count integer not null default 0;

alter table public.feeds
    add column with_holidays boolean not null default true;
