# Spec 1b: Outbox Event System — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the transactional-outbox event system in `platform::events`: durable publish in the same DB transaction as state changes, fan-out to multiple subscribers, retries with exponential backoff, and a dead-letter queue with replay.

**Architecture:** Events are written to `outbox_event`, and one `outbox_delivery` row is created per (event × subscriber) at publish time — this is what enables independent fan-out and a per-consumer DLQ. A background dispatcher polls due deliveries, opens a tracing span from the event's stored correlation id, invokes the matching subscriber, and on failure applies exponential backoff until `max_attempts`, after which the delivery is marked `dead`. The `EventPublisher` is a trait so a Kafka-backed implementation can later replace the outbox without touching call sites.

**Tech Stack:** sqlx (Postgres, runtime query API — no compile-time DB needed), tokio, tracing, serde_json, async-trait, chrono.

## Global Constraints

- Same dependency pins and rules as Plan 1a (`docs/superpowers/plans/2026-06-24-rust-spec1a-workspace-and-platform.md`, "Global Constraints").
- Use the **runtime** sqlx API (`sqlx::query`, `sqlx::query_as`, `.bind(...)`) — NOT the compile-time `query!` macros — so the crate builds without a live database. (Upgrading to `query!` + `sqlx prepare` is a later optional improvement.)
- Delivery statuses are exactly the strings `"pending"`, `"delivered"`, `"dead"`.
- `max_attempts` default = **5**. Backoff = `2^attempts` seconds, capped at 300s.
- Correlation-id header name `X-Correlation-Id`; cid is stored on every `outbox_event` row.
- Tests use `#[sqlx::test]` (auto-provisions an isolated Postgres DB per test and runs `./migrations`). Requires `DATABASE_URL` set to a reachable Postgres instance when running tests.
- Run `cargo fmt` + `cargo clippy --all-targets -- -D warnings` before each commit.

---

### Task 1: Outbox migration

**Files:**
- Create: `migrations/0001_outbox.sql`

**Interfaces:**
- Produces: tables `outbox_event`, `outbox_delivery` with the unique constraint `(event_id, subscriber_name)` and an index on `(status, next_attempt_at)`.

- [ ] **Step 1: Write the migration**

`migrations/0001_outbox.sql`:
```sql
create table outbox_event (
    id             bigserial primary key,
    event_type     text        not null,
    aggregate_id   text        not null,
    payload        jsonb       not null,
    correlation_id text        not null,
    created_at     timestamptz not null default now()
);

create table outbox_delivery (
    id              bigserial primary key,
    event_id        bigint      not null references outbox_event (id),
    subscriber_name text        not null,
    status          text        not null default 'pending',
    attempts        int         not null default 0,
    last_error      text,
    next_attempt_at timestamptz not null default now(),
    created_at      timestamptz not null default now(),
    updated_at      timestamptz not null default now(),
    unique (event_id, subscriber_name)
);

create index outbox_delivery_due_idx
    on outbox_delivery (status, next_attempt_at);
```

- [ ] **Step 2: Verify the migration is well-formed**

Run: `cargo build -p platform`
Expected: PASS (the `sqlx::migrate!` macro from Plan 1a Task 8 validates the migrations directory at build time).

- [ ] **Step 3: Commit**

```bash
git add migrations/0001_outbox.sql
git commit -m "feat(events): outbox_event + outbox_delivery migration"
```

---

### Task 2: Event domain types + subscriber trait + registry

**Files:**
- Create: `crates/platform/src/events/mod.rs`
- Create: `crates/platform/src/events/types.rs`
- Modify: `crates/platform/src/lib.rs` (add `pub mod events;`)
- Test: inline `#[cfg(test)]` in `types.rs`

