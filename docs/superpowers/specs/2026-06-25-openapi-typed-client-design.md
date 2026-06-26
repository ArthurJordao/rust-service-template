# OpenAPI Schema + Typed TS Client (utoipa) — Design

**Date:** 2026-06-25
**Status:** Approved design (decisions delegated), ready for implementation planning
**Scope of this spec:** Annotate the axum API with `utoipa` to produce a single OpenAPI
document, serve it (+ Swagger UI), and generate TypeScript types the SPA's existing
fetch client is typed against — so a wrong path, wrong params, or mismatched
request/response body is a compile-time error. Resolves design doc §12.

---

## 1. Goal & decisions

The goal (design §12): compile-time schema guarantees between the React SPA and the Rust
API for the *whole calling contract*, not just shared types.

Decisions (the §12 open questions, resolved):
1. **utoipa / OpenAPI**, not ts-rs — we want endpoint contracts (path/params/body), not
   just type shapes.
2. **`openapi-typescript` (types only), NOT `openapi-fetch`.** The SPA already has a
   custom `fetchClient` (Spec 3b) with single-flight 401→refresh, the `X-Correlation-Id`
   header, and `ApiError`. We generate **types** and type the existing client + api
   modules against them, rather than adopting openapi-fetch's client (which would force
   reimplementing that logic as middleware and changing `ApiError`). We still get
   wrong-path/param/body compile errors by typing `apiFetch` against the generated
   `paths`.
3. **Spec emitted by a small `openapi-gen` binary** (`ApiDoc::openapi().to_pretty_json()`
   — builds statically from annotations, no DB/server). Also served at runtime
   (`GET /api/openapi.json`) with **Swagger UI** at `/swagger-ui` for humans.
4. The generated `schema.d.ts` is **committed**, so the frontend builds without the
   backend; a `make gen-api` target regenerates it.

Non-goals: adopting openapi-fetch; API versioning; runtime contract/consumer-driven tests
beyond build-time type-checking; generating a client for any non-TS consumer.

---

## 2. Backend (Rust) — Plan A

### 2.1 Dependencies
Workspace: `utoipa = { version = "5", features = ["axum_extras", "chrono"] }`,
`utoipa-swagger-ui = { version = "8", features = ["axum"] }` (versions pinned at
implementation time to the current releases).

### 2.2 Annotate DTOs (`#[derive(ToSchema)]`)
On every request/response type: `Account`, `RegisterRequest`, `LoginRequest`,
`RefreshRequest`, `LogoutRequest`, `AuthTokens`, `UserWithScopes`, `SetScopesRequest`,
`ScopeRow`, `DeadLetter`, and a **new typed `ReplayResponse { replayed: bool }`** that
replaces the `json!({"replayed": ...})` literal in the DLQ replay handler (this also
clears the earlier review nit about the untyped reply). `ToSchema` derives live next to
the structs in their crates (`domain-account`, `domain-auth`, `platform`).

### 2.3 Annotate handlers (`#[utoipa::path]`)
Each handler gets a `#[utoipa::path(METHOD, path = "…", responses(...), request_body?,
params?, security?, tag = "…")]`:
- auth: `/auth/register|login|refresh|logout`, `/scopes`, `/users`, `/users/{id}/scopes`
  (get/put) — tag `auth`/`admin`.
- account: `/accounts`, `/accounts/{id}`, `/accounts/me` — tag `accounts`.
- dlq: `/admin/dlq`, `/admin/dlq/{id}/replay` — tag `admin`.
Paths are written **domain-relative** (e.g. `/auth/login`); the aggregated doc declares a
server with base `/api` so generated client paths resolve to the real `/api/...` mounts.
Admin/authed operations declare a `bearer_auth` security requirement.

### 2.4 Aggregate in `app`
A `#[derive(OpenApi)] struct ApiDoc` in `app` with `paths(...)` listing every handler,
`components(schemas(...))` listing every DTO, a `bearer_auth` SecurityScheme (HTTP bearer,
JWT), and a `servers([("/api")])` entry. Two outputs:
- **`openapi-gen` bin** (`crates/app/src/bin/openapi_gen.rs`): prints
  `ApiDoc::openapi().to_pretty_json()` to stdout. Deterministic, no DB.
