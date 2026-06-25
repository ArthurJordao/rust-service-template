# Spec 2b: Refresh / Logout / Postgres Revocation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add refresh-token persistence + `POST /auth/refresh` and `POST /auth/logout`, and enforce **Postgres-backed token revocation** on every protected route via a `RevocationChecker` port in `platform`.

**Architecture:** Refresh tokens are persisted by `jti` in `refresh_token`; logout flips `revoked` and denylists the access `jti` in `revoked_access_token` (one transaction). `platform::auth` gains a `RevocationChecker` trait consulted by the `Authenticated` extractor after signature/exp validation; `domain-auth` ships the Postgres implementation (denylist lookup **or** `iat < auth_user.tokens_valid_from`). A background prune task removes expired denylist rows. No Redis.

**Tech Stack:** axum 0.7, sqlx (runtime API), tokio, tracing, jsonwebtoken 9, chrono, async-trait.

## Global Constraints

- Same as Spec 2a (`docs/superpowers/plans/2026-06-25-rust-spec2a-auth-core.md`, "Global Constraints"). Depends on 2a being complete.
- Delivery/denylist semantics: a token is revoked iff its `jti` is in `revoked_access_token` OR `claims.iat < auth_user.tokens_valid_from` for its subject.
- Refresh tokens do not rotate: `/auth/refresh` returns a new access token and echoes the same refresh token.
- Run `cargo fmt --all` + `cargo clippy --all-targets -- -D warnings` before each commit.

---

### Task 1: Refresh-token + denylist migration

**Files:**
- Create: `migrations/0004_refresh_revocation.sql`

**Interfaces:**
- Produces: `refresh_token` and `revoked_access_token` tables.

- [ ] **Step 1: Write the migration**

`migrations/0004_refresh_revocation.sql`:
```sql
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
```

- [ ] **Step 2: Verify well-formed**

Run: `cargo build -p platform`
Expected: PASS (migrations dir validated).

- [ ] **Step 3: Commit**

```bash
git add migrations/0004_refresh_revocation.sql
git commit -m "feat(auth): refresh_token + revoked_access_token migration"
```

---

### Task 2: `RevocationChecker` port + `Authenticated` extractor change (platform)

**Files:**
- Create: `crates/platform/src/auth/revocation.rs` — NOTE: `auth` is currently a single file. First convert `crates/platform/src/auth.rs` to `crates/platform/src/auth/mod.rs`, then add the submodule.
- Modify: `crates/platform/src/auth/mod.rs` (extractor bound; add `JwtVerifier::decode`)
- Modify: `crates/domain-account/src/ports/http.rs` (AccountState gets a checker field + FromRef)
- Modify: `crates/domain-account/tests/http.rs` and `crates/app/tests/e2e.rs` (provide a checker in test state)
- Test: inline `#[cfg(test)]` in `revocation.rs`

**Interfaces:**
- Produces:
  - `#[async_trait] pub trait RevocationChecker: Send + Sync { async fn is_revoked(&self, claims: &AccessClaims) -> anyhow::Result<bool>; }`
  - `pub struct NoopRevocationChecker;` implementing it (always `Ok(false)`).
  - `pub fn JwtVerifier::decode<T: serde::de::DeserializeOwned>(&self, token: &str) -> Result<T, AppError>` (generic; `verify` delegates to it for `AccessClaims`).
  - `Authenticated` now requires `Arc<dyn RevocationChecker>: FromRef<S>` in addition to `Arc<JwtVerifier>: FromRef<S>`, and rejects revoked tokens with `401`.

- [ ] **Step 1: Convert `auth.rs` to a module directory**

```bash
mkdir -p crates/platform/src/auth
git mv crates/platform/src/auth.rs crates/platform/src/auth/mod.rs
```

- [ ] **Step 2: Write the failing test**

