# Scalable Outbox Dispatcher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single global outbox dispatcher loop with one independent per-subscriber consumer loop that claims deliveries via `FOR UPDATE SKIP LOCKED` into a `processing` state (so handlers run outside the DB transaction), plus a reaper that recovers rows orphaned by a crashed worker.

**Architecture:** Each registered `Subscriber` gets its own polling loop with its own batch size, concurrency, and poll interval (read from a new `consumer_config()` trait method). A claim is a short transaction: `SELECT … FOR UPDATE SKIP LOCKED` → flip claimed rows to `status='processing'` → commit. Handlers then run with no transaction open and ack each row to `delivered`/`pending`+backoff/`dead`. A reaper periodically returns stale `processing` rows to `pending` (bumping attempts, dead-lettering at the cap).

**Tech Stack:** Rust (stable, edition 2024 toolchain), tokio, sqlx (runtime query API, Postgres), async-trait, futures (`buffer_unordered`), anyhow, tracing.

## Global Constraints

- **sqlx runtime query API only** (`sqlx::query`, `query_as`, `query_scalar`, `.bind`) — NEVER the compile-time `query!`/`query_as!` macros. The crate must build without a live DB.
- **At-least-once delivery; handlers MUST be idempotent.** The reaper can cause a redelivery; this is safe only because of idempotency.
- **`crates/platform` is framework-light infra** — no axum/domain types leak into the dispatcher.
- **Before every commit:** `cargo fmt --all` and `cargo clippy --all-targets -- -D warnings` must both be clean.
- **Integration tests** use `#[sqlx::test(migrations = "../../migrations")]` (provisions an isolated DB per test; needs `DATABASE_URL` pointing at a role with CREATEDB).
- **Migrations stay clean:** the claim index is folded into `0001_outbox.sql` (no new migration). Anyone with an existing dev DB must reset it (`make` drop/recreate or `sqlx database drop && create`) before tests pass.
- **Keep the outbox wiring linear and cycle-free** (`Routes → publisher → subscribers → registry → consumers`). The publisher is NOT modified.

---

## File Structure

- `migrations/0001_outbox.sql` — **modify**: replace the `(status, next_attempt_at)` index with a `(subscriber_name, status, next_attempt_at)` claim index.
- `crates/platform/src/events/types.rs` — **modify**: add `ConsumerConfig` struct + `Default`; add `consumer_config()` default method to the `Subscriber` trait; add `SubscriberRegistry::subscribers()` accessor.
- `crates/platform/src/events/dispatcher.rs` — **modify**: reshape `DispatcherConfig` (drop `batch_size`, add `Clone`); add `ReaperConfig`; make `DueRow` public; add `claim_batch`, `ack_delivered`, `dispatch_subscriber_once`, `run_subscriber_loop`; add `reap_stale`, `run_reaper`; add `run_consumers`; delete `dispatch_once`/`run_dispatcher` at the end.
- `crates/platform/tests/outbox_claim.rs` — **create**: claim isolation + processing-flip tests.
- `crates/platform/tests/outbox_subscriber.rs` — **create**: `dispatch_subscriber_once` delivery / ordering / failure-to-dead tests.
- `crates/platform/tests/outbox_reaper.rs` — **create**: reaper reset / leave-fresh / dead-letter-at-cap tests.
- `crates/platform/tests/outbox_loop.rs` — **create**: `run_subscriber_loop` drains pending then idles.
- `crates/platform/tests/outbox_dispatch.rs` — **delete** at the end (superseded by `outbox_subscriber.rs`).
- `crates/app/src/state.rs` — **modify**: replace `dispatcher_handle` with `consumers_handle`.
- `crates/app/src/main.rs` — **modify**: spawn `run_consumers` instead of `run_dispatcher`.

Unchanged: `publisher.rs`, `dlq.rs`, `dlq_http.rs`, `outbox_publish.rs`, `outbox_dlq.rs`, the two domain `Subscriber` impls (they inherit the default `consumer_config()`).

---

## Task 1: Config types + `Subscriber::consumer_config()`

Add the new per-subscriber config and reshape the dispatcher/reaper config, without removing anything the old `dispatch_once` still needs (it keeps working until Task 6).

**Files:**
- Modify: `crates/platform/src/events/types.rs`
- Modify: `crates/platform/src/events/dispatcher.rs`

**Interfaces:**
- Produces:
  - `ConsumerConfig { batch_size: i64, concurrency: usize, poll_interval: std::time::Duration }` with `Default` (`batch_size: 10, concurrency: 5, poll_interval: 2s`), `#[derive(Debug, Clone)]`.
  - `Subscriber::consumer_config(&self) -> ConsumerConfig` (default method returning `ConsumerConfig::default()`).
  - `DispatcherConfig { max_attempts: i32 }` now `#[derive(Debug, Clone)]`, `Default` = `max_attempts: 5`. **`batch_size` removed.**
  - `ReaperConfig { visibility_timeout: Duration, poll_interval: Duration }`, `#[derive(Debug, Clone)]`, `Default` = `visibility_timeout: 300s, poll_interval: 30s`.

