# Spec 4: domain-notification — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `domain-notification` crate that subscribes to `account.created`, renders a `handlebars` template, dispatches via a pluggable `Notifier` port (dev impl logs it), records a `sent_notification` row, and exposes an admin read endpoint — completing the `user.registered → account.created → notification` chain.

**Architecture:** Hexagonal domain crate mirroring `domain-account`: a `Subscriber` consuming `account.created` (idempotent via a unique `source_event_id`), templates rendered with handlebars, dispatch behind a `Notifier` trait. Cross-domain only via the event payload (a local `AccountCreated` type — no dependency on `domain-account`).

**Tech Stack:** sqlx (runtime API), handlebars 6, axum 0.7, tokio, tracing, serde, async-trait.

## Global Constraints

- Depends on Specs 1–3 (merged). Spec: `docs/superpowers/specs/2026-06-25-domain-notification-design.md`.
- New workspace dep: `handlebars = "6"`. sqlx **runtime** query API only (no `query!`).
- `domain-notification` depends only on `platform` (NOT on `domain-account`); it deserializes the `account.created` payload into a local type.
- Subscriber name `"notification.on-account-created"`, event_type `"account.created"`. **Idempotent**: unique `source_event_id`; redelivery is a no-op.
- Failure (unknown template / render error / `@fail.test` recipient) → `Err` → outbox retries → dead-letters.
- handlebars **strict mode** (missing variable → error).
- `created_by_cid` is set from the event's `correlation_id`. **Never log secrets.**
- `#[sqlx::test(migrations = "../../migrations")]` for integration tests; `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres`. axum 0.7 `:id` syntax.
- `cargo fmt --all` + `cargo clippy --all-targets -- -D warnings` clean before each commit.

---

### Task 1: Crate scaffold + dep + migration + models

**Files:** Modify `Cargo.toml` (member + handlebars dep); Create `crates/domain-notification/Cargo.toml`, `crates/domain-notification/src/lib.rs`, `crates/domain-notification/src/models.rs`, `migrations/0006_notification.sql`.

**Interfaces:**
- Produces: `SentNotification` (`sqlx::FromRow` + `Serialize`), `NewSentNotification`, `NotificationChannel`; the `sent_notification` table.

- [ ] **Step 1: Workspace member + dep**

Root `Cargo.toml`: add `"crates/domain-notification"` to `members`. Add to `[workspace.dependencies]`:
```toml
handlebars = "6"
```

- [ ] **Step 2: Crate manifest**

`crates/domain-notification/Cargo.toml`:
```toml
[package]
name = "domain-notification"
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
handlebars.workspace = true

[dev-dependencies]
tower = { workspace = true, features = ["util"] }
```

- [ ] **Step 3: Migration**

`migrations/0006_notification.sql`:
```sql
create table sent_notification (
    id              bigserial primary key,
    source_event_id bigint      not null unique,
    template        text        not null,
    channel         text        not null,
    recipient       text        not null,
    body            text        not null,
    created_at      timestamptz not null default now(),
    created_by_cid  text        not null
);
```

- [ ] **Step 4: Models + lib**

`crates/domain-notification/src/models.rs`:
```rust
use serde::Serialize;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SentNotification {
    pub id: i64,
    pub source_event_id: i64,
    pub template: String,
    pub channel: String,
    pub recipient: String,
    pub body: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by_cid: String,
}

#[derive(Debug, Clone)]
pub struct NewSentNotification {
    pub source_event_id: i64,
    pub template: String,
    pub channel: String,
    pub recipient: String,
    pub body: String,
    pub created_by_cid: String,
}

/// Delivery channel. Only Email today; the enum keeps the door open for more.
#[derive(Debug, Clone)]
pub enum NotificationChannel {
    Email(String),
}

impl NotificationChannel {
    /// (channel-kind string, recipient) for storage/logging.
    pub fn parts(&self) -> (&'static str, &str) {
        match self {
            NotificationChannel::Email(addr) => ("email", addr),
        }
    }
}
```

`crates/domain-notification/src/lib.rs`:
```rust
//! Notification domain: consumes account.created, renders + dispatches notifications.
pub mod models;
```

