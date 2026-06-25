# domain-auth — Design (Spec 2)

**Date:** 2026-06-25
**Status:** Approved design, ready for implementation planning
**Scope of this spec:** A new `domain-auth` crate — JWT issuance (register / login /
refresh / logout), bcrypt passwords, Postgres-backed token revocation, and admin
scope management. It becomes the real producer of `user.registered`, replacing the
dev stand-in in `domain-account`. Builds on Spec 1 (platform + outbox + domain-account).

---

## 1. Goal & guiding principles

Port the `auth-service` of the Haskell template to this monolith as `domain-auth`,
keeping the established invariants from Spec 1:

- **Hexagonal layering** — pure rules in `domain.rs`; HTTP, DB, crypto in adapters.
- **Ports are traits; DI via `Arc<dyn Port>`.**
- **One crate per domain.** Cross-domain communication via events only.
- **sqlx runtime query API** (no compile-time `query!`).
- **Idiomatic Rust, not a transliteration.**

**Key deviation from the Haskell original: no Redis.** The Haskell service used Redis
for token revocation. This is a monolith that already chose the transactional outbox
over Kafka to avoid running a broker; the same reasoning applies to Redis. Token
revocation is backed by **Postgres** instead — a single indexed lookup on the verify
path, transactionally consistent with the rest of the request, and one fewer piece of
infrastructure. Performance is a non-issue at this template's scale; the verify-path
cost (~sub-millisecond indexed lookup) is dwarfed by the request's own DB work.

---

## 2. Architecture decisions (resolved during brainstorming)

1. **Postgres-backed revocation, not Redis.** Consistent with the outbox-over-Kafka
   decision (durability without extra infra). See §5.
2. **User tokens now; flexible for more later.** Only user access + refresh tokens are
   issued. A `type` claim (value `"user"`) and scope-based roles leave room for future
   `service` tokens and new roles (e.g. `admin` is already just a scope) without a
   schema or token-shape change. Service-token issuance is **out of scope** (YAGNI —
   nothing in a monolith mints or consumes one).
3. **RS256, matching the shipped verifier.** Spec 1's `platform::auth::JwtVerifier`
   already verifies RS256 against an RSA public key. `domain-auth` signs with the
   corresponding RSA **private** key.
4. **Issuance lives in `domain-auth`, not `platform`.** Design §4 of Spec 1 scoped
   `platform::auth` to the *verify* side. Issuance is the auth domain's job and reuses
   platform's claim types.
5. **Revocation is enforced via a `platform` port (Approach A).** `platform::auth`
   defines a `RevocationChecker` trait; the `Authenticated` extractor consults it after
   signature/exp validation. `domain-auth` provides the Postgres implementation. This
   keeps `platform` schema-agnostic while enforcing revocation uniformly for every
   protected route in every domain. See §5.
6. **Refresh tokens do not rotate.** Refresh returns a new access token and the same
   refresh token (matches Haskell; simpler). Rotation is a documented future option.

---

## 3. Crate layout

```
crates/domain-auth/src/
  domain.rs        # pure rules: credential check, token-pair assembly, scope/admin policy
  models.rs        # User, NewUser, RefreshToken, Scope, UserScope
  auth/
    jwt.rs         # JwtIssuer: RS256 issuance (access + refresh); reuses platform AccessClaims
    password.rs    # bcrypt hash/verify (cost 12)
  ports/
    repository.rs  # UserRepository, RefreshTokenRepository, ScopeRepository traits + Postgres adapters
    revocation.rs  # PostgresRevocationChecker (implements platform::auth::RevocationChecker)
    http.rs        # axum handlers + AuthState + router()
  lib.rs           # pub use router, AuthState; exported event payload types
```

Dependency graph stays acyclic: `domain-auth → platform`; `app → domain-auth`.
`domain-auth` does **not** depend on `domain-account` (they communicate only via the
`user.registered` event).

New dependency: `bcrypt` (or `argon2`; bcrypt chosen for parity with the Haskell
cost-12 hashes). Added to the workspace dependency table.

---

## 4. Token design

### Claim shape (extends `platform::auth::AccessClaims`)

```rust
pub struct AccessClaims {
    pub sub: String,           // "user-{id}"
    pub scopes: Vec<String>,
    pub exp: usize,
    pub iat: usize,            // NEW — for tokens_valid_from comparison + logout TTL
    pub jti: String,           // NEW — denylist key
    pub email: Option<String>, // NEW — frontend convenience
    #[serde(rename = "type", default)]
    pub token_type: String,    // NEW — "user" now; room for "service"
}
```

