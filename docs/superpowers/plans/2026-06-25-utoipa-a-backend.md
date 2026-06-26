# utoipa Plan A: Backend OpenAPI annotations + aggregation + emit — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Annotate the axum API with `utoipa` so the workspace produces one OpenAPI document — served at `/api/openapi.json` (+ Swagger UI) and emitted by an `openapi-gen` binary for codegen.

**Architecture:** Each crate that owns handlers/DTOs annotates them (`#[derive(ToSchema)]` on DTOs, `#[utoipa::path]` on handlers) and defines its own `#[derive(OpenApi)]` partial doc (so private handlers stay private). The `app` crate merges the per-crate docs into a top-level `ApiDoc` with a bearer security scheme and an `/api` server, exposes it at runtime, and ships an `openapi-gen` bin that prints it.

**Tech Stack:** utoipa 5 (`axum_extras`, `chrono`), utoipa-swagger-ui 8, axum 0.7, serde_json.

## Global Constraints

- Same dependency rules as Spec 1 (`docs/superpowers/plans/2026-06-24-rust-spec1a-workspace-and-platform.md`). Depends on Specs 1–3 being complete.
- Per-crate partial `OpenApi` docs merged in `app` — do NOT make handlers `pub` across crates; the `OpenApi` derive lives in the same module as the handlers so it can reference them.
- Annotated handler paths are **domain-relative** (`/auth/login`, not `/api/auth/login`); the `/api` prefix lives in the top-level doc's `servers` entry.
- Authenticated/admin operations declare the `bearer_auth` security requirement.
- sqlx runtime API unchanged; this plan adds annotations + a doc, it does not change routing or handler behavior (except replacing the DLQ replay `json!` literal with a typed `ReplayResponse`).
- Run `cargo fmt --all` + `cargo clippy --all-targets -- -D warnings` before each commit; both clean. Postgres for the existing integration tests (`DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres`).

---

### Task 1: Add utoipa dependencies

**Files:** Modify `Cargo.toml` (workspace), `crates/platform/Cargo.toml`, `crates/domain-account/Cargo.toml`, `crates/domain-auth/Cargo.toml`, `crates/app/Cargo.toml`.

**Interfaces:** Produces: `utoipa` + `utoipa-swagger-ui` available to the crates.

- [ ] **Step 1: Workspace deps**

In root `Cargo.toml` `[workspace.dependencies]`:
```toml
utoipa = { version = "5", features = ["axum_extras", "chrono"] }
utoipa-swagger-ui = { version = "8", features = ["axum"] }
```

- [ ] **Step 2: Per-crate deps**

Add `utoipa.workspace = true` to `[dependencies]` of `platform`, `domain-account`, `domain-auth`, and `app`. Add `utoipa-swagger-ui.workspace = true` to `app` only.

- [ ] **Step 3: Verify**

Run: `cargo build`
Expected: PASS (deps resolve; nothing uses them yet).

- [ ] **Step 4: Commit**
```bash
git add Cargo.toml crates/*/Cargo.toml
git commit -m "chore(openapi): add utoipa + utoipa-swagger-ui deps"
```

---

### Task 2: `ToSchema` on all DTOs + typed `ReplayResponse`

**Files:** Modify `crates/domain-account/src/models.rs` (`Account`), `crates/domain-auth/src/ports/dto.rs` (all DTOs), `crates/domain-auth/src/models.rs` (`ScopeRow`), `crates/platform/src/events/dlq.rs` (`DeadLetter`), `crates/platform/src/events/dlq_http.rs` (add `ReplayResponse`, use it).

**Interfaces:**
- Produces: every request/response DTO derives `utoipa::ToSchema`; `pub struct ReplayResponse { pub replayed: bool }` (`Serialize, ToSchema`) returned by the replay handler.

- [ ] **Step 1: Derive `ToSchema` on DTOs**

