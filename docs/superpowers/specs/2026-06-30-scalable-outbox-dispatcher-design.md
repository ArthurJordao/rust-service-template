# Scalable outbox dispatcher — Design

**Date:** 2026-06-30
**Status:** Approved design, ready for implementation planning
**Scope of this spec:** Rework the outbox dispatcher so deliveries are claimed
safely (no double-processing under concurrency or multiple instances) and handlers
can do real network IO without holding a database transaction open. Replaces the
single global polling loop with **one independent consumer loop per subscriber**
plus a **reaper** for crash recovery. No change to the publisher, the event/data
contract, or the DLQ.

---

## 1. Goal & principles

The current dispatcher (`crates/platform/src/events/dispatcher.rs`) is a single
`run_dispatcher` loop that `SELECT`s pending `outbox_delivery` rows and updates their
status *after* the handler runs, all against the connection pool. Two problems:

1. **Not safe to run concurrently.** With no row locking between the select and the
   later status update, two concurrent dispatchers (e.g. two app instances) both see
   the same pending rows and both run the handlers.
2. **Not safe for slow handlers.** There is no in-flight marker, so any move toward
   claim-and-release / locking would otherwise hold a transaction open across handler
   work.

**Driver (confirmed during brainstorming): build it right before IO arrives.** A
single instance is fine for now, but handlers will soon do real network IO (email
provider, external APIs). We want the architecture ready so it isn't retrofitted
under pressure. Horizontal-scale-safety comes along for free as the correct claim
primitive, even though only one instance runs today.

Invariants preserved:
- **At-least-once delivery; idempotent handlers.** Unchanged from today. The new
  reaper path can cause a redelivery, which is safe precisely because handlers are
  already required to be idempotent.
- **Hexagonal layering / ports as traits.** The `Subscriber` trait stays the seam;
  the new per-subscriber config is expressed on that trait.
- **sqlx runtime query API** (`sqlx::query`/`query_as`/`.bind`), no compile-time
  macros.
- **Outbox wiring stays linear and cycle-free** (`Routes → publisher → subscribers
  → registry → dispatcher`). The publisher is untouched.

**Non-goals (explicitly out of scope):**
- **Event ordering.** Confirmed not needed — events are independent one-shots per
  aggregate. The work queue gives no per-key ordering guarantee, and that is
  acceptable. (If a per-key ordering requirement ever appears, the future direction
  is a partition-key + per-partition lease applied selectively — *not* a switch to a
  Kafka-style consumer-offset model, which would cost the per-message retry/DLQ
  ergonomics this design keeps.)
- **Replacing Postgres with a broker.** The Postgres-as-queue model is deliberate
  (one datastore, transactional correctness, no extra infra), consistent with the
  repo's existing "Postgres-backed, no Redis/Kafka" decisions.

---

## 2. Decisions (resolved during brainstorming)

1. **Claim-and-release with a `processing` state.** Claim due rows in a short
   transaction (`FOR UPDATE SKIP LOCKED` → flip to `status='processing'` → commit),
   then handle them with **no transaction open**. The lock is held only for the
   few-millisecond claim, never across handler IO. This is what keeps Postgres
   healthy under slow handlers (no long transactions → no VACUUM/bloat stall, no
   connection-pool starvation).
2. **One consumer loop per subscriber (Approach 2).** Each registered subscriber is
   its own independent consumer with its own claim query (filtered by
   `subscriber_name`), batch size, concurrency, and poll cadence. A slow or failing
   subscriber cannot stall another. Chosen over a single grouped-dispatch loop for
   maximum isolation; the per-subscriber config makes the message-by-message
   capability natural.
3. **Per-subscriber processing config on the `Subscriber` trait**, with a default so
   existing subscribers need no change. Message-by-message = `concurrency: 1`.
4. **Reaper for crash recovery.** A periodic task returns rows stuck in `processing`
   (worker crashed mid-flight) back to `pending`. Visibility timeout is
   **configurable, default 5 minutes**; a reclaim **bumps `attempts`** so a poison
   message that crashes (rather than errors) is still eventually dead-lettered.
5. **Migrations kept clean.** The claim index is folded directly into
   `0001_outbox.sql` (this is a template with no production data) rather than added
   as a new migration. Requires a DB reset for anyone with an existing dev DB.