The new fields are additive for *deserialization* (`#[serde(default)]` on `token_type`
and `scopes` keeps it tolerant), but `iat`/`jti` are required struct fields. Existing
direct constructions of `AccessClaims` in Spec 1 tests (`platform::auth` unit tests and
`domain-account`'s `domain.rs`/`http.rs` tests) must be updated to set the new fields —
Plan 2a covers this.

### Issuance — `domain-auth/src/auth/jwt.rs`

`JwtIssuer` holds the RSA private key (`EncodingKey`) + TTLs:

- `issue_access(user_id, email, scopes, now) -> anyhow::Result<(String, AccessClaims)>`
  — builds claims (`sub = "user-{id}"`, `type = "user"`, fresh `jti`, `iat = now`,
  `exp = now + access_ttl`), signs RS256.
- `issue_refresh(user_id, now) -> anyhow::Result<(String /*jti*/, String /*token*/)>`
  — refresh JWT (`sub`, `iat`, `exp = now + refresh_ttl`, `jti`, `type = "refresh"`).

Verification reuses `platform::auth::JwtVerifier` (RS256 public key). A refresh token is
verified for signature/exp, then its `jti` is checked against `refresh_token` in the DB.

### Config (new keys)

| Key | Meaning | Default |
|--|--|--|
| `APP__AUTH__JWT_PRIVATE_KEY_PEM` | RSA private key for issuance | (required) |
| `APP__AUTH__JWT_PUBLIC_KEY_PEM` | RSA public key for verification (from Spec 1) | (required) |
| `APP__AUTH__ACCESS_TOKEN_TTL_SECONDS` | access token lifetime | 900 |
| `APP__AUTH__REFRESH_TOKEN_TTL_DAYS` | refresh token lifetime | 7 |
| `APP__AUTH__ADMIN_EMAILS` | comma-separated bootstrap admin emails | "" |

`AuthSettings` in `platform::config` gains `jwt_private_key_pem`, the two TTLs, and
`admin_emails: Vec<String>`.

---

## 5. Revocation (Postgres-backed)

### Tables (migration `0003_auth.sql`)

- `auth_user(id bigserial pk, email text unique, password_hash text, tokens_valid_from timestamptz not null default now(), created_at timestamptz default now(), created_by_cid text not null)`
- `refresh_token(id bigserial pk, jti text unique, user_id bigint references auth_user(id), expires_at timestamptz, revoked bool default false, created_at timestamptz default now())`
- `revoked_access_token(jti text primary key, expires_at timestamptz not null)`
- `scope(id bigserial pk, name text unique, description text not null)`
- `user_scope(id bigserial pk, user_id bigint references auth_user(id), scope text not null, granted_by bigint, unique(user_id, scope))`

`auth_user.id` is the `auth_user_id` that `domain-account` already keys on.

### The `RevocationChecker` port (in `platform::auth`)

```rust
#[async_trait]
pub trait RevocationChecker: Send + Sync {
    /// Returns true if the token represented by these claims must be rejected.
    async fn is_revoked(&self, claims: &AccessClaims) -> anyhow::Result<bool>;
}

/// Default for domains/tests with no revocation store. Never revokes.
pub struct NoopRevocationChecker;
```

The `Authenticated` extractor (Spec 1) is extended: after `JwtVerifier::verify`
(signature + exp), it resolves `Arc<dyn RevocationChecker>` from state and calls
`is_revoked`; a `true` result (or `Err`) yields `401`. `JwtVerifier::verify` itself
stays a pure function.

`domain-auth::ports::revocation::PostgresRevocationChecker` implements the port:
`is_revoked` returns true when **either** the `jti` exists in `revoked_access_token`
**or** `claims.iat < auth_user.tokens_valid_from` for `claims.sub`. One indexed query
(jti lookup) plus the per-user epoch check.

### Revocation operations

- **Logout:** in one transaction, insert `revoked_access_token(jti, expires_at = access exp)`
  (if an access token is supplied and valid) and set `refresh_token.revoked = true` for the
  refresh token's `jti`. Idempotent: invalid/expired tokens are treated as already logged out.
- **Scope change:** `PUT /users/:id/scopes` replaces rows in `user_scope` and bumps
  `auth_user.tokens_valid_from = now()`, invalidating that user's existing access tokens.
- **Pruning:** a lightweight `tokio` task in `app` (same spawn pattern as the dispatcher)
  periodically deletes `revoked_access_token` rows past `expires_at`.

---

## 6. Endpoints

`domain-auth` exposes a `router(AuthState) -> axum::Router`, merged by `app`.

| Method & path | Auth | Behavior |
|--|--|--|
| `POST /auth/register` | none | bcrypt-hash password, insert `auth_user`, insert default scope `read:accounts:own`, publish `user.registered` **in the same txn**, issue token pair. `409` on duplicate email. `201`. |
| `POST /auth/login` | none | verify credentials (always run a hash compare to reduce timing signal), issue token pair. `401` on failure. `200`. |
| `POST /auth/refresh` | none | verify refresh JWT, look up `jti`, reject if missing/revoked/expired, issue a fresh access token (same refresh). `401` on failure. `200`. |
| `POST /auth/logout` | none | denylist access `jti` + revoke refresh token (one txn). Idempotent. `204`. |
| `GET /scopes` | `admin` | scope catalog. |
| `GET /users` | `admin` | users + their scopes. |
| `GET /users/:id/scopes` | `admin` | a user's scopes. |
| `PUT /users/:id/scopes` | `admin` | replace scopes; bump `tokens_valid_from`. `204`. |
| `GET /status` | none | `"OK"`. |
| `GET /metrics` | none | Prometheus text. |

Admin-email bootstrap: when a user whose email is in `ADMIN_EMAILS` has no `admin`
scope at token-issue time, the `admin` scope is granted first (mirrors the Haskell
bootstrap). DTOs: `RegisterRequest`, `LoginRequest`, `RefreshRequest`, `LogoutRequest`,
`AuthTokens`, `ScopeInfo`, `UserWithScopes`, `SetScopesRequest`.

---

## 7. Wiring changes (`app` + `domain-account`)

- **`domain-account`:** remove `POST /dev/register` and its `DevRegister` type. The
  `user.registered` subscriber (`account.on-user-registered`) is unchanged — it already
  consumes `{auth_user_id, email}`, which is exactly what `domain-auth` publishes.
- **`app/state.rs`:** construct `AuthState`; build a `PostgresRevocationChecker` and
  inject `Arc<dyn RevocationChecker>` into **both** `AccountState` and `AuthState`; merge
  `domain_auth::router`. `Routes` stays `user.registered → account.on-user-registered`,
  now fed by the real producer. Spawn the `revoked_access_token` prune task alongside the
  dispatcher in the `tokio::select!`.
- **`docker-compose.yml`:** unchanged infra — **no Redis**.
- **`.env.example`:** add the new `APP__AUTH__*` keys.

---

## 8. Testing

- **Unit (no DB):** bcrypt hash/verify round-trip; credential check rejects wrong
  password; JWT issue→verify round-trip with a test RSA keypair fixture; admin-email
  bootstrap logic; scope policy. `NoopRevocationChecker` returns false.
- **Integration (`#[sqlx::test(migrations = "../../migrations")]`):**
  - register → login → refresh → logout happy path.
  - duplicate-email register → `409`.
  - refresh with a revoked/expired refresh token → `401`.
  - **denylisted access token rejected by the `Authenticated` extractor** (via
    `PostgresRevocationChecker`).
  - scope change bumps `tokens_valid_from`; an access token issued before the change is
    rejected.
  - full loop: `POST /auth/register` → outbox `user.registered` → dispatcher →
    `domain-account` creates the account and emits `account.created`.

---

## 9. Plan decomposition (for writing-plans)

- **2a — user store + issuance + register/login + producer:** `auth_user`/`scope`/
  `user_scope` migration, `models`, `password`, `JwtIssuer`, `AccessClaims` extension,
  `UserRepository`/`ScopeRepository`, `register`/`login` domain + HTTP, publish
  `user.registered`. Remove `domain-account`'s `/dev/register`.
- **2b — refresh / logout / revocation:** `refresh_token` + `revoked_access_token`
  migration, `RefreshTokenRepository`, `RevocationChecker` port in `platform` +
  `Authenticated` extractor change + `NoopRevocationChecker`, `PostgresRevocationChecker`,
  `refresh`/`logout` domain + HTTP, prune task, app wiring of the checker.
- **2c — admin scope management:** `/scopes`, `/users`, `/users/:id/scopes` (GET/PUT),
  admin-email bootstrap, `tokens_valid_from` bump on scope change.

---

## 10. Out of scope / future

- Service-to-service tokens (`type = "service"`) — the claim shape leaves room; no
  issuance or consumer until a domain is lifted into its own service.
- Refresh-token rotation.
- Email verification / password reset flows.
- Rate limiting on auth endpoints.
- Redis (explicitly rejected for this monolith; revisit only if the verify-path lookup
  ever becomes a measured bottleneck).
