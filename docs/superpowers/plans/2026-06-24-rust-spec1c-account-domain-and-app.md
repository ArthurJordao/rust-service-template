# Spec 1c: Account Domain + App Wiring — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the `domain-account` vertical slice (models, repository port, pure domain rules, event subscriber, HTTP adapter) and wire everything together in the `app` binary, with integration tests proving the full outbox loop, plus tooling (docker-compose, Makefile, scaffolder).

**Architecture:** `domain-account` keeps pure business rules (authorization, account construction) in `domain.rs` — unit-tested with no DB. Reads go through the `AccountRepository` port (fakeable). The single write path (`process_user_registered`) is transactional — it inserts the account and publishes `AccountCreated` to the outbox in one DB transaction — so it lives in the Postgres adapter and is covered by integration tests. Each domain exposes its own axum state + `router()`; the `app` crate builds shared resources, constructs each domain's state, merges routers, registers subscribers, and runs the HTTP server and outbox dispatcher concurrently.

**Tech Stack:** axum 0.7, sqlx (runtime API), tokio, tracing, serde, tower (ServiceExt for tests).

## Global Constraints

- Same dependency pins and rules as Plan 1a; same sqlx runtime-API rule as Plan 1b.
- Depends on Plan 1a (platform foundations) and Plan 1b (events) being complete.
- JWT subject convention `user-{auth_user_id}`; scopes claim `scopes`. Authorization: `admin` scope grants all; otherwise `read:accounts:own` scope AND `sub == "user-{auth_user_id}"`.
- Event type strings: `"user.registered"` and `"account.created"`.
- axum 0.7 path syntax `:id`.
- Run `cargo fmt` + `cargo clippy --all-targets -- -D warnings` before each commit.

---

### Task 1: Account migration

**Files:**
- Create: `migrations/0002_account.sql`

**Interfaces:**
- Produces: `account` table with metadata columns `created_at`, `created_by_cid` and `unique(auth_user_id)`.

- [ ] **Step 1: Write the migration**

`migrations/0002_account.sql`:
```sql
create table account (
    id             bigserial primary key,
    email          text        not null,
    name           text        not null,
    auth_user_id   bigint      not null,
    created_at     timestamptz not null default now(),
    created_by_cid text        not null,
    unique (auth_user_id)
);
```

- [ ] **Step 2: Verify it is well-formed**

Run: `cargo build -p platform`
Expected: PASS (migrations dir validated).

- [ ] **Step 3: Commit**

```bash
git add migrations/0002_account.sql
git commit -m "feat(account): account table migration with metadata columns"
```

---

### Task 2: Models + repository port + pure domain rules

**Files:**
- Create: `crates/domain-account/src/models.rs`
- Create: `crates/domain-account/src/ports/mod.rs`
- Create: `crates/domain-account/src/ports/repository.rs`
- Create: `crates/domain-account/src/domain.rs`
- Modify: `crates/domain-account/src/lib.rs`
- Modify: `crates/domain-account/Cargo.toml` (add deps)
- Test: inline `#[cfg(test)]` in `domain.rs`

**Interfaces:**
- Produces:
  - `pub struct Account { pub id: i64, pub email: String, pub name: String, pub auth_user_id: i64, pub created_at: chrono::DateTime<chrono::Utc>, pub created_by_cid: String }` (derives `sqlx::FromRow`, `serde::Serialize`)
  - `pub struct NewAccount { pub email: String, pub name: String, pub auth_user_id: i64 }`
  - `#[async_trait] pub trait AccountRepository: Send + Sync { async fn list(&self) -> anyhow::Result<Vec<Account>>; async fn find_by_id(&self, id: i64) -> anyhow::Result<Option<Account>>; async fn find_by_auth_user_id(&self, uid: i64) -> anyhow::Result<Option<Account>>; }`
  - `pub fn can_access(claims: &AccessClaims, account: &Account) -> bool`
  - `pub fn authorize(claims: &AccessClaims, account: &Account) -> Result<(), AppError>`

- [ ] **Step 1: Add domain-account deps**

`crates/domain-account/Cargo.toml`:
```toml
[package]
name = "domain-account"
edition.workspace = true
version.workspace = true

[dependencies]
platform = { path = "../platform" }
axum.workspace = true
sqlx.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
async-trait.workspace = true
anyhow.workspace = true
chrono.workspace = true
http.workspace = true
```

- [ ] **Step 2: Write the failing test (authorization rules)**

`crates/domain-account/src/domain.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use platform::auth::AccessClaims;

    fn account(uid: i64) -> Account {
        Account {
            id: 1,
            email: "a@b.c".into(),
            name: "A".into(),
            auth_user_id: uid,
            created_at: chrono::Utc::now(),
            created_by_cid: "cid".into(),
        }
    }
    fn claims(sub: &str, scopes: &[&str]) -> AccessClaims {
        AccessClaims { sub: sub.into(), scopes: scopes.iter().map(|s| s.to_string()).collect(), exp: 9_999_999_999 }
    }

    #[test]
    fn admin_can_access_any_account() {
        assert!(can_access(&claims("user-999", &["admin"]), &account(1)));
    }

    #[test]
    fn owner_with_scope_can_access_own_account() {
        assert!(can_access(&claims("user-7", &["read:accounts:own"]), &account(7)));
    }

    #[test]
    fn non_owner_without_admin_cannot_access() {
        assert!(!can_access(&claims("user-8", &["read:accounts:own"]), &account(7)));
    }

    #[test]
    fn owner_without_scope_cannot_access() {
        assert!(!can_access(&claims("user-7", &[]), &account(7)));
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p domain-account domain::`
Expected: FAIL — `can_access`/`Account` not found.

