# Build & Ship — Design (Production-readiness, Spec 2 of 2)

**Date:** 2026-07-01
**Status:** Approved design, ready for implementation planning
**Scope of this spec:** Make the service buildable as a single container image and
deployable to Fly.io, with CI quality gates. Delivers a multi-stage Dockerfile +
`.dockerignore`, a `[profile.release]`, a `migrate` binary + `fly.toml` (release
command, `/readyz` health check, SIGTERM grace), GitHub Actions CI (fmt/clippy/
DB-backed tests/web build/`cargo-audit`/Docker-build/OpenAPI-drift), committing
`Cargo.lock`, and the `.env.example` JWT-key guardrails. **Out of scope:** auto-deploy
to Fly (CI stops at build verification; deploy is manual/documented), auth-feature
hardening (MFA, refresh rotation, security headers — separate later specs), and any
runtime behavior (covered by Spec 1, "Runtime Hardening", already merged).

Second of two production-readiness specs. Builds directly on Spec 1 (which added
`/readyz`, graceful shutdown, and config knobs). Deployment target: **Fly.io**.

---

## 1. Goal & principles

The gap analysis found the core and (after Spec 1) the runtime are solid, but there
is **no way to build or ship the service**: no Dockerfile, no CI, default release
profile, `Cargo.lock` gitignored, and an `.env.example` that points at committed test
JWT keys. This spec closes the "produce and deploy the artifact" gap so a product can
be built on the template.

Principles:
- **One self-contained image.** A multi-stage build produces one image containing the
  `app` binary, the `migrate` binary, and the built SPA. The current `ServeDir`
  serving code is unchanged.
- **DB-free build.** The runtime sqlx query API (no `query!` macros) means `cargo
  build` needs no database — Docker and CI builds have no DB dependency. Migrations
  are compile-time embedded via `sqlx::migrate!`, so they ship inside the binaries and
  are needed only in the build stage, not the runtime image.
- **12-factor config.** Non-secret config in `fly.toml [env]`; secrets via `fly
  secrets set`, never committed.
- **Reproducible builds.** Commit `Cargo.lock`; all cargo invocations use `--locked`.
- **Template pragmatism.** Ship artifacts that build and run and are understandable;
  document (rather than commit dead) the pieces that need a real Fly account/token.

---

## 2. Decisions (resolved during brainstorming)

1. **CI depth = quality gates + Docker build verification; deploy is manual.** No Fly
   token in GitHub. A committed auto-deploy workflow with no real token would be dead
   weight; the deploy step is documented in the README (`fly deploy`).
2. **Migrations run via a dedicated `migrate` binary** invoked by Fly's
   `release_command` (one-off machine, before the new release serves). `auto_migrate`
   defaults to `false` in prod; stays `true` in `.env.example` for local dev.
3. **Runtime image base: `debian:bookworm-slim`**, non-root. (Distroless is a
   documented one-line `FROM` swap; not adopted now.)
4. **`.env.example` JWT footgun: keep frictionless local dev + guardrails** — a loud
   banner, a `make gen-keys` helper (real keypair into gitignored `secrets/`), and a
   README production section. Not fail-closed.
5. **Commit `Cargo.lock`** (remove from `.gitignore`) — needed for `--locked` and
   `cargo-audit`.
6. **Build SHA surfaced via a startup log line only** (Dockerfile `ARG GIT_SHA` →
   `ENV APP_BUILD_SHA` → logged in `main`). No `/version` endpoint, `/status`
   untouched.
7. **OpenAPI schema-drift check included in CI** (guards the committed typed-client
   `schema.d.ts` against backend drift).
8. **Release profile:** `lto = "thin"`, `codegen-units = 1`, `strip = true`.
   Deliberately **not** `panic = "abort"` — a server should isolate handler panics,
   not abort the process.

---

## 3. Multi-stage Dockerfile + `.dockerignore`

**`Dockerfile`** (repo root), three stages:

