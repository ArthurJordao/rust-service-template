# Runtime Hardening — Design (Production-readiness, Spec 1 of 2)

**Date:** 2026-06-30
**Status:** Approved design, ready for implementation planning
**Scope of this spec:** Make the running service survive load, slow/hung dependencies,
and deploys. Adds an HTTP middleware stack (request timeout, body-size limit, per-IP
rate limiting on auth routes), connection-level timeouts (DB pool + outbound HTTP),
graceful shutdown (drain HTTP + cooperatively stop the outbox consumers/reaper/pruner),
HTTP metrics wiring, and a `/readyz` readiness probe. **Out of scope (Spec 2 — "Build &
ship"):** Dockerfile, `[profile.release]`, `fly.toml`, CI, `cargo-audit`, the `.env.example`
JWT-key fix, and the `migrate` binary mode. Auth *feature* hardening (MFA, refresh-token
rotation, lockout, password policy, security headers) is a separate later spec.

This is the first of two production-readiness specs. Deployment target is **Fly.io**
(a PaaS); health checks, TLS, and rollout are the platform's job, so this spec builds
the portable primitives the platform wires to (`/readyz`, SIGTERM handling).

---

## 1. Goal & principles

A five-dimension gap analysis found the core (hexagonal domains, the reworked outbox,
correlation IDs, typed OpenAPI client, sqlx binds) is sound, but the **production shell**
around it is missing. This spec closes the "process falls over under load/failure/deploy"
gaps:

- No HTTP request timeout and no outbound `reqwest` timeout → one slow client or
  downstream hangs a Tokio task forever.
- DB pool configured only with `max_connections` → a runaway query holds a connection
  indefinitely and starves the service.
- No graceful shutdown → every Fly deploy drops in-flight requests and strands outbox
  rows in `processing` for 5 minutes.
- `record_http` exists but is never called → `/metrics` returns zeros.
- No rate limiting on auth routes → unbounded brute force.
- No body-size limit; `/status` is a static `"OK"` that passes even when the DB is down.

Invariants preserved:
- **Config is 12-factor** (`APP__*` env vars via the `config` crate). New knobs get sane
  defaults so the service runs without extra configuration.
- **Hexagonal layering / framework-light platform.** Middleware lives at the app/router
  composition layer; `crates/platform` gains config + pool + client + shutdown plumbing,
  no domain logic.
- **At-least-once delivery; idempotent handlers.** Graceful shutdown *reduces* redelivery
  (finishes the in-flight batch) but does not change the at-least-once contract.
- **sqlx runtime query API** (no `query!` macros).

---

## 2. Decisions (resolved during brainstorming)

1. **Deployment target: Fly.io.** Commit the portable primitives here; `fly.toml` health
   check + SIGTERM grace + migration release-command land in Spec 2.
2. **Rate limiting: in-process `tower-governor`, auth routes only.** Per-IP token bucket,
   keyed off the real client IP (`Fly-Client-IP`, fallback `X-Forwarded-For`) since the
   socket peer is Fly's proxy. Per-instance buckets are an accepted, documented limitation.
3. **SPA packaging unchanged** — `ServeDir` stays; the `web/dist` `COPY` is a Spec 2 concern.
4. **Graceful shutdown via `tokio_util::sync::CancellationToken`** (not a hand-rolled
   `watch` channel). Consumers/reaper/pruner take a token, check it at the top of each
   cycle, and **finish the current claimed batch before exiting** (no mid-batch abort →
   no stranded `processing` rows on a normal deploy).
5. **Reqwest timeouts are constants** (5s connect / 15s total), not config — outbound
   timeouts rarely need per-env tuning. Server + DB timeouts that *do* matter are config.
6. **Metrics path label uses `MatchedPath`** (route template, e.g. `/api/accounts/:id`),
   not the raw URI, to bound label cardinality. Add an `http_request_duration_seconds`
   histogram alongside the existing counter (same middleware touch).

---

## 3. HTTP middleware stack

Composed in `build_router` (`crates/app/src/state.rs`) via `tower::ServiceBuilder`.
Layer order (outermost first):

```
correlation_id  →  http_metrics  →  request_timeout  →  body_limit  →  cors  →  handlers
                                                          (rate_limit applied to the /api/auth sub-router only)
```

- **correlation-id outermost** so even a rejected (429/408/413) response carries a cid.
- **request timeout:** `tower_http::timeout::TimeoutLayer::new(Duration)` →
  `408 Request Timeout`. Default 30s, `APP__SERVER__REQUEST_TIMEOUT_SECONDS`.
- **body limit:** `axum::extract::DefaultBodyLimit::max(bytes)` → `413 Payload Too
  Large`. Default 1 MiB (1_048_576), `APP__SERVER__MAX_BODY_BYTES`.
- **rate limit:** `tower_governor` `GovernorLayer` applied to the auth sub-router
  (`/login`, `/register`, `/refresh`) only — not the whole API. Default ~10 req/min/IP
  (`APP__SERVER__AUTH_RATE_LIMIT_PER_MINUTE`, burst configurable) → `429 Too Many
  Requests`. **Key extractor** reads `Fly-Client-IP`, falling back to the leftmost
  `X-Forwarded-For` entry, falling back to the socket addr. (A custom
  `KeyExtractor` impl; `tower_governor`'s `SmartIpKeyExtractor` covers `X-Forwarded-For`/
  `X-Real-IP` but not Fly's header, so we wrap it.)

New deps: `tower-governor`. `tower` / `tower-http` (timeout feature) already present.

---

## 4. Connection-level timeouts

**DB pool** (`crates/platform/src/db.rs`, `PgPoolOptions`). New `DatabaseSettings`
fields with defaults:

| Setting | Default | Purpose |
|---|---|---|
| `min_connections` | 1 | avoid cold pool on first burst |
| `acquire_timeout_seconds` | 5 | waiting-for-connection fails fast (pairs with the 30s request timeout) |
| `idle_timeout_seconds` | 600 | recycle idle connections |
| `max_lifetime_seconds` | 1800 | recycle long-lived connections (survives DB failover) |
| `statement_timeout_ms` | 10000 | Postgres kills a runaway query, freeing the connection |
| `lock_timeout_ms` | 5000 | bound lock waits |

`statement_timeout` / `lock_timeout` are applied per connection via
`PgPoolOptions::after_connect`, issuing `SET statement_timeout = $ms; SET lock_timeout = $ms;`
(runtime query API). `max_connections` stays as today.

**Outbound HTTP** (`crates/platform/src/http_client.rs`). Replace
`reqwest::Client::new()` with a builder: `.connect_timeout(5s).timeout(15s)` (constants).

---

## 5. Graceful shutdown

Two coordinated halves, orchestrated in `crates/app/src/main.rs`.

**5.1 Shutdown signal.** A `shutdown_signal()` async fn resolving on **SIGTERM or
SIGINT** (`tokio::signal::unix::{signal, SignalKind}` for SIGTERM; `tokio::signal::ctrl_c`
or a SIGINT stream for local Ctrl-C). Used both for the HTTP drain and to trigger the
token.

**5.2 HTTP drain.** `axum::serve(listener, app).with_graceful_shutdown(shutdown_signal())`
— stops accepting new connections, lets in-flight requests finish.

**5.3 Cooperative background-task stop.** A single `tokio_util::sync::CancellationToken`
(new dep `tokio-util`) is cloned into every background loop:
- `run_consumers(pool, registry, dispatcher, reaper, token)` passes child/clone tokens to
  each `run_subscriber_loop` and `run_reaper`.
- `run_subscriber_loop` and `run_reaper` change shape to:
  ```
  loop {
      if token.is_cancelled() { break; }
      // ... one cycle (claim → handle → ack for the subscriber loop) ...
      tokio::select! {
          _ = token.cancelled() => break,
          _ = tokio::time::sleep(poll_interval) => {}
      }
  }
  ```
  The cancellation check is at the **top of the cycle**, so a cycle already in progress
  (a claimed batch) **runs to completion** — every claimed row is acked to
  `delivered`/`pending`/`dead` before the loop exits. No mid-batch abort ⇒ no rows left
  in `processing` on a normal deploy.
- The denylist pruner loop (`main.rs`) takes the same token and `select!`s its hourly
  sleep against it.

**5.4 Ordering in `main`.** signal received → server drains (5.2) → cancel the token →
await the consumers `JoinSet` + pruner handle with a **bounded 10s timeout**
(`tokio::time::timeout`); if exceeded, log and exit anyway → process exits `0`. This
matches Fly's SIGTERM-then-grace model (grace window set in `fly.toml`, Spec 2).

`run_consumers`'s current "return if any task exits" supervision still applies during
normal operation; on shutdown, all tasks exit cooperatively and the `JoinSet` drains.

---

## 6. Metrics wiring + readiness probe

**6.1 HTTP metrics middleware.** A middleware (e.g. `axum::middleware::from_fn_with_state`
holding `Metrics`) positioned in the stack to observe the final response status. For each
request it records:
- `http_requests_total{method, path, status}` (existing counter — finally called).
- `http_request_duration_seconds{method, path, status}` (new `HistogramVec`, default
  buckets) — measured around the inner service.

`path` is the **`axum::extract::MatchedPath`** route template (`/api/accounts/:id`), or
the literal `"unmatched"` when no route matches, to keep label cardinality bounded.
`Metrics` gains the histogram field + a `record_http_duration` method (or `record_http`
is extended to take latency).

**6.2 `/readyz` readiness probe.** New root route (beside `/status` + `/metrics`):
`GET /readyz` runs `SELECT 1` against the pool (bounded by `acquire_timeout`) → `200`
with `"ready"` on success, `503` on error. `/status` stays the static `"OK"` liveness
check. Fly's health check (Spec 2) targets `/readyz`.

---

## 7. Config additions (summary)

New `APP__*` env vars, all with defaults (service runs with none set):

- `APP__SERVER__REQUEST_TIMEOUT_SECONDS` (30)
- `APP__SERVER__MAX_BODY_BYTES` (1048576)
- `APP__SERVER__AUTH_RATE_LIMIT_PER_MINUTE` (10) + a burst size (e.g.
  `APP__SERVER__AUTH_RATE_LIMIT_BURST`, default 5)
- `APP__DATABASE__MIN_CONNECTIONS` (1)
- `APP__DATABASE__ACQUIRE_TIMEOUT_SECONDS` (5)
- `APP__DATABASE__IDLE_TIMEOUT_SECONDS` (600)
- `APP__DATABASE__MAX_LIFETIME_SECONDS` (1800)
- `APP__DATABASE__STATEMENT_TIMEOUT_MS` (10000)
- `APP__DATABASE__LOCK_TIMEOUT_MS` (5000)

`.env.example` documents each (Spec 2 also fixes the committed-JWT-key footgun in the
same file; this spec only appends the new keys).

---

## 8. Testing strategy

Integration tests (`#[sqlx::test]` where a DB is needed; axum `Router` + `tower::ServiceExt`
`oneshot` for middleware):

1. **Request timeout** — a handler that sleeps past the configured timeout returns `408`.
2. **Body limit** — a body over `max_body_bytes` returns `413`; one under passes.
3. **Rate limit** — N+1 rapid hits to an auth route from the same `Fly-Client-IP` return
   `429`; a different `Fly-Client-IP` is unaffected (verifies the key extractor reads the
   header, not the socket peer).
4. **DB `statement_timeout`** — a deliberately slow query (`select pg_sleep(...)`) is
   aborted by Postgres; **`acquire_timeout`** — with the pool saturated, a further acquire
   fails fast within the timeout.
5. **Graceful shutdown** — server drains an in-flight request to completion after the
   signal; a consumer cancelled mid-run finishes its claimed batch and leaves **zero rows
   in `processing`** (the key assertion); shutdown completes within the 10s cap.
6. **`/readyz`** — `200` when the DB is reachable; `503` when it is not (e.g. pool pointed
   at a closed/unreachable DB).
7. **Metrics middleware** — after a request, `http_requests_total` increments and
   `http_request_duration_seconds` observes a sample, both labeled with the **matched-path
   template** (assert `/api/accounts/:id`, not a concrete id).

---

## 9. Files touched (anticipated)

- `crates/platform/src/config.rs` — new `ServerSettings` + `DatabaseSettings` fields + defaults.
- `crates/platform/src/db.rs` — pool options + `after_connect` timeouts.
- `crates/platform/src/http_client.rs` — reqwest builder timeouts.
- `crates/platform/src/metrics.rs` — histogram + duration recording.
- `crates/platform/src/server.rs` — `/readyz` handler; possibly the metrics + key-extractor
  helpers (or a new small `middleware.rs` in platform).
- `crates/platform/src/events/dispatcher.rs` — `CancellationToken` in
  `run_consumers`/`run_subscriber_loop`/`run_reaper`.
- `crates/app/src/state.rs` — `build_router` middleware stack; mount `/readyz`.
- `crates/app/src/main.rs` — `shutdown_signal`, `with_graceful_shutdown`, token wiring,
  bounded shutdown await; pruner takes the token.
- `crates/platform/Cargo.toml` / root `Cargo.toml` — `tower-governor`, `tokio-util`
  (workspace deps).
- `.env.example` — document new keys.
- Tests across `crates/platform/tests` and `crates/app/tests`.