---

## 3. Status state machine

`outbox_delivery.status` gains one state, `processing` (today: `pending` /
`delivered` / `dead`):

```
pending ──claim──▶ processing ──handler Ok──▶ delivered (terminal)
   ▲                   │
   │                   ├─ handler Err, attempts+1 <  max ──▶ pending (backoff via next_attempt_at)
   │                   ├─ handler Err, attempts+1 >= max ──▶ dead (terminal, DLQ)
   └─ reaper (stale processing, attempts+1) ───────────────┘   (or ──▶ dead at max)
```

- **Claim** (one transaction):
  ```sql
  select d.id as delivery_id, d.subscriber_name, d.attempts,
         e.id as event_id, e.event_type, e.aggregate_id, e.payload, e.correlation_id
  from outbox_delivery d
  join outbox_event e on e.id = d.event_id
  where d.subscriber_name = $1 and d.status = 'pending' and d.next_attempt_at <= now()
  order by d.id
  limit $2
  for update of d skip locked
  ```
  followed by `update outbox_delivery set status='processing', updated_at=now() where
  id = any($claimed_ids)`, then `COMMIT`. `FOR UPDATE OF d` locks only the claimed
  delivery rows; `SKIP LOCKED` makes a second claimer skip them and take the next
  free rows — no contention, no double-grab.
- **Handle** happens after commit, no transaction open, under
  `buffer_unordered(concurrency)`.
- **Ack** is a single-row update reusing the existing logic: `delivered` on success;
  on error, `pending` with bumped attempts + `next_attempt_at = now() + backoff`
  (`2^attempts` capped at 300s), or `dead` once attempts reach `max_attempts` (5).

`processing` is the in-flight marker that makes claim-and-release safe and gives the
reaper something to find.

---

## 4. Schema / migration

Folded into `0001_outbox.sql` (no new migration; reset the dev DB):

- Replace the existing `outbox_delivery (status, next_attempt_at)` index with a claim
  index whose leading column is the subscriber:
  ```sql
  create index outbox_delivery_claim_idx
      on outbox_delivery (subscriber_name, status, next_attempt_at);
  ```
  This serves the per-subscriber claim query directly. The reaper's
  `where status='processing' and updated_at < …` scan is acceptable against this
  index (or a small partial index can be added if it ever matters; not needed now).

No new columns: `updated_at` already exists and is the reaper's freshness signal
(set on every claim and every ack).

---

## 5. Per-subscriber consumer config

A new method on the `Subscriber` trait returns a small config struct, defaulted so
existing subscribers compile unchanged:

```rust
pub struct ConsumerConfig {
    pub batch_size: i64,       // rows claimed per cycle
    pub concurrency: usize,    // handlers run at once; 1 = message-by-message (serial, id order)
    pub poll_interval: Duration,
}

impl Default for ConsumerConfig {
    fn default() -> Self {
        ConsumerConfig {
            batch_size: 10,
            concurrency: 5,
            poll_interval: Duration::from_secs(2),
        }
    }
}

// on the Subscriber trait:
fn consumer_config(&self) -> ConsumerConfig {
    ConsumerConfig::default()
}
```

- **Message-by-message subscribers** override to `{ concurrency: 1, .. }` — claim a
  batch, handle strictly one at a time in `id` order. A subscriber wanting minimal
  in-flight rows can also set `batch_size: 1`.
- **Default** stays batched-and-concurrent. `concurrency` must be `<= batch_size`
  in effect (claiming fewer than `concurrency` rows just runs fewer in parallel).

---

## 6. The consumer loop

`run_subscriber_loop(pool, subscriber, max_attempts)` — one per subscriber:

1. Read the subscriber's `consumer_config()`.
2. **Claim** up to `batch_size` due rows (the §3 transaction). If none, sleep
   `poll_interval` and repeat.
3. **Handle** the claimed rows under `buffer_unordered(concurrency)`
   (`concurrency: 1` ⇒ strictly serial in `id` order). Each handle runs inside the
   existing `event.handle` tracing span (cid / subscriber / event_type).
4. **Ack** each row individually (`delivered` / `pending`+backoff / `dead`), reusing
   the current `mark_failure` logic.
5. If the batch was full, loop immediately (drain under load); otherwise sleep
   `poll_interval`.

