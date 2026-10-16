-- Whether the red "sign in again" notice is up in this Google calendar, so
-- that it is taken down by its id once the password works again — wherever
-- the day it was left on now lies.
alter table public.google_links
    add column notice_shown boolean not null default false;
