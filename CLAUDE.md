# rust-service-template — agent guide

## What this repo is

An idiomatic-Rust service template: a **monolith of internal domains** (not
microservices) with a transactional outbox for events, correlation-id tracing,
JWT auth, and Prometheus metrics. It is the Rust reinterpretation of a Haskell
microservices template — see the design doc for the full rationale.

## Current state

Specs 1–3 are **implemented and merged to `main`** (full Rust + web test suites
green; `cargo clippy -D warnings` + fmt clean). What exists today:

- `crates/platform` — config, db (Postgres pool + migrations), the transactional
  **outbox** (publish / dispatcher / retries / DLQ + `dlq_http` admin routes), `auth`
  (RS256 JWT verify, `Authenticated` extractor, scope guard, `RevocationChecker` port),
  metrics, http_client, observability (correlation-id request middleware), server
  (`AppError`, CORS).
- `crates/domain-account` — `/accounts`, `/accounts/:id`, `/accounts/me`; consumes
  `user.registered`, emits `account.created`.
- `crates/domain-auth` — register / login / refresh / logout (bcrypt, RS256 issuance),
  **Postgres-backed token revocation (no Redis)**, admin scope management; the real
  producer of `user.registered`.
- `crates/app` — composition root (**lib + bin**): builds resources, assembles the
  router via `build_router` (API nested under `/api`, `/status`+`/metrics` at root,
  serves the SPA from `web/dist` with an SPA fallback), runs server + outbox dispatcher
  + denylist prune task.
- `web/` — React 19 + Vite + TS + Tailwind + shadcn SPA: auth (access in memory,
  refresh in localStorage, silent refresh), account view, `/admin/*` (users + DLQ).
- `migrations/` 0001–0005; `docker-compose.yml`, `Makefile`, `scripts/new-domain.sh`.

**Remaining work is specced but not yet built** — see the Status table.

## Start here (read in this order)

1. `docs/superpowers/specs/2026-06-24-rust-service-template-design.md` — the approved
   architecture + rationale (read §2 "Architecture decisions" before writing code).
2. The per-area design doc in `docs/superpowers/specs/` for whatever you're touching.
3. The Status table below, then execute the relevant plan(s) in
   `docs/superpowers/plans/` with `superpowers:subagent-driven-development`.

## Status (done vs pending)

| Area | Spec | Plan(s) | Built |
|---|---|---|---|
| Spec 1 — platform + outbox + domain-account | ✅ | ✅ 1a/1b/1c | ✅ merged |
| Spec 2 — domain-auth | ✅ | ✅ 2a/2b/2c | ✅ merged |
| Spec 3 — web SPA + backend prereqs | ✅ | ✅ 3a + web 3b/3c | ✅ merged |
| Correlation IDs + structured logging | ✅ `…-correlation-id-logging-design.md` | ❌ needs `writing-plans` | ❌ |
| domain-notification (Spec 4) | ✅ `…-domain-notification-design.md` | ❌ needs `writing-plans` | ❌ |
| utoipa typed API client | ✅ `…-openapi-typed-client-design.md` | ✅ `utoipa-a-backend.md` + `utoipa-b-frontend.md` | ❌ |

Each pending item follows the repo's **brainstorm → spec → plan → execute** cycle.
The cid/logging and notification specs still need plans written; **utoipa is
plan-ready to execute**. Author's stated priority order: correlation-ids/logging first.
Open design musings deliberately *not* yet recorded as decisions: synchronous
cross-domain queries (lean: events-carry-data → query-port traits → a `contracts`
crate if bidirectional; Cargo forbids crate cycles) and a frontend BFF/aggregate
layer (lean: none yet; add view endpoints in `app` when round-trips hurt).

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
