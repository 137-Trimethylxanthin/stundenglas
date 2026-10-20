-- A ban is kept apart from being let in, so lifting one restores what was.
alter table public.profiles add column banned_at timestamptz;

-- Every mail the service sends, so that the sending may be limited.
create table public.mail_sends (
    id       bigserial primary key,
    address  text not null,
    ip       inet,
    kind     text not null,
    sent_at  timestamptz not null default now()
);
create index mail_sends_address_idx on public.mail_sends (address, sent_at);
create index mail_sends_ip_idx on public.mail_sends (ip, sent_at);
create index mail_sends_sent_idx on public.mail_sends (sent_at);

alter table public.mail_sends enable row level security;
revoke all on public.mail_sends from anon, authenticated;
grant all on public.mail_sends to service_role;
grant usage, select on sequence public.mail_sends_id_seq to service_role;