- [ ] **Step 5: Verify**

Run: `cargo build -p domain-notification` (validates the migration via `sqlx::migrate!` at build time and compiles the crate).
Expected: PASS.

- [ ] **Step 6: Commit**
```bash
git add Cargo.toml crates/domain-notification migrations/0006_notification.sql
git commit -m "chore(notification): scaffold domain-notification crate + migration + models"
```

---

### Task 2: Templates (handlebars) + render

**Files:** Create `crates/domain-notification/src/ports/mod.rs`, `crates/domain-notification/src/ports/templates.rs`, `crates/domain-notification/src/templates/welcome.txt.hbs`; modify `lib.rs`.

**Interfaces:**
- Produces: `pub struct Templates`; `Templates::new() -> anyhow::Result<Templates>`; `render(&self, name: &str, vars: &serde_json::Value) -> anyhow::Result<String>`.

- [ ] **Step 1: Embedded template**

`crates/domain-notification/src/templates/welcome.txt.hbs`:
```
Hi {{email}},

Welcome! Your account (#{{account_id}}) is ready.
```

- [ ] **Step 2: Write the failing test**

`crates/domain-notification/src/ports/templates.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_welcome_with_vars() {
        let t = Templates::new().unwrap();
        let body = t
            .render("welcome", &serde_json::json!({ "email": "a@b.c", "account_id": 7 }))
            .unwrap();
        assert!(body.contains("a@b.c"));
        assert!(body.contains("#7"));
    }

    #[test]
    fn unknown_template_errors() {
        let t = Templates::new().unwrap();
        assert!(t.render("nope", &serde_json::json!({})).is_err());
    }

    #[test]
    fn missing_variable_errors_in_strict_mode() {
        let t = Templates::new().unwrap();
        assert!(t.render("welcome", &serde_json::json!({ "email": "a@b.c" })).is_err());
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p domain-notification templates::`
Expected: FAIL — `Templates` not found.

- [ ] **Step 4: Implement**

Top of `crates/domain-notification/src/ports/templates.rs`:
```rust
use handlebars::Handlebars;

/// Registry of embedded notification templates (handlebars, strict mode).
pub struct Templates {
    hb: Handlebars<'static>,
}

impl Templates {
    pub fn new() -> anyhow::Result<Templates> {
        let mut hb = Handlebars::new();
        hb.set_strict_mode(true); // missing variable -> render error -> DLQ
        hb.register_template_string("welcome", include_str!("../templates/welcome.txt.hbs"))?;
        Ok(Templates { hb })
    }

    pub fn render(&self, name: &str, vars: &serde_json::Value) -> anyhow::Result<String> {
        if !self.hb.has_template(name) {
            anyhow::bail!("unknown template: {name}");
        }
        Ok(self.hb.render(name, vars)?)
    }
}
```

`crates/domain-notification/src/ports/mod.rs`:
```rust
pub mod templates;
```
`crates/domain-notification/src/lib.rs` (add):
```rust
pub mod ports;
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p domain-notification templates::`
Expected: PASS.

- [ ] **Step 6: Commit**
```bash
cargo fmt --all && cargo clippy -p domain-notification --all-targets -- -D warnings
git add crates/domain-notification
git commit -m "feat(notification): handlebars Templates registry (strict) + welcome template"
```

---

### Task 3: `Notifier` port + `LogNotifier`

**Files:** Create `crates/domain-notification/src/ports/notifier.rs`; modify `crates/domain-notification/src/ports/mod.rs`.

**Interfaces:**
- Consumes: `NotificationChannel`.
- Produces: `#[async_trait] pub trait Notifier: Send + Sync { async fn send(&self, channel: &NotificationChannel, subject: &str, body: &str) -> anyhow::Result<()>; }`; `pub struct LogNotifier`.

- [ ] **Step 1: Write the failing test**

`crates/domain-notification/src/ports/notifier.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::NotificationChannel;

    #[tokio::test]
    async fn log_notifier_sends_ok() {
        let n = LogNotifier;
        let ch = NotificationChannel::Email("a@b.c".into());
        assert!(n.send(&ch, "Welcome", "body").await.is_ok());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p domain-notification notifier::`
