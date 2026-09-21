-- Anyone may sign up; nobody may use the service until an admin says so.
-- Keeping this apart from auth.users leaves Supabase's own table untouched.
create table public.profiles (
    user_id      uuid primary key references auth.users (id) on delete cascade,
    email        text,
    approved     boolean not null default false,
    is_admin     boolean not null default false,
    approved_at  timestamptz,
    approved_by  uuid references auth.users (id) on delete set null,
    created_at   timestamptz not null default now()
);
create index profiles_pending_idx on public.profiles (approved) where not approved;

alter table public.profiles enable row level security;
revoke all on public.profiles from anon, authenticated;
-- A user may see whether they are through the door, and nothing else.
grant select (user_id, approved, is_admin, created_at) on public.profiles to authenticated;
create policy own_profile on public.profiles
    for select using (user_id = (select auth.uid()));
grant all on public.profiles to service_role;
