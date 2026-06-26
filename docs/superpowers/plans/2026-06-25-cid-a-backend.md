# Correlation-ID + Logging — Plan A (backend) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the flat correlation id into a hierarchical dotted path that appends a segment at each communication hop (HTTP request, event publish), add a structured request access log, configurable log level, and cid-tagged domain/event logs.

**Architecture:** `platform::observability` replaces the uuid cid generator with a short `new_segment()` + `append()`; the request middleware appends a segment to the incoming (or fresh) cid, opens the request span, and logs a "request completed" line; `OutboxPublisher::publish` stamps each event row with `append(current_cid)`; domains/events gain cid-tagged structured logs. Logs are already JSON via `init_tracing`, now `RUST_LOG`-configurable.

**Tech Stack:** tracing + tracing-subscriber (JSON), axum 0.7 middleware, uuid (existing), sqlx runtime API.

## Global Constraints

- Depends on Specs 1–3 (merged). Spec doc: `docs/superpowers/specs/2026-06-25-correlation-id-logging-design.md`.
- A `CorrelationId` is a dotted path of short segments. `new_segment()` = 6 chars; `append(cid)` = `format!("{cid}.{seg}")`. Segment derived from uuid (no new dep).
- **A child is minted for each new work unit:** the HTTP middleware appends to the incoming/root cid; `publish()` appends per event row. The dispatcher runs handlers under the event row's cid (no further append). `http_client` forwards the current cid (unchanged).
- Correlation-id header: `X-Correlation-Id`. Echoed on every response (already).
- Access log excludes `/status` and `/metrics` (Prometheus noise).
- **Secrets rule:** never log passwords, tokens, `Authorization` headers, or full auth request bodies. Log emails/ids/scopes/event-types only.
- sqlx runtime query API only. Run `cargo fmt --all` + `cargo clippy --all-targets -- -D warnings` clean before each commit. Postgres for integration tests (`DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres`).

---

### Task 1: `new_segment` + `append` (replace the uuid cid generator)

**Files:** Modify `crates/platform/src/observability.rs`.

**Interfaces:**
- Produces: `pub fn new_segment() -> String` (6 chars), `pub fn append(cid: &str) -> String`. Removes `new_correlation_id`.

- [ ] **Step 1: Replace the test for the generator**

In `crates/platform/src/observability.rs`, replace the existing `generates_non_empty_cid` test with:
```rust
    #[test]
    fn segment_is_short_and_append_grows_path() {
        let seg = new_segment();
        assert_eq!(seg.len(), 6);
        assert!(seg.chars().all(|c| c.is_ascii_alphanumeric()));

        let child = append("abc123");
        assert!(child.starts_with("abc123."), "child must extend the parent: {child}");
        assert_eq!(child.matches('.').count(), 1);
        assert_eq!(child.len(), "abc123.".len() + 6);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p platform observability::`
Expected: FAIL — `new_segment`/`append` not found (and `generates_non_empty_cid` gone).

- [ ] **Step 3: Implement, replacing `new_correlation_id`**

In `observability.rs`, replace
```rust
pub fn new_correlation_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
```
with
```rust
/// A short correlation-id segment (6 hex chars derived from a uuid v4).
pub fn new_segment() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

/// Extend a correlation id with a fresh child segment: `parent` -> `parent.<seg>`.
pub fn append(cid: &str) -> String {
    format!("{cid}.{}", new_segment())
}
```
Then update the two existing internal callers of `new_correlation_id` to `new_segment`:
- in `correlation_id_middleware` (the `.unwrap_or_else(new_correlation_id)` — Task 2 rewrites this method anyway, but make it compile now by using `new_segment`),
- in the `CorrelationId` `FromRequestParts` impl (`.unwrap_or_else(|| CorrelationId(new_correlation_id()))` → `new_segment`).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p platform observability::`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
cargo fmt --all && cargo clippy -p platform --all-targets -- -D warnings
git add crates/platform/src/observability.rs
git commit -m "feat(observability): hierarchical cid (new_segment + append) replacing flat uuid"
```

---

### Task 2: Middleware appends + access log; `RUST_LOG`-configurable `init_tracing`