**Interfaces:**
- Produces:
  - `pub struct NewEvent { pub event_type: String, pub aggregate_id: String, pub payload: serde_json::Value, pub correlation_id: String }`
  - `pub struct DeliveredEvent { pub event_id: i64, pub event_type: String, pub aggregate_id: String, pub payload: serde_json::Value, pub correlation_id: String }`
  - `#[async_trait] pub trait Subscriber: Send + Sync { fn name(&self) -> &'static str; fn event_type(&self) -> &'static str; async fn handle(&self, event: &DeliveredEvent) -> anyhow::Result<()>; }`
  - `pub struct SubscriberRegistry { subscribers: Vec<Arc<dyn Subscriber>> }`
  - `impl SubscriberRegistry { pub fn new() -> Self; pub fn register(&mut self, s: Arc<dyn Subscriber>); pub fn names_for(&self, event_type: &str) -> Vec<&'static str>; pub fn find(&self, name: &str) -> Option<Arc<dyn Subscriber>>; }`
  - `#[derive(Clone, Default)] pub struct Routes { map: std::collections::HashMap<String, Vec<String>> }` — a plain event_type → subscriber-name routing table. This is **data only** (no subscriber instances), which keeps the publisher a leaf in the dependency graph (publisher → Routes, never publisher → subscribers). Construction is linear: build `Routes` from static declarations, build the publisher over it, then build subscribers that hold the publisher.
  - `impl Routes { pub fn new() -> Self; pub fn add(self, event_type: &str, subscriber_name: &str) -> Self; pub fn names_for(&self, event_type: &str) -> Vec<String>; }`

- [ ] **Step 1: Write the failing test**

`crates/platform/src/events/types.rs`:
```rust
use std::sync::Arc;

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy;
    #[async_trait::async_trait]
    impl Subscriber for Dummy {
        fn name(&self) -> &'static str { "dummy" }
        fn event_type(&self) -> &'static str { "thing.happened" }
        async fn handle(&self, _e: &DeliveredEvent) -> anyhow::Result<()> { Ok(()) }
    }

    #[test]
    fn registry_finds_subscribers_by_event_type_and_name() {
        let mut reg = SubscriberRegistry::new();
        reg.register(Arc::new(Dummy));
        assert_eq!(reg.names_for("thing.happened"), vec!["dummy"]);
        assert!(reg.names_for("other").is_empty());
        assert!(reg.find("dummy").is_some());
        assert!(reg.find("missing").is_none());
    }

    #[test]
    fn routes_maps_event_types_to_subscriber_names() {
        let routes = Routes::new()
            .add("user.registered", "account.on-user-registered")
            .add("user.registered", "audit.log");
        assert_eq!(
            routes.names_for("user.registered"),
            vec!["account.on-user-registered".to_string(), "audit.log".to_string()]
        );
        assert!(routes.names_for("other").is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p platform events::`
Expected: FAIL — module/types not found.

- [ ] **Step 3: Write the implementation**

`crates/platform/src/events/types.rs` (above the test module):
```rust
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct NewEvent {
    pub event_type: String,
    pub aggregate_id: String,
    pub payload: serde_json::Value,
    pub correlation_id: String,
}

#[derive(Debug, Clone)]
pub struct DeliveredEvent {
    pub event_id: i64,
    pub event_type: String,
    pub aggregate_id: String,
    pub payload: serde_json::Value,
    pub correlation_id: String,
}

/// A consumer of a single event type. Concrete implementations hold whatever
/// domain dependencies (repositories, publishers) they need.
#[async_trait::async_trait]
pub trait Subscriber: Send + Sync {
    fn name(&self) -> &'static str;
    fn event_type(&self) -> &'static str;
    async fn handle(&self, event: &DeliveredEvent) -> anyhow::Result<()>;
}

#[derive(Default)]
pub struct SubscriberRegistry {
    subscribers: Vec<Arc<dyn Subscriber>>,
}

impl SubscriberRegistry {
    pub fn new() -> Self {
        SubscriberRegistry { subscribers: Vec::new() }
    }

    pub fn register(&mut self, s: Arc<dyn Subscriber>) {
        self.subscribers.push(s);
    }

    /// Names of all subscribers interested in `event_type` (drives fan-out).
    pub fn names_for(&self, event_type: &str) -> Vec<&'static str> {
        self.subscribers
            .iter()
            .filter(|s| s.event_type() == event_type)
            .map(|s| s.name())
            .collect()
    }

    pub fn find(&self, name: &str) -> Option<Arc<dyn Subscriber>> {
        self.subscribers.iter().find(|s| s.name() == name).cloned()
    }
}

/// Event-type -> subscriber-name routing table used by the publisher to fan out
/// delivery rows. Plain data (no subscriber instances) so the publisher never
/// depends on the subscribers it routes to.
#[derive(Debug, Clone, Default)]
pub struct Routes {
    map: std::collections::HashMap<String, Vec<String>>,
}

impl Routes {
    pub fn new() -> Self {
        Routes::default()
    }

    pub fn add(mut self, event_type: &str, subscriber_name: &str) -> Self {
        self.map
            .entry(event_type.to_string())
            .or_default()
            .push(subscriber_name.to_string());
        self
    }

    pub fn names_for(&self, event_type: &str) -> Vec<String> {
        self.map.get(event_type).cloned().unwrap_or_default()
    }
}
```