Add `utoipa::ToSchema` to the derive list of each:
- `domain-account/src/models.rs`: `Account`.
- `domain-auth/src/ports/dto.rs`: `RegisterRequest`, `LoginRequest`, `AuthTokens`, `RefreshRequest`, `LogoutRequest`, `UserWithScopes`, `SetScopesRequest`.
- `domain-auth/src/models.rs`: `ScopeRow`.
- `platform/src/events/dlq.rs`: `DeadLetter`.

Example (apply the same pattern to each):
```rust
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Account { /* unchanged fields */ }
```
For request DTOs that are `Deserialize`, add `ToSchema` alongside (`#[derive(Debug, Deserialize, utoipa::ToSchema)]`).

- [ ] **Step 2: Add the typed `ReplayResponse`**

In `crates/platform/src/events/dlq_http.rs`, add:
```rust
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ReplayResponse {
    pub replayed: bool,
}
```
Change `replay_handler` to return `Json<ReplayResponse>` instead of `Json<serde_json::Value>`:
```rust
) -> Result<Json<ReplayResponse>, AppError> {
    require_scope(&claims, "admin")?;
    let replayed = replay_dead_letter(&state.pool, delivery_id)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(ReplayResponse { replayed }))
}
```
(Remove the now-unused `serde_json::json` import if it is no longer referenced.)

- [ ] **Step 3: Verify**

Run: `cargo build` then `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p platform --test dlq_http`
Expected: PASS (the replay test asserts `replayed` — still valid; the body is now typed).

- [ ] **Step 4: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/platform crates/domain-account crates/domain-auth
git commit -m "feat(openapi): ToSchema on all DTOs + typed ReplayResponse"
```

---

### Task 3: Annotate `domain-auth` handlers + partial `ApiDoc`

**Files:** Modify `crates/domain-auth/src/ports/http.rs`; create `crates/domain-auth/src/openapi.rs`; modify `crates/domain-auth/src/lib.rs` (`pub mod openapi;`).

**Interfaces:**
- Produces: `domain_auth::openapi::ApiDoc` (`utoipa::OpenApi`) covering the auth + admin endpoints.

- [ ] **Step 1: Annotate each handler with `#[utoipa::path]`**

Above each handler fn in `http.rs`, add the annotation. Exact specs (method, path, body, responses, security):
```rust
#[utoipa::path(post, path = "/auth/register", request_body = RegisterRequest,
    responses((status = 201, body = AuthTokens), (status = 409)), tag = "auth")]
// register

#[utoipa::path(post, path = "/auth/login", request_body = LoginRequest,
    responses((status = 200, body = AuthTokens), (status = 401)), tag = "auth")]
// login

#[utoipa::path(post, path = "/auth/refresh", request_body = RefreshRequest,
    responses((status = 200, body = AuthTokens), (status = 401)), tag = "auth")]
// refresh

#[utoipa::path(post, path = "/auth/logout", request_body = LogoutRequest,
    responses((status = 204)), tag = "auth")]
// logout

#[utoipa::path(get, path = "/scopes",
    responses((status = 200, body = [ScopeRow]), (status = 401), (status = 403)),
    security(("bearer_auth" = [])), tag = "admin")]
// list_scopes

#[utoipa::path(get, path = "/users",
    responses((status = 200, body = [UserWithScopes]), (status = 401), (status = 403)),
    security(("bearer_auth" = [])), tag = "admin")]
// list_users

#[utoipa::path(get, path = "/users/{id}/scopes",
    params(("id" = i64, Path,)),
    responses((status = 200, body = [String]), (status = 401), (status = 403)),
    security(("bearer_auth" = [])), tag = "admin")]
// get_user_scopes

#[utoipa::path(put, path = "/users/{id}/scopes",
    params(("id" = i64, Path,)), request_body = SetScopesRequest,
    responses((status = 204), (status = 401), (status = 403)),
    security(("bearer_auth" = [])), tag = "admin")]
// set_user_scopes
```
(`ScopeRow` is in `crate::models`; import it into `http.rs` if not already in scope for the macro reference. The DTOs are already imported.)