- [ ] **Step 4: Write the implementations**

`crates/domain-account/src/models.rs`:
```rust
use serde::Serialize;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Account {
    pub id: i64,
    pub email: String,
    pub name: String,
    pub auth_user_id: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by_cid: String,
}

#[derive(Debug, Clone)]
pub struct NewAccount {
    pub email: String,
    pub name: String,
    pub auth_user_id: i64,
}
```

`crates/domain-account/src/ports/repository.rs`:
```rust
use crate::models::Account;

#[async_trait::async_trait]
pub trait AccountRepository: Send + Sync {
    async fn list(&self) -> anyhow::Result<Vec<Account>>;
    async fn find_by_id(&self, id: i64) -> anyhow::Result<Option<Account>>;
    async fn find_by_auth_user_id(&self, uid: i64) -> anyhow::Result<Option<Account>>;
}
```

`crates/domain-account/src/ports/mod.rs`:
```rust
pub mod repository;
pub use repository::AccountRepository;
```

`crates/domain-account/src/domain.rs` (above the test module):
```rust
use crate::models::Account;
use platform::auth::AccessClaims;
use platform::server::AppError;

/// Owner-or-admin policy (mirrors the Haskell AccessPolicy for Account).
pub fn can_access(claims: &AccessClaims, account: &Account) -> bool {
    if claims.has_scope("admin") {
        return true;
    }
    claims.has_scope("read:accounts:own")
        && claims.sub == format!("user-{}", account.auth_user_id)
}

pub fn authorize(claims: &AccessClaims, account: &Account) -> Result<(), AppError> {
    if can_access(claims, account) {
        Ok(())
    } else {
        Err(AppError::Forbidden("not allowed to access this account".into()))
    }
}
```

`crates/domain-account/src/lib.rs`:
```rust
//! Account domain: pure rules + ports + HTTP/event adapters.
pub mod domain;
pub mod models;
pub mod ports;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p domain-account domain::`
Expected: PASS — 4 authorization tests.

- [ ] **Step 6: Commit**

```bash
git add crates/domain-account
git commit -m "feat(account): models, repository port, pure authorization rules"
```

---

### Task 3: Postgres repository adapter + transactional create-with-event

**Files:**
- Create: `crates/domain-account/src/ports/postgres.rs`
- Modify: `crates/domain-account/src/ports/mod.rs`
- Test: `crates/domain-account/tests/repository.rs`

**Interfaces:**
- Consumes: `Account`, `NewAccount` (Task 2); `AccountRepository`; `platform::db::Db`; `platform::events::{EventPublisher, NewEvent}`.
- Produces:
  - `pub struct PostgresAccountRepository { pool: Db }` + `pub fn new(pool: Db) -> Self`
  - `impl AccountRepository for PostgresAccountRepository` (the three reads)
  - `pub async fn create_account_with_event(pool: &Db, publisher: &dyn EventPublisher, new: NewAccount, cid: &str) -> anyhow::Result<Account>` — inserts the account and publishes `account.created` in ONE transaction. Treats a unique-violation on `auth_user_id` as already-created (idempotent) by returning the existing row.

- [ ] **Step 1: Write the failing test**

`crates/domain-account/tests/repository.rs`:
```rust
use domain_account::models::NewAccount;
use domain_account::ports::postgres::{create_account_with_event, PostgresAccountRepository};
use domain_account::ports::AccountRepository;
use platform::events::{OutboxPublisher, Routes};

#[sqlx::test]
async fn create_inserts_account_and_emits_event(pool: sqlx::PgPool) {
    let publisher = OutboxPublisher::new(Routes::new());
    let repo = PostgresAccountRepository::new(pool.clone());

    let acc = create_account_with_event(
        &pool,
        &publisher,
        NewAccount { email: "a@b.c".into(), name: "A".into(), auth_user_id: 42 },
        "cid-1",
    )
    .await
    .unwrap();
    assert_eq!(acc.auth_user_id, 42);
    assert_eq!(acc.created_by_cid, "cid-1");

    // Event row written in the same txn.
    let events: i64 = sqlx::query_scalar(
        "select count(*) from outbox_event where event_type = 'account.created'",
    )
    .fetch_one(&pool).await.unwrap();
    assert_eq!(events, 1);

    // Reads via the port.
    assert!(repo.find_by_auth_user_id(42).await.unwrap().is_some());
    assert_eq!(repo.list().await.unwrap().len(), 1);

    // Idempotent: second create returns existing, no duplicate.
    let again = create_account_with_event(
        &pool, &publisher,
        NewAccount { email: "a@b.c".into(), name: "A".into(), auth_user_id: 42 },
        "cid-2",
    ).await.unwrap();
    assert_eq!(again.id, acc.id);
    assert_eq!(repo.list().await.unwrap().len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-account --test repository`
Expected: FAIL — items not found.

