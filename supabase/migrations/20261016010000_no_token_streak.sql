-- Logins in a row that the school neither granted nor plainly refused. A
-- wrong password answered by a bare redirect looketh just like that, and is
-- taken for a refusal once it hath happened often enough; an outage, which
-- also faileth, never counteth toward it.
alter table public.sync_state
    add column if not exists no_token_streak integer not null default 0;