`crates/platform/src/auth/revocation.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AccessClaims;

    fn claims() -> AccessClaims {
        AccessClaims {
            sub: "user-1".into(),
            scopes: vec![],
            exp: 9_999_999_999,
            iat: 0,
            jti: "j".into(),
            email: None,
            token_type: "user".into(),
        }
    }

    #[tokio::test]
    async fn noop_never_revokes() {
        let c = NoopRevocationChecker;
        assert!(!c.is_revoked(&claims()).await.unwrap());
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p platform revocation::`
Expected: FAIL — `NoopRevocationChecker` not found.

- [ ] **Step 4: Write the port + Noop**

`crates/platform/src/auth/revocation.rs` (above the test module):
```rust
use crate::auth::AccessClaims;

/// Decides whether an otherwise-valid access token must be rejected
/// (logged-out jti, or issued before the user's tokens_valid_from epoch).
#[async_trait::async_trait]
pub trait RevocationChecker: Send + Sync {
    async fn is_revoked(&self, claims: &AccessClaims) -> anyhow::Result<bool>;
}

/// Default checker for contexts with no revocation store (and tests). Never revokes.
pub struct NoopRevocationChecker;

#[async_trait::async_trait]
impl RevocationChecker for NoopRevocationChecker {
    async fn is_revoked(&self, _claims: &AccessClaims) -> anyhow::Result<bool> {
        Ok(false)
    }
}
```

- [ ] **Step 5: Wire the submodule + generic decode + extractor change**

At the TOP of `crates/platform/src/auth/mod.rs`, add the submodule declaration and re-export:
```rust
mod revocation;
pub use revocation::{NoopRevocationChecker, RevocationChecker};
```

Replace `JwtVerifier::verify` with a generic `decode` plus a thin `verify`:
```rust
    pub fn verify(&self, token: &str) -> Result<AccessClaims, AppError> {
        self.decode::<AccessClaims>(token)
    }

    /// Decode + validate (signature, exp) any claims shape signed with this key.
    pub fn decode<T: serde::de::DeserializeOwned>(&self, token: &str) -> Result<T, AppError> {
        decode::<T>(token, &self.key, &self.validation)
            .map(|data| data.claims)
            .map_err(|e| AppError::Unauthorized(format!("invalid token: {e}")))
    }
```

Update the `Authenticated` extractor to consult the checker. Replace its impl with:
```rust
#[async_trait::async_trait]
impl<S> FromRequestParts<S> for Authenticated
where
    Arc<JwtVerifier>: FromRef<S>,
    Arc<dyn RevocationChecker>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::Unauthorized("missing Authorization header".into()))?;
        let token = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| AppError::Unauthorized("expected Bearer token".into()))?;
        let verifier = Arc::<JwtVerifier>::from_ref(state);
        let claims = verifier.verify(token)?;

        let checker = Arc::<dyn RevocationChecker>::from_ref(state);
        if checker.is_revoked(&claims).await.map_err(AppError::Internal)? {
            return Err(AppError::Unauthorized("token revoked".into()));
        }
        Ok(Authenticated(claims))
    }
}
```

- [ ] **Step 6: Give `AccountState` a revocation checker**

In `crates/domain-account/src/ports/http.rs`:
- Add the import: `use platform::auth::RevocationChecker;`
- Add a field to `AccountState`:
  ```rust
      pub revocation: Arc<dyn RevocationChecker>,
  ```
- Add the `FromRef` impl:
  ```rust
  impl FromRef<AccountState> for Arc<dyn RevocationChecker> {
      fn from_ref(state: &AccountState) -> Self {
          state.revocation.clone()
      }
  }
  ```

- [ ] **Step 7: Fix `AccountState` constructions in tests**

In `crates/domain-account/tests/http.rs`, add to the `state(...)` builder:
```rust
        revocation: Arc::new(platform::auth::NoopRevocationChecker),
```
In `crates/app/tests/e2e.rs`, add the same field to the `AccountState { … }` literal:
```rust
        revocation: Arc::new(platform::auth::NoopRevocationChecker),
```

- [ ] **Step 8: Run tests + build to verify**

