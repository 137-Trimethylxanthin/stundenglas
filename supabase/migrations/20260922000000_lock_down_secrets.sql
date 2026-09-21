-- The previous migration tried to hide the ciphertext with a column-level
-- REVOKE. That does nothing while a table-level SELECT grant stands, and
-- Supabase grants exactly that to anon and authenticated on every new table
-- in public. Take the table grant away, then hand back only the safe columns.
--
-- Writes are not granted at all: every insert and update goes through the
-- server, which alone holds the encryption key.

revoke all on public.untis_accounts from anon, authenticated;
revoke all on public.google_links  from anon, authenticated;
revoke all on public.feeds         from anon, authenticated;
revoke all on public.sync_state    from anon, authenticated;

grant select (id, user_id, server, school, username, display_name, person_id,
              enabled, created_at, updated_at)
    on public.untis_accounts to anon, authenticated;
grant delete on public.untis_accounts to authenticated;

grant select (untis_account_id, calendar_id, calendar_name, last_push_at, created_at)
    on public.google_links to anon, authenticated;

grant select (id, untis_account_id, token, keep_cancelled, refresh_minutes,
              label, last_served_at, serve_count, created_at)
    on public.feeds to anon, authenticated;
grant delete on public.feeds to authenticated;

grant select (untis_account_id, lesson_count, window_start, window_end,
              last_ok_at, last_error, last_error_at, consecutive_fails, updated_at)
    on public.sync_state to anon, authenticated;

-- The server works as service_role and must keep the run of the place.
grant all on public.untis_accounts, public.google_links, public.feeds,
             public.sync_state to service_role;
