# rust-service-template — agent guide

## What this repo is

An idiomatic-Rust service template: a **monolith of internal domains** (not
microservices) with a transactional outbox for events, correlation-id tracing,
JWT auth, and Prometheus metrics. It is the Rust reinterpretation of a Haskell
microservices template — see the design doc for the full rationale.

## Current state

Specs 1–4 **plus** correlation-id/logging and the utoipa typed client are all
**implemented and merged to `main`** (full Rust + web test suites green;
`cargo clippy -D warnings` + fmt clean). What exists today:

- `crates/platform` — config (env + `.env` via dotenvy, key-file JWT config), db
  (Postgres pool + migrations), the transactional **outbox** (publish / **per-subscriber
  consumer loops** that claim with `FOR UPDATE SKIP LOCKED` into a `processing` state so
  handlers run outside the DB transaction / retries / a **reaper** that reclaims stale
  `processing` rows after a crash / DLQ + `dlq_http` admin routes), `auth` (RS256 JWT verify, `Authenticated`
  extractor, scope guard, `RevocationChecker` port), metrics, http_client,
  observability (**hierarchical correlation IDs** — `new_segment`/`append`, request
  middleware appends per hop + access log, `RUST_LOG`), server (`AppError`, CORS).
- `crates/domain-account` — `/accounts`, `/accounts/:id`, `/accounts/me`; consumes
  `user.registered`, emits `account.created`.
- `crates/domain-auth` — register / login / refresh / logout (bcrypt, RS256 issuance),
  **Postgres-backed token revocation (no Redis)**, admin scope management; the real
  producer of `user.registered`.
- `crates/domain-notification` — consumes `account.created`, renders a handlebars
  welcome template (strict mode), dispatches via a `Notifier` port (dev `LogNotifier`),
  records `sent_notification` idempotently; admin `GET /notifications`. Depends only on
  `platform` (re-derives the event payload locally — no `domain-account` dep).
- `crates/app` — composition root (**lib + bin**): builds resources, assembles the
  router via `build_router` (API nested under `/api`, `/status`+`/metrics` at root,
  serves the SPA from `web/dist` with an SPA fallback; serves `/api/openapi.json` +
  Swagger UI at `/swagger-ui`), runs server + outbox consumers (per-subscriber loops +
  reaper) + denylist prune task.
  Ships an `openapi-gen` bin that prints the merged OpenAPI doc.
- `web/` — React 19 + Vite + TS + Tailwind + shadcn SPA: auth (access in memory,
  refresh in localStorage, silent refresh), account view, `/admin/*` (users + DLQ).
  Custom fetch client sends `X-Correlation-Id` per call (stable across the 401 retry),
  surfaces the cid on error toasts; request/response types are **generated from the
  OpenAPI doc** (`make gen-api` → committed `web/src/api/schema.d.ts`) and `apiFetch`'s
  path is constrained to the API's real routes (wrong path = `tsc` error).
- `migrations/` 0001–0006; `docker-compose.yml`, `Makefile` (incl. `make gen-api`),
  `scripts/new-domain.sh`.

**All specced work to date is built.** The only remaining items are the
not-yet-specced roadmap entries below.

## Start here (read in this order)

1. `docs/superpowers/specs/2026-06-24-rust-service-template-design.md` — the approved
   architecture + rationale (read §2 "Architecture decisions" before writing code).
2. The per-area design doc in `docs/superpowers/specs/` for whatever you're touching.
3. The Status table below, then (for any *new* spec) execute the relevant plan(s) in
   `docs/superpowers/plans/` with `superpowers:subagent-driven-development`.

## Status (all specced work is built)

