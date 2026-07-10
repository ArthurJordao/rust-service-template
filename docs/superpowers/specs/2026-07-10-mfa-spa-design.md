# MFA SPA (frontend) — Design

**Date:** 2026-07-10
**Status:** Approved design, ready for implementation planning
**Scope of this spec:** The React SPA for the MFA backend (spec `2026-07-07-mfa-backend`):
the two-step login (verify / forced-enroll), self-service enrollment + disable +
recovery-code regeneration on the account page, an admin MFA-reset control, the client
plumbing (a `bearer` override + fixing the `LoginResponse` break), and one small backend
addition (`GET /auth/mfa` status). Built on branch `mfa-backend` and **merged together
with the backend** so `main`'s login never breaks.

Depends on the merged-in-this-branch MFA backend endpoints (setup/confirm/verify/
recovery-codes/self-disable/admin-reset) and the regenerated typed client
(`web/src/api/schema.d.ts` already reflects the tagged `LoginResponse` + MFA paths).

---

## 1. Goal & principles

The backend changed `POST /auth/login` from a flat `AuthTokens` to a tagged
`LoginResponse`, which breaks the current SPA login. This spec builds the MFA frontend
and closes that gap.

Principles:
- **Reuse the existing kit** — shadcn/`@base-ui/react` components (`Button`, `Dialog`,
  `Input`, `Card`, `Label`), `sonner` toasts, TanStack Query hooks, the custom
  `apiFetch` client, plain controlled-state forms. No new form/validation library.
- **Typed against the generated client** — new calls go through `apiFetch` with the
  generated `schema.d.ts` types; paths are already valid `ApiPath` values.
- **The `mfa_token` is ephemeral** — it lives only in `LoginPage` local state, never in
  the URL, history, or `tokenStore`.
- **Components shared** between login-flow and self-service so enrollment UI exists once.

---

## 2. Decisions (resolved during brainstorming)

1. **Two-step login inline in `LoginPage`** via local state (`step` + stashed challenge)
   — no new routes; keeps the short-lived `mfa_token` out of the URL/history.
2. **`bearer` override on `apiFetch`** — MFA setup/confirm/verify send the `mfa_token`
   as `Authorization` and skip the 401→refresh path; self-service omits it (uses the
   session access token).
3. **`GET /auth/mfa` status endpoint** (small backend addition) → `{ enabled, policy }`
   — authoritative Account-page state, correct even right after a self-enroll; `policy`
   lets the UI gate the Disable control.
4. **QR via `react-qr-code`** (pure-SVG, declarative); base32 secret shown as copyable
   manual-entry fallback.
5. Shared components: `MfaCodeInput`, `MfaEnrollWizard`, `RecoveryCodesDialog`.

---

## 3. Backend addition + client plumbing

**Backend (`domain-auth`, on this branch):**
- `GET /auth/mfa` — `Authenticated`; returns `MfaStatusResponse { enabled: bool, policy:
  String }` (`enabled` = `mfa.confirmed_factor(user).is_some()`, `policy` =
  `mfa_config.policy` rendered as `"off"|"optional"|"required"`). Registered in
  `openapi.rs`; `make gen-api` regenerates `web/src/api/schema.d.ts` (the openapi-drift
  CI job must stay green).

**Client (`web/`):**
- **`apiFetch` `bearer` override** — `Opts` gains `bearer?: string`; `raw()` uses it as
  the `Authorization` header when present, and `apiFetch` skips the 401→refresh-retry
  for `bearer` calls (mfa tokens aren't refreshable). Default behavior unchanged.
- **Fix login break** — `api/auth.ts` `login()` returns `LoginResponse`;
  `AuthProvider.login` discriminates: `status === "authenticated"` →
  `applyTokens(tokens.access_token, tokens.refresh_token)`; `status === "mfa_required"`
  → return the challenge to the caller (`LoginPage`), applying no tokens. Update
  `AuthProvider.test.tsx`'s login mock to the tagged shape.
- **`web/src/api/mfa.ts`** — thin `apiFetch` wrappers; **`web/src/api/hooks.ts`** —
  TanStack-Query hooks (mirroring `useSetUserScopes`):
  - `useMfaStatus()` — `useQuery` on `GET /auth/mfa`.
  - `mfaSetup(bearer?)`, `mfaConfirm(code, bearer?)` — the `bearer?` carries the
    `mfa_enroll` token in the login-enroll context; omitted for self-service.
  - `mfaVerify(code, mfaToken)` — `bearer` = the `mfa_pending` token.
  - `useRegenRecoveryCodes()`, `useDisableMfa()`, `useAdminResetMfa()` — session-token
    mutations; invalidate `useMfaStatus` on success; `sonner` toast + cid `refSuffix`
    on error.
- **Add `react-qr-code`** to `web/package.json`.

---

## 4. Two-step login (`LoginPage`)

Local state: `step: "password" | "verify" | "enroll"` + `challenge: { mfa_token,
purpose, factor_types } | null`.

1. **password** — existing form → `login(email, password)`:
   - `authenticated` → `applyTokens` + redirect (unchanged).
   - `mfa_required` + `purpose "verify"` → stash challenge → **verify**.
   - `mfa_required` + `purpose "enroll"` → stash challenge → **enroll**.
2. **verify** — `MfaCodeInput` (accepts a 6-digit TOTP **or** a recovery code) →
   `mfaVerify(code, challenge.mfa_token)` → success `applyTokens(tokens)` + redirect;
   error → toast (with cid + lockout message).
3. **enroll** — `MfaEnrollWizard` (QR + secret + `MfaCodeInput`) →
   `mfaConfirm(code, { bearer: challenge.mfa_token })` → returns `{ recovery_codes,
   tokens }` → show `RecoveryCodesDialog`; **only after the user acknowledges** →
   `applyTokens(tokens)` + redirect.

Edge cases: an expired `mfa_token` (verify/confirm 401) → toast "session expired, please
log in again" → reset to **password**. Back/refresh returns to a clean login (challenge
lost — correct for a short-lived token).