**Files:** Modify `crates/platform/src/observability.rs`; modify `crates/app/src/main.rs` (no change needed if it already passes `"info"` — confirm).

**Interfaces:**
- Consumes: `new_segment`, `append`.
- Produces: `correlation_id_middleware` appends a child to the incoming/root cid, echoes it, and logs `request completed {method, path, status, latency_ms}` (excluding `/status`,`/metrics`); `init_tracing(default_level)` honors `RUST_LOG`.

- [ ] **Step 1: Rewrite the middleware**

Replace `correlation_id_middleware` in `observability.rs` with:
```rust
/// axum middleware: derive this request's cid by appending a fresh segment to the
/// incoming `X-Correlation-Id` (or a new root), run the stack inside a cid span,
/// emit one access log on completion, and echo the cid on the response.
pub async fn correlation_id_middleware(mut req: Request, next: Next) -> Response {
    let incoming = req
        .headers()
        .get(CORRELATION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let cid = append(&incoming.unwrap_or_else(new_segment));

    req.extensions_mut().insert(CorrelationId(cid.clone()));

    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let infra = matches!(path.as_str(), "/status" | "/metrics");

    let span = tracing::info_span!("request", %cid);
    let mut res = async move {
        let start = std::time::Instant::now();
        let res = next.run(req).await;
        if !infra {
            tracing::info!(
                method = %method,
                path = %path,
                status = res.status().as_u16(),
                latency_ms = start.elapsed().as_millis() as u64,
                "request completed"
            );
        }
        res
    }
    .instrument(span)
    .await;

    if let Ok(val) = HeaderValue::from_str(&cid) {
        res.headers_mut()
            .insert(HeaderName::from_static(CORRELATION_ID_HEADER), val);
    }
    res
}
```
(The access log is emitted *inside* the instrumented block, so it carries the `cid` span field. Infra paths still get a span but no access-log line — that removes the Prometheus/health noise, which is the goal.)

- [ ] **Step 2: Make `init_tracing` honor `RUST_LOG`**

Replace `init_tracing` with:
```rust
/// Install a JSON tracing subscriber. Level comes from `RUST_LOG` if set, else
/// `default_level`. Idempotent: a second call is a no-op.
pub fn init_tracing(default_level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(default_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .json()
                .with_current_span(true)
                .with_span_list(false),
        )
        .try_init();
}
```
(`main.rs` already calls `init_tracing("info")` — no change needed; confirm it still compiles.)

- [ ] **Step 3: Write the integration test (append-on-response)**

Add to `crates/platform` a test that drives the middleware. Create `crates/platform/tests/cid_middleware.rs`:
```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use platform::observability::{correlation_id_middleware, CORRELATION_ID_HEADER};
use tower::ServiceExt;

fn app() -> Router {
    Router::new()
        .route("/x", get(|| async { "ok" }))
        .layer(axum::middleware::from_fn(correlation_id_middleware))
}

#[tokio::test]
async fn appends_a_segment_to_the_incoming_cid() {
    let res = app()
        .oneshot(
            Request::builder()
                .uri("/x")
                .header(CORRELATION_ID_HEADER, "root")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let echoed = res.headers().get(CORRELATION_ID_HEADER).unwrap().to_str().unwrap();
    assert!(echoed.starts_with("root."), "expected child of root, got {echoed}");
    assert_eq!(echoed.matches('.').count(), 1);
}

#[tokio::test]
async fn mints_a_root_when_no_header() {
    let res = app()
        .oneshot(Request::builder().uri("/x").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let echoed = res.headers().get(CORRELATION_ID_HEADER).unwrap().to_str().unwrap();
    // new root segment + appended child = two dotted segments
    assert_eq!(echoed.matches('.').count(), 1, "got {echoed}");
}
```
`platform` already has `tower` (util) + `axum` dev-deps from earlier specs; add them to `[dev-dependencies]` if missing (`tower = { workspace = true, features = ["util"] }`).

- [ ] **Step 4: Run tests**