- [ ] **Step 3: Write the implementation**

`crates/domain-account/src/ports/postgres.rs`:
```rust
use crate::models::{Account, NewAccount};
use crate::ports::AccountRepository;
use platform::db::Db;
use platform::events::{EventPublisher, NewEvent};

#[derive(Clone)]
pub struct PostgresAccountRepository {
    pool: Db,
}

impl PostgresAccountRepository {
    pub fn new(pool: Db) -> Self {
        PostgresAccountRepository { pool }
    }
}

#[async_trait::async_trait]
impl AccountRepository for PostgresAccountRepository {
    async fn list(&self) -> anyhow::Result<Vec<Account>> {
        let rows = sqlx::query_as::<_, Account>(
            "select id, email, name, auth_user_id, created_at, created_by_cid \
             from account order by id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn find_by_id(&self, id: i64) -> anyhow::Result<Option<Account>> {
        let row = sqlx::query_as::<_, Account>(
            "select id, email, name, auth_user_id, created_at, created_by_cid \
             from account where id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn find_by_auth_user_id(&self, uid: i64) -> anyhow::Result<Option<Account>> {
        let row = sqlx::query_as::<_, Account>(
            "select id, email, name, auth_user_id, created_at, created_by_cid \
             from account where auth_user_id = $1",
        )
        .bind(uid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }
}

/// Insert an account and publish `account.created` atomically. Idempotent on
/// `auth_user_id`: if the account already exists, returns it without inserting.
pub async fn create_account_with_event(
    pool: &Db,
    publisher: &dyn EventPublisher,
    new: NewAccount,
    cid: &str,
) -> anyhow::Result<Account> {
    let mut tx = pool.begin().await?;

    // Idempotency: return existing row if present.
    if let Some(existing) = sqlx::query_as::<_, Account>(
        "select id, email, name, auth_user_id, created_at, created_by_cid \
         from account where auth_user_id = $1",
    )
    .bind(new.auth_user_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        return Ok(existing);
    }

    let account = sqlx::query_as::<_, Account>(
        "insert into account (email, name, auth_user_id, created_by_cid) \
         values ($1, $2, $3, $4) \
         returning id, email, name, auth_user_id, created_at, created_by_cid",
    )
    .bind(&new.email)
    .bind(&new.name)
    .bind(new.auth_user_id)
    .bind(cid)
    .fetch_one(&mut *tx)
    .await?;

    publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "account.created".into(),
                aggregate_id: account.id.to_string(),
                payload: serde_json::json!({
                    "account_id": account.id,
                    "auth_user_id": account.auth_user_id,
                    "email": account.email,
                }),
                correlation_id: cid.to_string(),
            },
        )
        .await?;

    tx.commit().await?;
    Ok(account)
}
```

`crates/domain-account/src/ports/mod.rs` (replace):
```rust
pub mod postgres;
pub mod repository;
pub use repository::AccountRepository;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-account --test repository`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/domain-account
git commit -m "feat(account): Postgres repository + atomic create-with-event"
```

---

### Task 4: Event payloads + account subscriber

**Files:**
- Create: `crates/domain-account/src/ports/events.rs`
- Modify: `crates/domain-account/src/ports/mod.rs`
- Test: `crates/domain-account/tests/subscriber.rs`

**Interfaces:**
- Consumes: `create_account_with_event` (Task 3); `AccountRepository`; `platform::events::{Subscriber, DeliveredEvent, EventPublisher}`; `Db`.
- Produces:
  - `pub struct UserRegistered { pub auth_user_id: i64, pub email: String }` (`serde::Deserialize`)
  - `pub struct AccountSubscriber { pool: Db, repo: Arc<dyn AccountRepository>, publisher: Arc<dyn EventPublisher> }` + `pub fn new(...)`
  - `impl Subscriber for AccountSubscriber` — `name = "account.on-user-registered"`, `event_type = "user.registered"`, handler deserializes `UserRegistered`, calls `create_account_with_event`.

- [ ] **Step 1: Write the failing test**

`crates/domain-account/tests/subscriber.rs`:
```rust
use std::sync::Arc;
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_account::ports::AccountRepository;
use platform::events::{DeliveredEvent, EventPublisher, OutboxPublisher, Routes, Subscriber};

