create table outbox_event (
    id             bigserial primary key,
    event_type     text        not null,
    aggregate_id   text        not null,
    payload        jsonb       not null,
    correlation_id text        not null,
    created_at     timestamptz not null default now()
);

create table outbox_delivery (
    id              bigserial primary key,
    event_id        bigint      not null references outbox_event (id),
    subscriber_name text        not null,
    status          text        not null default 'pending',
    attempts        int         not null default 0,
    last_error      text,
    next_attempt_at timestamptz not null default now(),
    created_at      timestamptz not null default now(),
    updated_at      timestamptz not null default now(),
    unique (event_id, subscriber_name)
);

create index outbox_delivery_claim_idx
    on outbox_delivery (subscriber_name, status, next_attempt_at);
