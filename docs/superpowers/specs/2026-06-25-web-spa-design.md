# web SPA — Design (Spec 3)

**Date:** 2026-06-25
**Status:** Approved design, ready for implementation planning
**Scope of this spec:** A `web/` React SPA consuming the JSON API (login/register,
account view, and an `/admin/*` route group for users + the outbox DLQ), plus the
minimal backend endpoints it needs. Hand-written TS types now; the utoipa/OpenAPI typed
client is deferred to its own spec (design §12). Builds on Spec 1 (platform + outbox +
account) and Spec 2 (domain-auth).

---

## 1. Goal & guiding principles

Deliver the frontend the design doc §6 calls for: a **React SPA (Vite + TS + Tailwind +
shadcn)**, a separate `web/` app consuming the JSON APIs, with admin as a protected route
group `/admin/*` inside that one app (not a separate app). It mirrors the proven choices
of the Haskell template's `apps/admin-ui` while extending past admin-only to a
user-facing account view.

Principles:
- **Separate frontend app, JSON-API client.** No SSR (deliberately out of template per
  §6; documented branch for SEO-critical public surfaces).
- **Typed, hand-written API layer now.** Hand-written TS request/response types + a thin
  typed fetch client. The generated OpenAPI client (§12) is a cheap later retrofit.
- **Server is the real authority.** Client-side role/route gating is UX only; the backend
  enforces auth + scopes on every protected endpoint.
- **Small, focused units.** One responsibility per module (auth, fetch client, api
  modules, route guards, pages), each independently understandable and testable.

### Stack

| Concern | Choice |
|---|---|
| Build/dev | Vite |
| UI | React 19 + TypeScript |
| Styling | Tailwind CSS |
| Components | shadcn/ui (Radix primitives) + lucide icons |
| Routing | react-router-dom v7 |
| Server state | TanStack Query (react-query) |
| Toasts | sonner |
| Tests | Vitest + React Testing Library + jsdom + MSW |
| Lint/format | ESLint (typescript-eslint) + Prettier |

These match the Haskell `admin-ui`'s dependency choices (React 19, react-router 7,
radix/shadcn, tailwind, sonner, lucide), adding TanStack Query for server state.

---

## 2. Decisions (resolved during brainstorming)

1. **Scope: SPA + minimal backend, hand-written types.** Spec 3 delivers the SPA plus the
   small backend endpoints it requires. The utoipa/OpenAPI generated client is its own
   later spec (§12 says retrofitting is inexpensive).
2. **Data layer: TanStack Query + a thin typed fetch client.** Query/mutation hooks for
   caching, loading/error, refetch, and invalidation; a small fetch wrapper underneath.
3. **Token storage: access token in memory, refresh token in `localStorage`, silent
   refresh.** The backend issues tokens as JSON (no cookies), so the SPA stores them
   client-side. Access token never persisted; session survives reload by exchanging the
   refresh token on boot. Honest caveat: the refresh token in `localStorage` is
   XSS-exposed; true hardening needs httpOnly cookies (backend change) — documented as a
   future branch, out of scope here.
4. **Prod serving: the `app` crate serves the built SPA.** `vite build` → `web/dist`,
   served via `tower-http` `ServeDir` with an SPA fallback to `index.html`. Single origin,
   single deploy, no prod CORS. Dev uses the Vite dev server proxying to the API.
5. **API under `/api`.** To avoid SPA-route/API-route collisions on one origin (notably
   `/admin/dlq`), the whole API is mounted under `/api`; `/status` + `/metrics` stay at
   root for health/scraping.

---

## 3. `web/` layout

```
web/
  index.html  package.json  vite.config.ts  tsconfig.json
  tailwind.config.ts  postcss.config.js  components.json   # shadcn
  .eslintrc / .prettierrc
  .env.example                       # VITE_API_BASE_URL=/api
  src/
    main.tsx                         # root: QueryClientProvider, AuthProvider, RouterProvider
    App.tsx                          # route tree
    index.css                        # tailwind + shadcn theme tokens
    lib/
      fetchClient.ts                 # base URL, auth header, JSON parse, typed errors, 401->refresh
      queryClient.ts                 # TanStack Query client
      jwt.ts                         # decodeAccessToken(token) -> { sub, email, scopes, exp }
      utils.ts                       # cn() etc.
    auth/
      AuthProvider.tsx               # context: access token (memory) + refresh (localStorage) + boot refresh
      useAuth.ts                     # hook
      tokenStore.ts                  # module-level access-token holder the fetch client reads
      guards.tsx                     # <RequireAuth>, <RequireAdmin>
    api/
      types.ts                       # hand-written request/response types
      auth.ts                        # login, register, refresh, logout
      accounts.ts                    # getMe, listAccounts (admin)
      users.ts                       # listUsers, getUserScopes, setUserScopes, listScopes
      dlq.ts                         # listDeadLetters, replayDeadLetter
      hooks.ts                       # useMe, useUsers, useScopes, useDeadLetters, useLogin, ...
    components/
      ui/                            # shadcn primitives (button, input, card, table, dialog, badge, select, label, sonner, ...)
      AppLayout.tsx                  # top nav (email, logout, Admin link if admin) + <Outlet/>
    routes/
      LoginPage.tsx  RegisterPage.tsx
      AccountPage.tsx
      admin/UsersPage.tsx  admin/DlqPage.tsx
    test/
      setup.ts                       # jsdom + MSW server
      *.test.tsx
```

