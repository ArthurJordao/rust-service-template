create table refresh_token (
    id         bigserial primary key,
    jti        text        not null unique,
    user_id    bigint      not null references auth_user (id),
    expires_at timestamptz not null,
    revoked    boolean     not null default false,
    created_at timestamptz not null default now()
);

create table revoked_access_token (
    jti        text        primary key,
    expires_at timestamptz not null
);

create index refresh_token_user_idx on refresh_token (user_id);