#[sqlx::test]
async fn subscriber_creates_account_from_user_registered(pool: sqlx::PgPool) {
    let publisher: Arc<dyn EventPublisher> = Arc::new(OutboxPublisher::new(Routes::new()));
    let repo = Arc::new(PostgresAccountRepository::new(pool.clone()));
    let sub = AccountSubscriber::new(pool.clone(), repo.clone(), publisher);

    let event = DeliveredEvent {
        event_id: 1,
        event_type: "user.registered".into(),
        aggregate_id: "55".into(),
        payload: serde_json::json!({ "auth_user_id": 55, "email": "x@y.z" }),
        correlation_id: "cid-9".into(),
    };
    sub.handle(&event).await.unwrap();

    let acc = repo.find_by_auth_user_id(55).await.unwrap().unwrap();
    assert_eq!(acc.email, "x@y.z");
    assert_eq!(acc.created_by_cid, "cid-9");

    // Idempotent: handling the same event again does not duplicate.
    sub.handle(&event).await.unwrap();
    assert_eq!(repo.list().await.unwrap().len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-account --test subscriber`
Expected: FAIL — `AccountSubscriber` not found.

- [ ] **Step 3: Write the implementation**

`crates/domain-account/src/ports/events.rs`:
```rust
use crate::models::NewAccount;
use crate::ports::postgres::create_account_with_event;
use crate::ports::AccountRepository;
use platform::db::Db;
use platform::events::{DeliveredEvent, EventPublisher, Subscriber};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct UserRegistered {
    pub auth_user_id: i64,
    pub email: String,
}

pub struct AccountSubscriber {
    pool: Db,
    repo: Arc<dyn AccountRepository>,
    publisher: Arc<dyn EventPublisher>,
}

impl AccountSubscriber {
    pub fn new(
        pool: Db,
        repo: Arc<dyn AccountRepository>,
        publisher: Arc<dyn EventPublisher>,
    ) -> AccountSubscriber {
        AccountSubscriber { pool, repo, publisher }
    }
}

#[async_trait::async_trait]
impl Subscriber for AccountSubscriber {
    fn name(&self) -> &'static str {
        "account.on-user-registered"
    }
    fn event_type(&self) -> &'static str {
        "user.registered"
    }
    async fn handle(&self, event: &DeliveredEvent) -> anyhow::Result<()> {
        let payload: UserRegistered = serde_json::from_value(event.payload.clone())?;

        // Fast-path idempotency check (the create is also idempotent).
        if self.repo.find_by_auth_user_id(payload.auth_user_id).await?.is_some() {
            tracing::info!(uid = payload.auth_user_id, "account already exists; skipping");
            return Ok(());
        }

        create_account_with_event(
            &self.pool,
            self.publisher.as_ref(),
            NewAccount {
                email: payload.email.clone(),
                name: payload.email,
                auth_user_id: payload.auth_user_id,
            },
            &event.correlation_id,
        )
        .await?;
        Ok(())
    }
}
```

`crates/domain-account/src/ports/mod.rs` (replace):
```rust
pub mod events;
pub mod postgres;
pub mod repository;
pub use repository::AccountRepository;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-account --test subscriber`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/domain-account
git commit -m "feat(account): UserRegistered subscriber creates accounts idempotently"
```

---

### Task 5: JWT extractor (platform) + account HTTP router

**Files:**
- Modify: `crates/platform/src/auth.rs` (add `Authenticated` extractor)
- Create: `crates/domain-account/src/ports/http.rs`
- Modify: `crates/domain-account/src/ports/mod.rs`
- Modify: `crates/domain-account/src/lib.rs` (export `AccountState`, `router`)
- Test: `crates/domain-account/tests/http.rs`

**Interfaces:**
- Consumes: `JwtVerifier`, `AccessClaims` (platform::auth); `AccountRepository`; `create_account_with_event`; `Metrics`; `EventPublisher`; `Db`.
- Produces (platform):
  - `pub struct Authenticated(pub AccessClaims)` with `impl<S> FromRequestParts<S> for Authenticated where Arc<JwtVerifier>: FromRef<S>, S: Send + Sync` — reads `Authorization: Bearer <token>`, verifies, yields claims.
- Produces (domain-account):
  - `#[derive(Clone)] pub struct AccountState { pub pool: Db, pub repo: Arc<dyn AccountRepository>, pub publisher: Arc<dyn EventPublisher>, pub jwt: Arc<JwtVerifier>, pub metrics: Metrics }` with `impl FromRef<AccountState> for Arc<JwtVerifier>`
  - `pub fn router(state: AccountState) -> axum::Router` — routes: `GET /status`, `GET /accounts`, `GET /accounts/:id` (auth + scope), `GET /metrics`, `POST /dev/register`.

- [ ] **Step 1: Write the failing test (unauthorized path)**

`crates/domain-account/tests/http.rs`:
```rust
use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::http::{router, AccountState};
use domain_account::ports::postgres::PostgresAccountRepository;
use platform::auth::JwtVerifier;
use platform::events::{OutboxPublisher, Routes};
use platform::metrics::Metrics;
use tower::ServiceExt;

// A minimal valid RSA public key PEM is required to build a verifier; for these
// tests we only exercise unauthenticated paths, so any well-formed PEM works.
const TEST_PUB_PEM: &str = include_str!("fixtures/test_pub.pem");

fn state(pool: sqlx::PgPool) -> AccountState {
    AccountState {
        pool: pool.clone(),
        repo: Arc::new(PostgresAccountRepository::new(pool)),
        publisher: Arc::new(OutboxPublisher::new(Routes::new())),
        jwt: Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        metrics: Metrics::new().unwrap(),
    }
}

#[sqlx::test]
async fn status_returns_ok(pool: sqlx::PgPool) {
    let app = router(state(pool));
    let res = app
        .oneshot(Request::builder().uri("/status").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[sqlx::test]
async fn get_account_without_token_is_unauthorized(pool: sqlx::PgPool) {
    let app = router(state(pool));
    let res = app
        .oneshot(Request::builder().uri("/accounts/1").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 2: Create the test fixture key**

Run (generates an RSA public key PEM for tests):
```bash
mkdir -p crates/domain-account/tests/fixtures
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out /tmp/test_priv.pem
openssl rsa -in /tmp/test_priv.pem -pubout -out crates/domain-account/tests/fixtures/test_pub.pem
```

- [ ] **Step 3: Run test to verify it fails**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-account --test http`
Expected: FAIL — `router`/`AccountState` not found.