Run: `cargo test -p platform observability:: && cargo test -p platform --test cid_middleware`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/platform crates/app
git commit -m "feat(observability): middleware appends cid + request access log; RUST_LOG level"
```

---

### Task 3: `publish` appends a segment per event row

**Files:** Modify `crates/platform/src/events/publisher.rs`; extend `crates/platform/tests/outbox_publish.rs`.

**Interfaces:**
- Consumes: `append` (`platform::observability`).
- Produces: each `outbox_event` row's `correlation_id` is `append(NewEvent.correlation_id)`.

- [ ] **Step 1: Add the failing assertion**

Append to `crates/platform/tests/outbox_publish.rs` a test:
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn publish_appends_a_child_segment_to_the_event_cid(pool: sqlx::PgPool) {
    let publisher = OutboxPublisher::new(Routes::new());
    let mut tx = pool.begin().await.unwrap();
    let event_id = publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "user.registered".into(),
                aggregate_id: "1".into(),
                payload: serde_json::json!({}),
                correlation_id: "root.ab12cd".into(),
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let cid: String =
        sqlx::query_scalar("select correlation_id from outbox_event where id = $1")
            .bind(event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(cid.starts_with("root.ab12cd."), "event cid must extend the producer's cid: {cid}");
    assert_eq!(cid.matches('.').count(), 2);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p platform --test outbox_publish`
Expected: FAIL — the stored cid currently equals the input (`root.ab12cd`, no appended segment).

- [ ] **Step 3: Implement the append in `publish`**

In `crates/platform/src/events/publisher.rs`, in `OutboxPublisher::publish`, compute the child cid and store it. Add `use crate::observability::append;` at the top, and change the insert:
```rust
        let event_cid = append(&event.correlation_id);
        let event_id: i64 = sqlx::query_scalar(
            "insert into outbox_event (event_type, aggregate_id, payload, correlation_id) \
             values ($1, $2, $3, $4) returning id",
        )
        .bind(&event.event_type)
        .bind(&event.aggregate_id)
        .bind(&event.payload)
        .bind(&event_cid)
        .fetch_one(&mut *conn)
        .await?;
```
(The delivery-row insert loop is unchanged.)

- [ ] **Step 4: Run to verify it passes**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p platform --test outbox_publish`
Expected: PASS (the new test + the existing fan-out test).

- [ ] **Step 5: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/platform
git commit -m "feat(events): publish mints a child cid per outbox event"
```

---

### Task 4: cid-tagged structured logs in domains + events

**Files:** Modify `crates/domain-auth/src/ports/http.rs`, `crates/domain-account/src/ports/events.rs`, `crates/platform/src/events/dispatcher.rs`, `crates/platform/src/events/dlq_http.rs`.

**Interfaces:** No signature changes — adds `tracing` log calls only. All inherit the active span's cid.

- [ ] **Step 1: Add logs (no secrets)**

- `domain-auth/src/ports/http.rs`:
  - in `register`, after the user is created: `tracing::info!(email = %user.email, user_id = user.id, "user registered");`
  - in `login`, on success (after `check_credentials` ok): `tracing::info!(email = %user.email, "login succeeded");` and in the failure path — the `check_credentials` `Err` — log a warn. Since `check_credentials` returns `Err` directly via `?`, add a `.inspect_err` or log before `?`:
    ```rust
    let found = state.users.find_by_email(&body.email).await.map_err(AppError::Internal)?;
    let user = match check_credentials(found.as_ref(), &body.password) {
        Ok(u) => u.clone(),
        Err(e) => { tracing::warn!(email = %body.email, "login failed"); return Err(e); }
    };
    ```
  - in `logout`: `tracing::info!("logout");`
  - in `set_user_scopes`: `tracing::info!(target_user = id, "user scopes replaced");`
  - **Never** log `body.password` or any token.
- `domain-account/src/ports/events.rs` (`AccountSubscriber::handle`), after `create_account_with_event` succeeds: `tracing::info!(auth_user_id = payload.auth_user_id, "account created from user.registered");` (an "already exists; skipping" info already exists — keep it).
- `platform/src/events/dispatcher.rs`: standardize the existing delivered/retry/dead logs to carry `delivery_id`, `subscriber = %row.subscriber_name`, `event_type = %delivered.event_type`. Add on success: `tracing::info!(delivery_id = row.delivery_id, subscriber = %row.subscriber_name, event_type = %delivered.event_type, "delivery delivered");` (the retry/dead `warn!`/`error!` already exist — ensure they include `subscriber` + `event_type`).
- `platform/src/events/dlq_http.rs` (`replay_handler`): `tracing::info!(delivery_id, "dlq delivery replayed");`