> NOTE: removing `batch_size` from `DispatcherConfig` will break `dispatch_once` and `outbox_dispatch.rs`, which still reference it. To keep the build green, in this task **keep `batch_size` on `DispatcherConfig` for now** and only drop it in Task 6 when `dispatch_once` is deleted. So Task 1 adds `Clone` + the new types but leaves `DispatcherConfig`'s fields alone.

- [ ] **Step 1: Write the failing test** (append to `crates/platform/src/events/types.rs`, inside the existing `#[cfg(test)] mod tests`)

```rust
    #[test]
    fn consumer_config_defaults_and_override() {
        let d = ConsumerConfig::default();
        assert_eq!(d.batch_size, 10);
        assert_eq!(d.concurrency, 5);
        assert_eq!(d.poll_interval, std::time::Duration::from_secs(2));

        // A subscriber inherits the default unless it overrides.
        assert_eq!(Dummy.consumer_config().concurrency, 5);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p platform --lib consumer_config_defaults_and_override`
Expected: FAIL — `cannot find type ConsumerConfig` / no method `consumer_config`.

- [ ] **Step 3: Add `ConsumerConfig` and the trait method** in `crates/platform/src/events/types.rs`

Add the struct near the top (after the `use` line):

```rust
/// Per-subscriber processing knobs. Returned by `Subscriber::consumer_config`.
/// `concurrency: 1` means message-by-message (serial, in `id` order).
#[derive(Debug, Clone)]
pub struct ConsumerConfig {
    pub batch_size: i64,
    pub concurrency: usize,
    pub poll_interval: std::time::Duration,
}

impl Default for ConsumerConfig {
    fn default() -> Self {
        ConsumerConfig {
            batch_size: 10,
            concurrency: 5,
            poll_interval: std::time::Duration::from_secs(2),
        }
    }
}
```

Add the default method to the `Subscriber` trait (after `async fn handle`):

```rust
    /// How this subscriber's consumer loop claims and processes deliveries.
    /// Override to opt into message-by-message (`concurrency: 1`) or to tune
    /// batch size / poll cadence. Defaults are batched-and-concurrent.
    fn consumer_config(&self) -> ConsumerConfig {
        ConsumerConfig::default()
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p platform --lib consumer_config_defaults_and_override`
Expected: PASS.

- [ ] **Step 5: Add `ReaperConfig` and `Clone` on `DispatcherConfig`** in `crates/platform/src/events/dispatcher.rs`

Change the `DispatcherConfig` derive/struct (keep `batch_size` for now — removed in Task 6):

```rust
#[derive(Debug, Clone)]
pub struct DispatcherConfig {
    pub max_attempts: i32,
    pub batch_size: i64,
}
```

Add `ReaperConfig` right below `DispatcherConfig`'s `Default` impl:

```rust
/// How the reaper reclaims rows stuck in `processing` (worker crashed mid-flight).
#[derive(Debug, Clone)]
pub struct ReaperConfig {
    /// A `processing` row older than this is assumed orphaned and returned to the
    /// queue. Configurable; default 5 min. Raise only as a deliberate exception —
    /// a handler routinely exceeding it is the real smell.
    pub visibility_timeout: Duration,
    pub poll_interval: Duration,
}

impl Default for ReaperConfig {
    fn default() -> Self {
        ReaperConfig {
            visibility_timeout: Duration::from_secs(300),
            poll_interval: Duration::from_secs(30),
        }
    }
}
```

- [ ] **Step 6: Verify the crate builds**

Run: `cargo build -p platform`
Expected: builds clean (no references broken).

- [ ] **Step 7: fmt + clippy + commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add crates/platform/src/events/types.rs crates/platform/src/events/dispatcher.rs
git commit -m "feat(outbox): add ConsumerConfig + ReaperConfig + Subscriber::consumer_config"
```

---

## Task 2: Claim index + `claim_batch`

Add the per-subscriber claim that locks rows with `SKIP LOCKED` and flips them to `processing` in one short transaction.

**Files:**
- Modify: `migrations/0001_outbox.sql`
- Modify: `crates/platform/src/events/dispatcher.rs`
- Test: `crates/platform/tests/outbox_claim.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `pub struct DueRow` (make the existing struct public, with `pub` fields) — fields: `delivery_id: i64, subscriber_name: String, attempts: i32, event_id: i64, event_type: String, aggregate_id: String, payload: serde_json::Value, correlation_id: String`.
  - `pub async fn claim_batch(pool: &Db, subscriber_name: &str, batch_size: i64) -> anyhow::Result<Vec<DueRow>>` — claims due `pending` rows for `subscriber_name`, flips them to `processing`, returns them.

- [ ] **Step 1: Fold the claim index into the migration** — edit `migrations/0001_outbox.sql`, replacing the existing index at the bottom:

