-- A subject one doth not attend, or would rather keep out of this particular
-- calendar. Empty, which is the default, leaveth everything in.
alter table public.feeds
    add column hide_subjects text[] not null default '{}';

comment on column public.feeds.hide_subjects is
    'Subject short names to leave out of this feed. Matched against a lesson''s subjects and an exam''s subject.';
