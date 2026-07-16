# Refresh-Token Hardening (httpOnly cookie) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Move the refresh token out of `localStorage` into an httpOnly, `Secure`, `SameSite=Strict` cookie; the SPA keeps only the access token in memory.

**Architecture:** The backend sets/reads the refresh token via a cookie (`rt`, `Path=/api/auth`). Every browser-facing token response drops `refresh_token` from its JSON body and returns a new `AccessTokenResponse` instead. `/auth/refresh` and `/auth/logout` read the cookie rather than the request body. The SPA sends `credentials: "include"`, stores nothing in `localStorage`, and bootstraps a session by always attempting a refresh (the cookie is invisible to JS).

**Tech Stack:** Rust (axum 0.7, `axum-extra` cookie jar, sqlx 0.8 runtime API), React 19 + TanStack Query + generated OpenAPI types, vitest + msw.

## Global Constraints

- **Not deployed yet** — no back-compat for existing tokens; freely change the wire shape.
- **Refresh is cookie-only.** No browser-facing endpoint returns `refresh_token` in JSON.
- **No rotation/reuse-detection** — refresh keeps echoing the same jti (as today); only its transport changes.
- Cookie attributes are **exactly**: `HttpOnly`, `Secure`, `SameSite=Strict`, `Path=/api/auth`, `Max-Age = refresh TTL seconds`. Name: `rt`.
- sqlx runtime query API only. `cargo fmt --all` + `cargo clippy --all-targets -- -D warnings` clean before each Rust commit.
- Web checks: `CI=true npm --prefix web run lint` / `run build` / `test`, all clean.
- `#[sqlx::test(migrations = "../../migrations")]`. DB env for tests: `PATH` includes `/opt/homebrew/opt/postgresql@17/bin`, `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres`.
- `make gen-api` regenerates `web/src/api/schema.d.ts`; commit it (openapi-drift CI must stay green).
- One commit per task. Every commit message ends with a blank line then: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

### Task 1: Backend cookie infra + core session endpoints

**Files:**
- Modify: `Cargo.toml` (workspace deps), `crates/domain-auth/Cargo.toml`
- Modify: `crates/domain-auth/src/ports/dto.rs`
- Modify: `crates/domain-auth/src/ports/http.rs`
- Modify: `crates/domain-auth/src/openapi.rs`
- Test: `crates/domain-auth/tests/http.rs`, `crates/domain-auth/tests/refresh_logout.rs`

**Interfaces:**
- Produces: `AccessTokenResponse { access_token: String, token_type: String, expires_in: i64 }` (Serialize + ToSchema). `LoginResponse::Authenticated { tokens: AccessTokenResponse }`. A cookie helper module. `issue_session(state, user, amr) -> Result<(AccessTokenResponse, String /* refresh jwt */), AppError>`.
- Consumes (unchanged): `state.issuer.issue_access` / `issue_refresh` / `access_ttl_seconds`, `RefreshTokenRepository::{store,revoke,find_by_jti}`, `state.verifier.decode::<RefreshClaims>`.

**Context for the implementer:** Today `issue_token_pair` (http.rs ~88) returns `AuthTokens { access_token, refresh_token, token_type, expires_in }` and persists the refresh jti. Handlers `register` (~124, returns `(201, Json<AuthTokens>)`), `login` (~158, `Json<LoginResponse>`), `refresh` (~217, reads `RefreshRequest` body → `Json<AuthTokens>`), `logout` (~265, reads `LogoutRequest` body). `refresh` currently echoes the same refresh token (no rotation) — preserve that.

- [ ] **Step 1: Add the cookie dependency**

In root `Cargo.toml` `[workspace.dependencies]` add:
```toml
axum-extra = { version = "0.9", features = ["cookie"] }
```
In `crates/domain-auth/Cargo.toml` add under `[dependencies]`: `axum-extra = { workspace = true }`.
Run `cargo build -p domain-auth` to confirm it resolves. (If 0.9 is incompatible with the pinned axum 0.7, pick the `axum-extra` version whose axum-0.7 support matches — verify by building.)

- [ ] **Step 2: Add the DTOs**