Expected: FAIL — `Notifier`/`LogNotifier` not found.

- [ ] **Step 3: Implement**

Top of `crates/domain-notification/src/ports/notifier.rs`:
```rust
use crate::models::NotificationChannel;

/// Dispatches a rendered notification through some channel. Real providers
/// (SMTP/SES/Resend) implement this later; the dev impl just logs.
#[async_trait::async_trait]
pub trait Notifier: Send + Sync {
    async fn send(
        &self,
        channel: &NotificationChannel,
        subject: &str,
        body: &str,
    ) -> anyhow::Result<()>;
}

/// Dev notifier: logs the dispatch (cid-tagged via the active span). No real send.
pub struct LogNotifier;

#[async_trait::async_trait]
impl Notifier for LogNotifier {
    async fn send(
        &self,
        channel: &NotificationChannel,
        subject: &str,
        _body: &str,
    ) -> anyhow::Result<()> {
        let (kind, recipient) = channel.parts();
        tracing::info!(channel = kind, recipient = %recipient, subject = %subject, "notification dispatched");
        Ok(())
    }
}
```
`crates/domain-notification/src/ports/mod.rs` (add):
```rust
pub mod notifier;
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p domain-notification notifier::`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
cargo fmt --all && cargo clippy -p domain-notification --all-targets -- -D warnings
git add crates/domain-notification
git commit -m "feat(notification): Notifier port + LogNotifier (dev)"
```

---

### Task 4: `SentNotificationRepository` + Postgres adapter

**Files:** Create `crates/domain-notification/src/ports/repository.rs`, `crates/domain-notification/src/ports/postgres.rs`; modify `crates/domain-notification/src/ports/mod.rs`; Test `crates/domain-notification/tests/repository.rs`.

**Interfaces:**
- Consumes: `SentNotification`, `NewSentNotification`; `platform::db::Db`.
- Produces:
  - `#[async_trait] pub trait SentNotificationRepository: Send + Sync { async fn find_by_event_id(&self, source_event_id: i64) -> anyhow::Result<Option<SentNotification>>; async fn record(&self, new: NewSentNotification) -> anyhow::Result<()>; async fn list(&self) -> anyhow::Result<Vec<SentNotification>>; }`
  - `pub struct PostgresSentNotificationRepository { pool: Db }` + `new`.

- [ ] **Step 1: Write the failing test**

`crates/domain-notification/tests/repository.rs`:
```rust
use domain_notification::models::NewSentNotification;
use domain_notification::ports::postgres::PostgresSentNotificationRepository;
use domain_notification::ports::repository::SentNotificationRepository;

fn new_row(event_id: i64) -> NewSentNotification {
    NewSentNotification {
        source_event_id: event_id,
        template: "welcome".into(),
        channel: "email".into(),
        recipient: "a@b.c".into(),
        body: "hi".into(),
        created_by_cid: "cid".into(),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn record_find_list(pool: sqlx::PgPool) {
    let repo = PostgresSentNotificationRepository::new(pool.clone());
    assert!(repo.find_by_event_id(42).await.unwrap().is_none());

    repo.record(new_row(42)).await.unwrap();
    let found = repo.find_by_event_id(42).await.unwrap().unwrap();
    assert_eq!(found.recipient, "a@b.c");
    assert_eq!(found.source_event_id, 42);
    assert_eq!(repo.list().await.unwrap().len(), 1);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p domain-notification --test repository`
Expected: FAIL — items not found.

- [ ] **Step 3: Implement**

`crates/domain-notification/src/ports/repository.rs`:
```rust
use crate::models::SentNotification;

#[async_trait::async_trait]
pub trait SentNotificationRepository: Send + Sync {
    async fn find_by_event_id(&self, source_event_id: i64) -> anyhow::Result<Option<SentNotification>>;
    async fn record(&self, new: crate::models::NewSentNotification) -> anyhow::Result<()>;
    async fn list(&self) -> anyhow::Result<Vec<SentNotification>>;
}
```