Run: `cargo test -p platform revocation:: && DATABASE_URL=postgres://localhost/postgres cargo test -p domain-account --test http && DATABASE_URL=postgres://localhost/postgres cargo test -p app --test e2e`
Expected: PASS (Noop checker lets existing flows through).

- [ ] **Step 9: Format + lint, then commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/platform crates/domain-account crates/app
git commit -m "feat(platform): RevocationChecker port + Authenticated enforces it (Noop default)"
```

---

### Task 3: `PostgresRevocationChecker` + `RefreshTokenRepository`

**Files:**
- Create: `crates/domain-auth/src/ports/revocation.rs`
- Modify: `crates/domain-auth/src/ports/repository.rs` (add `RefreshTokenRepository`)
- Modify: `crates/domain-auth/src/ports/postgres.rs` (impl repo + store/find/revoke helpers)
- Modify: `crates/domain-auth/src/ports/mod.rs`
- Test: `crates/domain-auth/tests/revocation.rs`

**Interfaces:**
- Consumes: `platform::{auth::{AccessClaims, RevocationChecker}, db::Db}`.
- Produces:
  - `pub struct StoredRefreshToken { pub id: i64, pub jti: String, pub user_id: i64, pub expires_at: DateTime<Utc>, pub revoked: bool }` (`sqlx::FromRow`)
  - `#[async_trait] pub trait RefreshTokenRepository: Send + Sync { async fn store(&self, jti: &str, user_id: i64, expires_at: DateTime<Utc>) -> anyhow::Result<()>; async fn find_by_jti(&self, jti: &str) -> anyhow::Result<Option<StoredRefreshToken>>; async fn revoke(&self, jti: &str) -> anyhow::Result<()>; }`
  - `impl RefreshTokenRepository for PostgresUserRepository` (reuse the same struct/pool).
  - `pub async fn denylist_access_token(pool_or_tx, jti: &str, expires_at: DateTime<Utc>)` — see logout (Task 5); declared here as a free fn on a connection.
  - `pub struct PostgresRevocationChecker { pool: Db }` + `new`, implementing `platform::auth::RevocationChecker`.

- [ ] **Step 1: Write the failing test**

`crates/domain-auth/tests/revocation.rs`:
```rust
use chrono::Utc;
use domain_auth::ports::postgres::PostgresUserRepository;
use domain_auth::ports::revocation::PostgresRevocationChecker;
use domain_auth::ports::RefreshTokenRepository;
use platform::auth::{AccessClaims, RevocationChecker};

fn claims(sub: &str, jti: &str, iat: usize) -> AccessClaims {
    AccessClaims {
        sub: sub.into(),
        scopes: vec![],
        exp: 9_999_999_999,
        iat,
        jti: jti.into(),
        email: None,
        token_type: "user".into(),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn denylisted_jti_is_revoked(pool: sqlx::PgPool) {
    sqlx::query("insert into revoked_access_token (jti, expires_at) values ('bad', now() + interval '1 hour')")
        .execute(&pool).await.unwrap();
    let checker = PostgresRevocationChecker::new(pool.clone());
    assert!(checker.is_revoked(&claims("user-1", "bad", 0)).await.unwrap());
    assert!(!checker.is_revoked(&claims("user-1", "good", 9_999_999_999)).await.unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn token_issued_before_tokens_valid_from_is_revoked(pool: sqlx::PgPool) {
    // Seed a user; bump tokens_valid_from to "now".
    let uid: i64 = sqlx::query_scalar(
        "insert into auth_user (email, password_hash, tokens_valid_from, created_by_cid) \
         values ('a@b.c', 'h', now(), 'cid') returning id",
    )
    .fetch_one(&pool).await.unwrap();
    let checker = PostgresRevocationChecker::new(pool.clone());
    // iat = 0 (epoch) is before tokens_valid_from -> revoked.
    assert!(checker.is_revoked(&claims(&format!("user-{uid}"), "j", 0)).await.unwrap());
    // iat far in the future -> not revoked.
    assert!(!checker.is_revoked(&claims(&format!("user-{uid}"), "j", 9_999_999_999)).await.unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn refresh_token_store_find_revoke(pool: sqlx::PgPool) {
    let uid: i64 = sqlx::query_scalar(
        "insert into auth_user (email, password_hash, created_by_cid) values ('a@b.c','h','cid') returning id",
    )
    .fetch_one(&pool).await.unwrap();
    let repo = PostgresUserRepository::new(pool.clone());
    repo.store("jti-1", uid, Utc::now() + chrono::Duration::days(7)).await.unwrap();
    let found = repo.find_by_jti("jti-1").await.unwrap().unwrap();
    assert!(!found.revoked);
    repo.revoke("jti-1").await.unwrap();
    assert!(repo.find_by_jti("jti-1").await.unwrap().unwrap().revoked);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-auth --test revocation`
