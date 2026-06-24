# rust-service-template — Design (Spec 1)

**Date:** 2026-06-24
**Status:** Approved design, ready for implementation planning
**Scope of this spec:** Backend walking skeleton — workspace + `platform` crate + `domain-account` as a complete vertical slice. Later domains/frontend are separate spec/plan cycles (see Roadmap).

---

## 1. Goal & guiding principles

Port the architecture *principles* of the `haskell-service-template` to **idiomatic Rust**, as a **single monolith broken into internal domains** (rather than separate microservices). This is a reusable template the author will seed future products from (e.g. a "find a psychologist" marketplace + a practitioner management system).

Principles carried over from the Haskell template:

- **Hexagonal / ports-and-adapters** — domain logic is pure and framework-free; HTTP, DB, and events live in adapters.
- **Capability isolation** — domains cannot reach into each other's internals; they communicate only through public interfaces and events.
- **Cross-cutting concerns in a shared lib** — auth, correlation-id logging, metrics, db, events, http client.
- **Correlation-id structured logging** end-to-end, including across asynchronous event handling.

Explicit non-goal: a literal transliteration of Haskell's `RIO` Reader + `HasX` typeclass machinery. We translate the *spirit* (testable ports, least-privilege dependencies) into idiomatic Rust.

### Stack

| Concern | Choice | Replaces (Haskell) |
|---|---|---|
| Async runtime | `tokio` | — |
| Web framework | `axum` + `tower`/`tower-http` | Servant |
| DB | `sqlx` (Postgres, compile-time-checked SQL, built-in migrations) | Persistent |
| Config | `config` crate + `serde` | `envy` / `Settings.hs` |
| Logging/tracing | `tracing` + `tracing-subscriber` (JSON) | `Service.Logging` + `CorrelationId` |
| Errors | `thiserror` (domain) + central `AppError: IntoResponse` | `jsonErrorFormatters` |
| HTTP client | `reqwest` (cid header injection) | `Service.HttpClient` |
| Metrics | Prometheus exporter | `Service.Metrics` |
| Eventing | **Transactional outbox**, in-process dispatch | Kafka |
| Auth | `jsonwebtoken` / `jose`-style JWT verify | `Service.Auth` |

---

## 2. Architecture decisions (resolved during brainstorming)