`crates/domain-notification/src/ports/postgres.rs`:
```rust
use crate::models::{NewSentNotification, SentNotification};
use crate::ports::repository::SentNotificationRepository;
use platform::db::Db;

const COLS: &str =
    "id, source_event_id, template, channel, recipient, body, created_at, created_by_cid";

#[derive(Clone)]
pub struct PostgresSentNotificationRepository {
    pool: Db,
}

impl PostgresSentNotificationRepository {
    pub fn new(pool: Db) -> Self {
        PostgresSentNotificationRepository { pool }
    }
}

#[async_trait::async_trait]
impl SentNotificationRepository for PostgresSentNotificationRepository {
    async fn find_by_event_id(&self, source_event_id: i64) -> anyhow::Result<Option<SentNotification>> {
        let row = sqlx::query_as::<_, SentNotification>(&format!(
            "select {COLS} from sent_notification where source_event_id = $1"
        ))
        .bind(source_event_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn record(&self, new: NewSentNotification) -> anyhow::Result<()> {
        sqlx::query(
            "insert into sent_notification \
             (source_event_id, template, channel, recipient, body, created_by_cid) \
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(new.source_event_id)
        .bind(&new.template)
        .bind(&new.channel)
        .bind(&new.recipient)
        .bind(&new.body)
        .bind(&new.created_by_cid)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list(&self) -> anyhow::Result<Vec<SentNotification>> {
        let rows = sqlx::query_as::<_, SentNotification>(&format!(
            "select {COLS} from sent_notification order by id desc"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}
```
`crates/domain-notification/src/ports/mod.rs` (add):
```rust
pub mod postgres;
pub mod repository;
```

- [ ] **Step 4: Run to verify it passes**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p domain-notification --test repository`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/domain-notification
git commit -m "feat(notification): SentNotificationRepository + Postgres adapter"
```

---

### Task 5: `NotificationSubscriber` (idempotent)

**Files:** Create `crates/domain-notification/src/ports/events.rs`; modify `crates/domain-notification/src/ports/mod.rs`; Test `crates/domain-notification/tests/subscriber.rs`.

**Interfaces:**
- Consumes: `Templates`, `Notifier`, `SentNotificationRepository`, `NotificationChannel`, `NewSentNotification`; `platform::events::{Subscriber, DeliveredEvent}`.
- Produces:
  - `pub struct NotificationSubscriber { repo: Arc<dyn SentNotificationRepository>, notifier: Arc<dyn Notifier>, templates: Arc<Templates> }` + `pub fn new(...)`.
  - `impl Subscriber for NotificationSubscriber` (`name = "notification.on-account-created"`, `event_type = "account.created"`).

- [ ] **Step 1: Write the failing test**

`crates/domain-notification/tests/subscriber.rs`:
```rust
use std::sync::Arc;
use domain_notification::ports::events::NotificationSubscriber;
use domain_notification::ports::notifier::LogNotifier;
use domain_notification::ports::postgres::PostgresSentNotificationRepository;
use domain_notification::ports::repository::SentNotificationRepository;
use domain_notification::ports::templates::Templates;
use platform::events::{DeliveredEvent, Subscriber};

fn event(event_id: i64, email: &str) -> DeliveredEvent {
    DeliveredEvent {
        event_id,
        event_type: "account.created".into(),
        aggregate_id: "1".into(),
        payload: serde_json::json!({ "account_id": 1, "auth_user_id": 1, "email": email }),
        correlation_id: "root.ab.cd".into(),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn sends_and_records_then_is_idempotent(pool: sqlx::PgPool) {
    let repo = Arc::new(PostgresSentNotificationRepository::new(pool.clone()));
    let sub = NotificationSubscriber::new(repo.clone(), Arc::new(LogNotifier), Arc::new(Templates::new().unwrap()));

    let e = event(10, "a@b.c");
    sub.handle(&e).await.unwrap();
    let row = repo.find_by_event_id(10).await.unwrap().unwrap();
    assert_eq!(row.recipient, "a@b.c");
    assert_eq!(row.template, "welcome");
    assert_eq!(row.created_by_cid, "root.ab.cd");

    // Redelivery is a no-op.
    sub.handle(&e).await.unwrap();
    assert_eq!(repo.list().await.unwrap().len(), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn fail_test_recipient_errors_for_dlq(pool: sqlx::PgPool) {
    let repo = Arc::new(PostgresSentNotificationRepository::new(pool.clone()));
    let sub = NotificationSubscriber::new(repo, Arc::new(LogNotifier), Arc::new(Templates::new().unwrap()));
    assert!(sub.handle(&event(11, "boom@fail.test")).await.is_err());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p domain-notification --test subscriber`