- [ ] **Step 2: Verify the suite still passes (logs don't change behavior)**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test`
Expected: PASS (no behavioral change). `cargo clippy --all-targets -- -D warnings` clean (watch for unused-variable / format-string lints).

- [ ] **Step 3: Commit**
```bash
cargo fmt --all
git add crates/domain-auth crates/domain-account crates/platform
git commit -m "feat(logging): cid-tagged structured logs at auth/account/dispatch/dlq (no secrets)"
```

---

### Task 5: cid-lineage end-to-end test

**Files:** Create `crates/app/tests/cid_lineage_e2e.rs`.

**Interfaces:** Consumes `app::state::build_router`, the dispatcher, the outbox tables.

- [ ] **Step 1: Write the test**

`crates/app/tests/cid_lineage_e2e.rs`:
```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::http::AccountState;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::AuthState;
use domain_auth::ports::postgres::PostgresUserRepository;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::events::dlq_http::DlqState;
use platform::events::{dispatch_once, DispatcherConfig, EventPublisher, OutboxPublisher, Routes, SubscriberRegistry};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_priv.pem");
const TEST_PUB_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_pub.pem");

#[sqlx::test(migrations = "../../migrations")]
async fn cid_lineage_grows_through_the_event_chain(pool: sqlx::PgPool) {
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

    // Register with an explicit root cid.
    let res = app.oneshot(
        Request::builder().method("POST").uri("/api/auth/register")
            .header("content-type", "application/json")
            .header("x-correlation-id", "root")
            .body(Body::from(r#"{"email":"e2e@x.y","password":"pw"}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // user.registered row cid extends "root" (middleware appended .a, publish appended .b).
    let ur_cid: String = sqlx::query_scalar(
        "select correlation_id from outbox_event where event_type = 'user.registered'"
    ).fetch_one(&pool).await.unwrap();
    assert!(ur_cid.starts_with("root."), "user.registered cid: {ur_cid}");
    assert!(ur_cid.matches('.').count() >= 2, "expected request+publish segments: {ur_cid}");

    // Dispatch -> account subscriber runs under ur_cid and publishes account.created,
    // whose row cid extends ur_cid further.
    dispatch_once(&pool, &registry, &DispatcherConfig::default()).await.unwrap();
    let ac_cid: String = sqlx::query_scalar(
        "select correlation_id from outbox_event where event_type = 'account.created'"
    ).fetch_one(&pool).await.unwrap();
    assert!(ac_cid.starts_with(&format!("{ur_cid}.")), "account.created cid {ac_cid} must extend {ur_cid}");
}
```

- [ ] **Step 2: Run + full-suite gate**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p app --test cid_lineage_e2e`
Then: `cargo build --all-targets && DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test && cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: PASS, clean. (Report the new full-suite total.)

- [ ] **Step 3: Commit**
```bash
git add crates/app
git commit -m "test(app): cid lineage grows through register -> user.registered -> account.created"
```

---

## Self-Review

**Spec coverage (design §2/§3/§5):** `new_segment`+`append` ✓ (T1); middleware appends + access log + infra exclusion + `RUST_LOG` ✓ (T2); publish appends per event row ✓ (T3); cid-tagged domain/event logs + secrets rule ✓ (T4); cid-lineage e2e ✓ (T5). §7 compat (the old `generates_non_empty_cid` len-36 test) handled by T1 Step 1 (replaced). `http_client` unchanged (already forwards the cid).

**Placeholder scan:** no TBDs; complete code per step. The login-failure logging restructure (T4) shows the full match.

**Type consistency:** `new_segment()`/`append(&str)->String` (T1) consumed by the middleware (T2) and `publish` (T3). `CorrelationId`/`CORRELATION_ID_HEADER` unchanged. The e2e (T5) builds the real `build_router`, so the middleware append + publish append + dispatcher both apply — the assertions check the growing prefix.

**Frontend note:** Plan B (separate) makes the SPA mint + send `X-Correlation-Id` and surface it on error toasts.
