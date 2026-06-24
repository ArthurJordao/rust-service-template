# rust-service-template — agent guide

## What this repo is

An idiomatic-Rust service template: a **monolith of internal domains** (not
microservices) with a transactional outbox for events, correlation-id tracing,
JWT auth, and Prometheus metrics. It is the Rust reinterpretation of a Haskell
microservices template — see the design doc for the full rationale.

## Current state

**Nothing is built yet.** The repo currently contains only design + planning
docs and git history. Your job is to implement it from the plans below.

## Start here (read in this order)

1. `docs/superpowers/specs/2026-06-24-rust-service-template-design.md` — the
   approved architecture and the rationale behind every decision (read §2
   "Architecture decisions" before writing any code).
2. Then execute these three plans **in order** — each is self-contained,
   independently testable, and produces working software:
   1. `docs/superpowers/plans/2026-06-24-rust-spec1a-workspace-and-platform.md`
   2. `docs/superpowers/plans/2026-06-24-rust-spec1b-outbox-events.md`
   3. `docs/superpowers/plans/2026-06-24-rust-spec1c-account-domain-and-app.md`

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

- **Rust** stable (a `rust-toolchain.toml` is created in Plan 1a Task 1).
- **Postgres** reachable for tests in Plans 1b/1c. Tests use `#[sqlx::test]`,
  which provisions an isolated DB per test and runs `./migrations` — it needs
  `DATABASE_URL` set, e.g.
  `export DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres`.
  Plan 1c Task 8 adds a `docker-compose.yml` (`docker compose up -d postgres`);
  until then, any local Postgres works.
- **`sqlx-cli`** for the `migrate` Makefile target: `cargo install sqlx-cli`.
- **`openssl`** to generate the RSA test-key fixture (Plan 1c Task 5 Step 2).

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

## Roadmap beyond Spec 1 (not yet specced)

Spec 2 = `domain-auth` (JWT issuance, login/refresh/logout, Redis revocation;
becomes the real producer of `user.registered`, replacing the dev stand-in).
Spec 3 = `web/` React SPA (Vite + TS + Tailwind + shadcn) with `/admin/*` route
group. Spec 4+ = `domain-notification` and further domains.

Also planned (see design doc §11–§12), do NOT start these until a slice runs:
- **Templatize** via `cargo generate` — approach **(B)**: keep concrete names so
  the repo always builds; generation renames the known strings. (Revisit native
  `{{placeholder}}` templating, approach (A), only once fully working.)
- **API schema / typed TS client** (open question, leaning `utoipa` + OpenAPI +
  `openapi-typescript`/`openapi-fetch`) — its own brainstorm → spec → plan cycle.

When you finish Spec 1, brainstorm + spec + plan the next spec before
implementing it (same spec → plan → execute cycle this repo was built with).