```sql
create index outbox_delivery_claim_idx
    on outbox_delivery (subscriber_name, status, next_attempt_at);
```

(Delete the old `create index outbox_delivery_due_idx on outbox_delivery (status, next_attempt_at);` line.)

- [ ] **Step 2: Reset the dev DB so the changed migration applies**

Run (adjust `DATABASE_URL` to your local Postgres):
```bash
sqlx database drop -y && sqlx database create
```
Expected: succeeds. (`#[sqlx::test]` provisions its own per-test DBs from `./migrations`, so this is just for any persistent dev DB.)

- [ ] **Step 3: Write the failing test** — create `crates/platform/tests/outbox_claim.rs`

```rust
use platform::events::claim_batch;

async fn insert_event(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar(
        "insert into outbox_event (event_type, aggregate_id, payload, correlation_id) \
         values ('e', '1', '{}'::jsonb, 'cid') returning id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn claim_flips_to_processing_and_does_not_reclaim(pool: sqlx::PgPool) {
    let event_id = insert_event(&pool).await;
    for _ in 0..4 {
        sqlx::query("insert into outbox_delivery (event_id, subscriber_name) values ($1, 's')")
            .bind(event_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    let first = claim_batch(&pool, "s", 2).await.unwrap();
    assert_eq!(first.len(), 2);
    let second = claim_batch(&pool, "s", 2).await.unwrap();
    assert_eq!(second.len(), 2);

    // The two claims are disjoint — a claimed (processing) row is never re-handed-out.
    let f: Vec<i64> = first.iter().map(|r| r.delivery_id).collect();
    let s: Vec<i64> = second.iter().map(|r| r.delivery_id).collect();
    assert!(f.iter().all(|id| !s.contains(id)), "claims overlapped: {f:?} vs {s:?}");

    // All four are now processing, so a further claim sees nothing.
    assert!(claim_batch(&pool, "s", 10).await.unwrap().is_empty());
    let processing: i64 =
        sqlx::query_scalar("select count(*) from outbox_delivery where status = 'processing'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(processing, 4);
}

#[sqlx::test(migrations = "../../migrations")]
async fn claim_is_scoped_to_the_subscriber(pool: sqlx::PgPool) {
    let event_id = insert_event(&pool).await;
    sqlx::query("insert into outbox_delivery (event_id, subscriber_name) values ($1, 'a')")
        .bind(event_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into outbox_delivery (event_id, subscriber_name) values ($1, 'b')")
        .bind(event_id)
        .execute(&pool)
        .await
        .unwrap();

    let claimed = claim_batch(&pool, "a", 10).await.unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].subscriber_name, "a");
}
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo test -p platform --test outbox_claim`
Expected: FAIL — `claim_batch` not found / `DueRow` private.

- [ ] **Step 5: Make `DueRow` public and add `claim_batch`** in `crates/platform/src/events/dispatcher.rs`

Change the struct definition:

```rust
#[derive(sqlx::FromRow)]
pub struct DueRow {
    pub delivery_id: i64,
    pub subscriber_name: String,
    pub attempts: i32,
    pub event_id: i64,
    pub event_type: String,
    pub aggregate_id: String,
    pub payload: serde_json::Value,
    pub correlation_id: String,
}
```

Add the claim function (after the `DueRow` struct):

```rust
/// Claim up to `batch_size` due deliveries for one subscriber, flipping them to
/// `processing` in a single short transaction. `FOR UPDATE … SKIP LOCKED` makes a
/// concurrent claimer skip these rows and take the next free ones — no double-grab.
/// The lock is released on commit, BEFORE any handler runs.
pub async fn claim_batch(
    pool: &Db,
    subscriber_name: &str,
    batch_size: i64,
) -> anyhow::Result<Vec<DueRow>> {
    let mut tx = pool.begin().await?;
    let rows: Vec<DueRow> = sqlx::query_as(
        "select d.id as delivery_id, d.subscriber_name, d.attempts, \
                e.id as event_id, e.event_type, e.aggregate_id, e.payload, e.correlation_id \
         from outbox_delivery d \
         join outbox_event e on e.id = d.event_id \
         where d.subscriber_name = $1 and d.status = 'pending' and d.next_attempt_at <= now() \
         order by d.id \
         limit $2 \
         for update of d skip locked",
    )
    .bind(subscriber_name)
    .bind(batch_size)
    .fetch_all(&mut *tx)
    .await?;

    if !rows.is_empty() {
        let ids: Vec<i64> = rows.iter().map(|r| r.delivery_id).collect();
        sqlx::query(
            "update outbox_delivery set status = 'processing', updated_at = now() \
             where id = any($1)",
        )
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(rows)
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p platform --test outbox_claim`
Expected: PASS (both tests).