Expected: FAIL — `NotificationSubscriber` not found.

- [ ] **Step 3: Implement**

`crates/domain-notification/src/ports/events.rs`:
```rust
use crate::models::{NewSentNotification, NotificationChannel};
use crate::ports::notifier::Notifier;
use crate::ports::repository::SentNotificationRepository;
use crate::ports::templates::Templates;
use platform::events::{DeliveredEvent, Subscriber};
use serde::Deserialize;
use std::sync::Arc;

/// Local view of the `account.created` payload (no dependency on domain-account).
#[derive(Debug, Deserialize)]
struct AccountCreated {
    account_id: i64,
    #[allow(dead_code)]
    auth_user_id: i64,
    email: String,
}

pub struct NotificationSubscriber {
    repo: Arc<dyn SentNotificationRepository>,
    notifier: Arc<dyn Notifier>,
    templates: Arc<Templates>,
}

impl NotificationSubscriber {
    pub fn new(
        repo: Arc<dyn SentNotificationRepository>,
        notifier: Arc<dyn Notifier>,
        templates: Arc<Templates>,
    ) -> NotificationSubscriber {
        NotificationSubscriber { repo, notifier, templates }
    }
}

#[async_trait::async_trait]
impl Subscriber for NotificationSubscriber {
    fn name(&self) -> &'static str {
        "notification.on-account-created"
    }
    fn event_type(&self) -> &'static str {
        "account.created"
    }
    async fn handle(&self, event: &DeliveredEvent) -> anyhow::Result<()> {
        // Idempotency: at-least-once delivery, so skip if already sent for this event.
        if self.repo.find_by_event_id(event.event_id).await?.is_some() {
            tracing::info!(event_id = event.event_id, "notification already sent; skipping");
            return Ok(());
        }
        let payload: AccountCreated = serde_json::from_value(event.payload.clone())?;

        // Dev/test failure hook (mirrors the Haskell @fail.com): forces a DLQ path.
        if payload.email.ends_with("@fail.test") {
            anyhow::bail!("simulated notification failure for {}", payload.email);
        }

        let body = self.templates.render(
            "welcome",
            &serde_json::json!({ "email": payload.email, "account_id": payload.account_id }),
        )?;
        let channel = NotificationChannel::Email(payload.email.clone());
        self.notifier.send(&channel, "Welcome", &body).await?;

        let (kind, recipient) = channel.parts();
        self.repo
            .record(NewSentNotification {
                source_event_id: event.event_id,
                template: "welcome".into(),
                channel: kind.into(),
                recipient: recipient.into(),
                body,
                created_by_cid: event.correlation_id.clone(),
            })
            .await?;
        Ok(())
    }
}
```
`crates/domain-notification/src/ports/mod.rs` (add):
```rust
pub mod events;
```

- [ ] **Step 4: Run to verify it passes**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p domain-notification --test subscriber`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/domain-notification
git commit -m "feat(notification): account.created subscriber (idempotent welcome notification)"
```

---

### Task 6: Admin `GET /notifications` router

**Files:** Create `crates/domain-notification/src/ports/http.rs`; modify `crates/domain-notification/src/ports/mod.rs`, `crates/domain-notification/src/lib.rs`; Test `crates/domain-notification/tests/http.rs`.

**Interfaces:**
- Consumes: `SentNotificationRepository`; `platform::auth::{Authenticated, JwtVerifier, RevocationChecker, require_scope}`; `platform::metrics::Metrics`; `platform::server::AppError`.
- Produces:
  - `#[derive(Clone)] pub struct NotificationState { pub repo: Arc<dyn SentNotificationRepository>, pub jwt: Arc<JwtVerifier>, pub revocation: Arc<dyn RevocationChecker>, pub metrics: Metrics }` with `FromRef` impls for `Arc<JwtVerifier>` and `Arc<dyn RevocationChecker>`.
  - `pub fn router(state: NotificationState) -> axum::Router` — `GET /notifications` (admin).

