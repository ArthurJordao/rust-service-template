# Refresh-Token Hardening (httpOnly cookie) — Design

**Date:** 2026-07-12
**Status:** Approved design, ready for implementation planning
**Scope:** Move the refresh token out of browser-readable `localStorage` into an
httpOnly, `Secure`, `SameSite=Strict` cookie. The SPA keeps only the short-lived
access token in memory. Backend + SPA change; merged together so login never breaks.

---

## 1. Goal & principles

Today the SPA stores the refresh token in `localStorage` (`tokenStore.ts`), so any
XSS can exfiltrate it and mint access tokens indefinitely. Moving it to an httpOnly
cookie makes it unreadable to JavaScript while keeping the SPA's silent-refresh UX.

Principles:
- **Access token stays in memory** (unchanged); **refresh token becomes cookie-only**
  — never in JSON, `localStorage`, or JS reach.
- **Same-origin serving** (the app serves `web/dist`; the Vite dev server proxies
  `/api` → `:8080`) lets us use `SameSite=Strict`, which neutralizes CSRF on the
  refresh endpoint. Every other mutation authenticates with the `Authorization`
  header (access token), which is immune to CSRF. **No CSRF token needed.**
- **No rotation/reuse-detection** in this spec (the simpler chosen option): refresh
  keeps echoing the same refresh jti, exactly as today — only its transport changes.
- **Not deployed yet** → no back-compat/migration concerns for existing tokens.

---

## 2. Decisions

1. **Cookie attributes:** `Set-Cookie: rt=<refresh_jwt>; HttpOnly; Secure;
   SameSite=Strict; Path=/api/auth; Max-Age=<refresh_ttl_seconds>`. Name `rt`.
   `Secure` is honored on `localhost` (treated as a secure context), so dev over
   http works; the Vite proxy (`changeOrigin`) keeps it same-origin at `:5173`.
2. **Cookie-only refresh.** Login / refresh / MFA-complete responses set the cookie
   and **omit `refresh_token` from the JSON body**. Non-browser clients (mobile/CLI)
   would need a separate bearer-refresh path later — out of scope; this template is
   browser-first.
3. **`Path=/api/auth`** so the cookie is only sent to the auth routes, not on every
   API call.
4. **Bootstrap:** on app load the SPA attempts a refresh (the cookie may exist even
   with no in-memory access token) to restore the session.

---

## 3. Backend changes (`domain-auth`)

Cookies are set/read via `axum`'s header plumbing (a small helper; or the
`axum-extra` `CookieJar` — decide in the plan, but keep it dependency-light).

- **`issue_token_pair` callers → set-cookie:** the three places that currently return
  a refresh token to the browser must instead attach the `rt` cookie and return a
  body **without** the refresh token:
  - `login` — the `LoginResponse::Authenticated { tokens }` branch.
  - `mfa_verify` and `mfa_confirm` (login-flow enroll) — where they apply/return the
    token pair after a successful second factor.
- **`LoginResponse::Authenticated` / token DTO:** the browser-facing token payload
  drops `refresh_token`, keeping `{ access_token, token_type, expires_in }`. Introduce
  an `AccessTokenResponse` (or trim `AuthTokens`) for these responses. `AccessClaims`
  and issuance are unchanged.
- **`POST /auth/refresh`:** stop taking `RefreshRequest` from the body; read `rt` from
  the cookie. Same validation as today (type == "refresh", found, not revoked, user
  exists), issue a fresh access token, **re-set the `rt` cookie** (sliding
  expiration), return `AccessTokenResponse`. Missing/!parse cookie → 401.
- **`POST /auth/logout`:** read `rt` from the cookie (not the body), revoke that jti
  (idempotent), and clear the cookie (`Set-Cookie: rt=; Max-Age=0; Path=/api/auth`).
- **OpenAPI:** update the three route response bodies; regenerate `schema.d.ts`
  (openapi-drift CI stays green). Document the cookie in the route descriptions.

## 4. Frontend changes (`web/`)

- **`tokenStore.ts`:** remove all `localStorage` refresh handling — keep only the
  in-memory access token. Drop `getRefreshToken`/`setRefreshToken`.
- **`fetchClient.ts`:** add `credentials: "include"` so the `rt` cookie rides along on
  `/api/auth/*` calls. The 401→refresh single-flight stays, but `refresh()` now POSTs
  to `/api/auth/refresh` with **no body** and reads the new access token from the JSON.
- **`api/auth.ts`:** `login`/`refresh`/`logout` signatures drop the refresh token;
  `logout` sends no body (cookie carries it).
- **`AuthProvider.tsx`:** `applyTokens`/`applySession` take only the access token;
  bootstrap calls `refresh()` on load to restore a session from the cookie; MFA verify
  and enroll-complete apply just the access token (the cookie is already set by the
  backend response).

## 5. Error handling

- No cookie on `/auth/refresh` → 401 → SPA drops to logged-out (login page). Same as
  today's "refresh failed" path.
- `logout` with no/garbage cookie → still 204 (idempotent), cookie cleared.
- Bootstrap refresh 401 on first load → treated as "not logged in", no error toast.

## 6. Testing strategy

- **Backend (`domain-auth` tests):** login sets an httpOnly `rt` cookie with the right
  attributes and the body has no `refresh_token`; `/auth/refresh` with the cookie
  returns a fresh access token and re-sets the cookie; `/auth/refresh` with no cookie
  → 401; logout clears the cookie and revokes the jti; MFA verify/enroll-complete set
  the cookie.
- **Frontend (vitest + msw):** `fetchClient` sends `credentials: "include"` and
  refresh posts no body; nothing is written to `localStorage`; a 401 on a normal call
  triggers a single cookie-based refresh then retry; bootstrap restores a session when
  the mocked refresh succeeds and lands on login when it 401s. Update
  `AuthProvider.test.tsx` / `fetchClient.test.ts` to the new shapes.

## 7. Files touched

- **Backend:** `crates/domain-auth/src/ports/http.rs` (login/refresh/logout + MFA
  verify/confirm cookie handling), `crates/domain-auth/src/ports/dto.rs`
  (`AccessTokenResponse`, `LoginResponse` token shape), `crates/domain-auth/src/openapi.rs`;
  regenerate `web/src/api/schema.d.ts`. Possibly add `axum-extra` (cookie jar).
- **Frontend:** `web/src/auth/tokenStore.ts`, `web/src/lib/fetchClient.ts`,
  `web/src/api/auth.ts`, `web/src/auth/AuthProvider.tsx` (+ their tests).
