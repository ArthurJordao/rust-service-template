create table sent_notification (
    id              bigserial primary key,
    source_event_id bigint      not null unique,
    template        text        not null,
    channel         text        not null,
    recipient       text        not null,
    body            text        not null,
    created_at      timestamptz not null default now(),
    created_by_cid  text        not null
);