`crates/platform/src/events/mod.rs`:
```rust
mod types;
pub use types::*;
```

Add to `crates/platform/src/lib.rs`:
```rust
pub mod events;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p platform events::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/platform
git commit -m "feat(events): event types, Subscriber trait, registry"
```

---

### Task 3: `EventPublisher` trait + `OutboxPublisher` (publish with fan-out)

**Files:**
- Create: `crates/platform/src/events/publisher.rs`
- Modify: `crates/platform/src/events/mod.rs`
- Test: `crates/platform/tests/outbox_publish.rs`

**Interfaces:**
- Consumes: `NewEvent`, `Routes` (Task 2); `Db` (`platform::db`).
- Produces:
  - `#[async_trait] pub trait EventPublisher: Send + Sync { async fn publish(&self, conn: &mut sqlx::PgConnection, event: NewEvent) -> anyhow::Result<i64>; }` — returns the new `outbox_event.id`. Takes a `&mut PgConnection` so callers publish **inside their own transaction** (atomic with the state change).
  - `pub struct OutboxPublisher { routes: Routes }`
  - `pub fn OutboxPublisher::new(routes: Routes) -> OutboxPublisher`

- [ ] **Step 1: Write the failing test**

`crates/platform/tests/outbox_publish.rs`:
```rust
use platform::events::{EventPublisher, NewEvent, OutboxPublisher, Routes};

#[sqlx::test]
async fn publish_fans_out_one_delivery_per_subscriber(pool: sqlx::PgPool) {
    let routes = Routes::new()
        .add("user.registered", "sub-a")
        .add("user.registered", "sub-b");
    let publisher = OutboxPublisher::new(routes);

    let mut tx = pool.begin().await.unwrap();
    let event_id = publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "user.registered".into(),
                aggregate_id: "42".into(),
                payload: serde_json::json!({ "uid": 42, "email": "a@b.c" }),
                correlation_id: "cid-123".into(),
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let deliveries: i64 =
        sqlx::query_scalar("select count(*) from outbox_delivery where event_id = $1")
            .bind(event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(deliveries, 2, "one delivery row per subscriber");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p platform --test outbox_publish`
Expected: FAIL — `OutboxPublisher`/`EventPublisher` not found.

- [ ] **Step 3: Write the implementation**

`crates/platform/src/events/publisher.rs`:
```rust
use crate::events::{NewEvent, Routes};

#[async_trait::async_trait]
pub trait EventPublisher: Send + Sync {
    /// Persist an event and a pending delivery row per interested subscriber,
    /// using the caller's transaction so it commits atomically with state.
    async fn publish(
        &self,
        conn: &mut sqlx::PgConnection,
        event: NewEvent,
    ) -> anyhow::Result<i64>;
}

pub struct OutboxPublisher {
    routes: Routes,
}

impl OutboxPublisher {
    pub fn new(routes: Routes) -> OutboxPublisher {
        OutboxPublisher { routes }
    }
}

#[async_trait::async_trait]
impl EventPublisher for OutboxPublisher {
    async fn publish(
        &self,
        conn: &mut sqlx::PgConnection,
        event: NewEvent,
    ) -> anyhow::Result<i64> {
        let event_id: i64 = sqlx::query_scalar(
            "insert into outbox_event (event_type, aggregate_id, payload, correlation_id) \
             values ($1, $2, $3, $4) returning id",
        )
        .bind(&event.event_type)
        .bind(&event.aggregate_id)
        .bind(&event.payload)
        .bind(&event.correlation_id)
        .fetch_one(&mut *conn)
        .await?;

        for name in self.routes.names_for(&event.event_type) {
            sqlx::query(
                "insert into outbox_delivery (event_id, subscriber_name) values ($1, $2)",
            )
            .bind(event_id)
            .bind(&name)
            .execute(&mut *conn)
            .await?;
        }

        Ok(event_id)
    }
}
```