- [ ] **Step 2: Define the partial `ApiDoc`**

`crates/domain-auth/src/openapi.rs`:
```rust
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::ports::http::register,
        crate::ports::http::login,
        crate::ports::http::refresh,
        crate::ports::http::logout,
        crate::ports::http::list_scopes,
        crate::ports::http::list_users,
        crate::ports::http::get_user_scopes,
        crate::ports::http::set_user_scopes,
    ),
    components(schemas(
        crate::ports::dto::RegisterRequest,
        crate::ports::dto::LoginRequest,
        crate::ports::dto::AuthTokens,
        crate::ports::dto::RefreshRequest,
        crate::ports::dto::LogoutRequest,
        crate::ports::dto::UserWithScopes,
        crate::ports::dto::SetScopesRequest,
        crate::models::ScopeRow,
    )),
    tags((name = "auth"), (name = "admin"))
)]
pub struct ApiDoc;
```
> The handlers are referenced by path from within the crate, so they need not be `pub` outside it — but `#[utoipa::path]` generates a `__path_<fn>` item with the fn's visibility; if the derive (in `openapi.rs`) cannot see a private handler in `http.rs`, make those handlers `pub(crate)`. Apply `pub(crate)` to the eight handlers if the build complains.

Add to `crates/domain-auth/src/lib.rs`: `pub mod openapi;`

- [ ] **Step 3: Verify**

Run: `cargo build -p domain-auth`
Expected: PASS. (If "cannot find function" in the `paths(...)` list, bump the handlers to `pub(crate)` per the note.)

- [ ] **Step 4: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy -p domain-auth --all-targets -- -D warnings
git add crates/domain-auth
git commit -m "feat(openapi): annotate domain-auth handlers + partial ApiDoc"
```

---

### Task 4: Annotate `domain-account` handlers + partial `ApiDoc`

**Files:** Modify `crates/domain-account/src/ports/http.rs`; create `crates/domain-account/src/openapi.rs`; modify `crates/domain-account/src/lib.rs`.

**Interfaces:** Produces `domain_account::openapi::ApiDoc`.

- [ ] **Step 1: Annotate handlers**
```rust
#[utoipa::path(get, path = "/accounts",
    responses((status = 200, body = [Account]), (status = 401), (status = 403)),
    security(("bearer_auth" = [])), tag = "accounts")]
// list_accounts

#[utoipa::path(get, path = "/accounts/me",
    responses((status = 200, body = Account), (status = 401), (status = 404)),
    security(("bearer_auth" = [])), tag = "accounts")]
// account_me

#[utoipa::path(get, path = "/accounts/{id}",
    params(("id" = i64, Path,)),
    responses((status = 200, body = Account), (status = 401), (status = 403), (status = 404)),
    security(("bearer_auth" = [])), tag = "accounts")]
// get_account
```

- [ ] **Step 2: Partial `ApiDoc`**

`crates/domain-account/src/openapi.rs`:
```rust
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::ports::http::list_accounts,
        crate::ports::http::account_me,
        crate::ports::http::get_account,
    ),
    components(schemas(crate::models::Account)),
    tags((name = "accounts"))
)]
pub struct ApiDoc;
```
Add `pub mod openapi;` to `crates/domain-account/src/lib.rs`. Apply `pub(crate)` to the three handlers if the derive can't see them.

- [ ] **Step 3: Verify + commit**

Run: `cargo build -p domain-account && cargo fmt --all && cargo clippy -p domain-account --all-targets -- -D warnings`
```bash
git add crates/domain-account
git commit -m "feat(openapi): annotate domain-account handlers + partial ApiDoc"
```

---

### Task 5: Annotate platform DLQ handlers + partial `ApiDoc`

**Files:** Modify `crates/platform/src/events/dlq_http.rs`; create `crates/platform/src/events/dlq_openapi.rs` (or add the derive in `dlq_http.rs`); modify `crates/platform/src/events/mod.rs`.

**Interfaces:** Produces `platform::events::dlq_http::ApiDoc` (or `platform::events::DlqApiDoc`).

- [ ] **Step 1: Annotate handlers** (in `dlq_http.rs`)
```rust
#[utoipa::path(get, path = "/admin/dlq",
    responses((status = 200, body = [DeadLetter]), (status = 401), (status = 403)),
    security(("bearer_auth" = [])), tag = "admin")]