- [ ] **Step 4: Add the `Authenticated` extractor to platform**

Append to `crates/platform/src/auth.rs`:
```rust
use axum::extract::{FromRef, FromRequestParts};
use std::sync::Arc;

/// Extractor that verifies a Bearer token and yields its claims.
/// Works with any axum state from which `Arc<JwtVerifier>` can be borrowed.
pub struct Authenticated(pub AccessClaims);

#[async_trait::async_trait]
impl<S> FromRequestParts<S> for Authenticated
where
    Arc<JwtVerifier>: FromRef<S>,
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
        Ok(Authenticated(claims))
    }
}
```

- [ ] **Step 5: Write the account HTTP router**

`crates/domain-account/src/ports/http.rs`:
```rust
use crate::domain::authorize;
use crate::models::Account;
use crate::ports::AccountRepository;
use axum::extract::{FromRef, Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use http::StatusCode;
use platform::auth::{Authenticated, JwtVerifier};
use platform::db::Db;
use platform::events::{EventPublisher, NewEvent};
use platform::metrics::Metrics;
use platform::observability::CorrelationId;
use platform::server::{status_handler, AppError};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
pub struct AccountState {
    pub pool: Db,
    pub repo: Arc<dyn AccountRepository>,
    pub publisher: Arc<dyn EventPublisher>,
    pub jwt: Arc<JwtVerifier>,
    pub metrics: Metrics,
}

impl FromRef<AccountState> for Arc<JwtVerifier> {
    fn from_ref(state: &AccountState) -> Self {
        state.jwt.clone()
    }
}

pub fn router(state: AccountState) -> Router {
    Router::new()
        .route("/status", get(status_handler))
        .route("/accounts", get(list_accounts))
        .route("/accounts/:id", get(get_account))
        .route("/metrics", get(metrics_handler))
        .route("/dev/register", post(dev_register))
        .with_state(state)
}

async fn list_accounts(
    State(state): State<AccountState>,
) -> Result<Json<Vec<Account>>, AppError> {
    let accounts = state.repo.list().await.map_err(AppError::Internal)?;
    Ok(Json(accounts))
}

async fn get_account(
    State(state): State<AccountState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<i64>,
) -> Result<Json<Account>, AppError> {
    let account = state
        .repo
        .find_by_id(id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound("account not found".into()))?;
    authorize(&claims, &account)?;
    Ok(Json(account))
}

async fn metrics_handler(State(state): State<AccountState>) -> String {
    state.metrics.render()
}

#[derive(Deserialize)]
struct DevRegister {
    auth_user_id: i64,
    email: String,
}

/// DEV-ONLY: publish a `user.registered` event to exercise the outbox loop.
/// Replaced by the real auth domain in Spec 2.
async fn dev_register(
    State(state): State<AccountState>,
    CorrelationId(cid): CorrelationId,
    Json(body): Json<DevRegister>,
) -> Result<StatusCode, AppError> {
    let mut tx = state.pool.begin().await.map_err(|e| AppError::Internal(e.into()))?;
    state
        .publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "user.registered".into(),
                aggregate_id: body.auth_user_id.to_string(),
                payload: serde_json::json!({
                    "auth_user_id": body.auth_user_id,
                    "email": body.email,
                }),
                correlation_id: cid,
            },
        )
        .await
        .map_err(AppError::Internal)?;
    tx.commit().await.map_err(|e| AppError::Internal(e.into()))?;
    Ok(StatusCode::ACCEPTED)
}
```

`crates/domain-account/src/ports/mod.rs` (replace):
```rust
pub mod events;
pub mod http;
pub mod postgres;
pub mod repository;
pub use repository::AccountRepository;
```

`crates/domain-account/src/lib.rs` (replace):
```rust
//! Account domain: pure rules + ports + HTTP/event adapters.
pub mod domain;
pub mod models;
pub mod ports;

pub use ports::http::{router, AccountState};
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-account --test http`
Expected: PASS — status 200, unauthorized 401.

- [ ] **Step 7: Format + lint, removing the placeholder aliases**

Run: `cargo fmt --all && cargo clippy -p domain-account --all-targets -- -D warnings`
Expected: clean (delete the placeholder `use`/alias lines noted above until it passes).

- [ ] **Step 8: Commit**

```bash
git add crates/platform crates/domain-account
git commit -m "feat(account): Authenticated extractor + HTTP router (accounts, dev/register, metrics)"
```

---

### Task 6: `app` crate — resources, wiring, server + dispatcher

**Files:**
- Modify: `crates/app/Cargo.toml` (add deps)
- Create: `crates/app/src/state.rs`
- Modify: `crates/app/src/main.rs`
- Create: `.env.example`

**Interfaces:**
- Consumes: `platform::{config::Settings, db, observability, events, auth, metrics, server}`; `domain_account::{router, AccountState}`, `domain_account::ports::{postgres::PostgresAccountRepository, events::AccountSubscriber}`.
- Produces: a runnable binary that serves HTTP and runs the dispatcher.

- [ ] **Step 1: Add app deps**