Expected: FAIL — items not found.

- [ ] **Step 3: Add the `RefreshTokenRepository` trait**

Append to `crates/domain-auth/src/ports/repository.rs`:
```rust
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredRefreshToken {
    pub id: i64,
    pub jti: String,
    pub user_id: i64,
    pub expires_at: DateTime<Utc>,
    pub revoked: bool,
}

#[async_trait::async_trait]
pub trait RefreshTokenRepository: Send + Sync {
    async fn store(&self, jti: &str, user_id: i64, expires_at: DateTime<Utc>) -> anyhow::Result<()>;
    async fn find_by_jti(&self, jti: &str) -> anyhow::Result<Option<StoredRefreshToken>>;
    async fn revoke(&self, jti: &str) -> anyhow::Result<()>;
}
```

- [ ] **Step 4: Implement the repo on `PostgresUserRepository`**

Append to `crates/domain-auth/src/ports/postgres.rs`:
```rust
use crate::ports::repository::{RefreshTokenRepository, StoredRefreshToken};
use chrono::{DateTime, Utc};

#[async_trait::async_trait]
impl RefreshTokenRepository for PostgresUserRepository {
    async fn store(&self, jti: &str, user_id: i64, expires_at: DateTime<Utc>) -> anyhow::Result<()> {
        sqlx::query(
            "insert into refresh_token (jti, user_id, expires_at) values ($1, $2, $3)",
        )
        .bind(jti)
        .bind(user_id)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn find_by_jti(&self, jti: &str) -> anyhow::Result<Option<StoredRefreshToken>> {
        let row = sqlx::query_as::<_, StoredRefreshToken>(
            "select id, jti, user_id, expires_at, revoked from refresh_token where jti = $1",
        )
        .bind(jti)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn revoke(&self, jti: &str) -> anyhow::Result<()> {
        sqlx::query("update refresh_token set revoked = true where jti = $1")
            .bind(jti)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
```

> `PostgresUserRepository.pool` must be visible here. It is defined in the same file (`postgres.rs`), so the private field is accessible.

- [ ] **Step 5: Implement `PostgresRevocationChecker`**

`crates/domain-auth/src/ports/revocation.rs`:
```rust
use platform::auth::{AccessClaims, RevocationChecker};
use platform::db::Db;

#[derive(Clone)]
pub struct PostgresRevocationChecker {
    pool: Db,
}

impl PostgresRevocationChecker {
    pub fn new(pool: Db) -> Self {
        PostgresRevocationChecker { pool }
    }
}

#[async_trait::async_trait]
impl RevocationChecker for PostgresRevocationChecker {
    async fn is_revoked(&self, claims: &AccessClaims) -> anyhow::Result<bool> {
        // 1. Explicit denylist by jti (logout).
        let denylisted: bool = sqlx::query_scalar(
            "select exists (select 1 from revoked_access_token where jti = $1)",
        )
        .bind(&claims.jti)
        .fetch_one(&self.pool)
        .await?;
        if denylisted {
            return Ok(true);
        }

        // 2. Per-user invalidation epoch: reject tokens issued before tokens_valid_from.
        if let Some(user_id) = claims.sub.strip_prefix("user-").and_then(|s| s.parse::<i64>().ok()) {
            let valid_from: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
                "select tokens_valid_from from auth_user where id = $1",
            )
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;
            if let Some(valid_from) = valid_from {
                if (claims.iat as i64) < valid_from.timestamp() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
```