`web/node_modules` and `web/dist` are gitignored. `web/` is a plain npm app, not a cargo
crate (not in the workspace members).

---

## 4. Backend prerequisites (Plan 3a — Rust)

TDD'd additions so the SPA has a complete, gated API.

### 4.1 `GET /accounts/me` (`domain-account`)
Authenticated (any user). Resolves `auth_user_id` from the token `sub` (`user-{id}`),
returns the caller's `Account` via `find_by_auth_user_id`; `404` if none yet.

### 4.2 Gate `GET /accounts` (`domain-account`)
Add `Authenticated` + `require_scope("admin")` to `list_accounts` (currently un-gated).
`/accounts/me` serves the normal user.

### 4.3 DLQ admin endpoints (`platform`)
Expose the existing `platform::events::{list_dead_letters, replay_dead_letter}`:
- `GET  /admin/dlq` → `[DeadLetter]` (add `serde::Serialize` to the existing `DeadLetter`)
- `POST /admin/dlq/:delivery_id/replay` → `200 { "replayed": bool }`
Both `admin`-gated. New module `platform::events::dlq_http` providing the handlers + a
`DlqState { pool: Db, jwt: Arc<JwtVerifier>, revocation: Arc<dyn RevocationChecker> }`
(with the two `FromRef` impls the `Authenticated` extractor needs) and
`dlq_router(DlqState) -> axum::Router`. Keeps DLQ ops in `platform` (where the outbox
lives), reusing the extractor; integration-tested with `#[sqlx::test]` (seed a `dead`
delivery → list → replay → assert `pending`) plus the 401/403 gating paths.

### 4.4 API mounting + serving (`app`)
- Build the API router = `domain_account::router` ⊕ `domain_auth::router` ⊕ `dlq_router`,
  and mount under `/api` via `.nest("/api", api_router)`.
- Move `/status` + `/metrics` to the **app root** (drop them from `domain-account`'s
  router, as was already done for `domain-auth` in Spec 2). The app registers
  `GET /status` (→ `platform::server::status_handler`) and `GET /metrics` (renders the
  shared `Metrics`) at root. Note: `domain-account`'s `tests/http.rs` currently has a
  `status_returns_ok` test hitting the domain router's `/status`; removing that route
  means Plan 3a must drop/relocate that test (and the now-unused `metrics_handler` +
  imports in `domain-account`), keeping the suite green.
- Serve the SPA: `.fallback_service(ServeDir::new("web/dist").not_found_service(ServeFile::new("web/dist/index.html")))`
  (or equivalent SPA fallback) so unknown non-API paths return `index.html`.
- Middleware order unchanged (CORS → correlation-id → routes); the layers apply to the
  whole router. The dev CORS origin `:5173` stays (used in dev only).
- A `web/dist` may not exist in dev/CI; guard the `ServeDir` so the binary still runs for
  API-only/integration tests (e.g. only attach the fallback if the dir exists, or ship a
  placeholder `index.html`). Integration tests target `/api/...` and `/status`.

---

## 5. SPA architecture (Plans 3b/3c — frontend)

### 5.1 Auth/session (`auth/`)
- `tokenStore.ts`: a module-level holder for the current access token; the fetch client
  reads it. `AuthProvider` keeps it and React state in sync.
- `AuthProvider`: access token in state; refresh token in `localStorage`
  (`<app>:refresh_token`). On mount, if a refresh token exists, call `POST /auth/refresh`
  once (silent login) and populate the access token; render a splash until it resolves so
  guards don't bounce a logged-in user to `/login`.
- `login(email,password)` / `register(email,password)`: POST, then store access (memory)
  + refresh (localStorage). `logout()`: `POST /auth/logout` (refresh + access) then clear
  both + reset query cache.
- `jwt.ts`: `decodeAccessToken` (no signature verification — UX only) → `{ sub, email,
  scopes, exp }`. `useAuth()` exposes `{ user: {email, scopes} | null, isAdmin, login,
  register, logout, status }`.

### 5.2 Fetch client (`lib/fetchClient.ts`)
- Prepends `VITE_API_BASE_URL` (default `/api`), attaches `Authorization: Bearer
  <access>` from `tokenStore`, parses JSON, throws a typed `ApiError { status, message }`
  on non-2xx.
- **401 handling:** single-flight refresh — the first 401 triggers one `POST /auth/refresh`;
  concurrent 401s await the same in-flight refresh. On success, retry the original request
  once with the new token. On refresh failure, clear the session → redirect to `/login`.
  The login/register/refresh calls themselves bypass the 401-refresh loop.