`crates/platform/src/events/mod.rs` (add):
```rust
mod publisher;
pub use publisher::*;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p platform --test outbox_publish`
Expected: PASS — 2 delivery rows.

- [ ] **Step 5: Commit**

```bash
git add crates/platform
git commit -m "feat(events): EventPublisher trait + OutboxPublisher with fan-out"
```

---

### Task 4: Dispatcher — deliver pending events (happy path)

**Files:**
- Create: `crates/platform/src/events/dispatcher.rs`
- Modify: `crates/platform/src/events/mod.rs`
- Test: `crates/platform/tests/outbox_dispatch.rs`

**Interfaces:**
- Consumes: `SubscriberRegistry`, `DeliveredEvent` (Task 2); `Db`.
- Produces:
  - `pub struct DispatcherConfig { pub max_attempts: i32, pub batch_size: i64 }` with `Default` (`max_attempts: 5`, `batch_size: 50`).
  - `pub async fn dispatch_once(pool: &Db, registry: &SubscriberRegistry, config: &DispatcherConfig) -> anyhow::Result<usize>` — processes one batch of due deliveries; returns how many it attempted.

- [ ] **Step 1: Write the failing test**

`crates/platform/tests/outbox_dispatch.rs`:
```rust
use std::sync::{Arc, Mutex};
use platform::events::{
    dispatch_once, DeliveredEvent, DispatcherConfig, EventPublisher, NewEvent, OutboxPublisher,
    Routes, Subscriber, SubscriberRegistry,
};

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);
#[async_trait::async_trait]
impl Subscriber for Recorder {
    fn name(&self) -> &'static str { "recorder" }
    fn event_type(&self) -> &'static str { "user.registered" }
    async fn handle(&self, e: &DeliveredEvent) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(e.correlation_id.clone());
        Ok(())
    }
}

#[sqlx::test]
async fn dispatch_delivers_pending_and_marks_delivered(pool: sqlx::PgPool) {
    let rec = Recorder::default();
    let mut reg = SubscriberRegistry::new();
    reg.register(Arc::new(rec.clone()));
    let reg = Arc::new(reg);

    let publisher = OutboxPublisher::new(Routes::new().add("user.registered", "recorder"));
    let mut tx = pool.begin().await.unwrap();
    publisher
        .publish(&mut tx, NewEvent {
            event_type: "user.registered".into(),
            aggregate_id: "7".into(),
            payload: serde_json::json!({}),
            correlation_id: "cid-xyz".into(),
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let n = dispatch_once(&pool, &reg, &DispatcherConfig::default()).await.unwrap();
    assert_eq!(n, 1);
    assert_eq!(rec.0.lock().unwrap().as_slice(), &["cid-xyz".to_string()]);

    let delivered: i64 = sqlx::query_scalar(
        "select count(*) from outbox_delivery where status = 'delivered'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(delivered, 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p platform --test outbox_dispatch`
Expected: FAIL — `dispatch_once` not found.

- [ ] **Step 3: Write the implementation**