- [ ] **Step 6: Export from `ports`**

`crates/domain-auth/src/ports/mod.rs` (replace):
```rust
pub mod dto;
pub mod http;
pub mod postgres;
pub mod repository;
pub mod revocation;
pub use repository::{RefreshTokenRepository, UserRepository};
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-auth --test revocation`
Expected: PASS — all three tests.

- [ ] **Step 8: Commit**

```bash
git add crates/domain-auth
git commit -m "feat(auth): PostgresRevocationChecker + RefreshTokenRepository"
```

---

### Task 4: Persist refresh tokens on issuance; add refresh-token verification

**Files:**
- Modify: `crates/domain-auth/src/ports/http.rs` (`AuthState` gains `refresh_tokens` + `verifier`; `issue_token_pair` persists the refresh token)
- Test: extend `crates/domain-auth/tests/http.rs`

**Interfaces:**
- Consumes: `RefreshTokenRepository`, `platform::auth::JwtVerifier`.
- Produces: `AuthState { …, pub refresh_tokens: Arc<dyn RefreshTokenRepository>, pub verifier: Arc<JwtVerifier>, pub revocation: Arc<dyn RevocationChecker> }`; `issue_token_pair` now writes the refresh row.

- [ ] **Step 1: Extend `AuthState` and `issue_token_pair`**

In `crates/domain-auth/src/ports/http.rs`:
- Add imports:
  ```rust
  use crate::ports::RefreshTokenRepository;
  use platform::auth::{JwtVerifier, RevocationChecker};
  ```
- Add fields to `AuthState`:
  ```rust
      pub refresh_tokens: Arc<dyn RefreshTokenRepository>,
      pub verifier: Arc<JwtVerifier>,
      pub revocation: Arc<dyn RevocationChecker>,
  ```
- In `issue_token_pair`, persist the refresh token after issuing it:
  ```rust
      let (jti, refresh_token, refresh_exp) =
          state.issuer.issue_refresh(user.id, now).map_err(AppError::Internal)?;
      state
          .refresh_tokens
          .store(&jti, user.id, refresh_exp)
          .await
          .map_err(AppError::Internal)?;
  ```
  (Replace the old `let (_jti, refresh_token, _exp) = …` line.)

- [ ] **Step 2: Update the `http` test `state(...)` builder**

In `crates/domain-auth/tests/http.rs`, add the new fields (reusing one `PostgresUserRepository` for both repos):
```rust
fn state(pool: sqlx::PgPool) -> AuthState {
    let repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    AuthState {
        pool: pool.clone(),
        users: repo.clone(),
        refresh_tokens: repo.clone(),
        publisher: Arc::new(OutboxPublisher::new(
            Routes::new().add("user.registered", "account.on-user-registered"),
        )),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        verifier: Arc::new(platform::auth::JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        revocation: Arc::new(platform::auth::NoopRevocationChecker),
        admin_emails: Arc::new(vec![]),
        metrics: Metrics::new().unwrap(),
    }
}
```
Add at the top of the test file:
```rust
const TEST_PUB_PEM: &str = include_str!("fixtures/test_pub.pem");
```

- [ ] **Step 2c: Update the `auth_e2e.rs` `AuthState` literal (created in Plan 2a)**

`crates/app/tests/auth_e2e.rs` constructs `AuthState` with the 2a field set; the new
fields make it not compile. Update its literal to reuse one repo for both ports and
supply the verifier + checker. Replace the `AuthState { … }` it builds with:
```rust
    let auth_repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    let auth = router(AuthState {
        pool: pool.clone(),
        users: auth_repo.clone(),
        refresh_tokens: auth_repo.clone(),
        publisher: publisher.clone(),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        verifier: Arc::new(platform::auth::JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        revocation: Arc::new(platform::auth::NoopRevocationChecker),
        admin_emails: Arc::new(vec![]),
        metrics: Metrics::new().unwrap(),
    });
```
and add `const TEST_PUB_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_pub.pem");`
alongside the existing `TEST_PRIV_PEM` const.