1. **Web** (`node:20-bookworm-slim` as `web`): copy `web/package.json` +
   `web/package-lock.json`, `npm ci`, copy `web/`, `npm run build` → `/web/dist`.
2. **Build** (`rust:1-bookworm` as `build`): copy `Cargo.toml`, `Cargo.lock`,
   `crates/`, `migrations/`; `cargo build --release --locked -p app --bin app --bin
   migrate`. (`migrations/` is present so the `sqlx::migrate!` macro can embed it at
   compile time.)
3. **Runtime** (`debian:bookworm-slim`): `apt-get install -y --no-install-recommends
   ca-certificates` (outbound TLS) + create non-root user (uid 10001); `WORKDIR
   /app`; `COPY --from=build` the `app` and `migrate` binaries; `COPY --from=web
   /web/dist /app/web/dist`; `ARG GIT_SHA` → `ENV APP_BUILD_SHA=$GIT_SHA`;
   `EXPOSE 8080`; `USER app`; `CMD ["/app/app"]`.

The runtime image does **not** copy `migrations/` (they are embedded in the
binaries). The app serves the SPA from the relative path `web/dist`, which resolves
against `WORKDIR /app` — no code change.

Caching: a simple single `cargo build` (no `cargo-chef`); a comment notes cargo-chef
as the optional dependency-layer-caching upgrade.

**`.dockerignore`**: `target/`, `**/target`, `web/node_modules`, `web/dist`, `.git`,
`docs/`, `.env`, `secrets/`, `*.md` (except what the build needs — keep it lean).

---

## 4. `migrate` binary + release profile + build SHA

**`crates/app/src/bin/migrate.rs`** (new, alongside the existing `openapi-gen` bin):
```rust
// Applies pending DB migrations and exits. Invoked by Fly's release_command so
// migrations run once per deploy, before the new version serves traffic.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    platform::observability::init_tracing("info");
    let settings = platform::config::Settings::load()?;
    let pool = platform::db::make_pool(&settings.database).await?;
    platform::db::run_migrations(&pool).await?;
    tracing::info!("migrations applied");
    Ok(())
}
```

**`main.rs` build-SHA log**: near startup, `tracing::info!(build_sha = %std::env::var("APP_BUILD_SHA").unwrap_or_default(), "starting app")` (empty when unset locally).

**Root `Cargo.toml`**:
```toml
[profile.release]
lto = "thin"
codegen-units = 1
strip = true
```

`auto_migrate=false` in prod is documented (README + `fly.toml [env]`), not enforced
in code.

---

## 5. `fly.toml`