// list_handler

#[utoipa::path(post, path = "/admin/dlq/{delivery_id}/replay",
    params(("delivery_id" = i64, Path,)),
    responses((status = 200, body = ReplayResponse), (status = 401), (status = 403)),
    security(("bearer_auth" = [])), tag = "admin")]
// replay_handler
```

- [ ] **Step 2: Partial `ApiDoc`** (append to `dlq_http.rs`)
```rust
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list_handler, replay_handler),
    components(schemas(crate::events::DeadLetter, ReplayResponse)),
    tags((name = "admin"))
)]
pub struct ApiDoc;
```
(`list_handler`/`replay_handler` are in the same module, so the derive sees them.)

- [ ] **Step 3: Verify + commit**

Run: `cargo build -p platform && cargo fmt --all && cargo clippy -p platform --all-targets -- -D warnings`
```bash
git add crates/platform
git commit -m "feat(openapi): annotate platform DLQ handlers + partial ApiDoc"
```

---

### Task 6: Aggregate `ApiDoc` in `app` + `openapi-gen` bin + test

**Files:** Create `crates/app/src/openapi.rs`; modify `crates/app/src/lib.rs` (`pub mod openapi;`); create `crates/app/src/bin/openapi_gen.rs`; test in `openapi.rs`.

**Interfaces:**
- Produces: `app::openapi::api_doc() -> utoipa::openapi::OpenApi` — the merged doc with bearer security + `/api` server; the `openapi-gen` binary.

- [ ] **Step 1: Write the aggregation**

`crates/app/src/openapi.rs`:
```rust
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{ServerBuilder, OpenApi};
use utoipa::OpenApi as _;