- [ ] **Step 3: Verify the refresh row is written on register**

Append a test to `crates/domain-auth/tests/http.rs`:
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn register_persists_refresh_token(pool: sqlx::PgPool) {
    let app = router(state(pool.clone()));
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"a@b.c","password":"hunter2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let n: i64 = sqlx::query_scalar("select count(*) from refresh_token")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1);
}
```

- [ ] **Step 4: Run tests**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-auth --test http`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/domain-auth
git commit -m "feat(auth): persist refresh tokens on issuance; AuthState carries verifier + checker"
```

---

### Task 5: `/auth/refresh` + `/auth/logout` handlers

**Files:**
- Modify: `crates/domain-auth/src/ports/dto.rs` (`RefreshRequest`, `LogoutRequest`)
- Modify: `crates/domain-auth/src/ports/http.rs` (routes + handlers)
- Modify: `crates/domain-auth/src/auth/jwt.rs` (re-export `RefreshClaims` already public)
- Test: `crates/domain-auth/tests/refresh_logout.rs`

**Interfaces:**
- Consumes: `RefreshClaims` (2a), `JwtVerifier::decode`, `RefreshTokenRepository`, `JwtIssuer`, `UserRepository`.
- Produces: `POST /auth/refresh` → `200 AuthTokens` (new access, same refresh); `POST /auth/logout` → `204`.

- [ ] **Step 1: Write the failing test**

`crates/domain-auth/tests/refresh_logout.rs`:
```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::{router, AuthState};
use domain_auth::ports::postgres::PostgresUserRepository;
use http_body_util::BodyExt;
use platform::events::{OutboxPublisher, Routes};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("fixtures/test_priv.pem");
const TEST_PUB_PEM: &str = include_str!("fixtures/test_pub.pem");

fn state(pool: sqlx::PgPool) -> AuthState {
    let repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    AuthState {
        pool: pool.clone(),
        users: repo.clone(),
        refresh_tokens: repo.clone(),
        publisher: Arc::new(OutboxPublisher::new(Routes::new())),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        verifier: Arc::new(platform::auth::JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        revocation: Arc::new(platform::auth::NoopRevocationChecker),
        admin_emails: Arc::new(vec![]),
        metrics: Metrics::new().unwrap(),
    }
}

async fn register_and_get_tokens(app: &axum::Router) -> (String, String) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"a@b.c","password":"pw"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    (
        json["access_token"].as_str().unwrap().to_string(),
        json["refresh_token"].as_str().unwrap().to_string(),
    )
}

