create table auth_user (
    id                bigserial primary key,
    email             text        not null unique,
    password_hash     text        not null,
    tokens_valid_from timestamptz not null default now(),
    created_at        timestamptz not null default now(),
    created_by_cid    text        not null
);

create table scope (
    id          bigserial primary key,
    name        text not null unique,
    description text not null
);

create table user_scope (
    id         bigserial primary key,
    user_id    bigint not null references auth_user (id),
    scope      text   not null,
    granted_by bigint,
    unique (user_id, scope)
);