/// The merged OpenAPI document for the whole API (served under `/api`).
pub fn api_doc() -> OpenApi {
    let mut doc = domain_auth::openapi::ApiDoc::openapi();
    doc.merge(domain_account::openapi::ApiDoc::openapi());
    doc.merge(platform::events::dlq_http::ApiDoc::openapi());

    // Bearer (JWT) security scheme.
    let components = doc.components.get_or_insert_with(Default::default);
    components.add_security_scheme(
        "bearer_auth",
        SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).bearer_format("JWT").build()),
    );

    // All paths are served under /api.
    doc.servers = Some(vec![ServerBuilder::new().url("/api").build()]);
    doc
}
```
> `OpenApi::merge` combines paths + components from each partial doc. If a utoipa version names the method differently (`merge_from` / consuming `merge`), adjust to the installed API — the intent is "combine the three partial docs."

Add `pub mod openapi;` to `crates/app/src/lib.rs`.

- [ ] **Step 2: Write the test**

In `crates/app/src/openapi.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregated_doc_has_all_endpoints_and_schemas() {
        let doc = api_doc();
        let json = serde_json::to_string(&doc).unwrap();
        for path in ["/auth/login", "/auth/register", "/accounts/me", "/users/{id}/scopes", "/admin/dlq"] {
            assert!(json.contains(path), "missing path {path}");
        }
        for schema in ["AuthTokens", "Account", "DeadLetter", "ReplayResponse", "UserWithScopes"] {
            assert!(json.contains(schema), "missing schema {schema}");
        }
        // bearer scheme + /api server present
        assert!(json.contains("bearer_auth"));
        assert!(json.contains("\"/api\""));
    }
}
```

- [ ] **Step 3: Write the `openapi-gen` bin**

`crates/app/src/bin/openapi_gen.rs`:
```rust
fn main() {
    let doc = app::openapi::api_doc();
    println!("{}", doc.to_pretty_json().expect("serialize openapi"));
}
```

- [ ] **Step 4: Verify**

Run: `cargo test -p app openapi::` then `cargo run -p app --bin openapi-gen | head -5`
Expected: test PASS; the bin prints the OpenAPI JSON (starts with `{`). No DB needed.

- [ ] **Step 5: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/app
git commit -m "feat(openapi): aggregate ApiDoc in app + openapi-gen bin + test"
```

---

### Task 7: Serve `/api/openapi.json` + Swagger UI

**Files:** Modify `crates/app/src/state.rs` (`build_router`).

**Interfaces:** Produces: `GET /api/openapi.json` (the doc) and `/swagger-ui` (interactive).

- [ ] **Step 1: Mount the doc + Swagger UI in `build_router`**

In `build_router` (where the API is nested under `/api`), serve the JSON inside the `/api` nest and Swagger UI at root. Add imports:
```rust
use utoipa_swagger_ui::SwaggerUi;
use axum::Json;
```
Add a route for the spec (inside the `api` router, before `.nest`):
```rust
    let api = domain_account::router(account)
        .merge(domain_auth::router(auth))
        .merge(platform::events::dlq_http::dlq_router(dlq))
        .route("/openapi.json", axum::routing::get(|| async { Json(crate::openapi::api_doc()) }));
```
After building `app` (the root router with the `/api` nest), mount Swagger UI pointing at the served spec:
```rust
    let mut app = app.merge(SwaggerUi::new("/swagger-ui").url("/api/openapi.json", crate::openapi::api_doc()));
```
(Place the `SwaggerUi` merge on the root router so `/swagger-ui` is a root path, not under `/api`. `SwaggerUi::url` both registers the spec URL for the UI and can serve the doc; serving it under `/api/openapi.json` via the explicit route above keeps the spec under the API origin for the codegen step.)

- [ ] **Step 2: Verify the served spec via the api_router test**

Add to `crates/app/tests/api_router.rs` an assertion:
```rust
    let spec = app.clone().oneshot(Request::builder().uri("/api/openapi.json").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(spec.status(), StatusCode::OK);
```
(The existing `build(pool)` helper builds the router with `web_dist=None`; the openapi route is unaffected by that.)

- [ ] **Step 3: Run + gate**

Run: `cargo build --all-targets && DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p app && cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: PASS, clean.

- [ ] **Step 4: Commit**
```bash
git add crates/app
git commit -m "feat(openapi): serve /api/openapi.json + Swagger UI"
```

---

## Self-Review

**Spec coverage (design §2):** utoipa deps ✓ (T1); ToSchema on all DTOs + ReplayResponse ✓ (T2); handler annotations ✓ (T3 auth, T4 account, T5 dlq); aggregation in app with bearer + /api server ✓ (T6); openapi-gen bin ✓ (T6); served /api/openapi.json + Swagger UI ✓ (T7); aggregation test + served-spec test ✓ (T6/T7). Per-crate partial docs keep handlers crate-private (the `pub(crate)` fallback is noted where the derive needs visibility).

**Placeholder scan:** the only adaptive notes are version-API hedges (utoipa `merge` method name; `pub(crate)` if the derive can't see a handler) — these are real utoipa-version contingencies with a concrete fallback, not TBDs. Every code step shows complete code.

**Type consistency:** `ReplayResponse { replayed: bool }` defined in T2, referenced by T5's annotation + T6's schema list + the existing replay test. Partial `ApiDoc` names (`domain_auth::openapi::ApiDoc`, `domain_account::openapi::ApiDoc`, `platform::events::dlq_http::ApiDoc`) consistent between T3/4/5 and the T6 merge. `api_doc()` consumed by the bin (T6) and the served route (T7). Annotated paths are domain-relative; `/api` is the server base (T6).

**Cross-plan note:** Plan B consumes `cargo run --bin openapi-gen` output via `openapi-typescript`.