#[sqlx::test(migrations = "../../migrations")]
async fn refresh_returns_new_access_token(pool: sqlx::PgPool) {
    let app = router(state(pool));
    let (_at, rt) = register_and_get_tokens(&app).await;

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/refresh")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"refresh_token":"{rt}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn logout_then_refresh_is_unauthorized(pool: sqlx::PgPool) {
    let app = router(state(pool));
    let (at, rt) = register_and_get_tokens(&app).await;

    let logout = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/logout")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"refresh_token":"{rt}","access_token":"{at}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);

    let refresh = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/refresh")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"refresh_token":"{rt}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refresh.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-auth --test refresh_logout`
Expected: FAIL — routes not found / 404.

- [ ] **Step 3: Add the DTOs**

Append to `crates/domain-auth/src/ports/dto.rs`:
```rust
#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub struct LogoutRequest {
    pub refresh_token: String,
    #[serde(default)]
    pub access_token: Option<String>,
}
```

- [ ] **Step 4: Add routes + handlers**

In `crates/domain-auth/src/ports/http.rs`, add to `router`:
```rust
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
```
Add imports:
```rust
use crate::auth::jwt::RefreshClaims;
use crate::ports::dto::{LogoutRequest, RefreshRequest};
```
Add handlers:
```rust
async fn refresh(
    State(state): State<AuthState>,
    Json(body): Json<RefreshRequest>,
) -> Result<Json<AuthTokens>, AppError> {
    let claims: RefreshClaims = state.verifier.decode(&body.refresh_token)?;
    let stored = state
        .refresh_tokens
        .find_by_jti(&claims.jti)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::Unauthorized("refresh token not found".into()))?;
    if stored.revoked {
        return Err(AppError::Unauthorized("refresh token revoked".into()));
    }
    let user = state
        .users
        .find_by_id(stored.user_id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::Unauthorized("user not found".into()))?;

    // Issue a fresh access token; echo the same refresh token (no rotation).
    let db_scopes = state.users.scope_names(user.id).await.map_err(AppError::Internal)?;
    let scopes = effective_scopes(&user.email, db_scopes, &state.admin_emails);
    let now = chrono::Utc::now();
    let (access_token, _claims) = state
        .issuer
        .issue_access(user.id, &user.email, scopes, now)
        .map_err(AppError::Internal)?;
    Ok(Json(AuthTokens {
        access_token,
        refresh_token: body.refresh_token,
        token_type: "Bearer".into(),
        expires_in: state.issuer.access_ttl_seconds(),
    }))
}