`crates/app/Cargo.toml`:
```toml
[package]
name = "app"
edition.workspace = true
version.workspace = true

[dependencies]
platform = { path = "../platform" }
domain-account = { path = "../domain-account" }
tokio.workspace = true
axum.workspace = true
tracing.workspace = true
anyhow.workspace = true
```

- [ ] **Step 2: Write the resource/wiring module**

`crates/app/src/state.rs`:
```rust
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_account::AccountState;
use platform::auth::JwtVerifier;
use platform::config::Settings;
use platform::db::{self, Db};
use platform::events::{DispatcherConfig, EventPublisher, OutboxPublisher, Routes, SubscriberRegistry};
use platform::metrics::Metrics;

/// All shared resources, constructed once at startup.
pub struct Resources {
    pub settings: Settings,
    pub pool: Db,
    pub registry: Arc<SubscriberRegistry>,
    pub publisher: Arc<dyn EventPublisher>,
    pub jwt: Arc<JwtVerifier>,
    pub metrics: Metrics,
}

/// Static routing table: every (event_type, subscriber_name) pair the system
/// knows about. Declared here so the publisher never depends on subscriber
/// instances — this is what keeps construction linear and cycle-free.
fn routes() -> Routes {
    Routes::new().add("user.registered", "account.on-user-registered")
}

pub async fn build_resources(settings: Settings) -> anyhow::Result<Resources> {
    let pool = db::make_pool(&settings.database)
        .await
        .context("create db pool")?;

    if settings.database.auto_migrate {
        tracing::info!("running migrations (auto_migrate=true)");
        db::run_migrations(&pool).await.context("run migrations")?;
    }

    let jwt = Arc::new(
        JwtVerifier::from_rsa_pem(&settings.auth.jwt_public_key_pem)
            .context("parse JWT public key")?,
    );
    let metrics = Metrics::new().context("init metrics")?;

    // Linear construction (no cycle):
    // 1) publisher depends only on Routes (plain data),
    let publisher: Arc<dyn EventPublisher> = Arc::new(OutboxPublisher::new(routes()));
    // 2) subscribers depend on the publisher,
    let account_repo = Arc::new(PostgresAccountRepository::new(pool.clone()));
    let mut registry = SubscriberRegistry::new();
    registry.register(Arc::new(AccountSubscriber::new(
        pool.clone(),
        account_repo.clone(),
        publisher.clone(),
    )));
    // 3) the registry (subscriber instances) is consumed only by the dispatcher.
    let registry = Arc::new(registry);

    Ok(Resources {
        settings,
        pool,
        registry,
        publisher,
        jwt,
        metrics,
    })
}

pub fn account_state(res: &Resources) -> AccountState {
    AccountState {
        pool: res.pool.clone(),
        repo: Arc::new(PostgresAccountRepository::new(res.pool.clone())),
        publisher: res.publisher.clone(),
        jwt: res.jwt.clone(),
        metrics: res.metrics.clone(),
    }
}

pub fn dispatcher_handle(
    res: &Resources,
) -> (Db, Arc<SubscriberRegistry>, DispatcherConfig, Duration) {
    (
        res.pool.clone(),
        res.registry.clone(),
        DispatcherConfig::default(),
        Duration::from_secs(2),
    )
}
```

> **Why this compiles cleanly:** the routing table (`routes()`) is plain data, so `OutboxPublisher` is a leaf — it depends on nothing else. The dependency chain is strictly `Routes → publisher → subscribers → registry → dispatcher`, with no back-edges, so there is no `Arc` cycle and no need for `Arc::new_cyclic` or interior mutability. The only contract to keep in sync is that every subscriber registered in `registry` has a matching entry in `routes()` (same event_type + name); a mismatch means an event is published but no delivery row is created (or vice-versa). Keep both in this file so they are edited together.

- [ ] **Step 3: Write `main`**

`crates/app/src/main.rs`:
```rust
mod state;

use platform::config::Settings;
use platform::events::run_dispatcher;
use platform::observability::{correlation_id_middleware, init_tracing};
use platform::server::cors_layer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing("info");
    let settings = Settings::load()?;
    let port = settings.server.port;
    let cors = cors_layer(&settings.cors_allowed_origins);

    let res = state::build_resources(settings).await?;

    let app = domain_account::router(state::account_state(&res))
        .layer(axum::middleware::from_fn(correlation_id_middleware))
        .layer(cors);

    let (pool, registry, dispatcher_cfg, interval) = state::dispatcher_handle(&res);
    let dispatcher = tokio::spawn(run_dispatcher(pool, registry, dispatcher_cfg, interval));

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(port, "HTTP server listening");
    let server = axum::serve(listener, app);

    tokio::select! {
        r = server => { r?; }
        _ = dispatcher => { tracing::error!("dispatcher exited unexpectedly"); }
    }
    Ok(())
}
```

- [ ] **Step 4: Write `.env.example`**