- [ ] **Step 1: Write the failing test**

`crates/domain-notification/tests/http.rs`:
```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_notification::ports::http::{router, NotificationState};
use domain_notification::ports::postgres::PostgresSentNotificationRepository;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PUB_PEM: &str = include_str!("fixtures/test_pub.pem");

#[sqlx::test(migrations = "../../migrations")]
async fn list_notifications_without_token_is_unauthorized(pool: sqlx::PgPool) {
    let state = NotificationState {
        repo: Arc::new(PostgresSentNotificationRepository::new(pool)),
        jwt: Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        revocation: Arc::new(NoopRevocationChecker),
        metrics: Metrics::new().unwrap(),
    };
    let app = router(state);
    let res = app
        .oneshot(Request::builder().uri("/notifications").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 2: Create the test fixture**

```bash
mkdir -p crates/domain-notification/tests/fixtures
cp crates/domain-auth/tests/fixtures/test_pub.pem crates/domain-notification/tests/fixtures/test_pub.pem
```
Add to `crates/domain-notification/Cargo.toml` `[dev-dependencies]`: `http-body-util = { workspace = true }` (not strictly needed by this test, but matches the pattern; optional).

- [ ] **Step 3: Run to verify it fails**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p domain-notification --test http`
Expected: FAIL — `router`/`NotificationState` not found.

- [ ] **Step 4: Implement**

`crates/domain-notification/src/ports/http.rs`:
```rust
use crate::models::SentNotification;
use crate::ports::repository::SentNotificationRepository;
use axum::extract::{FromRef, State};
use axum::routing::get;
use axum::{Json, Router};
use platform::auth::{require_scope, Authenticated, JwtVerifier, RevocationChecker};
use platform::metrics::Metrics;
use platform::server::AppError;
use std::sync::Arc;

#[derive(Clone)]
pub struct NotificationState {
    pub repo: Arc<dyn SentNotificationRepository>,
    pub jwt: Arc<JwtVerifier>,
    pub revocation: Arc<dyn RevocationChecker>,
    pub metrics: Metrics,
}

impl FromRef<NotificationState> for Arc<JwtVerifier> {
    fn from_ref(state: &NotificationState) -> Self {
        state.jwt.clone()
    }
}

impl FromRef<NotificationState> for Arc<dyn RevocationChecker> {
    fn from_ref(state: &NotificationState) -> Self {
        state.revocation.clone()
    }
}

pub fn router(state: NotificationState) -> Router {
    Router::new()
        .route("/notifications", get(list_notifications))
        .with_state(state)
}

async fn list_notifications(
    State(state): State<NotificationState>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<SentNotification>>, AppError> {
    require_scope(&claims, "admin")?;
    let rows = state.repo.list().await.map_err(AppError::Internal)?;
    Ok(Json(rows))
}
```
> `metrics` is held on the state for symmetry with the other domain states (and future per-domain metric hooks); it is not read by this handler yet.

`crates/domain-notification/src/ports/mod.rs` (add):
```rust
pub mod http;
```
`crates/domain-notification/src/lib.rs` (add):
```rust
pub use ports::http::{router, NotificationState};
```