In `dto.rs`: add
```rust
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AccessTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
}
```
Change `LoginResponse::Authenticated` to `{ tokens: AccessTokenResponse }`. Delete `RefreshRequest` (refresh now reads the cookie). In `LogoutRequest`, remove `refresh_token` (keep `access_token: Option<String>`).

- [ ] **Step 3: Add a cookie helper**

Add a small module (e.g. `crates/domain-auth/src/auth/cookie.rs`, exported from `auth/mod.rs`) with the constants and builders:
```rust
use axum_extra::extract::cookie::{Cookie, SameSite};

pub const RT_COOKIE: &str = "rt";
pub const RT_PATH: &str = "/api/auth";

/// The refresh-token cookie: httpOnly, Secure, SameSite=Strict, scoped to /api/auth.
pub fn rt_cookie(value: String, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build((RT_COOKIE, value))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .path(RT_PATH)
        .max_age(time::Duration::seconds(max_age_secs))
        .build()
}

/// A removal cookie (same name/path, expired) to clear the refresh token.
pub fn clear_rt_cookie() -> Cookie<'static> {
    Cookie::build((RT_COOKIE, ""))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .path(RT_PATH)
        .max_age(time::Duration::seconds(0))
        .build()
}
```
(`axum-extra`'s cookie feature re-exports `time`; if `time` isn't directly available, use `axum_extra::extract::cookie` types and the crate's own duration path — verify while building. The `max_age` on the refresh TTL should come from the issuer's refresh TTL; if no `refresh_ttl_seconds()` accessor exists, add one mirroring `access_ttl_seconds()`.)

- [ ] **Step 4: Refactor issuance into `issue_session`**

Replace `issue_token_pair` with `issue_session` returning the access response **and** the raw refresh jwt string (still persisting the jti exactly as before):
```rust
pub async fn issue_session(
    state: &AuthState,
    user: &User,
    amr: Vec<String>,
) -> Result<(AccessTokenResponse, String), AppError> {
    // ... same scope resolution + issue_access + issue_refresh + refresh_tokens.store ...
    Ok((AccessTokenResponse { access_token, token_type: "Bearer".into(), expires_in: state.issuer.access_ttl_seconds() }, refresh_token))
}
```

- [ ] **Step 5: Wire `register` and `login` to set the cookie**

`register` returns `(StatusCode, CookieJar, Json<AccessTokenResponse>)`:
```rust
    let (tokens, refresh) = issue_session(&state, &user, vec!["pwd".into()]).await?;
    let jar = jar.add(rt_cookie(refresh, state.issuer.refresh_ttl_seconds()));
    Ok((StatusCode::CREATED, jar, Json(tokens)))
```
(Add `jar: CookieJar` to the handler args — `use axum_extra::extract::cookie::CookieJar;`.)

`login` returns `(CookieJar, Json<LoginResponse>)`. In the `Authenticated` branch, `issue_session` + `jar.add(rt_cookie(...))`; in the `MfaRequired` branches leave the jar unchanged. Return `(jar, Json(response))`.

- [ ] **Step 6: Rewrite `refresh` to read the cookie**

Handler args: `State(state)`, `jar: CookieJar`. Read `jar.get(RT_COOKIE)`; missing → `AppError::Unauthorized`. Validate the token exactly as today (decode `RefreshClaims`, `token_type == "refresh"`, `find_by_jti`, not revoked, user exists). Issue a fresh access token (no rotation — reuse the same refresh value), **re-set** the cookie (`jar.add(rt_cookie(same_refresh_value, refresh_ttl))`) for sliding expiration, return `(jar, Json(AccessTokenResponse { ... }))`. Update the `#[utoipa::path]`: drop `request_body`, response body `AccessTokenResponse`.

- [ ] **Step 7: Rewrite `logout` to read + clear the cookie**

Handler args: `State(state)`, `jar: CookieJar`, `Json(body): Json<LogoutRequest>`. Read `rt` from the cookie (not the body); if it decodes as a refresh token, revoke the jti (idempotent). Keep the existing access-token denylist path from `body.access_token`. Return `(jar.remove(clear_rt_cookie()), StatusCode::NO_CONTENT)`. Update `#[utoipa::path]` request_body to the trimmed `LogoutRequest`.

- [ ] **Step 8: Update the OpenAPI registration**