`.env.example`:
```bash
APP__SERVER__PORT=8080
APP__SERVER__ENVIRONMENT=local
APP__DATABASE__URL=postgres://postgres:postgres@localhost:5432/app
APP__DATABASE__MAX_CONNECTIONS=5
APP__DATABASE__AUTO_MIGRATE=true
APP__AUTH__JWT_PUBLIC_KEY_PEM="-----BEGIN PUBLIC KEY-----\n...replace...\n-----END PUBLIC KEY-----"
APP__CORS_ALLOWED_ORIGINS=http://localhost:5173
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo build -p app`
Expected: PASS. (If the publisher/subscriber construction does not compile, apply the Implementation Note in Step 2 until it builds.)

- [ ] **Step 6: Commit**

```bash
git add crates/app .env.example
git commit -m "feat(app): wire resources, account router, dispatcher, server"
```

---

### Task 7: End-to-end integration test (the full outbox loop)

**Files:**
- Create: `crates/app/tests/e2e.rs`
- Modify: `crates/app/Cargo.toml` (add dev-deps)

**Interfaces:**
- Consumes: `domain_account::router`, `AccountState`, the dispatcher, repository.

- [ ] **Step 1: Add dev-deps**

Append to `crates/app/Cargo.toml`:
```toml
[dev-dependencies]
domain-account = { path = "../domain-account" }
platform = { path = "../platform" }
sqlx.workspace = true
tower = { workspace = true, features = ["util"] }
serde_json.workspace = true
http.workspace = true
axum.workspace = true
```

- [ ] **Step 2: Write the failing e2e test**

`crates/app/tests/e2e.rs`:
```rust
use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_account::ports::AccountRepository;
use domain_account::ports::http::{router, AccountState};
use platform::auth::JwtVerifier;
use platform::events::{
    dispatch_once, DispatcherConfig, EventPublisher, OutboxPublisher, Routes, SubscriberRegistry,
};
use platform::metrics::Metrics;
use tower::ServiceExt;

const TEST_PUB_PEM: &str = include_str!("../../domain-account/tests/fixtures/test_pub.pem");

#[sqlx::test]
async fn dev_register_then_dispatch_creates_account(pool: sqlx::PgPool) {
    // Build publisher -> subscriber -> registry (mirrors app wiring; linear, no cycle).
    let repo = Arc::new(PostgresAccountRepository::new(pool.clone()));
    let publisher: Arc<dyn EventPublisher> = Arc::new(OutboxPublisher::new(
        Routes::new().add("user.registered", "account.on-user-registered"),
    ));
    let mut registry = SubscriberRegistry::new();
    registry.register(Arc::new(AccountSubscriber::new(
        pool.clone(),
        repo.clone(),
        publisher.clone(),
    )));
    let registry = Arc::new(registry);

    let state = AccountState {
        pool: pool.clone(),
        repo: repo.clone(),
        publisher: publisher.clone(),
        jwt: Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        metrics: Metrics::new().unwrap(),
    };
    let app = router(state);

    // 1. POST /dev/register publishes user.registered.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/dev/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"auth_user_id":77,"email":"e2e@x.y"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    // 2. Dispatcher delivers the event -> account subscriber creates account.
    dispatch_once(&pool, &registry, &DispatcherConfig::default()).await.unwrap();

    // 3. Account now exists, and account.created was emitted.
    let acc = repo.find_by_auth_user_id(77).await.unwrap();
    assert!(acc.is_some(), "account created by event handler");

    let created: i64 = sqlx::query_scalar(
        "select count(*) from outbox_event where event_type = 'account.created'",
    )
    .fetch_one(&pool).await.unwrap();
    assert_eq!(created, 1);
}
```

- [ ] **Step 3: Run test to verify it fails, then passes**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p app --test e2e`
Expected: initially FAIL if wiring exports are missing; once `domain_account::ports::http` is public and the subscriber/publisher compile, PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/app
git commit -m "test(app): e2e outbox loop (dev/register -> dispatch -> account.created)"
```

---

### Task 8: Tooling — docker-compose, Makefile, scaffolder, README

**Files:**
- Create: `docker-compose.yml`
- Create: `Makefile`
- Create: `scripts/new-domain.sh`
- Create: `README.md`

**Interfaces:**
- Produces: local-dev tooling. No code dependencies.

- [ ] **Step 1: docker-compose**

`docker-compose.yml`:
```yaml
services:
  postgres:
    image: postgres:16
    environment:
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: postgres
      POSTGRES_DB: app
    ports:
      - "5432:5432"
    volumes:
      - pgdata:/var/lib/postgresql/data

  prometheus:
    image: prom/prometheus:latest
    ports:
      - "9090:9090"
    volumes:
      - ./infra/prometheus.yml:/etc/prometheus/prometheus.yml:ro

  grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
    environment:
      GF_AUTH_ANONYMOUS_ENABLED: "true"
      GF_AUTH_ANONYMOUS_ORG_ROLE: Admin

volumes:
  pgdata:
```

Create `infra/prometheus.yml`:
```yaml
global:
  scrape_interval: 15s
scrape_configs:
  - job_name: app
    static_configs:
      - targets: ["host.docker.internal:8080"]
    metrics_path: /metrics
```

- [ ] **Step 2: Makefile**

`Makefile`:
```makefile
DATABASE_URL ?= postgres://postgres:postgres@localhost:5432/app

.PHONY: up down run test migrate fmt lint new-domain

up:
	docker compose up -d postgres prometheus grafana

down:
	docker compose down

run:
	cargo run -p app

test:
	DATABASE_URL=$(DATABASE_URL) cargo test

migrate:
	DATABASE_URL=$(DATABASE_URL) sqlx migrate run

fmt:
	cargo fmt --all

lint:
	cargo clippy --all-targets -- -D warnings

new-domain:
	./scripts/new-domain.sh $(name)
```