- [ ] **Step 5: Run to verify it passes**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p domain-notification --test http`
Expected: PASS.

- [ ] **Step 6: Commit**
```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/domain-notification
git commit -m "feat(notification): admin GET /notifications router"
```

---

### Task 7: App wiring + e2e

**Files:** Modify `crates/app/Cargo.toml` (dep), `crates/app/src/state.rs` (register subscriber, route, notification_state, build_router merge); Create `crates/app/tests/notification_e2e.rs`.

**Interfaces:** Consumes `domain_notification::{router, NotificationState}`, its subscriber + repo + notifier + templates.

- [ ] **Step 1: Add app dep**

`crates/app/Cargo.toml` `[dependencies]`: `domain-notification = { path = "../domain-notification" }`. `[dev-dependencies]`: `domain-notification = { path = "../domain-notification" }`.

- [ ] **Step 2: Wire resources + routes in `state.rs`**

In `crates/app/src/state.rs`:
- Imports:
  ```rust
  use domain_notification::ports::events::NotificationSubscriber;
  use domain_notification::ports::notifier::LogNotifier;
  use domain_notification::ports::postgres::PostgresSentNotificationRepository;
  use domain_notification::ports::templates::Templates;
  use domain_notification::NotificationState;
  ```
- In `routes()`, add the new subscription:
  ```rust
  fn routes() -> Routes {
      Routes::new()
          .add("user.registered", "account.on-user-registered")
          .add("account.created", "notification.on-account-created")
  }
  ```
- In `build_resources`, after building the account repo/registry, register the notification subscriber (build templates once; on failure, `.context`):
  ```rust
      let templates = std::sync::Arc::new(Templates::new().context("load notification templates")?);
      let notif_repo = Arc::new(PostgresSentNotificationRepository::new(pool.clone()));
      registry.register(Arc::new(NotificationSubscriber::new(
          notif_repo,
          Arc::new(LogNotifier),
          templates,
      )));
  ```
  (Place this **before** `let registry = Arc::new(registry);`.)
- Add a `notification_state` builder:
  ```rust
  pub fn notification_state(res: &Resources) -> NotificationState {
      NotificationState {
          repo: Arc::new(PostgresSentNotificationRepository::new(res.pool.clone())),
          jwt: res.jwt.clone(),
          revocation: res.revocation.clone(),
          metrics: res.metrics.clone(),
      }
  }
  ```
- In `build_router`, add the notification router param + merge it into the `/api` nest. Update the signature and the `api` assembly:
  ```rust
  pub fn build_router(
      account: AccountState,
      auth: AuthState,
      dlq: DlqState,
      notification: NotificationState,
      metrics: Metrics,
      cors_origins: &[String],
      web_dist: Option<PathBuf>,
  ) -> Router {
      let api = domain_account::router(account)
          .merge(domain_auth::router(auth))
          .merge(platform::events::dlq_http::dlq_router(dlq))
          .merge(domain_notification::router(notification));
      // ... rest unchanged ...
  ```

- [ ] **Step 3: Update `main.rs` call**

In `crates/app/src/main.rs`, pass the notification state to `build_router`:
```rust
    let app = state::build_router(
        state::account_state(&res),
        state::auth_state(&res),
        state::dlq_state(&res),
        state::notification_state(&res),
        res.metrics.clone(),
        &res.settings.cors_allowed_origins,
        web_dist,
    );
```

- [ ] **Step 4: Update existing app tests that call `build_router`**

`crates/app/tests/api_router.rs`, `crates/app/tests/account_me_e2e.rs`, and `crates/app/tests/cid_lineage_e2e.rs` (if present) call `build_router` — add a `NotificationState` argument to each. Build one inline in each test's setup:
```rust
    let notification = domain_notification::NotificationState {
        repo: Arc::new(domain_notification::ports::postgres::PostgresSentNotificationRepository::new(pool.clone())),
        jwt: jwt.clone(),
        revocation: revocation.clone(),
        metrics: metrics.clone(),
    };
    // ...build_router(account, auth, dlq, notification, metrics, &[], None)
```
Run `cargo build --all-targets` and fix each `build_router` call the compiler flags (arity changed).

- [ ] **Step 5: Write the e2e test**

`crates/app/tests/notification_e2e.rs`:
```rust
use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::AuthState;
use domain_auth::ports::postgres::PostgresUserRepository;
use domain_notification::ports::events::NotificationSubscriber;
use domain_notification::ports::notifier::LogNotifier;
use domain_notification::ports::postgres::PostgresSentNotificationRepository;
use domain_notification::ports::repository::SentNotificationRepository;
use domain_notification::ports::templates::Templates;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::events::{dispatch_once, DispatcherConfig, EventPublisher, OutboxPublisher, Routes, SubscriberRegistry};
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_priv.pem");

#[sqlx::test(migrations = "../../migrations")]
async fn register_dispatches_to_account_then_notification(pool: sqlx::PgPool) {
    let user_repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    let account_repo = Arc::new(PostgresAccountRepository::new(pool.clone()));
    let notif_repo = Arc::new(PostgresSentNotificationRepository::new(pool.clone()));
    let publisher: Arc<dyn EventPublisher> = Arc::new(OutboxPublisher::new(
        Routes::new()
            .add("user.registered", "account.on-user-registered")
            .add("account.created", "notification.on-account-created"),
    ));
    let mut registry = SubscriberRegistry::new();
    registry.register(Arc::new(AccountSubscriber::new(pool.clone(), account_repo.clone(), publisher.clone())));
    registry.register(Arc::new(NotificationSubscriber::new(
        notif_repo.clone(), Arc::new(LogNotifier), Arc::new(Templates::new().unwrap()),
    )));
    let registry = Arc::new(registry);

    // Register a user via the auth router (publishes user.registered).
    let auth = domain_auth::ports::http::router(AuthState {
        pool: pool.clone(), users: user_repo.clone(), refresh_tokens: user_repo.clone(),
        scopes: user_repo.clone(), publisher: publisher.clone(),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        verifier: Arc::new(JwtVerifier::from_rsa_pem(include_str!("../../domain-auth/tests/fixtures/test_pub.pem")).unwrap()),
        revocation: Arc::new(NoopRevocationChecker),
        admin_emails: Arc::new(vec![]),
        metrics: platform::metrics::Metrics::new().unwrap(),
    });
    let res = auth.oneshot(
        Request::builder().method("POST").uri("/auth/register")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"email":"e2e@x.y","password":"pw"}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // 1st dispatch: account.on-user-registered creates the account + emits account.created.
    dispatch_once(&pool, &registry, &DispatcherConfig::default()).await.unwrap();
    // 2nd dispatch: notification.on-account-created consumes account.created.
    dispatch_once(&pool, &registry, &DispatcherConfig::default()).await.unwrap();

    // A welcome notification was recorded for the new account's email.
    let sent = notif_repo.list().await.unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].recipient, "e2e@x.y");
    assert_eq!(sent[0].template, "welcome");
}
```

- [ ] **Step 6: Full-suite gate**

Run: `cargo build --all-targets && DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test && cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: PASS, clean. (Report the new full-suite total.)