`crates/platform/src/events/dispatcher.rs`:
```rust
use crate::db::Db;
use crate::events::{DeliveredEvent, SubscriberRegistry};
use tracing::Instrument;

pub struct DispatcherConfig {
    pub max_attempts: i32,
    pub batch_size: i64,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        DispatcherConfig { max_attempts: 5, batch_size: 50 }
    }
}

#[derive(sqlx::FromRow)]
struct DueRow {
    delivery_id: i64,
    subscriber_name: String,
    attempts: i32,
    event_id: i64,
    event_type: String,
    aggregate_id: String,
    payload: serde_json::Value,
    correlation_id: String,
}

/// Process one batch of due deliveries. Returns the number attempted.
pub async fn dispatch_once(
    pool: &Db,
    registry: &SubscriberRegistry,
    config: &DispatcherConfig,
) -> anyhow::Result<usize> {
    let rows: Vec<DueRow> = sqlx::query_as(
        "select d.id as delivery_id, d.subscriber_name, d.attempts, \
                e.id as event_id, e.event_type, e.aggregate_id, e.payload, e.correlation_id \
         from outbox_delivery d \
         join outbox_event e on e.id = d.event_id \
         where d.status = 'pending' and d.next_attempt_at <= now() \
         order by d.id \
         limit $1",
    )
    .bind(config.batch_size)
    .fetch_all(pool)
    .await?;

    let attempted = rows.len();

    for row in rows {
        let delivered = DeliveredEvent {
            event_id: row.event_id,
            event_type: row.event_type,
            aggregate_id: row.aggregate_id,
            payload: row.payload,
            correlation_id: row.correlation_id.clone(),
        };

        let Some(subscriber) = registry.find(&row.subscriber_name) else {
            // No handler registered under this name (e.g. removed subscriber).
            tracing::warn!(name = %row.subscriber_name, "no subscriber registered; skipping");
            continue;
        };

        let span = tracing::info_span!(
            "event.handle",
            cid = %row.correlation_id,
            subscriber = subscriber.name(),
            event_type = %delivered.event_type,
        );

        let result = subscriber.handle(&delivered).instrument(span).await;

        match result {
            Ok(()) => {
                sqlx::query(
                    "update outbox_delivery set status = 'delivered', updated_at = now() \
                     where id = $1",
                )
                .bind(row.delivery_id)
                .execute(pool)
                .await?;
            }
            Err(e) => {
                mark_failure(pool, row.delivery_id, row.attempts, config.max_attempts, &e)
                    .await?;
            }
        }
    }

    Ok(attempted)
}

async fn mark_failure(
    pool: &Db,
    delivery_id: i64,
    attempts: i32,
    max_attempts: i32,
    err: &anyhow::Error,
) -> anyhow::Result<()> {
    let next_attempts = attempts + 1;
    if next_attempts >= max_attempts {
        sqlx::query(
            "update outbox_delivery \
             set status = 'dead', attempts = $2, last_error = $3, updated_at = now() \
             where id = $1",
        )
        .bind(delivery_id)
        .bind(next_attempts)
        .bind(err.to_string())
        .execute(pool)
        .await?;
        tracing::error!(delivery_id, error = %err, "delivery dead-lettered");
    } else {
        let backoff_secs = (2_i64.pow(next_attempts as u32)).min(300);
        sqlx::query(
            "update outbox_delivery \
             set attempts = $2, last_error = $3, \
                 next_attempt_at = now() + ($4 || ' seconds')::interval, updated_at = now() \
             where id = $1",
        )
        .bind(delivery_id)
        .bind(next_attempts)
        .bind(err.to_string())
        .bind(backoff_secs.to_string())
        .execute(pool)
        .await?;
        tracing::warn!(delivery_id, attempt = next_attempts, error = %err, "delivery failed; will retry");
    }
    Ok(())
}
```

`crates/platform/src/events/mod.rs` (add):
```rust
mod dispatcher;
pub use dispatcher::*;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p platform --test outbox_dispatch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/platform
git commit -m "feat(events): dispatcher delivers pending events with cid span"
```

---

### Task 5: Dispatcher retries + dead-lettering

**Files:**
- Modify: `crates/platform/tests/outbox_dispatch.rs` (add failure tests)

**Interfaces:**
- Consumes: `dispatch_once`, `mark_failure` behavior from Task 4.

- [ ] **Step 1: Write the failing tests**

Append to `crates/platform/tests/outbox_dispatch.rs`:
```rust
struct AlwaysFails;
#[async_trait::async_trait]
impl Subscriber for AlwaysFails {
    fn name(&self) -> &'static str { "always-fails" }
    fn event_type(&self) -> &'static str { "user.registered" }
    async fn handle(&self, _e: &DeliveredEvent) -> anyhow::Result<()> {
        anyhow::bail!("boom")
    }
}

#[sqlx::test]
async fn failing_delivery_dead_letters_after_max_attempts(pool: sqlx::PgPool) {
    let mut reg = SubscriberRegistry::new();
    reg.register(Arc::new(AlwaysFails));
    let reg = Arc::new(reg);

    let publisher = OutboxPublisher::new(Routes::new().add("user.registered", "always-fails"));
    let mut tx = pool.begin().await.unwrap();
    publisher
        .publish(&mut tx, NewEvent {
            event_type: "user.registered".into(),
            aggregate_id: "1".into(),
            payload: serde_json::json!({}),
            correlation_id: "cid".into(),
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // max_attempts = 2 so it dead-letters fast; reset next_attempt_at between runs.
    let config = DispatcherConfig { max_attempts: 2, batch_size: 50 };

    dispatch_once(&pool, &reg, &config).await.unwrap(); // attempt 1 -> retry scheduled
    sqlx::query("update outbox_delivery set next_attempt_at = now()")
        .execute(&pool).await.unwrap();
    dispatch_once(&pool, &reg, &config).await.unwrap(); // attempt 2 -> dead

    let row: (String, i32) = sqlx::query_as(
        "select status, attempts from outbox_delivery limit 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "dead");
    assert_eq!(row.1, 2);
}
```