Committed at repo root, with placeholder `app`/`primary_region` (commented "change
me"):

```toml
app = "rust-service-template"       # change me
primary_region = "gru"              # change me

kill_signal  = "SIGTERM"
kill_timeout = "15s"                # > the app's 10s bounded shutdown await (Spec 1)

[build]
dockerfile = "Dockerfile"

[deploy]
release_command = "/app/migrate"

[env]
APP__SERVER__PORT = "8080"
APP__SERVER__ENVIRONMENT = "production"
APP__DATABASE__AUTO_MIGRATE = "false"

[http_service]
internal_port = 8080
force_https = true
auto_stop_machines = true
auto_start_machines = true
min_machines_running = 1

[[http_service.checks]]
method = "GET"
path = "/readyz"
interval = "10s"
timeout = "2s"
grace_period = "5s"
```

Secrets (`APP__DATABASE__URL`, `APP__AUTH__JWT_PRIVATE_KEY_PEM` /
`APP__AUTH__JWT_PUBLIC_KEY_PEM`, `APP__AUTH__ADMIN_EMAILS`) are set via `fly secrets
set` and documented in the README — never in `fly.toml`.

`kill_timeout = 15s` is the load-bearing detail: Fly's 5s default would SIGKILL before
Spec 1's graceful shutdown (bounded 10s drain of in-flight outbox batches) completes.

---

## 6. GitHub Actions CI

**`.github/workflows/ci.yml`**, on push to `main` and on `pull_request`. Jobs run in
parallel; all cargo invocations use `--locked`.

1. **`rust`** — `Swatinem/rust-cache`; a Postgres 16 **service container** (`env
   POSTGRES_PASSWORD=postgres`, health-checked), `DATABASE_URL=postgres://postgres:
   postgres@localhost:5432/postgres`; steps: `cargo fmt --all -- --check`, `cargo
   clippy --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`.
2. **`web`** — `actions/setup-node@20` + npm cache; `npm --prefix web ci`, `npm
   --prefix web run lint`, `npm --prefix web run build`, `npm --prefix web test` (GH
   sets `CI=true`, so vitest runs once).
3. **`audit`** — `rustsec/audit-check` against the committed `Cargo.lock` (fails on
   known advisories).
4. **`docker`** — `docker/build-push-action` with `push: false`,
   `build-args: GIT_SHA=${{ github.sha }}` — proves the full multi-stage image builds.
5. **`openapi-drift`** — install rust + node, run `make gen-api`, then `git diff
   --exit-code web/src/api/schema.d.ts` — fails if the committed typed-client schema
   is stale vs. the backend OpenAPI doc.

---

## 7. Repo hygiene + docs

- **Commit `Cargo.lock`**: remove the `Cargo.lock` line from `.gitignore`; `git add`
  the lockfile.
- **`.gitignore`**: add `secrets/`.
- **`.env.example`**: add a prominent banner above the JWT key lines —
  `# LOCAL DEV ONLY: these are committed test-fixture keys. NEVER use in production.`
  `# Generate real keys with 'make gen-keys'; set prod keys via 'fly secrets set'.`
  Defaults still point at the test fixtures (frictionless local dev).
- **`make gen-keys`** (Makefile target): `mkdir -p secrets`; `openssl genpkey
  -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out secrets/jwt_private.pem`; `openssl
  rsa -pubout -in secrets/jwt_private.pem -out secrets/jwt_public.pem`.
- **README**: a "Deploy to Fly.io / Production" section — `make gen-keys`; `fly
  secrets set ...` for DB URL, JWT keys, admin emails; note `AUTO_MIGRATE=false` and
  that the release command runs migrations; `fly deploy`.

---

## 8. Verification strategy

These are infrastructure artifacts, so verification is build/run, not unit tests. Each
becomes a concrete plan step:

1. **Dockerfile** — `docker build --build-arg GIT_SHA=test -t rust-service:test .`
   succeeds; `docker run` (against the docker-compose Postgres, with the required env)
   starts, logs `build_sha=test`, and `curl localhost:8080/readyz` returns `200`.
2. **`migrate` bin** — `cargo run -p app --bin migrate` against local Postgres applies
   migrations and exits `0`; a second run is a no-op (idempotent).
3. **Release profile** — `cargo build --release --locked` succeeds; binary is stripped.
4. **`fly.toml`** — `fly config validate` passes (documented; requires flyctl).
5. **`make gen-keys`** — generates the keypair; pointing `.env` at `secrets/` lets the
   app boot and issue/verify a JWT.
6. **CI** — self-validating on the PR that adds it; YAML lints clean.
7. **`Cargo.lock` + `--locked`** — `cargo build --locked` succeeds with no lockfile
   changes.

---

## 9. Files touched (anticipated)

- **Create:** `Dockerfile`, `.dockerignore`, `fly.toml`,
  `crates/app/src/bin/migrate.rs`, `.github/workflows/ci.yml`.
- **Modify:** root `Cargo.toml` (`[profile.release]`), `.gitignore` (un-ignore
  `Cargo.lock`, add `secrets/`), `Makefile` (`gen-keys`), `.env.example` (banner),
  `crates/app/src/main.rs` (build-SHA log line), `README.md` (deploy section).
- **Add to git:** `Cargo.lock`.