- [ ] **Step 7: Commit**
```bash
git add crates/app
git commit -m "feat(app): wire domain-notification (subscriber + route + /api router); e2e chain"
```

---

## Self-Review

**Spec coverage (design §3–§6):** crate + migration + models ✓ (T1); handlebars Templates ✓ (T2); Notifier + LogNotifier ✓ (T3); repository ✓ (T4); idempotent subscriber + `@fail.test` DLQ hook + local `AccountCreated` ✓ (T5); admin `GET /notifications` ✓ (T6); app wiring (Routes `account.created → notification.on-account-created`, registry, `/api` merge) + e2e chain ✓ (T7). `created_by_cid` from `event.correlation_id` ✓ (T5).

**Placeholder scan:** none — complete code per step. The `auth_user_id` unused field carries `#[allow(dead_code)]` (it documents the payload shape).

**Type consistency:** `NewSentNotification`/`SentNotification` fields consistent across T1/T4/T5. `NotificationChannel::parts() -> (&'static str, &str)` used in T3/T5. `SentNotificationRepository` trait methods consistent T4→T5/T6. `NotificationState` fields (repo/jwt/revocation/metrics) consistent T6/T7. `build_router` arity change (added `notification`) propagated to `main.rs` + the three existing app tests (T7 S3/S4). Subscriber name/event_type match the `Routes` entry. The `account.created` payload shape (`account_id`,`auth_user_id`,`email`) matches what `domain-account`'s `create_account_with_event` emits.

**Cross-cutting note:** T7 changes `build_router`'s signature — every caller (main + `api_router.rs` + `account_me_e2e.rs` + `cid_lineage_e2e.rs` if the cid plan ran first) must be updated. The plan calls this out explicitly in T7 S4.
