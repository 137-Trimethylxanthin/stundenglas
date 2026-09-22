-- Exams live beside the timetable rather than in it: the endpoint is another
-- one, and a student weigheth them differently from an ordinary hour.
alter table public.sync_state
    add column exams jsonb not null default '[]'::jsonb,
    add column exam_count integer not null default 0;

-- An alarm is a per-calendar taste, not a property of the school, so it sits
-- on the feed. Null meaneth no alarm at all, which is the default: a calendar
-- that ringeth forty times a week is one nobody keeps.
alter table public.feeds
    add column remind_before_minutes integer
        check (remind_before_minutes is null
               or remind_before_minutes between 5 and 10080);

comment on column public.feeds.remind_before_minutes is
    'Minutes before an exam to ring. Only exams ring; ordinary lessons never do.';
