# Account Recovery: Password Reset + Email Verification — Design

**Date:** 2026-07-12
**Status:** Approved design, ready for implementation planning
**Scope:** Self-service **password reset** and **email verification** for
`domain-auth`, with token delivery through the existing event-driven notification
pipeline (visible in the `/admin/notifications` page until a real email provider is
wired). Includes the backend flows, config-gated verification enforcement, and the
SPA pages. Depends on the notifications-admin-page spec landing first (that is how a
developer retrieves the token in the no-email era).

---

## 1. Goal & principles

Two auth flows that are table-stakes for a real product but currently missing:
"forgot my password" and "verify your email". The wrinkle is that there is **no real
email provider** yet — so both flows deliver their token the same way the welcome
message already travels: `domain-auth` emits an outbox event carrying the token/link,
the notification domain renders + records it, and it shows up in the admin
notifications view. When a real `Notifier` is added later, the identical event flows
to email with **no flow changes**.

Principles:
- **Preserve the architecture.** Cross-domain delivery stays event-driven and
  cycle-free (`domain-auth` publishes; `domain-notification` consumes) — no new
  crate dependency, consistent with `user.registered → welcome`.
- **No user enumeration.** `forgot` always returns 200 regardless of whether the
  email exists.
- **Single-use, short-TTL, hashed tokens.** Reset/verify tokens are random, stored
  **hashed** (never plaintext at rest), expire quickly, and are consumed atomically —
  mirroring the recovery-code model already in the MFA work.
- **Config-gated, template-usable by default.** Email-verification enforcement is a
  policy (`off | optional | required`) that **defaults to `off`**, so the template
  still boots and logs in out of the box.

---

## 2. Decisions

1. **Delivery = outbox event → notification domain.** New event types
   `user.password_reset_requested` and `user.email_verification_requested`, each
   carrying the recipient email and the reset/verify token (or a link embedding it).
   `domain-notification` gains handlers that render dedicated templates and record a
   `sent_notification` (visible in `/admin/notifications`). The token lives in the
   event payload — acceptable in the DB-only-delivery era (admin-visible on purpose);
   it flows to real email once a `Notifier` exists.
2. **Verification enforcement policy** (`platform::config`, mirrors `MfaPolicy`):
   `EmailVerificationPolicy { Off, Optional, Required }`, **default `Off`**. Under
   `Required`, an unverified user's login returns a challenge (see §5) instead of
   tokens — the same shape as the MFA-pending pattern.
3. **One combined spec** — both flows share the token table pattern, the event
   delivery, the config plumbing, and the SPA form kit; the plan can split them into
   tasks.
4. **Reset revokes sessions.** A completed password reset bumps
   `auth_user.tokens_valid_from` (the existing per-user revocation epoch), killing all
   outstanding access/refresh tokens.

---

## 3. Data model (migrations)

- **`0009_password_reset.sql`:** `password_reset (id bigserial pk, user_id bigint not
  null references auth_user(id), token_hash text not null, expires_at timestamptz not
  null, used_at timestamptz, created_at timestamptz not null default now())`; index on
  `user_id`.
- **`0010_email_verification.sql`:**
  - `alter table auth_user add column email_verified_at timestamptz` (null = unverified).
  - `email_verification (id bigserial pk, user_id bigint not null references
    auth_user(id), token_hash text not null, expires_at timestamptz not null, used_at
    timestamptz, created_at timestamptz not null default now())`; index on `user_id`.

Token generation/hashing/verification reuses the recovery-code helper style
(`crates/domain-auth/src/auth/recovery.rs`) — random token, bcrypt/const-time compare,
single-use consume `where id=$1 and used_at is null` returning `rows_affected()==1`.

---

## 4. Password reset flow

- **`POST /auth/password/forgot { email }` → 200 always.** If the user exists:
  generate a token, store its hash with a short TTL (e.g. 30 min — exact value in the
  plan), publish `user.password_reset_requested` (payload: email + token) in the same
  transaction as the insert (outbox semantics). No body leaks whether the user exists.
- **`POST /auth/password/reset { token, new_password } ` → 200 | 400.** Look up the
  unused, unexpired token by hash; on match: set the new `password_hash` (bcrypt),
  consume the token atomically, bump `tokens_valid_from`. Invalid/expired/used → 400
  with a generic message.
