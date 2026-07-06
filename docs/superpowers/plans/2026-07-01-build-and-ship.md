# Build & Ship Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the service buildable as one container image and deployable to Fly.io, with GitHub Actions quality gates — Dockerfile, `migrate` bin, `fly.toml`, CI, committed `Cargo.lock`, and `.env.example` JWT guardrails.

**Architecture:** A three-stage Dockerfile (node builds the SPA → rust builds `app` + `migrate` → debian-slim runtime, non-root). Migrations are compile-time embedded and run on deploy via a dedicated `migrate` bin invoked by Fly's `release_command`. CI runs fmt/clippy/DB-backed tests/web build/`cargo-audit`/docker-build/OpenAPI-drift but does not deploy (deploy is manual `fly deploy`, documented).

**Tech Stack:** Docker (multi-stage), Fly.io, GitHub Actions, Rust (stable, edition 2024), sqlx (embedded migrations, runtime query API), Node 20 / npm, openssl (key gen).

## Global Constraints

- **DB-free build.** `cargo build` needs no live database (runtime query API, no `query!` macros); migrations are embedded via `sqlx::migrate!`. Docker/CI builds must not require a DB.
- **Reproducible builds.** `Cargo.lock` is committed; every cargo invocation in Docker/CI uses `--locked`.
- **Secrets never committed.** Non-secret config in `fly.toml [env]`; secrets via `fly secrets set`, documented in the README.
- **Runtime image is non-root** (uid 10001), debian:bookworm-slim, and installs `ca-certificates libssl3` (reqwest's default `native-tls` links OpenSSL on Linux — see Task 4 note).
- **Release profile:** `lto = "thin"`, `codegen-units = 1`, `strip = true`; **not** `panic = "abort"`.
- **`kill_timeout = "15s"`** in `fly.toml` (> the app's 10s bounded shutdown drain from Spec 1).
- **Local tooling reality:** this machine has **no Docker daemon and no flyctl**. Verify what builds natively (`cargo build --locked`, `npm run build`, `cargo run --bin migrate`, `make gen-keys` via openssl); the **Dockerfile is authoritatively validated by CI's `docker` job once pushed**, and `fly.toml` by `fly config validate` (documented). Do not claim a `docker build` you could not run.
- **Before every commit:** `cargo fmt --all` and `cargo clippy --all-targets --locked -- -D warnings` clean.
- DB-backed tests use `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres`.

---

## File Structure

- **Create:** `Dockerfile`, `.dockerignore`, `fly.toml`, `crates/app/src/bin/migrate.rs`, `.github/workflows/ci.yml`, `Cargo.lock` (generated + committed).
- **Modify:** root `Cargo.toml` (`[profile.release]`), `crates/app/Cargo.toml` (`[[bin]] migrate`), `.gitignore` (un-ignore `Cargo.lock`, add `secrets/`), `Makefile` (`gen-keys`), `.env.example` (banner), `crates/app/src/main.rs` (build-SHA log), `README.md` (deploy section).

---

## Task 1: Commit Cargo.lock, release profile, .gitignore

**Files:**
- Modify: `.gitignore`, root `Cargo.toml`
- Add to git: `Cargo.lock`

- [ ] **Step 1: Un-ignore `Cargo.lock`, ignore `secrets/`** — in `.gitignore`, delete the `Cargo.lock` line, and add `secrets/` (near the `.env` line):

Remove:
```
Cargo.lock
```
Add (after `.env`):
```
secrets/
```

- [ ] **Step 2: Add the release profile** — append to the root `Cargo.toml`:

```toml
[profile.release]
lto = "thin"
codegen-units = 1
strip = true
```

- [ ] **Step 3: Generate the lockfile and verify a locked build**

Run:
```bash
cargo generate-lockfile
cargo build --locked --workspace
```
Expected: `Cargo.lock` exists and `cargo build --locked` succeeds with no lockfile changes.

- [ ] **Step 4: fmt + clippy**

Run: `cargo fmt --all && cargo clippy --all-targets --locked -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit (including Cargo.lock)**

```bash
git add .gitignore Cargo.toml Cargo.lock
git commit -m "build: commit Cargo.lock, add release profile, ignore secrets/"
```
Verify: `git ls-files --error-unmatch Cargo.lock` exits 0 (the lockfile is now tracked).

---

## Task 2: `migrate` binary + build-SHA startup log

**Files:**
- Create: `crates/app/src/bin/migrate.rs`
- Modify: `crates/app/Cargo.toml` (add `[[bin]]`), `crates/app/src/main.rs` (build-SHA log)

**Interfaces:**
- Produces: a `migrate` binary (`cargo build -p app --bin migrate`) that applies migrations and exits.

- [ ] **Step 1: Register the bin** — in `crates/app/Cargo.toml`, after the `openapi-gen` `[[bin]]` block, add:

```toml
[[bin]]
name = "migrate"
path = "src/bin/migrate.rs"
```

- [ ] **Step 2: Create the migrate bin** — `crates/app/src/bin/migrate.rs`:

```rust
//! Applies pending DB migrations and exits. Invoked by Fly's `release_command`
//! so migrations run once per deploy, before the new version serves traffic.

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

- [ ] **Step 3: Add the build-SHA startup log** — in `crates/app/src/main.rs`, right after `init_tracing("info");`, add:

```rust
    tracing::info!(
        build_sha = %std::env::var("APP_BUILD_SHA").unwrap_or_default(),
        "starting app"
    );
```

- [ ] **Step 4: Build both bins (the exact build the Dockerfile runs)**

Run: `cargo build --release --locked -p app --bin app --bin migrate`
Expected: both binaries compile.

- [ ] **Step 5: Verify migrate applies migrations and is idempotent**

Run (against local Postgres):
```bash
DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo run -p app --bin migrate
DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo run -p app --bin migrate
```
Expected: both runs log `migrations applied` and exit 0 (the second is a no-op — sqlx tracks applied migrations).

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --all-targets --locked -- -D warnings
git add crates/app/Cargo.toml crates/app/src/bin/migrate.rs crates/app/src/main.rs Cargo.lock
git commit -m "feat(app): add migrate bin + build-SHA startup log"
```

---

## Task 3: Multi-stage Dockerfile + .dockerignore

**Files:**
- Create: `Dockerfile`, `.dockerignore`

> Local Docker is unavailable, so this task cannot run `docker build`. Verify the two build halves natively (Steps 4–5); the full image build is validated by CI's `docker` job (Task 5) once pushed. Write the Dockerfile precisely from the content below.

- [ ] **Step 1: Create `.dockerignore`** (repo root):

```
target/
**/target
web/node_modules/
web/dist/
.git/
.github/
docs/
.superpowers/
secrets/
.env
*.md
```

- [ ] **Step 2: Create `Dockerfile`** (repo root):

```dockerfile
# syntax=docker/dockerfile:1

# --- Stage 1: build the SPA (web/dist) ---
FROM node:20-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

# --- Stage 2: build the Rust binaries ---
# migrations/ must be present: `sqlx::migrate!` embeds them at COMPILE time.
# (Optional rebuild-speed upgrade: cargo-chef to cache the dependency layer.)
FROM rust:1-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/
RUN cargo build --release --locked -p app --bin app --bin migrate

# --- Stage 3: runtime ---
FROM debian:bookworm-slim AS runtime
# ca-certificates + libssl3: reqwest's default `native-tls` backend links OpenSSL
# on Linux. (Future slimming: switch reqwest to rustls-tls to drop libssl3.)
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libssl3 \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --create-home --uid 10001 app
WORKDIR /app
COPY --from=build /app/target/release/app /app/app
COPY --from=build /app/target/release/migrate /app/migrate
COPY --from=web /web/dist /app/web/dist
ARG GIT_SHA=""
ENV APP_BUILD_SHA=$GIT_SHA
ENV APP__SERVER__PORT=8080
EXPOSE 8080
USER app
CMD ["/app/app"]
```

- [ ] **Step 3: Sanity-check the Dockerfile references** — confirm every `COPY` source path exists in the repo root context: `web/package.json`, `web/package-lock.json`, `Cargo.toml`, `Cargo.lock` (committed in Task 1), `crates/`, `migrations/`. Confirm `.dockerignore` does NOT exclude any of them (it excludes `web/node_modules`, `web/dist`, `target`, `.git`, `docs`, `secrets`, `.env`, `*.md` — none of the needed inputs).

- [ ] **Step 4: Verify the Rust build half natively**

Run: `cargo build --release --locked -p app --bin app --bin migrate`
Expected: succeeds (this is exactly stage 2's command).

- [ ] **Step 5: Verify the web build half natively**

Run: `npm --prefix web ci && npm --prefix web run build`
Expected: `web/dist/` is produced (this is exactly stage 1).

- [ ] **Step 6: Attempt `docker build` only if a daemon is present**

Run:
```bash
docker info >/dev/null 2>&1 && docker build --build-arg GIT_SHA=local -t rust-service:test . || echo "SKIP: no local Docker — CI docker job is the gate"
```
Expected: builds if Docker is available; otherwise explicitly skipped (documented — not a failure).

- [ ] **Step 7: Commit**

```bash
git add Dockerfile .dockerignore
git commit -m "build: multi-stage Dockerfile (node+rust build, debian-slim runtime, non-root)"
```

---

## Task 4: `fly.toml`

**Files:**
- Create: `fly.toml`

> `flyctl` is unavailable locally, so `fly config validate` cannot run here — verify by review + TOML parse; it's validated on the first real `fly deploy`.

- [ ] **Step 1: Create `fly.toml`** (repo root):

```toml
# Fly.io config. Change `app` and `primary_region` for your deployment.
app = "rust-service-template"       # change me
primary_region = "gru"              # change me

# SIGTERM grace must exceed the app's ~10s bounded shutdown drain (Spec 1),
# or Fly would SIGKILL mid-drain (its default kill_timeout is 5s).
kill_signal  = "SIGTERM"
kill_timeout = "15s"

[build]
dockerfile = "Dockerfile"

[deploy]
# Runs the migrate bin in a one-off machine before the new release serves.
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

- [ ] **Step 2: Verify the TOML parses**

Run:
```bash
python3 -c "import tomllib,sys; tomllib.load(open('fly.toml','rb')); print('fly.toml OK')"
```
Expected: `fly.toml OK` (valid TOML). (Structural/field validation happens via `fly config validate` at deploy — documented in the README, Task 6.)

- [ ] **Step 3: Commit**

```bash
git add fly.toml
git commit -m "build: fly.toml (release_command, /readyz check, 15s kill_timeout)"
```

---

## Task 5: GitHub Actions CI

**Files:**
- Create: `.github/workflows/ci.yml`

> GitHub Actions cannot run locally (no `act`). Verify YAML validity (Step 2) + review; the workflow self-validates on the PR/push that adds it.

- [ ] **Step 1: Create `.github/workflows/ci.yml`:**

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

jobs:
  rust:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16
        env:
          POSTGRES_PASSWORD: postgres
        ports:
          - 5432:5432
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5
    env:
      DATABASE_URL: postgres://postgres:postgres@localhost:5432/postgres
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --all-targets --locked -- -D warnings
      - run: cargo test --workspace --locked

  web:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "20"
          cache: npm
          cache-dependency-path: web/package-lock.json
      - run: npm --prefix web ci
      - run: npm --prefix web run lint
      - run: npm --prefix web run build
      - run: npm --prefix web test

  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: rustsec/audit-check@v2
        with:
          token: ${{ secrets.GITHUB_TOKEN }}

  docker:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: docker/setup-buildx-action@v3
      - uses: docker/build-push-action@v6
        with:
          context: .
          push: false
          build-args: |
            GIT_SHA=${{ github.sha }}

  openapi-drift:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: actions/setup-node@v4
        with:
          node-version: "20"
          cache: npm
          cache-dependency-path: web/package-lock.json
      - run: npm --prefix web ci
      - run: make gen-api
      - name: Fail if generated OpenAPI schema is stale
        run: git diff --exit-code web/src/api/schema.d.ts
```

- [ ] **Step 2: Verify the YAML parses**

Run:
```bash
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('ci.yml OK')"
```
Expected: `ci.yml OK`. (If `yaml` is missing, `pip install pyyaml` or accept review-only.)

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: fmt/clippy/tests + web + cargo-audit + docker build + openapi drift"
```

---

## Task 6: gen-keys, .env.example banner, README deploy section

**Files:**
- Modify: `Makefile`, `.env.example`, `README.md`

- [ ] **Step 1: Add the `gen-keys` Makefile target** — add to `.PHONY` and append a target:

Update the `.PHONY` line to include `gen-keys`, then add:

```makefile
gen-keys:
	mkdir -p secrets
	openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out secrets/jwt_private.pem
	openssl rsa -pubout -in secrets/jwt_private.pem -out secrets/jwt_public.pem
	@echo "Wrote secrets/jwt_private.pem and secrets/jwt_public.pem (gitignored)."
```

- [ ] **Step 2: Verify gen-keys works and the app accepts the keys**

Run:
```bash
make gen-keys
test -s secrets/jwt_private.pem && test -s secrets/jwt_public.pem && echo "keys OK"
```
Expected: `keys OK`, and `secrets/` is gitignored (`git status --porcelain secrets/` prints nothing).

- [ ] **Step 3: Add the `.env.example` banner** — insert, immediately above the `APP__AUTH__JWT_PUBLIC_KEY_FILE` line:

```
# ============================================================================
# LOCAL DEV ONLY: the paths below point at COMMITTED test-fixture keys.
# NEVER use these in production. For real keys, run `make gen-keys` (writes to
# gitignored secrets/) and in prod set them via `fly secrets set` (see README).
# ============================================================================
```

- [ ] **Step 4: Add a README "Deploy to Fly.io" section** — append to `README.md`:

```markdown
## Deploy to Fly.io

CI (GitHub Actions) runs the quality gates and builds the image, but does not
deploy — deploy is a manual step.

1. **Generate real JWT keys** (never use the committed test fixtures in prod):
   ```bash
   make gen-keys   # writes secrets/jwt_{private,public}.pem (gitignored)
   ```
2. **Create the app and set secrets** (non-secret config lives in `fly.toml`):
   ```bash
   fly apps create <your-app>          # then set app = "<your-app>" in fly.toml
   fly secrets set \
     APP__DATABASE__URL="postgres://..." \
     APP__AUTH__JWT_PRIVATE_KEY_PEM="$(cat secrets/jwt_private.pem)" \
     APP__AUTH__JWT_PUBLIC_KEY_PEM="$(cat secrets/jwt_public.pem)" \
     APP__AUTH__ADMIN_EMAILS="you@example.com"
   ```
   `APP__DATABASE__AUTO_MIGRATE` is `false` in prod (see `fly.toml`); the
   `release_command` (`/app/migrate`) applies migrations on each deploy.
3. **Validate and deploy:**
   ```bash
   fly config validate
   fly deploy
   ```
```

- [ ] **Step 5: Commit**

```bash
git add Makefile .env.example README.md
git commit -m "docs: make gen-keys, .env.example prod-key warning, Fly deploy README"
```

---

## Self-Review Notes (coverage vs. spec)

- **§3 Dockerfile + .dockerignore:** Task 3. **Refinement over spec:** runtime installs `ca-certificates` **and `libssl3`** — reqwest's default `native-tls` links OpenSSL on Linux (verified: `native-tls` is in the dep tree). The spec said "ca-certificates"; `libssl3` is required for the binary to run. rustls-for-reqwest noted as future slimming. ✅
- **§4 migrate bin + release profile + build SHA:** Task 2 (bin + SHA log) + Task 1 (`[profile.release]`). ✅
- **§5 fly.toml (release_command, /readyz, kill_timeout=15s, secrets via fly):** Task 4 + README (Task 6). ✅
- **§6 CI (rust/web/audit/docker/openapi-drift):** Task 5. ✅
- **§7 hygiene (commit Cargo.lock, secrets/ ignore, .env banner, gen-keys, README):** Tasks 1 + 6. ✅
- **§8 verification:** native sub-builds (Tasks 2–3), `make gen-keys` (Task 6), migrate idempotency (Task 2), TOML/YAML parse (Tasks 4–5). **Docker build + `fly config validate` cannot run locally** (no daemon/flyctl) — the Dockerfile is authoritatively validated by CI's `docker` job on push, `fly.toml` on first deploy. This is called out in Global Constraints and per-task, not silently skipped.
- **Post-merge:** because the Dockerfile isn't locally buildable, the controller MUST watch the GitHub Actions run after pushing `main` and fix forward if the `docker` or any job fails.