1. **Monolith, not microservices.** One binary; domains are internal modules with enforced boundaries.
2. **Cargo workspace, crate per domain.** Boundaries are compile-time enforced (a domain physically cannot import another domain's internals). Closest analog to Haskell's package-per-service isolation; makes "lift a domain into its own service later" near-mechanical.
3. **Ports as traits + trait objects (`Arc<dyn …>`), concrete `AppState` as composition root.** Domain logic depends only on the port traits it needs (least privilege). Adapters implement the ports. Trait objects (dynamic dispatch) chosen over generics for readability and gentler compiler errors. This is the idiomatic-Rust translation of the `HasX` capability pattern.
4. **Messaging = transactional outbox with in-process dispatch.** Chosen for durability without running a broker. Requirements: **multiple consumers per event (fan-out)** and **a DLQ**. The `EventPublisher` is a trait, so a Kafka-backed impl can replace the outbox later behind the same call sites.
5. **Correlation IDs via `tracing` spans.** Set once per request by a tower layer; propagated implicitly through the async task tree. The cid is persisted on the outbox row so asynchronous event handlers re-establish the same cid span.
6. **Frontend = React SPA (Vite + TS + Tailwind + shadcn), separate `web/` app, consuming JSON APIs.** Admin is a protected route group `/admin/*` inside that one app — not a separate app. SSR (Next.js/Remix) is deliberately **out of the template**; it is the documented branch for SEO-critical public surfaces (e.g. the patient-facing marketplace) and is added per-product against the same APIs. Decision driven by the author's concrete future products (marketplace + dashboard) needing rich interactivity (booking/calendars), shared components across audiences, likely mobile reuse, and SEO on the public marketplace.

---

## 3. Workspace layout

```
rust-service-template/
  Cargo.toml                 # [workspace] manifest
  crates/
    platform/                # shared lib  ← analog of haskell-service-lib
    domain-account/          # first domain (vertical slice)
    app/                     # binary: config, wiring, runs server + dispatcher
  migrations/                # sqlx migrations (workspace-level)
  scripts/new-domain.sh      # scaffolder ← analog of new-service.sh
  docker-compose.yml         # Postgres + Prometheus + Grafana (Loki/Promtail later)
  Makefile                   # run / test / migrate / new-domain
  docs/superpowers/specs/    # design docs
```

Dependency graph (acyclic): `domain-account → platform`; `app → domain-account + platform`. Nothing depends back on `app`.

---

## 4. `platform` crate (shared lib)

One module per cross-cutting concern, mirroring `Service/*`:

| Module | Responsibility | Haskell analog |
|---|---|---|
| `config` | env-based settings via `serde` + `config`; fails loudly on missing required vars | `Settings.hs` / `envy` |
| `db` | `PgPool` creation, `sqlx::migrate!` runner, `run_in_txn` helper | `Service.Database` |
| `observability` | `tracing` JSON subscriber init, **correlation-id tower layer**, dispatcher span helper | `Logging` + `CorrelationId` |
| `events` | the **outbox**: `EventPublisher` port, `OutboxPublisher`, dispatcher, subscriber registry (fan-out, retries, DLQ) | `Service.Kafka` + DLQ service |
| `auth` | JWT **verification**, `AuthClaims` axum extractor, `require_scope` guard | `Service.Auth` (verify side) |
| `http_client` | `reqwest` wrapper injecting the current span's cid as an outgoing header | `Service.HttpClient` |
| `metrics` | Prometheus registry, `/metrics` handler, http + db metric hooks | `Service.Metrics` |
| `server` | shared axum builder, CORS layer (`tower-http`), `AppError: IntoResponse` JSON errors, `/status` | `Service.Server` + `Cors` |
| `metadata` | `created_at` / `created_by_cid` injection on insert | `Service.Persist` / `Metadata` |

### Error handling

- Domains define `thiserror` enums (e.g. `AccountError::NotFound`, `::Forbidden`).
- `platform::server::AppError` is the central HTTP error type implementing axum's `IntoResponse`, producing JSON bodies with appropriate status codes (replaces `jsonErrorFormatters`). Domain errors convert into `AppError` at the adapter boundary.

### Correlation-id logging (detail)

| Haskell today | Rust (`tracing`) |
|---|---|
| `correlationIdMiddleware` | tower layer reads `X-Correlation-Id` header (or generates one), opens a request span |
| `HasLogContext` map in `App` | span **fields**: `info_span!("request", %cid)` |
| `logInfoC "msg"` | `tracing::info!("msg")` — cid inherited from the active span |
| `withUtcLogFunc` JSON logs | `tracing-subscriber` JSON formatter |
| `produceMessageWithCid` | cid written onto the outbox row (`correlation_id` column) |
| consumer re-attaches cid | dispatcher opens a span from the row's `correlation_id` before invoking handlers |

Result: cid flows **request → outbox row → dispatcher → handler** as one continuous trace.

---

## 5. The outbox (durability + fan-out + DLQ)

### Tables

- **`outbox_event`** — `id`, `event_type`, `aggregate_id`, `payload` (jsonb), `correlation_id`, `created_at`
- **`outbox_delivery`** — `id`, `event_id` → `outbox_event`, `subscriber_name`, `status` (`pending` | `delivered` | `dead`), `attempts`, `last_error`, `next_attempt_at`, `created_at`, `updated_at`
  - **One row per (event × subscriber)** — this is what enables fan-out and independent per-consumer tracking.

### Flow

1. A domain calls `publish(event)` **inside the same `sqlx` transaction** as its state change. This inserts one `outbox_event` row plus one `pending` `outbox_delivery` row **per subscriber registered for that event type**. Same-transaction write ⇒ no lost events, no dual-write problem.
2. A background **dispatcher** task polls `outbox_delivery` rows where `status = 'pending'` and `next_attempt_at <= now()`. For each: open a span from the event's `correlation_id`, invoke that subscriber's handler.
3. Success → `delivered`. Failure → `attempts += 1`, set exponential-backoff `next_attempt_at`; once `attempts >= max_attempts` → `dead`.

### Requirements satisfied

- **Multiple consumers (fan-out):** independent delivery row per subscriber; one failing/slow consumer never blocks the others.
- **DLQ:** `status = 'dead'` rows *are* the dead-letter queue — queryable (payload + last error) and replayable (reset to `pending`). The admin UI for this is Spec 3, mirroring the current `admin-ui` DLQ page.
- **Durability:** same-transaction write; on crash/restart the dispatcher resumes from unprocessed `pending` rows.
- **At-least-once + idempotency:** handlers must be idempotent (the account handler checks for an existing account first, mirroring `processUserRegistered`).
- **Swappability:** `EventPublisher` is a trait; a Kafka-backed implementation can replace the outbox later without touching domain call sites.

---

## 6. `domain-account` crate (vertical slice)

```
domain-account/src/
  domain.rs        # pure logic: list_accounts, get_account (+scope check), process_user_registered
  models.rs        # Account, NewAccount
  ports/
    repository.rs  # AccountRepository trait + PostgresAccountRepository (sqlx)
    http.rs        # axum handlers: GET /status, /accounts, /accounts/:id (JWT+scope), /metrics, external-post example
    events.rs      # emits AccountCreated; subscribes to UserRegistered -> process_user_registered
  lib.rs           # public interface: router(), register_subscribers(), exported event types
```

- **Domain logic is framework-free** — no axum, no SQL — exactly like `Domain/Accounts.hs`.
- **Authorization** mirrors the Haskell `AccessPolicy`: a caller may read an account if it carries the `admin` scope, or owns the account (`read:accounts:own` scope and subject matches `user-{id}`).
- **Ports are traits**; `PostgresAccountRepository` is the sqlx adapter. Domain functions take `&dyn AccountRepository` / `&dyn EventPublisher` (least privilege).
- **End-to-end exercise without auth domain:** Spec 1 includes a temporary dev-only `POST /dev/register` that publishes a `UserRegistered` event. The account subscriber consumes it → idempotently creates an account → emits `AccountCreated`. This proves the full outbox loop. The dev endpoint is **replaced by the real `domain-auth` producer in Spec 2.**
- **External HTTP example** (`GET /external/posts/:id` against jsonplaceholder) is carried over to exercise `platform::http_client`, mirroring the Haskell template.

---

## 7. `app` crate (binary)

Startup sequence (analog of `App.hs`/`Lib.hs`):

1. Load config (`platform::config`).
2. Init tracing subscriber (`platform::observability`).
3. Build `PgPool`; optionally run migrations when `DB_AUTO_MIGRATE=true`.
4. Construct `AppState` holding the real resources as `Arc<dyn …>` adapters (repository, event publisher, http client, metrics, JWT config).
5. Register every domain's routers + event subscribers.
6. Spawn the outbox **dispatcher** task.
7. Run the axum server. Server and dispatcher run concurrently via `tokio::select!` (analog of Haskell's `race_`).

Middleware/layer order (outermost → inner): CORS → correlation-id/trace → metrics → routes.

---

## 8. Testing

- **Domain unit tests** — inject a fake in-memory `AccountRepository` and a recording `EventPublisher`; no DB. Fast, pure. Covers authorization logic and `process_user_registered` idempotency.
- **Integration tests** — `spawn_app` pattern (à la `zero2prod`): real Postgres, an ephemeral database per test, hit HTTP, assert DB + outbox rows. Includes an outbox test: publish → dispatcher delivers → assert `delivered`; force handler failures → assert retry/backoff → `dead`.

---

## 9. Tooling / infra

- **`docker-compose.yml`**: Postgres + Prometheus + Grafana. (Loki/Promtail deferred to a later spec to match the staged rollout.)
- **`Makefile`** targets: `run`, `test`, `migrate`, `new-domain`.
- **`scripts/new-domain.sh`**: scaffolds a new domain crate from the `domain-account` shape (analog of `new-service.sh`).

---

## 10. Roadmap (beyond Spec 1)

| Spec | Scope |
|---|---|
| **1 (this doc)** | Workspace skeleton + `platform` crate (db, outbox w/ fan-out + DLQ, tracing/cid, auth verification, metrics, http client, server) + `domain-account` JSON vertical slice + tests |
| **2** | `domain-auth` — JWT **issuance**: register / login / refresh / logout, Redis token revocation, admin scopes. Becomes the real producer of `UserRegistered`, replacing the dev stand-in |
| **3** | `web/` — **React SPA** (Vite + TS + Tailwind + shadcn) consuming JSON APIs: login, account views, and `/admin/*` route group (DLQ inspect/replay, users) |
| **4+** | `domain-notification` and further domains via `new-domain.sh` |
| **Templatize** (after a working slice) | Turn the repo into a reusable project template via `cargo generate` — see §11 |
| **API schema / typed TS client** (future, open) | OpenAPI + generated TypeScript client for frontend↔backend schema guarantees — see §12 |

Known future branch (not template scope): an **SSR** frontend (Next.js/Remix) for SEO-critical public surfaces (e.g. the patient-facing "find a psychologist" marketplace), hitting the same JSON APIs. The API-first backend means this is additive and changes nothing server-side.

---

## 11. Templating (reusable project base)

**Decision:** use **`cargo generate`** as the template mechanism (the idiomatic Rust approach: prompts for a project name and substitutes placeholders across `Cargo.toml`s, the app/binary name, docker-compose DB name, `.env.example`, etc.).

**Approach — start with (B), revisit (A) later:**
- **(B) Working-reference + rename (now):** keep concrete names (`rust-service-template`, `domain-account`) so the repo always `cargo build`s and stays a real, runnable reference. Generation renames the known strings via a small `cargo generate` config / post-generation hook. Chosen because a template you can also just run and see working is more valuable while the project is young.
- **(A) Template-first (later, optional):** once the system is fully working and stable, optionally migrate to native `{{project-name}}` placeholders. The repo would then exist to be generated (working reference moves to a generated instance or a `reference` branch).

**Sequencing:** templating does **not** block Spec 1. Build the concrete working version first; add the template layer as a separate step once a slice runs end-to-end. The B→A switch later is cheap and reversible.

**Likely generate-time parameters:** project/workspace name, app binary name, database name, default port, CORS origin; optional yes/no prompt to include the example `domain-account` or start from a bare placeholder domain.

## 12. API schema / typed TypeScript client (future spec — open)

**Goal:** compile-time schema guarantees between the React frontend and the Rust services — calling a wrong path, passing the wrong params, or mismatching a request/response body should be a TypeScript error.

**Status: open / not committed.** To be designed in its own brainstorm → spec → plan cycle. Current lean is the **OpenAPI route** over plain type-sharing, because the goal is about *calling* endpoints (the whole contract), not just sharing types:

- **Leaning toward `utoipa`** (annotate axum handlers + DTOs → generate an OpenAPI doc; `app` aggregates each domain's paths/schemas into one `/openapi.json`) paired on the frontend with `openapi-typescript` (types) + `openapi-fetch` (tiny typed client). Bonus: Swagger UI for free. Cost: annotation boilerplate on handlers/DTOs.
- **Alternative considered:** `ts-rs` (derive `TS` to emit `.ts` interfaces) — lighter, but shares only *types*, not endpoint contracts, so it does not catch wrong-path/wrong-param calls.

**Decision to make in that spec:** utoipa/OpenAPI (full typed client) vs ts-rs (shared types only); whether to bake annotations in incrementally or retrofit; and the `make gen-types` regeneration workflow. Keep the open question flagged: the Spec-1 DTOs are few, so retrofitting utoipa later is inexpensive.