- [ ] **Step 7: fmt + clippy + commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add migrations/0001_outbox.sql crates/platform/src/events/dispatcher.rs crates/platform/tests/outbox_claim.rs
git commit -m "feat(outbox): claim_batch with FOR UPDATE SKIP LOCKED + processing state"
```

---

## Task 3: `dispatch_subscriber_once` (claim → handle → ack)

The per-subscriber replacement for `dispatch_once`: claim a batch, handle it honoring the subscriber's `concurrency`, ack each row. Reuses the existing `mark_failure` and extracts a small `ack_delivered`.

**Files:**
- Modify: `crates/platform/src/events/dispatcher.rs`
- Test: `crates/platform/tests/outbox_subscriber.rs`

**Interfaces:**
- Consumes: `claim_batch` (Task 2), `DispatcherConfig` (Task 1), `Subscriber::consumer_config` (Task 1), existing `mark_failure`.
- Produces:
  - `async fn ack_delivered(pool: &Db, delivery_id: i64) -> anyhow::Result<()>`.
  - `pub async fn dispatch_subscriber_once(pool: &Db, subscriber: &dyn Subscriber, config: &DispatcherConfig) -> anyhow::Result<usize>` — returns the number of deliveries attempted in this cycle.

- [ ] **Step 1: Write the failing test** — create `crates/platform/tests/outbox_subscriber.rs`

```rust
use platform::events::{
    dispatch_subscriber_once, ConsumerConfig, DeliveredEvent, DispatcherConfig, EventPublisher,
    NewEvent, OutboxPublisher, Routes, Subscriber,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<i64>>>);
#[async_trait::async_trait]
impl Subscriber for Recorder {
    fn name(&self) -> &'static str {
        "recorder"
    }
    fn event_type(&self) -> &'static str {
        "user.registered"
    }
    async fn handle(&self, e: &DeliveredEvent) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(e.event_id);
        Ok(())
    }
    fn consumer_config(&self) -> ConsumerConfig {
        // Serial so the recorded order is deterministic (id order).
        ConsumerConfig { concurrency: 1, ..ConsumerConfig::default() }
    }
}