The exponential backoff and `max_attempts → dead` behavior are reused verbatim from
today's dispatcher.

---

## 7. Reaper & crash-recovery semantics

One periodic task (`run_reaper`), independent of the subscriber loops:

A single statement reclaims stale rows, bumping `attempts` and dead-lettering in the
same update when the bumped count reaches `max_attempts` (`$2`):

```sql
update outbox_delivery
set attempts = attempts + 1,
    status = case when attempts + 1 >= $2 then 'dead' else 'pending' end,
    last_error = 'reclaimed: processing timed out',
    next_attempt_at = now(),
    updated_at = now()
where status = 'processing'
  and updated_at < now() - ($1 || ' seconds')::interval
```

Two decisions, both confirmed:

1. **Visibility timeout: configurable, default 5 minutes.** Must exceed the slowest
   expected handler. Documented as: raise this only as a deliberate exception — a
   handler routinely exceeding 5 minutes is the real smell (push slow work into the
   handler differently, or split the event), not a reason to crank the timeout.
2. **A reclaim bumps `attempts`.** This bounds a *poison message that crashes the
   worker* (rather than returning `Err`): without bumping, such a row would be
   reclaimed and re-crash forever. The cost — a legitimately-slow handler could be
   reclaimed mid-flight and double-run — is safe because handlers are already
   required to be idempotent (at-least-once).

Config: `ReaperConfig { visibility_timeout: Duration, poll_interval: Duration }`
(default `visibility_timeout: 300s`, `poll_interval: 30s`), wired in the composition
root alongside the consumer config.

---

## 8. App wiring (`crates/app`)

- Replace the single `run_dispatcher` spawn (`main.rs`) with a `run_consumers(pool,
  registry, reaper_config)` helper that:
  - walks the `SubscriberRegistry`, spawning one `run_subscriber_loop` task per
    subscriber (each reading its own `consumer_config()`), and
  - spawns the `run_reaper` task.
- `state.rs::dispatcher_handle` is replaced by a `consumers_handle`-style accessor
  exposing pool + registry + reaper config.
- `main.rs`'s `tokio::select!` watches `server` + the consumers handle (`JoinSet`) +
  the existing denylist pruner (unchanged).

---

## 9. Backward compatibility

- **Publisher unchanged** — still writes one `outbox_event` row + one
  `outbox_delivery` row per interested subscriber. Wire/data contract untouched.
- **DLQ unchanged** — `dead` is still the terminal failure state; `dlq.rs` /
  `dlq_http.rs` and their admin routes keep working. `dead` is now reachable via both
  the handler-error path and the reaper path.
- **Existing tests:** `outbox_publish.rs` is unaffected; `outbox_dispatch.rs` moves
  from "call `dispatch_once` over all rows" to driving a single subscriber's
  claim+handle cycle; `outbox_dlq.rs` keeps asserting the same `dead`-lettering.

---

## 10. Testing strategy

New behaviors pinned down with `#[sqlx::test]` integration tests:

1. **Claim isolation** — two concurrent claims for the same subscriber never grab the
   same row (`SKIP LOCKED`); each gets a disjoint set.
2. **Claim-and-release** — a row flips `pending → processing` on claim and
   `processing → delivered` on success; the claim transaction commits before handling
   (no lock held during the handler).
3. **Per-subscriber concurrency=1** — rows handled strictly in `id` order; `>1`
   overlaps.
4. **Failure path** — handler error returns the row to `pending` with bumped attempts
   + future `next_attempt_at`; at `max_attempts` it goes `dead` (existing behavior).
5. **Reaper** — a row left in `processing` past the visibility timeout is reset to
   `pending` with attempts bumped; a fresh `processing` row (within timeout) is left
   alone; repeated reclaims eventually `dead`-letter.
6. **Cross-subscriber isolation** — a failing/slow subscriber does not block another
   subscriber's deliveries of the same event.

---

## 11. Observability

- Per-subscriber structured logs already exist (the `event.handle` span carries
  `cid` / `subscriber` / `event_type`).
- Add reaper logs: `warn!` per reclaim with `delivery_id` + `subscriber`.
- If cheap via the existing metrics module, add counters for
  claimed/delivered/failed/dead/reclaimed by subscriber. Kept light — not a new
  subsystem.