async fn logout(
    State(state): State<AuthState>,
    Json(body): Json<LogoutRequest>,
) -> Result<StatusCode, AppError> {
    // Revoke the refresh token if it parses (idempotent on garbage).
    if let Ok(claims) = state.verifier.decode::<RefreshClaims>(&body.refresh_token) {
        state
            .refresh_tokens
            .revoke(&claims.jti)
            .await
            .map_err(AppError::Internal)?;
    }
    // Denylist the access token jti for its remaining lifetime, if supplied + valid.
    if let Some(at) = body.access_token {
        if let Ok(claims) = state.verifier.decode::<platform::auth::AccessClaims>(&at) {
            let expires_at = chrono::DateTime::<chrono::Utc>::from_timestamp(claims.exp as i64, 0)
                .unwrap_or_else(chrono::Utc::now);
            sqlx::query(
                "insert into revoked_access_token (jti, expires_at) values ($1, $2) \
                 on conflict (jti) do nothing",
            )
            .bind(&claims.jti)
            .bind(expires_at)
            .execute(&state.pool)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}
```

> The two operations are written sequentially against the pool. Each is independently idempotent; a partial logout (refresh revoked, denylist skipped) still results in the refresh token being unusable, so wrapping them in one transaction is optional. Keep it simple (no explicit txn) unless a test shows otherwise.

- [ ] **Step 5: Run tests to verify they pass**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-auth --test refresh_logout`
Expected: PASS — refresh 200; logout 204 then refresh 401.

- [ ] **Step 6: Commit**

```bash
git add crates/domain-auth
git commit -m "feat(auth): /auth/refresh + /auth/logout (Postgres revocation)"
```

---

### Task 6: App wiring — Postgres checker + denylist prune task

**Files:**
- Modify: `crates/app/src/state.rs` (build `PostgresRevocationChecker`; inject into both states; extend `auth_state`)
- Modify: `crates/app/src/main.rs` (spawn the prune task)

**Interfaces:**
- Consumes: `domain_auth::ports::revocation::PostgresRevocationChecker`, `domain_auth::ports::postgres::PostgresUserRepository`.
- Produces: revocation enforced in the running binary; expired denylist rows pruned periodically.

- [ ] **Step 1: Build the checker in `Resources` and inject it**

In `crates/app/src/state.rs`:
- Imports:
  ```rust
  use domain_auth::ports::revocation::PostgresRevocationChecker;
  use platform::auth::RevocationChecker;
  ```
- Add to `Resources`:
  ```rust
      pub revocation: Arc<dyn RevocationChecker>,
  ```
- In `build_resources`, construct it:
  ```rust
      let revocation: Arc<dyn RevocationChecker> =
          Arc::new(PostgresRevocationChecker::new(pool.clone()));
  ```
  and add `revocation` to the returned `Resources`.
- In `account_state`, add the field:
  ```rust
          revocation: res.revocation.clone(),
  ```
- In `auth_state`, set the new fields (refresh repo, verifier, checker):
  ```rust
  pub fn auth_state(res: &Resources) -> AuthState {
      let repo = Arc::new(PostgresUserRepository::new(res.pool.clone()));
      AuthState {
          pool: res.pool.clone(),
          users: repo.clone(),
          refresh_tokens: repo.clone(),
          publisher: res.publisher.clone(),
          issuer: res.issuer.clone(),
          verifier: res.jwt.clone(),
          revocation: res.revocation.clone(),
          admin_emails: res.admin_emails.clone(),
          metrics: res.metrics.clone(),
      }
  }
  ```

- [ ] **Step 2: Add a prune helper to `domain-auth`**

Append to `crates/domain-auth/src/ports/revocation.rs`:
```rust
use platform::db::Db;

/// Delete denylist rows whose access tokens have already expired.
pub async fn prune_expired_denylist(pool: &Db) -> anyhow::Result<u64> {
    let result = sqlx::query("delete from revoked_access_token where expires_at < now()")
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}
```
(`use platform::db::Db;` may already be imported at the top of the file from Task 3 Step 5 — if so, do not duplicate it.)

- [ ] **Step 3: Spawn the prune task in `main`**

In `crates/app/src/main.rs`, after spawning the dispatcher, add a prune loop:
```rust
    let prune_pool = res.pool.clone();
    let pruner = tokio::spawn(async move {
        loop {
            if let Err(e) = domain_auth::ports::revocation::prune_expired_denylist(&prune_pool).await {
                tracing::error!(error = %e, "denylist prune failed");
            }
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });
```
Add `pruner` to the `tokio::select!`:
```rust
    tokio::select! {
        r = server => { r?; }
        _ = dispatcher => { tracing::error!("dispatcher exited unexpectedly"); }
        _ = pruner => { tracing::error!("prune task exited unexpectedly"); }
    }
```

- [ ] **Step 4: Verify it compiles + full suite**

Run: `cargo build -p app && DATABASE_URL=postgres://localhost/postgres cargo test`
Expected: PASS across all crates.

- [ ] **Step 5: Format + lint, then commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/app crates/domain-auth
git commit -m "feat(app): wire PostgresRevocationChecker into both domains + denylist prune task"
```

---

## Self-Review

**Spec coverage (against design §5/§6 for the 2b slice):** refresh_token + revoked_access_token migration ✓ (Task 1); `RevocationChecker` port + `NoopRevocationChecker` + extractor enforcement ✓ (Task 2); `PostgresRevocationChecker` (jti denylist + tokens_valid_from) + `RefreshTokenRepository` ✓ (Task 3); refresh persistence ✓ (Task 4); `/auth/refresh` + `/auth/logout` ✓ (Task 5); app wiring + prune task ✓ (Task 6). `tokens_valid_from` *bump* on scope change is part of 2c (the only writer of it).

**Placeholder scan:** none. The `use platform::db::Db;` possible-duplicate in Task 6 Step 2 is explicitly flagged.

**Type consistency:** `RevocationChecker::is_revoked(&AccessClaims) -> anyhow::Result<bool>` identical across Tasks 2/3. `AuthState` field set (users, refresh_tokens, publisher, issuer, verifier, revocation, admin_emails, metrics, pool) consistent across Tasks 4/5/6 and reused in tests. `JwtVerifier::decode::<T>` (Task 2) consumed by refresh/logout (Task 5). `StoredRefreshToken` columns match the migration (Task 1). `RefreshClaims` is the 2a issuance shape.

**Known follow-up (2c):** admin scope endpoints (`/scopes`, `/users`, `/users/:id/scopes`) and the `tokens_valid_from = now()` bump on scope replacement, which makes the per-user invalidation path (already enforced here) actually fire.