---

## 5. Account self-service (`AccountPage`)

A new MFA `Card`, driven by `useMfaStatus()`:
- `policy === "off"` → card not rendered.
- **not enabled** (reachable only under `optional`) → **Enable MFA** → `MfaEnrollWizard`
  in a `Dialog`, *self-service context*: setup/confirm use the **session access token**
  (no `bearer`); `confirm` returns `tokens: null` → show `RecoveryCodesDialog`, then
  invalidate `useMfaStatus` (no token swap).
- **enabled** →
  - **Regenerate recovery codes** → confirm ("invalidates existing codes") →
    `useRegenRecoveryCodes()` → `RecoveryCodesDialog` with the new set.
  - **Disable MFA** — rendered **only when `policy === "optional"`** → confirm →
    `useDisableMfa()` (`DELETE /auth/mfa`) → invalidate status. (If a `required`-policy
    409 is ever returned, surface it as a toast.)

All mutations invalidate `useMfaStatus` on success.

---

## 6. Admin reset (`UsersPage`)

In the admin users table action cell (beside `EditScopesDialog`): a **`ResetMfaDialog`**
per user — "Reset MFA" button → confirm ("clears the user's second factor + recovery
codes; under a required policy they re-enroll on next login") → `useAdminResetMfa(id)`
(`POST /admin/users/{id}/mfa/reset`) → success/error toast. Gated by the existing
`RequireAdmin` route wrapper + server-side `admin` scope.

---

## 7. Shared components

- **`MfaCodeInput`** — controlled input accepting a 6-digit TOTP or a recovery-code
  string; submit-on-enter; disabled while a mutation is pending.
- **`MfaEnrollWizard`** — QR (`react-qr-code` over `provisioning_uri`) + copyable
  `secret` + `MfaCodeInput`; takes a `confirm(code)` callback so login-flow (bearer =
  mfa_token, receives tokens) and self-service (no bearer, no tokens) both drive it.
- **`RecoveryCodesDialog`** — shown-once list; Copy-all + Download (.txt) + a required
  "I've saved these codes" checkbox gating the Done button.

---

## 8. Testing strategy

Vitest + `@testing-library/react` + `msw` (existing harness: shared `server`,
`onUnhandledRequest: "error"`), following `AuthProvider.test.tsx` (Probe pattern) and
`UsersPage.test.tsx` (page + captured request bodies):

- **`apiFetch` `bearer` override** (`fetchClient.test.ts`): sends the override token as
  `Authorization`; a 401 on a `bearer` call does NOT trigger the refresh path.
- **Login**: `mfa_required{verify}` → code entry → `mfaVerify` mock → tokens applied +
  redirect; `mfa_required{enroll}` → wizard → `mfaConfirm` mock → recovery dialog → ack
  gates token application; expired `mfa_token` (401) → returns to password step.
- **Account**: status `off` (card absent), `optional`+not-enabled (Enable shown),
  `enabled` (Regenerate shown; Disable shown only under `optional`); self-enroll flow
  invalidates status without a token swap.
- **Admin**: reset button → confirm → `useAdminResetMfa` request captured.
- Update `AuthProvider.test.tsx`'s login mock to the tagged `LoginResponse` shape.

---

## 9. Files touched (anticipated)

- **Backend:** `crates/domain-auth/src/ports/http.rs` (`GET /auth/mfa` handler + route),
  `ports/dto.rs` (`MfaStatusResponse`), `openapi.rs`; regenerate `web/src/api/schema.d.ts`.
- **Create (web):** `web/src/api/mfa.ts`; `web/src/components/mfa/MfaCodeInput.tsx`,
  `MfaEnrollWizard.tsx`, `RecoveryCodesDialog.tsx`; `web/src/routes/admin/ResetMfaDialog.tsx`;
  tests alongside each.
- **Modify (web):** `web/src/lib/fetchClient.ts` (`bearer` override), `web/src/api/auth.ts`
  (`LoginResponse`), `web/src/auth/AuthProvider.tsx` (discriminate) + `AuthProvider.test.tsx`,
  `web/src/api/hooks.ts` (MFA hooks), `web/src/routes/LoginPage.tsx` (two-step),
  `web/src/routes/AccountPage.tsx` (MFA card), `web/src/routes/admin/UsersPage.tsx` (reset
  action), `web/package.json` (`react-qr-code`).