- [ ] **Step 3: Scaffolder**

`scripts/new-domain.sh`:
```bash
#!/usr/bin/env bash
set -euo pipefail

name="${1:?usage: new-domain.sh <name>}"
crate="domain-${name}"
dir="crates/${crate}"

if [ -d "$dir" ]; then
  echo "error: $dir already exists" >&2
  exit 1
fi

mkdir -p "$dir/src/ports" "$dir/tests"

cat > "$dir/Cargo.toml" <<EOF
[package]
name = "${crate}"
edition.workspace = true
version.workspace = true

[dependencies]
platform = { path = "../platform" }
axum.workspace = true
sqlx.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
async-trait.workspace = true
anyhow.workspace = true
chrono.workspace = true
http.workspace = true
EOF

cat > "$dir/src/lib.rs" <<EOF
//! ${name} domain.
pub mod domain;
pub mod models;
pub mod ports;
EOF

cat > "$dir/src/domain.rs" <<EOF
//! Pure business rules for the ${name} domain.
EOF

cat > "$dir/src/models.rs" <<EOF
//! ${name} data models.
EOF

cat > "$dir/src/ports/mod.rs" <<EOF
pub mod repository;
EOF

cat > "$dir/src/ports/repository.rs" <<EOF
//! ${name} repository port + Postgres adapter.
EOF

echo "Scaffolded ${dir}. Add it to the workspace members in Cargo.toml."
```

Then: `chmod +x scripts/new-domain.sh`

- [ ] **Step 4: README**

`README.md`:
```markdown
# rust-service-template

Idiomatic-Rust service template: a monolith of internal domains with a
transactional outbox, correlation-id tracing, JWT auth, and Prometheus metrics.

## Quick start

    cp .env.example .env        # edit JWT key etc.
    make up                     # start Postgres + Prometheus + Grafana
    make migrate                # apply migrations
    make run                    # start the app on :8080

## Test

    make up
    make test                   # needs DATABASE_URL pointing at Postgres

## Architecture

See `docs/superpowers/specs/2026-06-24-rust-service-template-design.md`.

- `crates/platform` — cross-cutting: config, db, events (outbox), auth, metrics, http client, observability
- `crates/domain-*` — one crate per domain (pure rules + ports)
- `crates/app` — composition root: wires domains, runs server + outbox dispatcher

## Add a domain

    make new-domain name=billing
```

- [ ] **Step 5: Verify the workspace still builds and tests pass**

Run: `cargo build` then `DATABASE_URL=postgres://localhost/postgres cargo test`
Expected: PASS across all crates.

- [ ] **Step 6: Commit**

```bash
git add docker-compose.yml infra Makefile scripts README.md
git commit -m "chore: docker-compose, Makefile, domain scaffolder, README"
```

---

## Self-Review

**Spec coverage (against design §6 + §7 + §8 + §9):** account models ✓ (Task 2), repository port ✓ (Task 2), pure authorization mirroring `AccessPolicy` ✓ (Task 2), Postgres adapter ✓ (Task 3), atomic create + `account.created` ✓ (Task 3), `UserRegistered` subscriber idempotent ✓ (Task 4), JWT extractor + scope-protected `/accounts/:id` ✓ (Task 5), `/status` + `/metrics` ✓ (Task 5), dev/register stand-in ✓ (Task 5), app wiring + dispatcher concurrency ✓ (Task 6), unit tests with pure rules ✓ (Task 2), integration tests incl. full outbox loop ✓ (Task 7), docker-compose + Makefile + scaffolder ✓ (Task 8). `created_at`/`created_by_cid` metadata injection (design `metadata` module) ✓ folded into Task 3's insert (`created_by_cid` set from cid; `created_at` defaulted in SQL).

**Placeholder scan:** No TBD/TODO; all code shown. The publisher/subscriber/registry construction (Task 6 Step 2) is fully resolved via the `Routes` data table introduced in Plan 1b — the dependency chain `Routes → publisher → subscribers → registry → dispatcher` is linear and cycle-free, so no `Arc::new_cyclic`/interior-mutability hand-waving remains. The only contract to keep in sync (every registered subscriber has a matching `routes()` entry) is documented inline next to both.

**Type consistency:** `AccountState` fields (`pool`, `repo`, `publisher`, `jwt`, `metrics`) are identical across Tasks 5, 6, 7. `create_account_with_event(pool, &dyn EventPublisher, NewAccount, cid)` signature consistent across Tasks 3, 4, 5. `AccountSubscriber::new(Db, Arc<dyn AccountRepository>, Arc<dyn EventPublisher>)` consistent across Tasks 4, 6, 7. `router(AccountState) -> Router` consistent across Tasks 5, 6, 7. Event type strings `"user.registered"` / `"account.created"` consistent with Plan 1b.

**Known follow-up (not Spec 1):** full JWT happy-path HTTP test (minting a real RS256 token) is deferred to Spec 2, where the auth domain can issue tokens; Spec 1 covers the 401 path + the authorization rules via unit tests.