- **Runtime:** `build_router` mounts `GET /api/openapi.json` (serves `ApiDoc::openapi()`)
  and Swagger UI at `/swagger-ui` (utoipa-swagger-ui).

### 2.5 Tests
- a unit test asserting `ApiDoc::openapi()` builds and its JSON contains the expected
  operationIds/paths (`/auth/login`, `/accounts/me`, `/admin/dlq`) and schema names
  (`AuthTokens`, `Account`, `DeadLetter`) — guards against a handler/DTO being dropped
  from the aggregation.
- `cargo run --bin openapi-gen` emits parseable JSON (smoke).

---

## 3. Frontend (TS) — Plan B

### 3.1 Generate types
`make gen-api`:
```
cargo run --quiet --bin openapi-gen > web/openapi.json
npx openapi-typescript web/openapi.json -o web/src/api/schema.d.ts
```
`schema.d.ts` is committed; `web/openapi.json` is gitignored (a build artifact).
Add `openapi-typescript` as a `web` devDependency.

### 3.2 Retype the client + api modules against the schema
- Replace hand-written `web/src/api/types.ts` with thin aliases pulled from the generated
  `components["schemas"]` (e.g. `export type Account = components["schemas"]["Account"]`),
  so existing imports keep working but are now schema-derived.
- Type `apiFetch` so the path argument is constrained to `keyof paths` and the
  request/response bodies are inferred from the schema for that path+method. The api
  modules (`auth.ts`, `accounts.ts`, `users.ts`, `dlq.ts`) then get full path/param/body
  checking. The custom 401/cid/`ApiError` internals are unchanged.
- A deliberately wrong path or body shape now fails `tsc` (`npm run build`).

### 3.3 Tests
- `npm run build` (tsc) passing against the generated types IS the contract proof (the
  retyped api modules won't compile on a mismatch). Keep the existing Vitest/MSW suite
  green. Optionally add a type-level test (`expectTypeOf`) for one endpoint.

---

## 4. Workflow & drift

- `make gen-api` regenerates `schema.d.ts` from the live annotations. Run it after any
  DTO/route change.
- **Drift guard (optional, recommended):** a CI/`make` check that runs `gen-api` and fails
  if `git diff --exit-code web/src/api/schema.d.ts` is non-empty — i.e. the committed
  types are stale vs the Rust annotations. Documented; wiring it into CI is out of scope
  for this spec (no CI yet).

---

## 5. Plan decomposition (for writing-plans)

- **Plan A — backend:** utoipa deps; `ToSchema` on all DTOs + `ReplayResponse`;
  `#[utoipa::path]` on all handlers; `ApiDoc` aggregation in `app` with bearer security +
  `/api` server; `openapi-gen` bin; serve `/api/openapi.json` + `/swagger-ui`; the
  aggregation test.
- **Plan B — frontend:** `make gen-api`; `openapi-typescript` devDep; generate + commit
  `schema.d.ts`; retype `api/types.ts` + api modules + `apiFetch` against the schema;
  confirm `npm run build`/lint/tests green.

Plan B depends on Plan A (it consumes the emitted spec).

---

## 6. Risks / notes

- **Annotation boilerplate** (the §12 cost): every handler/DTO gains attributes. Mitigated
  by the aggregation test catching omissions.
- **Path prefix:** utoipa paths are domain-relative; the `/api` base lives in the `servers`
  entry so the generated client targets `/api/...`. If `openapi-typescript`'s path keys
  must include `/api`, fold the prefix into the annotated paths instead — decide at
  implementation time by inspecting the generated `paths` keys (the retype step will make
  the right choice obvious).
- **utoipa + axum 0.7:** use the `axum_extras` feature; handler annotations are
  framework-agnostic (they describe the contract, not the routing), so they don't disturb
  the existing `Router` wiring.