In `crates/domain-auth/src/openapi.rs`, register `AccessTokenResponse` as a component and drop `RefreshRequest`; ensure the changed response bodies compile.

- [ ] **Step 9: Update/extend the backend tests**

In `crates/domain-auth/tests/http.rs` and `refresh_logout.rs` (adapt to the real existing test helpers there):
- login (Authenticated) response: **no** `refresh_token` in the JSON body; a `Set-Cookie` header for `rt` containing `HttpOnly`, `Secure`, `SameSite=Strict`, `Path=/api/auth`.
- register: same cookie + no refresh in body.
- refresh: with the `rt` cookie from a prior login → 200 + a fresh `access_token` + a `Set-Cookie` re-setting `rt`; **without** the cookie → 401.
- logout: with the cookie → 204, a `Set-Cookie` clearing `rt` (`Max-Age=0`), and the jti is revoked (a subsequent refresh with that cookie → 401).

For asserting `Set-Cookie`, read the response header (`response.headers().get_all("set-cookie")`); for round-tripping the cookie into the next request, extract the `rt=` value from the login response's `Set-Cookie` and send it as a `Cookie` request header. Follow whatever request/response helper the existing tests use (e.g. a `TestApp` or tower `oneshot`); do not invent a new harness.

- [ ] **Step 10: Run tests, fmt, clippy, commit**

`DATABASE_URL=… cargo test -p domain-auth` (green), `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings` (clean).
```bash
git add Cargo.toml Cargo.lock crates/domain-auth/
git commit -m "feat(auth): refresh token via httpOnly cookie (register/login/refresh/logout)"
```

---

### Task 2: Backend — MFA endpoints set the cookie

**Files:**
- Modify: `crates/domain-auth/src/ports/dto.rs` (`MfaConfirmResponse.tokens`)
- Modify: `crates/domain-auth/src/ports/http.rs` (`mfa_verify`, `mfa_confirm`)
- Modify: `crates/domain-auth/src/openapi.rs` if needed
- Test: `crates/domain-auth/tests/mfa.rs`

**Interfaces:**
- Consumes: `issue_session`, `rt_cookie`, `CookieJar` from Task 1.
- Produces: `mfa_verify` returns `(CookieJar, Json<AccessTokenResponse>)`; `MfaConfirmResponse.tokens: Option<AccessTokenResponse>` and `mfa_confirm` sets the cookie on the login-enroll path.

**Context:** `mfa_verify` (http.rs ~448) currently returns `Json<AuthTokens>` after a successful second factor. `mfa_confirm` (~385) returns `MfaConfirmResponse { recovery_codes, tokens: Option<AuthTokens> }`; it sets `tokens = Some(...)` only when `from_mfa_token` (the login-flow enroll), and `None` for self-service enrollment.

- [ ] **Step 1: Change `MfaConfirmResponse.tokens` type**

`tokens: Option<AccessTokenResponse>` (keep `skip_serializing_if = "Option::is_none"`).

- [ ] **Step 2: `mfa_verify` sets the cookie**

Add `jar: CookieJar` to the args; replace the `issue_token_pair` call with `issue_session`, `jar.add(rt_cookie(refresh, refresh_ttl))`, return `(jar, Json(access_response))`.

- [ ] **Step 3: `mfa_confirm` sets the cookie on login-enroll only**

Add `jar: CookieJar`. When `from_mfa_token`: `issue_session` → `tokens = Some(access_response)` and `jar.add(rt_cookie(...))`. When self-service (not `from_mfa_token`): `tokens = None`, jar unchanged. Return `(jar, Json(MfaConfirmResponse { recovery_codes, tokens }))`.

- [ ] **Step 4: Tests**

In `crates/domain-auth/tests/mfa.rs` (adapt to existing helpers): a successful `mfa_verify` returns a body with **no** `refresh_token` and sets the `rt` cookie; `mfa_confirm` on the login-enroll path (driven with an `mfa_enroll` token) sets the `rt` cookie and returns `tokens` without a `refresh_token`; self-service `mfa_confirm` (session token) sets **no** `rt` cookie and `tokens` is absent.

- [ ] **Step 5: Run tests, fmt, clippy, commit**