### 5.3 Data layer (`api/`)
- `QueryClientProvider` at root. Query hooks: `useMe` (`GET /accounts/me`), `useUsers`,
  `useScopes`, `useDeadLetters`. Mutations: `useLogin`, `useRegister`, `useLogout`,
  `useSetUserScopes`, `useReplayDeadLetter` — invalidate related queries on success,
  sonner toast on success/error.
- `api/types.ts` hand-written types mirror the Rust DTOs: `AuthTokens`, `Account`,
  `UserWithScopes`, `ScopeInfo`, `DeadLetter`, request bodies. (These are exactly the
  types the future OpenAPI client will replace.)

### 5.4 Routing & pages (`routes/`, react-router v7)
- Public: `/login`, `/register`.
- Protected app shell `AppLayout` (top nav: email, logout, "Admin" link iff admin):
  - `/` → **AccountPage** (`useMe`; renders email / account id / created_at; graceful
    "no account yet" on 404).
- `/admin/*` (requires `admin` scope):
  - `/admin/users` → **UsersPage**: shadcn table (id, email, scope badges) + "Edit scopes"
    dialog (multi-select from `useScopes`, `PUT /users/:id/scopes`).
  - `/admin/dlq` → **DlqPage**: shadcn table (subscriber, event_type, aggregate_id,
    attempts, last_error, payload) + "Replay" button per row.
- Guards: `<RequireAuth>` (waits for boot refresh; redirects to `/login` if no session),
  `<RequireAdmin>` (redirects/403 if `scopes` lacks `admin`).

---

## 6. Error handling

- API errors surface as typed `ApiError`; mutations show sonner toasts; queries show
  inline error states.
- `401` is handled transparently by the fetch client (refresh/retry) and only bubbles to
  the user (→ `/login`) when refresh fails.
- `403` (server rejects an under-scoped call that slipped past client gating) shows a
  "not authorized" message — defense-in-depth proof that the server is the authority.
- `404` on `/accounts/me` is an expected state (user without an account yet), rendered as
  an empty/onboarding card, not an error toast.

---

## 7. Testing

- **Backend (Plan 3a):** `#[sqlx::test]` integration tests for `/accounts/me` (owner
  resolution + 404), `/accounts` admin gating (200 admin / 403 non-admin / 401 no token),
  and the DLQ router (seed dead → list → replay → pending; gating paths). Whole workspace
  suite stays green; clippy + fmt clean.
- **Frontend (Plans 3b/3c):** Vitest + React Testing Library + jsdom, **MSW** mocking the
  API at the network layer. Focused coverage:
  - fetch client: 401 → single-flight refresh → retry; refresh-fail → logout/redirect.
  - `AuthProvider`: boot silent-refresh, login, logout.
  - guards: `RequireAuth` and `RequireAdmin` redirect logic.
  - one admin mutation end-to-end against MSW (replay DLQ or set-scopes) incl. cache
    invalidation.
- **Deferred:** Playwright browser e2e (noted as a future addition).

---

## 8. Tooling / config

- `web/.env.example`: `VITE_API_BASE_URL=/api`. Dev: either Vite proxy (`/api` →
  `http://localhost:8080`) or direct CORS to `:8080` (already allowed).
- `Makefile`: `web-install` (`npm --prefix web ci`), `web-dev`, `web-build`, `web-test`,
  `web-lint`.
- `.gitignore`: add `web/node_modules`, `web/dist`.
- README quick-start: add `make web-install` / `make web-dev`; note `make web-build`
  before `make run` to have the app serve the SPA in prod mode.
- `docker-compose` + `infra/prometheus.yml` unchanged (`/metrics` still at root).

---

## 9. Plan decomposition (for writing-plans)

- **3a — backend prerequisites (Rust):** `GET /accounts/me`; gate `GET /accounts`;
  `DeadLetter: Serialize` + `platform::events::dlq_http` (`dlq_router` + `DlqState`);
  app `/api` nest + root `/status`+`/metrics` + SPA `ServeDir` fallback. Integration-tested.
- **3b — SPA foundation:** Vite/React/TS/Tailwind/shadcn scaffold; `tokenStore` +
  `fetchClient` (401 refresh) + `queryClient` + `jwt`; `AuthProvider` + `useAuth` +
  guards; `api/` modules + types + hooks; `AppLayout`; `/login` + `/register` +
  `AccountPage`; MSW + tests for the auth/fetch/guard logic.
- **3c — admin route group:** `/admin/users` (table + edit-scopes dialog) and
  `/admin/dlq` (table + replay); `RequireAdmin`; admin-mutation tests.

---

## 10. Out of scope / future

- utoipa/OpenAPI generated typed client (§12 — its own spec; replaces the hand-written
  `api/types.ts`).
- httpOnly-cookie auth hardening (needs backend cookie issuance).
- Playwright e2e.
- SSR frontend for SEO-critical public surfaces (§6 documented branch).
- `domain-notification` UI (Spec 4) — the Haskell `admin-ui` Notifications page has no
  backend yet here.
- Pagination/filtering on the users + DLQ tables (add when data volume warrants).
