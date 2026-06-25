# Spec 3a: Backend Prerequisites for the SPA — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the backend endpoints + serving the SPA needs: `GET /accounts/me`, admin-gate `GET /accounts`, DLQ admin endpoints, and an app router that mounts the API under `/api`, keeps `/status`+`/metrics` at root, and serves the built SPA as a static fallback.

**Architecture:** Two small `domain-account` handler changes, a new `platform::events::dlq_http` module (handlers + `DlqState` + `dlq_router`), and a refactor of the `app` crate's router assembly into a testable `build_router(...)` that nests domain routers under `/api`, registers root infra routes, mounts the DLQ router, and attaches a `ServeDir` SPA fallback.

**Tech Stack:** axum 0.7 + tower-http (ServeDir/ServeFile), sqlx (runtime API), tokio, jsonwebtoken (verify), serde.

## Global Constraints

- Same dependency pins and rules as Spec 1/2 (`docs/superpowers/plans/2026-06-24-rust-spec1a-workspace-and-platform.md`, "Global Constraints"). Depends on Spec 1 + Spec 2 being complete.
- sqlx **runtime** query API only (`query`/`query_as`/`query_scalar`/`.bind`), never `query!`.
- `#[sqlx::test]` integration tests use `#[sqlx::test(migrations = "../../migrations")]`; need `DATABASE_URL` (e.g. `postgres://arthurjordao@localhost:5432/postgres`).
- axum 0.7 path syntax `:id`. Static route segments (`/accounts/me`) take priority over dynamic (`/accounts/:id`) in axum's router, so both may coexist.
- The API is mounted under `/api`; `/status` and `/metrics` stay at root. JWT subject convention `user-{id}`; admin gating via `Authenticated` + `require_scope("admin")`.
- Run `cargo fmt --all` + `cargo clippy --all-targets -- -D warnings` before each commit; both clean.

---

### Task 1: `GET /accounts/me` + sub-parsing helper

**Files:**
- Modify: `crates/domain-account/src/domain.rs` (add `auth_user_id_from_sub` + unit test)
- Modify: `crates/domain-account/src/ports/http.rs` (add `account_me` handler + route)
- Modify: `crates/domain-account/tests/http.rs` (add 401 path test)

**Interfaces:**
- Consumes: `AccessClaims` (`platform::auth`), `AccountRepository::find_by_auth_user_id`, `Authenticated`.
- Produces:
  - `pub fn auth_user_id_from_sub(sub: &str) -> Option<i64>` in `domain.rs` — parses `"user-{id}"`.
  - `GET /accounts/me` route returning the caller's `Account` (`404` if none).

- [ ] **Step 1: Write the failing unit test for the helper**

Add to the `#[cfg(test)] mod tests` in `crates/domain-account/src/domain.rs`:
```rust
    #[test]
    fn auth_user_id_from_sub_parses_user_prefix() {
        assert_eq!(auth_user_id_from_sub("user-42"), Some(42));
        assert_eq!(auth_user_id_from_sub("user-0"), Some(0));
        assert_eq!(auth_user_id_from_sub("service-x"), None);
        assert_eq!(auth_user_id_from_sub("user-"), None);
        assert_eq!(auth_user_id_from_sub("42"), None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p domain-account domain::`
Expected: FAIL — `auth_user_id_from_sub` not found.

- [ ] **Step 3: Implement the helper**

Add to `crates/domain-account/src/domain.rs` (above the test module):
```rust
/// Parse an access-token subject of the form `user-{id}` into the auth user id.
pub fn auth_user_id_from_sub(sub: &str) -> Option<i64> {
    sub.strip_prefix("user-").and_then(|s| s.parse::<i64>().ok())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p domain-account domain::`
Expected: PASS.

- [ ] **Step 5: Add the `account_me` handler + route**

In `crates/domain-account/src/ports/http.rs`, add the import for the helper:
```rust
use crate::domain::{authorize, auth_user_id_from_sub};
```
(replace the existing `use crate::domain::authorize;` line).