| Area | Spec | Plan(s) | Built |
|---|---|---|---|
| Spec 1 — platform + outbox + domain-account | ✅ | ✅ 1a/1b/1c | ✅ merged |
| Spec 2 — domain-auth | ✅ | ✅ 2a/2b/2c | ✅ merged |
| Spec 3 — web SPA + backend prereqs | ✅ | ✅ 3a + web 3b/3c | ✅ merged |
| Correlation IDs + structured logging | ✅ | ✅ cid-a-backend + cid-b-frontend | ✅ merged |
| domain-notification (Spec 4) | ✅ | ✅ rust-spec4-domain-notification | ✅ merged |
| utoipa typed API client | ✅ | ✅ utoipa-a-backend + utoipa-b-frontend | ✅ merged |

Open design musings deliberately *not* yet recorded as decisions: synchronous
cross-domain queries (lean: events-carry-data → query-port traits → a `contracts`
crate if bidirectional; Cargo forbids crate cycles) and a frontend BFF/aggregate
layer (lean: none yet; add view endpoints in `app` when round-trips hurt). Any new
work follows the repo's **brainstorm → spec → plan → execute** cycle.

## How to execute a plan

- If you have the `superpowers:executing-plans` (or
  `superpowers:subagent-driven-development`) skill, use it.
- **If you don't have those skills, just work the plans manually:** each task
  is a checklist of bite-sized steps (write failing test → run it → implement →
  run again → commit). Do them top to bottom, one commit per task, exactly as
  written. The code in each step is complete — type it in as given.
- Run `cargo fmt --all` and `cargo clippy --all-targets -- -D warnings` before
  every commit; both must be clean.

## Environment prerequisites

- **Rust** stable, pinned via `rust-toolchain.toml` (needs 1.85+ — a transitive dep
  requires edition 2024).
- **Postgres** for `#[sqlx::test]` integration tests (provisions an isolated DB per
  test, runs `./migrations`; needs a role with CREATEDB). `make up` starts the
  docker-compose Postgres; set `DATABASE_URL`, e.g.
  `postgres://postgres:postgres@localhost:5432/postgres` (docker-compose) — any local
  Postgres works. (No Docker handy? A local `postgres` install works too; adjust the
  role/URL accordingly.)
- **Node ≥ 20 + npm** for the `web/` SPA: `make web-install` / `web-dev` / `web-build`.
- **`sqlx-cli`** for `make migrate`; **`openssl`** for the RSA test-key fixtures
  (under `crates/*/tests/fixtures/`).

## Architectural invariants — do NOT drift from these

These are the point of the template; preserve them while implementing:

- **Idiomatic Rust, not a Haskell transliteration.** Capture the *spirit* of
  the original (testable ports, least-privilege deps), not its `RIO`/typeclass
  machinery.
- **Hexagonal layering.** Domain logic (`domain.rs`) is pure and
  framework-free — no axum, no SQL. Adapters live under `ports/`.
- **Ports are traits; DI via `Arc<dyn Port>`.** Domain functions depend only on
  the port traits they need.
- **Outbox wiring is linear and cycle-free.** The publisher depends on a plain
  `Routes` (event_type → subscriber-name) table, never on subscriber instances:
  `Routes → publisher → subscribers → registry → dispatcher`. Keep a
  subscriber's `routes()` entry in sync with its registration.
- **sqlx runtime query API** (`sqlx::query`, `query_as`, `.bind`), NOT the
  compile-time `query!` macros — so the crate builds without a live DB.
- **One crate per domain.** Cross-domain access goes through public interfaces
  and events only.

## Further roadmap (not yet specced)

- **Templatize** via `cargo generate` — approach **(B)**: keep concrete names so the
  repo always builds; generation renames the known strings. (Revisit native
  `{{placeholder}}` templating, approach (A), only once fully working.) See design §11.
- Additional domains via `scripts/new-domain.sh`.

Decision history: Spec 2 ended up **Postgres-backed revocation, not Redis** (consistent
with outbox-over-Kafka — one datastore, no extra infra); the typed-client question
(design §12) resolved to **utoipa + openapi-typescript (types only)**, keeping the SPA's
custom fetch client. Continue the **brainstorm → spec → plan → execute** cycle for each
new spec.
