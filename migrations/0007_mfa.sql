create table auth_mfa_factor (
    id               bigserial primary key,
    user_id          bigint      not null references auth_user (id),
    type             text        not null default 'totp',
    secret_encrypted bytea       not null,
    confirmed_at     timestamptz,
    failed_attempts  int         not null default 0,
    locked_until     timestamptz,
    created_at       timestamptz not null default now(),
    unique (user_id, type)
);

create table auth_mfa_recovery_code (
    id         bigserial primary key,
    user_id    bigint      not null references auth_user (id),
    code_hash  text        not null,
    used_at    timestamptz,
    created_at timestamptz not null default now()
);
create index auth_mfa_recovery_code_user_idx on auth_mfa_recovery_code (user_id);