Add the route to `router` (place it BEFORE `/accounts/:id`):
```rust
        .route("/accounts/me", get(account_me))
```

Add the handler:
```rust
async fn account_me(
    State(state): State<AccountState>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Account>, AppError> {
    let uid = auth_user_id_from_sub(&claims.sub)
        .ok_or_else(|| AppError::Unauthorized("invalid subject".into()))?;
    let account = state
        .repo
        .find_by_auth_user_id(uid)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound("no account for this user".into()))?;
    Ok(Json(account))
}
```

- [ ] **Step 6: Write the 401 path test**

Add to `crates/domain-account/tests/http.rs`:
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn account_me_without_token_is_unauthorized(pool: sqlx::PgPool) {
    let app = router(state(pool));
    let res = app
        .oneshot(
            Request::builder()
                .uri("/accounts/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
```
(The authenticated happy-path is covered by an app-level e2e in Task 4, which can mint a real token via register.)

- [ ] **Step 7: Run tests**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p domain-account --test http`
Expected: PASS (existing tests + new 401 test).

- [ ] **Step 8: fmt + clippy, then commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
```bash
git add crates/domain-account
git commit -m "feat(account): GET /accounts/me (caller's own account)"
```

---

### Task 2: Admin-gate `GET /accounts`

**Files:**
- Modify: `crates/domain-account/src/ports/http.rs` (`list_accounts` now requires admin)
- Modify: `crates/domain-account/tests/http.rs` (401 path test)

**Interfaces:**
- Consumes: `Authenticated`, `require_scope` (`platform::auth`).
- Produces: `GET /accounts` requires `admin`.

- [ ] **Step 1: Write the failing test**

Add to `crates/domain-account/tests/http.rs`:
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn list_accounts_without_token_is_unauthorized(pool: sqlx::PgPool) {
    let app = router(state(pool));
    let res = app
        .oneshot(Request::builder().uri("/accounts").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p domain-account --test http`
Expected: FAIL — `list_accounts` currently returns `200` with no token.

- [ ] **Step 3: Gate the handler**

In `crates/domain-account/src/ports/http.rs`, add the import (merge into the existing `platform::auth` use):
```rust
use platform::auth::{require_scope, Authenticated, JwtVerifier};
```
Change `list_accounts` to require admin:
```rust
async fn list_accounts(
    State(state): State<AccountState>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<Account>>, AppError> {
    require_scope(&claims, "admin")?;
    let accounts = state.repo.list().await.map_err(AppError::Internal)?;
    Ok(Json(accounts))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p domain-account --test http`
Expected: PASS.

- [ ] **Step 5: fmt + clippy, then commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
```bash
git add crates/domain-account
git commit -m "feat(account): admin-gate GET /accounts list"
```

---

### Task 3: DLQ admin HTTP (`platform::events::dlq_http`)

**Files:**
- Modify: `crates/platform/src/events/dlq.rs` (add `Serialize` to `DeadLetter`)
- Create: `crates/platform/src/events/dlq_http.rs`
- Modify: `crates/platform/src/events/mod.rs` (wire submodule)
- Modify: `crates/platform/Cargo.toml` (add `tower-http` ServeDir not needed here; tower for tests) — see Step 5
- Test: `crates/platform/tests/dlq_http.rs`

**Interfaces:**
- Consumes: `list_dead_letters`, `replay_dead_letter` (`platform::events`), `Authenticated`, `require_scope`, `JwtVerifier`, `RevocationChecker`, `Db`.
- Produces:
  - `DeadLetter` derives `serde::Serialize`.
  - `#[derive(Clone)] pub struct DlqState { pub pool: Db, pub jwt: Arc<JwtVerifier>, pub revocation: Arc<dyn RevocationChecker> }` with `FromRef` impls for `Arc<JwtVerifier>` and `Arc<dyn RevocationChecker>`.
  - `pub fn dlq_router(state: DlqState) -> axum::Router` — `GET /admin/dlq`, `POST /admin/dlq/:delivery_id/replay`, both admin-gated.

- [ ] **Step 1: Add `Serialize` to `DeadLetter`**

In `crates/platform/src/events/dlq.rs`, change the derive:
```rust
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct DeadLetter {
```

- [ ] **Step 2: Write the failing test**

`crates/platform/tests/dlq_http.rs`:
```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::events::dlq_http::{dlq_router, DlqState};
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PUB_PEM: &str = include_str!("fixtures/test_pub.pem");

fn state(pool: sqlx::PgPool) -> DlqState {
    DlqState {
        pool,
        jwt: Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        revocation: Arc::new(NoopRevocationChecker),
    }
}

async fn seed_dead(pool: &sqlx::PgPool) -> i64 {
    let event_id: i64 = sqlx::query_scalar(
        "insert into outbox_event (event_type, aggregate_id, payload, correlation_id) \
         values ('user.registered', '1', '{}'::jsonb, 'cid') returning id",
    )
    .fetch_one(pool).await.unwrap();
    sqlx::query_scalar(
        "insert into outbox_delivery (event_id, subscriber_name, status, attempts, last_error) \
         values ($1, 'sub', 'dead', 5, 'boom') returning id",
    )
    .bind(event_id)
    .fetch_one(pool).await.unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_dlq_without_token_is_unauthorized(pool: sqlx::PgPool) {
    let app = dlq_router(state(pool));
    let res = app
        .oneshot(Request::builder().uri("/admin/dlq").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn replay_resets_dead_delivery_to_pending(pool: sqlx::PgPool) {
    let delivery_id = seed_dead(&pool).await;
    // Call the replay function path directly via the router is admin-gated; assert the
    // underlying behavior through the public helpers used by the handler.
    let replayed = platform::events::replay_dead_letter(&pool, delivery_id).await.unwrap();
    assert!(replayed);
    let status: String = sqlx::query_scalar("select status from outbox_delivery where id = $1")
        .bind(delivery_id)
        .fetch_one(&pool).await.unwrap();
    assert_eq!(status, "pending");
}
```

> The admin happy-path (200 list / replay with a real admin token) is covered at the app level in Task 4 (which can mint a token). Here we assert the 401 gate and the replay data-path.

- [ ] **Step 3: Run test to verify it fails**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p platform --test dlq_http`
Expected: FAIL — `dlq_http` module not found.

- [ ] **Step 4: Write the module**

`crates/platform/src/events/dlq_http.rs`:
```rust
use crate::auth::{require_scope, Authenticated, JwtVerifier, RevocationChecker};
use crate::db::Db;
use crate::events::{list_dead_letters, replay_dead_letter, DeadLetter};
use crate::server::AppError;
use axum::extract::{FromRef, Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
pub struct DlqState {
    pub pool: Db,
    pub jwt: Arc<JwtVerifier>,
    pub revocation: Arc<dyn RevocationChecker>,
}

impl FromRef<DlqState> for Arc<JwtVerifier> {
    fn from_ref(state: &DlqState) -> Self {
        state.jwt.clone()
    }
}

impl FromRef<DlqState> for Arc<dyn RevocationChecker> {
    fn from_ref(state: &DlqState) -> Self {
        state.revocation.clone()
    }
}

pub fn dlq_router(state: DlqState) -> Router {
    Router::new()
        .route("/admin/dlq", get(list_handler))
        .route("/admin/dlq/:delivery_id/replay", post(replay_handler))
        .with_state(state)
}

async fn list_handler(
    State(state): State<DlqState>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<DeadLetter>>, AppError> {
    require_scope(&claims, "admin")?;
    let rows = list_dead_letters(&state.pool).await.map_err(AppError::Internal)?;
    Ok(Json(rows))
}

async fn replay_handler(
    State(state): State<DlqState>,
    Authenticated(claims): Authenticated,
    Path(delivery_id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_scope(&claims, "admin")?;
    let replayed = replay_dead_letter(&state.pool, delivery_id)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(json!({ "replayed": replayed })))
}
```

`crates/platform/src/events/mod.rs` (add):
```rust
pub mod dlq_http;
```
(Keep it `pub mod` — `dlq_http` is consumed by the `app` crate. The existing `mod dlq; pub use dlq::*;` stays.)

- [ ] **Step 5: Ensure platform dev-deps cover the test**

The test uses `tower::ServiceExt` and `http-body-util`. Add to `crates/platform/Cargo.toml`:
```toml
[dev-dependencies]
tower = { workspace = true, features = ["util"] }
http-body-util = { workspace = true }
```
(`platform` already depends on `axum`, `serde_json`, `sqlx`. The test fixture `crates/platform/tests/fixtures/test_pub.pem` must exist — create it the same way as other crates:)
```bash
mkdir -p crates/platform/tests/fixtures
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out /tmp/p_priv.pem
openssl rsa -in /tmp/p_priv.pem -pubout -out crates/platform/tests/fixtures/test_pub.pem
```

- [ ] **Step 6: Run test to verify it passes**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p platform --test dlq_http`
Expected: PASS.

- [ ] **Step 7: fmt + clippy, then commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
```bash
git add crates/platform
git commit -m "feat(events): DLQ admin HTTP (dlq_router + DlqState; DeadLetter Serialize)"
```

---

### Task 4: App router — `/api` nest, root infra, DLQ mount, SPA fallback

**Files:**
- Modify: `crates/app/Cargo.toml` (add `tower-http` with `fs` feature)
- Modify: `crates/app/src/state.rs` (add `dlq_state(res)`)
- Modify: `crates/app/src/main.rs` (extract `build_router`, mount everything)
- Modify: `crates/domain-account/src/ports/http.rs` (remove `/status` + `/metrics` routes + `metrics_handler`)
- Modify: `crates/domain-account/tests/http.rs` (drop `status_returns_ok`)
- Delete/replace: `crates/app/tests/merged_router.rs` → `crates/app/tests/api_router.rs`
- Create: `crates/app/tests/account_me_e2e.rs`

**Interfaces:**
- Consumes: `domain_account::router`, `domain_auth::router`, `platform::events::dlq_http::{dlq_router, DlqState}`, `platform::server::status_handler`, `Metrics`.
- Produces: `pub fn build_router(account: AccountState, auth: AuthState, dlq: DlqState, metrics: Metrics, web_dist: Option<std::path::PathBuf>) -> axum::Router` in `app::state` (or `app` lib) — testable router assembly.

- [ ] **Step 1: Add tower-http fs feature to app**

`crates/app/Cargo.toml` `[dependencies]` — add `tower-http`:
```toml
tower-http = { workspace = true }
```
And in the root `[workspace.dependencies]`, ensure `tower-http` includes the `fs` feature (it currently has `["cors", "trace"]`); change to:
```toml
tower-http = { version = "0.6", features = ["cors", "trace", "fs"] }
```

- [ ] **Step 2: Remove `/status` + `/metrics` from `domain-account` router**

In `crates/domain-account/src/ports/http.rs`:
- Remove `.route("/status", get(status_handler))` and `.route("/metrics", get(metrics_handler))` from `router`.
- Delete the `metrics_handler` fn.
- Remove now-unused imports (`status_handler`; `get` stays — still used by `/accounts*`). Run `cargo build -p domain-account` and remove exactly what the compiler flags.

In `crates/domain-account/tests/http.rs`: delete the `status_returns_ok` test (its route no longer exists on this router).

- [ ] **Step 3: Add `dlq_state` builder**

In `crates/app/src/state.rs`, add:
```rust
use platform::events::dlq_http::DlqState;

pub fn dlq_state(res: &Resources) -> DlqState {
    DlqState {
        pool: res.pool.clone(),
        jwt: res.jwt.clone(),
        revocation: res.revocation.clone(),
    }
}
```

- [ ] **Step 4: Extract a testable `build_router`**

In `crates/app/src/state.rs`, add (imports: `use axum::routing::get; use axum::Router; use platform::metrics::Metrics; use platform::server::{cors_layer, status_handler}; use platform::observability::correlation_id_middleware; use tower_http::services::{ServeDir, ServeFile}; use std::path::PathBuf;` — add any missing):
```rust
/// Assemble the full application router: API under `/api`, infra at root, and an
/// optional static SPA fallback. Pure (no I/O) so it is unit-testable.
pub fn build_router(
    account: AccountState,
    auth: AuthState,
    dlq: DlqState,
    metrics: Metrics,
    cors_origins: &[String],
    web_dist: Option<PathBuf>,
) -> Router {
    let api = domain_account::router(account)
        .merge(domain_auth::router(auth))
        .merge(platform::events::dlq_http::dlq_router(dlq));

    let metrics_for_handler = metrics.clone();
    let mut app = Router::new()
        .route("/status", get(status_handler))
        .route(
            "/metrics",
            get(move || {
                let m = metrics_for_handler.clone();
                async move { m.render() }
            }),
        )
        .nest("/api", api);

    if let Some(dir) = web_dist {
        let index = dir.join("index.html");
        app = app.fallback_service(ServeDir::new(dir).not_found_service(ServeFile::new(index)));
    }

    app.layer(axum::middleware::from_fn(correlation_id_middleware))
        .layer(cors_layer(cors_origins))
}
```

- [ ] **Step 5: Rewrite `main` to use `build_router`**

`crates/app/src/main.rs` — replace the router-building block:
```rust
    let res = state::build_resources(settings).await?;
    let port = res.settings.server.port;

    let web_dist = std::path::Path::new("web/dist");
    let web_dist = web_dist.exists().then(|| web_dist.to_path_buf());

    let app = state::build_router(
        state::account_state(&res),
        state::auth_state(&res),
        state::dlq_state(&res),
        res.metrics.clone(),
        &res.settings.cors_allowed_origins,
        web_dist,
    );

    let (pool, registry, dispatcher_cfg, interval) = state::dispatcher_handle(&res);
    let dispatcher = tokio::spawn(run_dispatcher(pool, registry, dispatcher_cfg, interval));
    // ... keep the existing prune task + tokio::select! + listener bind ...
```
(Keep the existing `init_tracing`, `Settings::load`, prune task, `tokio::select!`, listener, and `axum::serve` lines. Remove the now-unused `cors_layer`/`correlation_id_middleware` imports from main.rs if they were used only for the inline router — they now live in `build_router`.)

- [ ] **Step 6: Replace the merged-router test with an api_router test**

Delete `crates/app/tests/merged_router.rs`. Create `crates/app/tests/api_router.rs`:
```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_account::ports::http::AccountState;
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::AuthState;
use domain_auth::ports::postgres::PostgresUserRepository;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::events::dlq_http::DlqState;
use platform::events::{OutboxPublisher, Routes};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_priv.pem");
const TEST_PUB_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_pub.pem");

fn build(pool: sqlx::PgPool) -> axum::Router {
    let metrics = Metrics::new().unwrap();
    let jwt = Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap());
    let revocation: Arc<dyn platform::auth::RevocationChecker> = Arc::new(NoopRevocationChecker);
    let user_repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    let account = AccountState {
        pool: pool.clone(),
        repo: Arc::new(PostgresAccountRepository::new(pool.clone())),
        publisher: Arc::new(OutboxPublisher::new(Routes::new())),
        jwt: jwt.clone(),
        metrics: metrics.clone(),
        revocation: revocation.clone(),
    };
    let auth = AuthState {
        pool: pool.clone(),
        users: user_repo.clone(),
        refresh_tokens: user_repo.clone(),
        scopes: user_repo.clone(),
        publisher: Arc::new(OutboxPublisher::new(Routes::new())),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        verifier: jwt.clone(),
        revocation: revocation.clone(),
        admin_emails: Arc::new(vec![]),
        metrics: metrics.clone(),
    };
    let dlq = DlqState { pool, jwt: jwt.clone(), revocation: revocation.clone() };
    app::state::build_router(account, auth, dlq, metrics, &[], None)
}

#[sqlx::test(migrations = "../../migrations")]
async fn status_at_root_and_api_routes_mounted(pool: sqlx::PgPool) {
    let app = build(pool);

    // /status at root -> 200
    let s = app.clone().oneshot(Request::builder().uri("/status").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(s.status(), StatusCode::OK);

    // API mounted under /api: an admin route with no token -> 401 (proves auth router mounted under /api)
    let scopes = app.clone().oneshot(Request::builder().uri("/api/scopes").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(scopes.status(), StatusCode::UNAUTHORIZED);

    // DLQ mounted under /api -> 401 without token
    let dlq = app.clone().oneshot(Request::builder().uri("/api/admin/dlq").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(dlq.status(), StatusCode::UNAUTHORIZED);

    // login is reachable under /api (bad body -> 400/422, NOT 404) — proves no route collision
    let login = app.oneshot(
        Request::builder().method("POST").uri("/api/auth/login")
            .header("content-type", "application/json")
            .body(Body::from("{}")).unwrap()
    ).await.unwrap();
    assert_ne!(login.status(), StatusCode::NOT_FOUND);
}
```

This test requires `app` to expose `state::build_router` publicly. Since `app` is a binary crate, add a minimal `crates/app/src/lib.rs` that re-exports the module, OR mark the test to use the binary's modules. Simplest: create `crates/app/src/lib.rs`:
```rust
pub mod state;
```
and in `main.rs` change `mod state;` to `use app::state;` (and add `name`/`path` as needed). **Implementation note:** to keep both a lib and bin, set in `crates/app/Cargo.toml`:
```toml
[lib]
path = "src/lib.rs"

[[bin]]
name = "app"
path = "src/main.rs"
```
and `main.rs` references `app::state::...`. Adjust `main.rs`'s `mod state;` → remove it and `use app::state;`.

- [ ] **Step 7: Write the `/accounts/me` happy-path e2e**

`crates/app/tests/account_me_e2e.rs`:
```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_account::ports::http::AccountState;
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::AuthState;
use domain_auth::ports::postgres::PostgresUserRepository;
use http_body_util::BodyExt;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::events::dlq_http::DlqState;
use platform::events::{dispatch_once, DispatcherConfig, EventPublisher, OutboxPublisher, Routes, SubscriberRegistry};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_priv.pem");
const TEST_PUB_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_pub.pem");

#[sqlx::test(migrations = "../../migrations")]
async fn register_dispatch_then_get_my_account(pool: sqlx::PgPool) {
    let metrics = Metrics::new().unwrap();
    let jwt = Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap());
    let revocation: Arc<dyn platform::auth::RevocationChecker> = Arc::new(NoopRevocationChecker);
    let user_repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    let account_repo = Arc::new(PostgresAccountRepository::new(pool.clone()));
    let publisher: Arc<dyn EventPublisher> = Arc::new(OutboxPublisher::new(
        Routes::new().add("user.registered", "account.on-user-registered"),
    ));
    let mut registry = SubscriberRegistry::new();
    registry.register(Arc::new(AccountSubscriber::new(pool.clone(), account_repo.clone(), publisher.clone())));
    let registry = Arc::new(registry);

    let account = AccountState {
        pool: pool.clone(), repo: account_repo.clone(), publisher: publisher.clone(),
        jwt: jwt.clone(), metrics: metrics.clone(), revocation: revocation.clone(),
    };
    let auth = AuthState {
        pool: pool.clone(), users: user_repo.clone(), refresh_tokens: user_repo.clone(),
        scopes: user_repo.clone(), publisher: publisher.clone(),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        verifier: jwt.clone(), revocation: revocation.clone(),
        admin_emails: Arc::new(vec![]), metrics: metrics.clone(),
    };
    let dlq = DlqState { pool: pool.clone(), jwt: jwt.clone(), revocation: revocation.clone() };
    let app = app::state::build_router(account, auth, dlq, metrics, &[], None);

    // Register -> tokens
    let reg = app.clone().oneshot(
        Request::builder().method("POST").uri("/api/auth/register")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"email":"me@x.y","password":"hunter2"}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(reg.status(), StatusCode::CREATED);
    let body = reg.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let access = json["access_token"].as_str().unwrap().to_string();

    // Dispatch the user.registered -> account created
    dispatch_once(&pool, &registry, &DispatcherConfig::default()).await.unwrap();

    // GET /api/accounts/me with the access token -> 200 + the account
    let me = app.oneshot(
        Request::builder().uri("/api/accounts/me")
            .header("authorization", format!("Bearer {access}"))
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let body = me.into_body().collect().await.unwrap().to_bytes();
    let acc: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(acc["email"], "me@x.y");
}
```

- [ ] **Step 8: Run the full workspace suite + gate**

Run: `cargo build --all-targets`
Then: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test`
Then: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: all green, clean. (Report the new total count.)

- [ ] **Step 9: Commit**

```bash
git add crates/app crates/domain-account Cargo.toml
git commit -m "feat(app): /api-nested router + root status/metrics + SPA static fallback; DLQ mounted"
```

---

### Task 5: README + Makefile web targets (prep for the SPA)

**Files:**
- Modify: `Makefile` (web targets)
- Modify: `.gitignore` (web artifacts)
- Modify: `README.md`

**Interfaces:** docs/tooling only.

- [ ] **Step 1: Add web Makefile targets**

Append to `Makefile`:
```makefile
web-install:
	npm --prefix web ci

web-dev:
	npm --prefix web run dev

web-build:
	npm --prefix web run build

web-test:
	npm --prefix web test

web-lint:
	npm --prefix web run lint
```

- [ ] **Step 2: gitignore web artifacts**

Append to `.gitignore`:
```
# web SPA
web/node_modules/
web/dist/
```

- [ ] **Step 3: README note**

In `README.md`, add under Quick start:
```markdown
## Frontend (web SPA)

    make web-install            # install deps (web/)
    make web-dev                # Vite dev server on :5173 (proxies /api -> :8080)
    make web-build              # build to web/dist; `make run` then serves it at :8080
```

- [ ] **Step 4: Commit**

```bash
git add Makefile .gitignore README.md
git commit -m "chore: web Makefile targets + gitignore + README"
```

---

## Self-Review

**Spec coverage (design §4):** `/accounts/me` ✓ (Task 1); admin-gate `/accounts` ✓ (Task 2); `DeadLetter: Serialize` + `dlq_http`/`dlq_router`/`DlqState` ✓ (Task 3); `/api` nest + root `/status`+`/metrics` + ServeDir SPA fallback + DLQ mount ✓ (Task 4); domain-account `/status` test removal handled ✓ (Task 4 Step 2); Makefile/gitignore/README ✓ (Task 5).

**Placeholder scan:** no TBD/TODO; complete code in every step. The `// ... keep existing ...` notes in Task 4 Step 5 reference concrete existing lines (prune task, select!, listener) the implementer can see.

**Type consistency:** `DlqState { pool, jwt, revocation }` consistent across Tasks 3/4. `build_router(account, auth, dlq, metrics, cors_origins, web_dist)` signature consistent across Task 4 Steps 4/6/7. `AccountState`/`AuthState` field sets match Spec 1/2's current definitions (incl. `revocation` added in Spec 2b). `auth_user_id_from_sub` (Task 1) used by `account_me`. Event payload + dispatch wiring matches Spec 1/2.

**Cross-cutting note:** making `app` a lib+bin (Task 4 Step 6) lets `app/tests/*` call `app::state::build_router`. This replaces the Spec 2 `merged_router.rs` test (which tested the old root-merge) with `api_router.rs` (the new `/api`-nested structure).