`DATABASE_URL=… cargo test -p domain-auth` green; fmt + clippy clean.
```bash
git add crates/domain-auth/
git commit -m "feat(auth): set refresh cookie on MFA verify and login-enroll confirm"
```

---

### Task 3: Frontend — cookie-based refresh, no localStorage

**Files:**
- Modify: `web/src/api/schema.d.ts` (regenerated), `web/src/api/types.ts`, `web/src/api/auth.ts`
- Modify: `web/src/auth/tokenStore.ts`, `web/src/lib/fetchClient.ts`, `web/src/auth/AuthProvider.tsx`
- Test: `web/src/lib/fetchClient.test.ts`, `web/src/auth/AuthProvider.test.tsx`

**Interfaces:**
- Consumes: the regenerated schema with `AccessTokenResponse`, `LoginResponse.Authenticated.tokens: AccessTokenResponse`, register/refresh/mfa_verify → `AccessTokenResponse`, `MfaConfirmResponse.tokens?: AccessTokenResponse`.

- [ ] **Step 1: Regenerate the typed client**

Run `make gen-api` (requires Tasks 1–2 merged on the branch). Confirm `AccessTokenResponse` exists in `schema.d.ts` and `AuthTokens` is gone (or no longer referenced by the auth responses).

- [ ] **Step 2: `tokenStore` — memory only**

Rewrite `tokenStore.ts` to keep only the in-memory access token; remove `REFRESH_KEY`, `getRefreshToken`, `setRefreshToken`, and the `localStorage` calls in `clear`:
```ts
let accessToken: string | null = null;
export const tokenStore = {
  getAccessToken: () => accessToken,
  setAccessToken: (t: string | null) => { accessToken = t; },
  clear: () => { accessToken = null; },
};
```

- [ ] **Step 3: `fetchClient` — credentials + cookie-based refresh**

In `raw()`, add `credentials: "include"` to the `fetch` options. Rewrite `refreshAccessToken` to POST `/auth/refresh` with `credentials: "include"`, **no body**, no `getRefreshToken` gate:
```ts
async function refreshAccessToken(): Promise<boolean> {
  const res = await fetch(`${BASE}/auth/refresh`, { method: "POST", credentials: "include" });
  if (!res.ok) return false;
  const data = await res.json();
  tokenStore.setAccessToken(data.access_token);
  return true;
}
```
Keep the single-flight logic in `apiFetch` unchanged.

- [ ] **Step 4: `api/auth` signatures**

`register` returns `AccessTokenResponse`; `logout` takes only `access_token: string | null` (drop `refresh_token`), body `{ access_token }`; `login` still returns `LoginResponse`. Update `types.ts`: export `AccessTokenResponse` from the schema, drop the now-unused `AuthTokens` export (update any importers).

- [ ] **Step 5: `AuthProvider` — always bootstrap via refresh**

- `applyTokens(access: string)` (drop the `refresh` param); `applySession` takes `AccessTokenResponse`.
- Initial `status` is always `"loading"` (JS can't see the cookie).
- The bootstrap effect **always** attempts refresh with `credentials: "include"` and no body; on success `applyTokens(d.access_token)`, on failure clear + set null; `finally` set `"ready"`.
- `login`: `applyTokens(res.tokens.access_token)`. `register`: `applyTokens((await authApi.register(...)).access_token)`. `logout`: `authApi.logout(tokenStore.getAccessToken())` then clear.

- [ ] **Step 6: Update the failing tests, then the rest of the suite**

- `fetchClient.test.ts`: a request sends `credentials: "include"`; a 401 triggers a single refresh that POSTs to `/auth/refresh` **with no body**, then retries; nothing is ever written to `localStorage`. (Update the existing bearer test as needed — the bearer override path is unchanged.)
- `AuthProvider.test.tsx`: bootstrap restores a session when the mocked `/auth/refresh` returns an access token (no localStorage seed needed), and lands logged-out when it 401s; login/register apply only the access token. Update the login mock to the `AccessTokenResponse` shape (Authenticated.tokens has no `refresh_token`).

- [ ] **Step 7: Full web gate + commit**

`CI=true npm --prefix web run lint` / `run build` / `test` all green.
```bash
git add web/
git commit -m "feat(web): cookie-based refresh, drop refresh token from localStorage"
```