- [ ] **Step 2: Run test to verify it fails or passes**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p platform --test outbox_dispatch`
Expected: PASS (the dead-letter logic from Task 4 already implements this — this task proves it with a test). If it FAILS, fix `mark_failure` until green.

- [ ] **Step 3: Commit**

```bash
git add crates/platform
git commit -m "test(events): cover retry backoff + dead-lettering"
```

---

### Task 6: DLQ inspection + replay helpers

**Files:**
- Create: `crates/platform/src/events/dlq.rs`
- Modify: `crates/platform/src/events/mod.rs`
- Test: `crates/platform/tests/outbox_dlq.rs`

**Interfaces:**
- Consumes: `Db`.
- Produces:
  - `pub struct DeadLetter { pub delivery_id: i64, pub subscriber_name: String, pub event_type: String, pub aggregate_id: String, pub payload: serde_json::Value, pub last_error: Option<String>, pub attempts: i32 }`
  - `pub async fn list_dead_letters(pool: &Db) -> anyhow::Result<Vec<DeadLetter>>`
  - `pub async fn replay_dead_letter(pool: &Db, delivery_id: i64) -> anyhow::Result<bool>` — resets a `dead` delivery to `pending` (attempts 0, due now); returns whether a row was updated.

- [ ] **Step 1: Write the failing test**

`crates/platform/tests/outbox_dlq.rs`:
```rust
use platform::events::{list_dead_letters, replay_dead_letter};