- **Notification:** `domain-notification` renders a `password_reset` template (link:
  `<app>/reset-password?token=…`) and records it.

## 5. Email verification flow

- **On register:** user is created unverified (`email_verified_at = null`); publish
  `user.email_verification_requested` (payload: email + token) in the register
  transaction (alongside the existing `user.registered`).
- **`POST /auth/email/verify { token } ` → 200 | 400.** Match the unused/unexpired
  token → set `email_verified_at = now()`, consume the token.
- **`POST /auth/email/verify/resend`** (Authenticated): re-issue a verification token +
  event for the current user (for the "didn't get it" case).
- **Enforcement at login** (only when policy `Required`): if the resolved user is
  unverified, `login` returns `LoginResponse::EmailVerificationRequired { email }`
  (new tagged variant) instead of tokens — the SPA routes to a "check your email"
  screen with a resend button. Under `Off`/`Optional`, login proceeds normally.
- **Notification:** a `email_verification` template (link:
  `<app>/verify-email?token=…`).

Interaction with MFA: verification is checked **before** the MFA branch in `login`, so
an unverified user under `Required` never reaches the MFA step.

## 6. SPA changes

- **`/forgot-password`** — email form → `POST /auth/password/forgot` → always shows
  "if that account exists, we sent a reset link" (no enumeration). Link from the login
  page.
- **`/reset-password?token=…`** — new-password form → `POST /auth/password/reset` →
  success routes to login with a toast; invalid/expired token → inline error.
- **`/verify-email?token=…`** — on mount calls `POST /auth/email/verify` → success/
  failure state.
- **Email-verification-required screen** — when login returns
  `EmailVerificationRequired`, show "verify your email" with a **Resend** button
  (`/auth/email/verify/resend`). Mirrors the MFA two-step `LoginPage` state machine.
- Generated types (`schema.d.ts`) regenerated for the new endpoints + the new
  `LoginResponse` variant.

## 7. Error handling

- All token failures (reset + verify) return a generic 400 — no distinction between
  "not found", "expired", "already used" (avoids probing).
- `forgot` and `resend` never reveal account existence.
- Event publish failures roll back the token insert (single transaction) so a token is
  never stored without its delivery event enqueued.

## 8. Testing strategy

- **Backend (`domain-auth` + `domain-notification` tests, `#[sqlx::test]`):**
  - `forgot` returns 200 for both existing and non-existing emails; for an existing
    user it publishes `user.password_reset_requested` and stores a hashed token.
  - `reset` with a valid token changes the password, is single-use (second use → 400),
    rejects expired tokens, and bumps `tokens_valid_from` (old access token now
    rejected).
  - register publishes `user.email_verification_requested` and creates an unverified
    user; `verify` marks verified and is single-use; `resend` issues a new token.
  - login under `Required` + unverified → `EmailVerificationRequired`; under `Off` →
    normal; verification is evaluated before MFA.
  - notification consumers record `password_reset` / `email_verification` rows
    (assert recipient + template).
- **Frontend (vitest + msw):** forgot form always shows the neutral message; reset
  form success + invalid-token paths; verify-email mount success/failure; the
  verification-required login screen with a working resend.

## 9. Files touched

- **Backend:** `migrations/0009_password_reset.sql`, `0010_email_verification.sql`
  (create); `crates/platform/src/config.rs` (`EmailVerificationPolicy` + settings);
  `crates/domain-auth/src/ports/{http.rs,dto.rs,repository.rs,postgres.rs}`,
  `crates/domain-auth/src/auth/` (token helper), `openapi.rs`;
  `crates/domain-notification/src/ports/{events.rs,templates.rs}` (new handlers +
  templates) + its routes/registry entries; regenerate `web/src/api/schema.d.ts`.
- **Frontend:** `web/src/routes/{ForgotPasswordPage,ResetPasswordPage,VerifyEmailPage}.tsx`
  (create) + tests; `web/src/routes/LoginPage.tsx` (verification-required state);
  `web/src/api/auth.ts` + `hooks.ts`; `web/src/App.tsx` (new public routes).
- **Outbox wiring:** register the new event types in the publisher `Routes` table and
  the notification subscriber's `routes()` (keep them in sync — the linear,
  cycle-free invariant).