async fn publish(pool: &sqlx::PgPool, agg: &str) {
    let publisher = OutboxPublisher::new(Routes::new().add("user.registered", "recorder"));
    let mut tx = pool.begin().await.unwrap();
    publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "user.registered".into(),
                aggregate_id: agg.into(),
                payload: serde_json::json!({}),
                correlation_id: "cid".into(),
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn delivers_pending_in_order_and_marks_delivered(pool: sqlx::PgPool) {
    let rec = Recorder::default();
    publish(&pool, "1").await;
    publish(&pool, "2").await;
    publish(&pool, "3").await;

    let n = dispatch_subscriber_once(&pool, &rec, &DispatcherConfig::default())
        .await
        .unwrap();
    assert_eq!(n, 3);

    let recorded = rec.0.lock().unwrap().clone();
    assert_eq!(recorded.len(), 3);
    let mut sorted = recorded.clone();
    sorted.sort();
    assert_eq!(recorded, sorted, "concurrency=1 should deliver in id order");

    let delivered: i64 =
        sqlx::query_scalar("select count(*) from outbox_delivery where status = 'delivered'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(delivered, 3);
}

struct AlwaysFails;
#[async_trait::async_trait]
impl Subscriber for AlwaysFails {
    fn name(&self) -> &'static str {
        "recorder"
    }
    fn event_type(&self) -> &'static str {
        "user.registered"
    }
    async fn handle(&self, _e: &DeliveredEvent) -> anyhow::Result<()> {
        anyhow::bail!("boom")
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn failing_delivery_dead_letters_after_max_attempts(pool: sqlx::PgPool) {
    publish(&pool, "1").await;
    let config = DispatcherConfig { max_attempts: 2, batch_size: 50 };

    dispatch_subscriber_once(&pool, &AlwaysFails, &config).await.unwrap(); // attempt 1 -> retry
    sqlx::query("update outbox_delivery set next_attempt_at = now()")
        .execute(&pool)
        .await
        .unwrap();
    dispatch_subscriber_once(&pool, &AlwaysFails, &config).await.unwrap(); // attempt 2 -> dead

    let row: (String, i32) =
        sqlx::query_as("select status, attempts from outbox_delivery limit 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "dead");
    assert_eq!(row.1, 2);
}
```

> NOTE: `DispatcherConfig { max_attempts: 2, batch_size: 50 }` still has `batch_size` here because Task 1 kept it. Task 6 removes the field and updates this literal to `DispatcherConfig { max_attempts: 2 }`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p platform --test outbox_subscriber`
Expected: FAIL — `dispatch_subscriber_once` not found.

- [ ] **Step 3: Add `ack_delivered` and `dispatch_subscriber_once`** in `crates/platform/src/events/dispatcher.rs`

Add `use futures::stream::StreamExt;` to the imports at the top of the file. Add the functions (after `claim_batch`):

```rust
async fn ack_delivered(pool: &Db, delivery_id: i64) -> anyhow::Result<()> {
    sqlx::query("update outbox_delivery set status = 'delivered', updated_at = now() where id = $1")
        .bind(delivery_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Run one claim-and-handle cycle for a single subscriber. Claims up to the
/// subscriber's `batch_size`, handles the batch under `buffer_unordered(concurrency)`
/// (concurrency 1 = strictly serial, in id order), and acks each row. Handlers run
/// with NO transaction open. Returns the number of deliveries attempted.
pub async fn dispatch_subscriber_once(
    pool: &Db,
    subscriber: &dyn Subscriber,
    config: &DispatcherConfig,
) -> anyhow::Result<usize> {
    let cfg = subscriber.consumer_config();
    let rows = claim_batch(pool, subscriber.name(), cfg.batch_size).await?;
    let attempted = rows.len();

    futures::stream::iter(rows)
        .map(|row| async move {
            let delivered = DeliveredEvent {
                event_id: row.event_id,
                event_type: row.event_type,
                aggregate_id: row.aggregate_id,
                payload: row.payload,
                correlation_id: row.correlation_id.clone(),
            };

            let span = tracing::info_span!(
                "event.handle",
                cid = %row.correlation_id,
                subscriber = subscriber.name(),
                event_type = %delivered.event_type,
            );

            match subscriber.handle(&delivered).instrument(span).await {
                Ok(()) => {
                    if let Err(e) = ack_delivered(pool, row.delivery_id).await {
                        tracing::error!(delivery_id = row.delivery_id, error = %e, "ack delivered failed");
                    } else {
                        tracing::info!(
                            delivery_id = row.delivery_id,
                            subscriber = %row.subscriber_name,
                            event_type = %delivered.event_type,
                            "delivery delivered"
                        );
                    }
                }
                Err(e) => {
                    if let Err(e2) = mark_failure(
                        pool,
                        row.delivery_id,
                        &row.subscriber_name,
                        &delivered.event_type,
                        row.attempts,
                        config.max_attempts,
                        &e,
                    )
                    .await
                    {
                        tracing::error!(delivery_id = row.delivery_id, error = %e2, "mark_failure failed");
                    }
                }
            }
        })
        .buffer_unordered(cfg.concurrency.max(1))
        .collect::<Vec<()>>()
        .await;

    Ok(attempted)
}
```

- [ ] **Step 4: Confirm `futures` is a dependency of `platform`**

Run: `grep -n '^futures' crates/platform/Cargo.toml`
Expected: a `futures = …` line. If absent, add `futures = "0.3"` under `[dependencies]` in `crates/platform/Cargo.toml` and commit it with this task.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p platform --test outbox_subscriber`
Expected: PASS (both tests).

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add crates/platform/src/events/dispatcher.rs crates/platform/tests/outbox_subscriber.rs crates/platform/Cargo.toml
git commit -m "feat(outbox): dispatch_subscriber_once with per-subscriber concurrency"
```

---

## Task 4: Reaper (`reap_stale` + `run_reaper`)

Reclaim rows orphaned in `processing` by a crashed worker; bump attempts; dead-letter at the cap.

**Files:**
- Modify: `crates/platform/src/events/dispatcher.rs`
- Test: `crates/platform/tests/outbox_reaper.rs`

**Interfaces:**
- Consumes: `ReaperConfig`, `DispatcherConfig` (Task 1), `Db`.
- Produces:
  - `pub async fn reap_stale(pool: &Db, visibility_timeout: Duration, max_attempts: i32) -> anyhow::Result<u64>` — returns rows reclaimed.
  - `pub async fn run_reaper(pool: Db, config: ReaperConfig, max_attempts: i32)` — long-running loop.

- [ ] **Step 1: Write the failing test** — create `crates/platform/tests/outbox_reaper.rs`

```rust
use platform::events::reap_stale;
use std::time::Duration;

async fn insert_processing(pool: &sqlx::PgPool, attempts: i32, age_minutes: i64) -> i64 {
    let event_id: i64 = sqlx::query_scalar(
        "insert into outbox_event (event_type, aggregate_id, payload, correlation_id) \
         values ('e', '1', '{}'::jsonb, 'cid') returning id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "insert into outbox_delivery (event_id, subscriber_name, status, attempts, updated_at) \
         values ($1, 's', 'processing', $2, now() - ($3 || ' minutes')::interval) returning id",
    )
    .bind(event_id)
    .bind(attempts)
    .bind(age_minutes.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn reaper_resets_stale_and_leaves_fresh(pool: sqlx::PgPool) {
    let stale = insert_processing(&pool, 0, 10).await; // older than 5 min
    let fresh = insert_processing(&pool, 0, 0).await; // within timeout

    let n = reap_stale(&pool, Duration::from_secs(300), 5).await.unwrap();
    assert_eq!(n, 1);

    let stale_row: (String, i32) =
        sqlx::query_as("select status, attempts from outbox_delivery where id = $1")
            .bind(stale)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stale_row, ("pending".to_string(), 1));

    let fresh_status: String =
        sqlx::query_scalar("select status from outbox_delivery where id = $1")
            .bind(fresh)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(fresh_status, "processing");
}

#[sqlx::test(migrations = "../../migrations")]
async fn reaper_dead_letters_at_max_attempts(pool: sqlx::PgPool) {
    let id = insert_processing(&pool, 4, 10).await; // attempts 4, max 5 -> bumped to 5 -> dead

    let n = reap_stale(&pool, Duration::from_secs(300), 5).await.unwrap();
    assert_eq!(n, 1);

    let row: (String, i32) =
        sqlx::query_as("select status, attempts from outbox_delivery where id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row, ("dead".to_string(), 5));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p platform --test outbox_reaper`
Expected: FAIL — `reap_stale` not found.

- [ ] **Step 3: Add `reap_stale` and `run_reaper`** in `crates/platform/src/events/dispatcher.rs`

```rust
/// Reclaim deliveries stuck in `processing` past the visibility timeout (worker
/// crashed mid-flight). Bumps attempts so a row that reliably crashes the worker is
/// still eventually dead-lettered. Redelivery is safe because handlers are idempotent.
pub async fn reap_stale(
    pool: &Db,
    visibility_timeout: Duration,
    max_attempts: i32,
) -> anyhow::Result<u64> {
    let secs = visibility_timeout.as_secs() as i64;
    let result = sqlx::query(
        "update outbox_delivery \
         set attempts = attempts + 1, \
             status = case when attempts + 1 >= $2 then 'dead' else 'pending' end, \
             last_error = 'reclaimed: processing timed out', \
             next_attempt_at = now(), updated_at = now() \
         where status = 'processing' \
           and updated_at < now() - ($1 || ' seconds')::interval",
    )
    .bind(secs.to_string())
    .bind(max_attempts)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Long-running reaper: sweep stale `processing` rows, sleep, repeat.
pub async fn run_reaper(pool: Db, config: ReaperConfig, max_attempts: i32) {
    tracing::info!("outbox reaper started");
    loop {
        match reap_stale(&pool, config.visibility_timeout, max_attempts).await {
            Ok(n) if n > 0 => tracing::warn!(reclaimed = n, "reaped stale processing deliveries"),
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "reaper sweep failed"),
        }
        tokio::time::sleep(config.poll_interval).await;
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p platform --test outbox_reaper`
Expected: PASS (both tests).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add crates/platform/src/events/dispatcher.rs crates/platform/tests/outbox_reaper.rs
git commit -m "feat(outbox): reaper reclaims stale processing deliveries"
```

---

## Task 5: `run_subscriber_loop`, `run_consumers`, registry accessor

The long-running per-subscriber loop and the supervisor that spawns one loop per registered subscriber plus the reaper.

**Files:**
- Modify: `crates/platform/src/events/types.rs`
- Modify: `crates/platform/src/events/dispatcher.rs`
- Test: `crates/platform/tests/outbox_loop.rs`

**Interfaces:**
- Consumes: `dispatch_subscriber_once`, `run_reaper`, `DispatcherConfig`, `ReaperConfig`, `Subscriber::consumer_config`.
- Produces:
  - `SubscriberRegistry::subscribers(&self) -> Vec<Arc<dyn Subscriber>>` (clones).
  - `pub async fn run_subscriber_loop(pool: Db, subscriber: Arc<dyn Subscriber>, config: DispatcherConfig)`.
  - `pub async fn run_consumers(pool: Db, registry: Arc<SubscriberRegistry>, dispatcher: DispatcherConfig, reaper: ReaperConfig)` — spawns all loops + reaper, returns if any task exits.

- [ ] **Step 1: Add the registry accessor** in `crates/platform/src/events/types.rs` (inside `impl SubscriberRegistry`)

```rust
    /// All registered subscribers (clones of the `Arc`s) — one consumer loop per entry.
    pub fn subscribers(&self) -> Vec<Arc<dyn Subscriber>> {
        self.subscribers.clone()
    }
```

- [ ] **Step 2: Write the failing test** — create `crates/platform/tests/outbox_loop.rs`

```rust
use platform::events::{
    run_subscriber_loop, ConsumerConfig, DeliveredEvent, DispatcherConfig, EventPublisher,
    NewEvent, OutboxPublisher, Routes, Subscriber,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

struct FastRecorder(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl Subscriber for FastRecorder {
    fn name(&self) -> &'static str {
        "recorder"
    }
    fn event_type(&self) -> &'static str {
        "user.registered"
    }
    async fn handle(&self, _e: &DeliveredEvent) -> anyhow::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn consumer_config(&self) -> ConsumerConfig {
        ConsumerConfig { poll_interval: Duration::from_millis(50), ..ConsumerConfig::default() }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn loop_drains_pending(pool: sqlx::PgPool) {
    let count = Arc::new(AtomicUsize::new(0));
    let sub: Arc<dyn Subscriber> = Arc::new(FastRecorder(count.clone()));

    let publisher = OutboxPublisher::new(Routes::new().add("user.registered", "recorder"));
    let mut tx = pool.begin().await.unwrap();
    publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "user.registered".into(),
                aggregate_id: "1".into(),
                payload: serde_json::json!({}),
                correlation_id: "cid".into(),
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let handle = tokio::spawn(run_subscriber_loop(pool.clone(), sub, DispatcherConfig::default()));

    // Poll up to ~3s for the loop to process the one delivery.
    let mut delivered = false;
    for _ in 0..30 {
        if count.load(Ordering::SeqCst) == 1 {
            delivered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    handle.abort();
    assert!(delivered, "loop did not drain the pending delivery");
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p platform --test outbox_loop`
Expected: FAIL — `run_subscriber_loop` not found.

- [ ] **Step 4: Add `run_subscriber_loop` and `run_consumers`** in `crates/platform/src/events/dispatcher.rs`

```rust
/// Long-running consumer loop for ONE subscriber: claim → handle → ack, then sleep
/// `poll_interval`. Drains immediately (no sleep) when a full batch was claimed.
pub async fn run_subscriber_loop(pool: Db, subscriber: Arc<dyn Subscriber>, config: DispatcherConfig) {
    let cfg = subscriber.consumer_config();
    let batch_size = cfg.batch_size as usize;
    tracing::info!(subscriber = subscriber.name(), "consumer loop started");
    loop {
        match dispatch_subscriber_once(&pool, subscriber.as_ref(), &config).await {
            Ok(n) if n >= batch_size && batch_size > 0 => continue,
            Ok(_) => {}
            Err(e) => tracing::error!(subscriber = subscriber.name(), error = %e, "dispatch cycle failed"),
        }
        tokio::time::sleep(cfg.poll_interval).await;
    }
}

/// Spawn one consumer loop per registered subscriber, plus the reaper. Returns if
/// any task exits (loops never return normally, so that signals a problem).
pub async fn run_consumers(
    pool: Db,
    registry: Arc<SubscriberRegistry>,
    dispatcher: DispatcherConfig,
    reaper: ReaperConfig,
) {
    let max_attempts = dispatcher.max_attempts;
    let mut set = tokio::task::JoinSet::new();
    for sub in registry.subscribers() {
        set.spawn(run_subscriber_loop(pool.clone(), sub, dispatcher.clone()));
    }
    set.spawn(run_reaper(pool, reaper, max_attempts));
    if set.join_next().await.is_some() {
        tracing::error!("a consumer task exited unexpectedly");
    }
}
```

Add `use crate::events::SubscriberRegistry;` to the imports if not already covered by the existing `use crate::events::{...}` line (it currently imports `DeliveredEvent, SubscriberRegistry` — confirm `SubscriberRegistry` is present; it is).

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p platform --test outbox_loop`
Expected: PASS.

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add crates/platform/src/events/types.rs crates/platform/src/events/dispatcher.rs crates/platform/tests/outbox_loop.rs
git commit -m "feat(outbox): per-subscriber consumer loops + run_consumers supervisor"
```

---

## Task 6: Wire into `app`, retire `dispatch_once`

Switch the composition root to `run_consumers`, then delete the old global dispatcher and the now-superseded test, and drop `batch_size` from `DispatcherConfig`.

**Files:**
- Modify: `crates/app/src/state.rs`
- Modify: `crates/app/src/main.rs`
- Modify: `crates/platform/src/events/dispatcher.rs`
- Modify: `crates/platform/tests/outbox_subscriber.rs`
- Delete: `crates/platform/tests/outbox_dispatch.rs`

**Interfaces:**
- Consumes: `run_consumers`, `DispatcherConfig`, `ReaperConfig` (Tasks 1 & 5).
- Produces: `state::consumers_handle(res: &Resources) -> (Db, Arc<SubscriberRegistry>)`.

- [ ] **Step 1: Replace `dispatcher_handle` with `consumers_handle`** in `crates/app/src/state.rs`

Remove the `dispatcher_handle` function (currently returns `(Db, Arc<SubscriberRegistry>, DispatcherConfig, Duration)`) and add:

```rust
pub fn consumers_handle(res: &Resources) -> (Db, Arc<SubscriberRegistry>) {
    (res.pool.clone(), res.registry.clone())
}
```

Update the `use platform::events::{…}` block: remove `DispatcherConfig` if no longer referenced in `state.rs` (it isn't, after this change) and remove the now-unused `Duration` import if nothing else uses it (check: `Duration` may be unused after removing `dispatcher_handle` — remove `use std::time::Duration;` if so).

- [ ] **Step 2: Switch `main.rs` to `run_consumers`** in `crates/app/src/main.rs`

Change the import line:

```rust
use platform::events::{run_consumers, DispatcherConfig, ReaperConfig};
```

Replace the dispatcher spawn block:

```rust
    let (pool, registry) = state::consumers_handle(&res);
    let consumers = tokio::spawn(run_consumers(
        pool,
        registry,
        DispatcherConfig::default(),
        ReaperConfig::default(),
    ));
```

Update the `tokio::select!` arm:

```rust
    tokio::select! {
        r = server => { r?; }
        _ = consumers => { tracing::error!("consumers exited unexpectedly"); }
        _ = pruner => { tracing::error!("prune task exited unexpectedly"); }
    }
```

- [ ] **Step 3: Build the workspace to confirm wiring compiles**

Run: `cargo build --workspace`
Expected: builds. (`dispatch_once`/`run_dispatcher` still exist and are now unused — that's fine until Step 5.)

- [ ] **Step 4: Drop `batch_size` from `DispatcherConfig`** in `crates/platform/src/events/dispatcher.rs`

```rust
#[derive(Debug, Clone)]
pub struct DispatcherConfig {
    pub max_attempts: i32,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        DispatcherConfig { max_attempts: 5 }
    }
}
```

Then update the literal in `crates/platform/tests/outbox_subscriber.rs`:

```rust
    let config = DispatcherConfig { max_attempts: 2 };
```

- [ ] **Step 5: Delete the old global dispatcher and its test**

In `crates/platform/src/events/dispatcher.rs`, delete the `dispatch_once` function and the `run_dispatcher` function entirely (the per-subscriber loop + reaper replace them). Keep `mark_failure` (still used by `dispatch_subscriber_once`).

Delete the superseded test file:
```bash
git rm crates/platform/tests/outbox_dispatch.rs
```

- [ ] **Step 6: Run the full platform + app test suites**

Run: `cargo test -p platform && cargo test -p app`
Expected: PASS. (`outbox_publish`, `outbox_dlq`, `outbox_claim`, `outbox_subscriber`, `outbox_reaper`, `outbox_loop` all green; no reference to `dispatch_once`.)

- [ ] **Step 7: Full workspace verification**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
Expected: fmt clean, clippy clean (no `-D warnings` failures, no dead-code warnings for removed functions), all tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/app/src/state.rs crates/app/src/main.rs crates/platform/src/events/dispatcher.rs crates/platform/tests/outbox_subscriber.rs
git rm crates/platform/tests/outbox_dispatch.rs
git commit -m "feat(outbox): wire per-subscriber consumers into app; retire dispatch_once"
```

---

## Task 7: Docs

Update the agent guide so the dispatcher description matches reality.

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update the platform bullet in `CLAUDE.md`**

In the `crates/platform` description, change the outbox phrase from "publish / dispatcher / retries / DLQ" to reflect the new model, e.g.:

> the transactional **outbox** (publish / **per-subscriber consumer loops** claiming with `FOR UPDATE SKIP LOCKED` into a `processing` state so handlers run outside the DB transaction / retries / **reaper** for crash recovery / DLQ + `dlq_http` admin routes)

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: describe per-subscriber outbox consumers + reaper in agent guide"
```

---

## Self-Review Notes (coverage vs. spec)

- **Spec §2 (claim-and-release + processing):** Tasks 2 (`claim_batch`) + 3 (`dispatch_subscriber_once`). ✅
- **Spec §2 (per-subscriber loops, Approach 2):** Task 5 (`run_subscriber_loop` + `run_consumers`). ✅
- **Spec §3 (status state machine):** `processing` introduced (Task 2), `delivered`/`pending`+backoff/`dead` acks reuse `mark_failure` (Task 3), reaper path (Task 4). ✅
- **Spec §4 (schema/index in 0001):** Task 2 Step 1–2. ✅
- **Spec §5 (`ConsumerConfig` on trait):** Task 1. ✅
- **Spec §6 (the loop):** Task 5. ✅
- **Spec §7 (reaper: 5-min configurable timeout, reclaim bumps attempts, dead at cap):** Tasks 1 (`ReaperConfig`) + 4 (`reap_stale`). ✅
- **Spec §8 (app wiring):** Task 6. ✅
- **Spec §9 (backward compat — publisher/DLQ unchanged, tests migrated):** Task 6 (delete `outbox_dispatch.rs`, keep `outbox_publish.rs`/`outbox_dlq.rs`). ✅
- **Spec §10 (testing):** claim isolation (Task 2), claim-and-release/delivered (Tasks 2–3), concurrency=1 ordering (Task 3), failure→dead (Task 3), reaper reset/leave-fresh/dead-at-cap (Task 4), loop drains (Task 5). ✅
- **Spec §11 (observability):** reaper `warn!`/`info!` logs + existing `event.handle` span retained (Tasks 3–5). **Per-subscriber metrics counters are deliberately deferred** — the platform dispatch functions don't currently receive a `Metrics` handle, so threading it through is more than the "if cheap" the spec scoped; logging covers the observability need for now. (Documented decision, not a gap.)