#[sqlx::test]
async fn list_and_replay_dead_letters(pool: sqlx::PgPool) {
    let event_id: i64 = sqlx::query_scalar(
        "insert into outbox_event (event_type, aggregate_id, payload, correlation_id) \
         values ('user.registered', '1', '{}'::jsonb, 'cid') returning id",
    )
    .fetch_one(&pool).await.unwrap();
    let delivery_id: i64 = sqlx::query_scalar(
        "insert into outbox_delivery (event_id, subscriber_name, status, attempts, last_error) \
         values ($1, 'sub', 'dead', 5, 'boom') returning id",
    )
    .bind(event_id)
    .fetch_one(&pool).await.unwrap();

    let dead = list_dead_letters(&pool).await.unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].delivery_id, delivery_id);
    assert_eq!(dead[0].last_error.as_deref(), Some("boom"));

    assert!(replay_dead_letter(&pool, delivery_id).await.unwrap());

    let status: String = sqlx::query_scalar(
        "select status from outbox_delivery where id = $1",
    )
    .bind(delivery_id)
    .fetch_one(&pool).await.unwrap();
    assert_eq!(status, "pending");
    assert!(list_dead_letters(&pool).await.unwrap().is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p platform --test outbox_dlq`
Expected: FAIL — functions not found.

- [ ] **Step 3: Write the implementation**

`crates/platform/src/events/dlq.rs`:
```rust
use crate::db::Db;

#[derive(Debug, sqlx::FromRow)]
pub struct DeadLetter {
    pub delivery_id: i64,
    pub subscriber_name: String,
    pub event_type: String,
    pub aggregate_id: String,
    pub payload: serde_json::Value,
    pub last_error: Option<String>,
    pub attempts: i32,
}

pub async fn list_dead_letters(pool: &Db) -> anyhow::Result<Vec<DeadLetter>> {
    let rows = sqlx::query_as::<_, DeadLetter>(
        "select d.id as delivery_id, d.subscriber_name, e.event_type, e.aggregate_id, \
                e.payload, d.last_error, d.attempts \
         from outbox_delivery d \
         join outbox_event e on e.id = d.event_id \
         where d.status = 'dead' \
         order by d.id desc",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn replay_dead_letter(pool: &Db, delivery_id: i64) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "update outbox_delivery \
         set status = 'pending', attempts = 0, last_error = null, \
             next_attempt_at = now(), updated_at = now() \
         where id = $1 and status = 'dead'",
    )
    .bind(delivery_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}
```

`crates/platform/src/events/mod.rs` (add):
```rust
mod dlq;
pub use dlq::*;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p platform --test outbox_dlq`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/platform
git commit -m "feat(events): DLQ listing + replay helpers"
```

---

### Task 7: Dispatcher background loop + wrap-up

**Files:**
- Modify: `crates/platform/src/events/dispatcher.rs`
- Modify: `crates/platform/src/events/mod.rs`

**Interfaces:**
- Consumes: `dispatch_once`, `DispatcherConfig`, `SubscriberRegistry`, `Db`.
- Produces:
  - `pub async fn run_dispatcher(pool: Db, registry: Arc<SubscriberRegistry>, config: DispatcherConfig, poll_interval: std::time::Duration)` — infinite loop: `dispatch_once` then sleep `poll_interval`. Logs and continues on transient errors. Intended to be `tokio::spawn`ed by the `app` crate (Plan 1c).

- [ ] **Step 1: Add the loop**

First, ensure these are imported at the TOP of `crates/platform/src/events/dispatcher.rs` (add any that are missing to the existing `use` block from Task 4):
```rust
use std::sync::Arc;
use std::time::Duration;
```

Then append to `crates/platform/src/events/dispatcher.rs`:
```rust
/// Long-running dispatcher: drain due deliveries, sleep, repeat.
pub async fn run_dispatcher(
    pool: Db,
    registry: Arc<SubscriberRegistry>,
    config: DispatcherConfig,
    poll_interval: Duration,
) {
    tracing::info!("outbox dispatcher started");
    loop {
        match dispatch_once(&pool, &registry, &config).await {
            Ok(n) if n > 0 => tracing::debug!(attempted = n, "dispatched batch"),
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "dispatch batch failed"),
        }
        tokio::time::sleep(poll_interval).await;
    }
}
```

- [ ] **Step 2: Verify it compiles and the suite is green**

Run: `cargo build -p platform`
Then: `DATABASE_URL=postgres://localhost/postgres cargo test -p platform`
Expected: PASS — all unit + outbox integration tests.

- [ ] **Step 3: Format + lint gate**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/platform
git commit -m "feat(events): background dispatcher loop; events module complete"
```

---

## Self-Review

**Spec coverage (against design §5):** outbox tables ✓ (Task 1), publish in same txn ✓ (Task 3, `&mut PgConnection`), fan-out one row per subscriber ✓ (Task 3 + test), dispatcher with cid span ✓ (Task 4), retries + backoff ✓ (Task 4/5), DLQ `status='dead'` + replay ✓ (Task 6), swappable publisher ✓ (`EventPublisher` trait), at-least-once + idempotency note ✓ (handlers must be idempotent — enforced by the account handler in Plan 1c).

**Placeholder scan:** no TBD/TODO; every code step is complete. The one caveat (`_Reg` alias) is explicitly flagged for deletion.

**Type consistency:** `NewEvent`/`DeliveredEvent`/`Subscriber`/`SubscriberRegistry` defined in Task 2 are used verbatim in Tasks 3–7. `EventPublisher::publish(&mut PgConnection, NewEvent) -> i64` signature in Task 3 matches the call in Plan 1c's account domain. `DispatcherConfig { max_attempts, batch_size }` consistent across Tasks 4/5/7. `dispatch_once`/`run_dispatcher`/`list_dead_letters`/`replay_dead_letter` names are stable.

**Cross-plan note:** Plan 1c registers the real subscribers (account) and spawns `run_dispatcher` from `app`. The `EventPublisher` trait object stored in app state is `Arc<dyn EventPublisher>` backed by `OutboxPublisher::new(routes)`. Because `OutboxPublisher` depends only on `Routes` (plain data), construction is linear and cycle-free: build `Routes` from static declarations → build the publisher → build subscribers (holding the publisher) → register subscribers in the `SubscriberRegistry` for the dispatcher. This is what removes the publisher↔subscriber cycle that a registry-backed publisher would create.
