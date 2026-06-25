create table account (
    id             bigserial primary key,
    email          text        not null,
    name           text        not null,
    auth_user_id   bigint      not null,
    created_at     timestamptz not null default now(),
    created_by_cid text        not null,
    unique (auth_user_id)
);
