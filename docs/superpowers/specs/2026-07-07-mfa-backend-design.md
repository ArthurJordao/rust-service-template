# MFA by Default (backend) — Design

**Date:** 2026-07-07
**Status:** Approved design, ready for implementation planning
**Scope of this spec:** Add multi-factor authentication to `domain-auth`, enforced by a
configurable policy (default `required`). v1 factor is TOTP (authenticator apps) plus
one-time recovery codes; the schema/ports are shaped so more factor types (WebAuthn,
email-OTP) can be added later. Covers the two-step login state machine, enforced
enrollment, secret encryption at rest, an `amr` access-token claim, and an audited
admin reset. **Out of scope:** the React SPA (enrollment wizard, QR, second-factor
entry, recovery-code display, admin reset UI) — a separate follow-on spec that will
consume the OpenAPI-typed endpoints this spec produces. Also out of scope: non-TOTP
factors, account-lockout beyond the per-factor MFA attempt cap.

This is the auth-hardening track from the production-readiness roadmap, after Spec 1
(runtime hardening) and Spec 2 (build & ship), both merged.

---

## 1. Goal & principles

The security gap analysis found no MFA. This spec adds it "by default" — enforced via a
policy that defaults to `required`, so the template ships secure but adaptable.

Invariants preserved:
- **Hexagonal / ports as traits, DI via `Arc<dyn Port>`** — new `FactorVerifier` and
  `MfaRepository` ports follow the existing `Notifier`/`RefreshTokenRepository` pattern.
- **sqlx runtime query API** (no `query!` macros).
- **One table per concern** — new `auth_mfa_factor` + `auth_mfa_recovery_code` tables,
  not columns bolted onto `auth_user`.
- **Secrets never stored recoverable-in-the-clear** — TOTP secrets are AEAD-encrypted at
  rest; recovery codes are hashed (like `password_hash`).
- **Config is 12-factor with a secure default** — `mfa_policy` defaults to `required`;
  `.env.example` sets `off` for frictionless local dev.
- **At-least-once/idempotent + correlation-id logging** unchanged; the admin reset emits
  an outbox audit event.

---

## 2. Decisions (resolved during brainstorming)

1. **Enforcement: config-gated `mfa_policy = off | optional | required`, default
   `required`.** Under `required`, a password-authenticated user without a confirmed
   factor gets only an enrollment-limited credential until they enroll; full tokens are
   withheld.
2. **Factor: TOTP + recovery codes in v1, schema/ports generic for future factors** —
   an `auth_mfa_factor.type` column (v1 only `'totp'`) and a `FactorVerifier` trait.
3. **At-rest protection:** TOTP secret encrypted with a dedicated `mfa_encryption_key`
   (ChaCha20-Poly1305 AEAD); recovery codes bcrypt-hashed.
4. **Access token records `amr`** (e.g. `["pwd","totp"]`) via a new
   `platform::AccessClaims.amr` field populated in `issue_access`. (No back-compat
   concern — not yet deployed.)
5. **Recovery:** one-time recovery codes (self-service) + an **audited admin reset**
   endpoint (last resort). Verify endpoint is rate-limited (already under the
   tower-governor `/auth/*` limiter) plus a per-factor attempt cap + lockout.
6. **Backend-first**; the SPA is a separate follow-on spec.
7. **Limited credentials are short-lived JWTs** (`token_type` `mfa_pending` /
   `mfa_enroll`, ~5-min TTL, no scopes) — no server-side challenge table.

---

## 3. Data model (migration `0007_mfa.sql`)

```sql
create table auth_mfa_factor (
    id               bigserial primary key,
    user_id          bigint      not null references auth_user (id),
    type             text        not null default 'totp',   -- pluggable factor dimension
    secret_encrypted bytea       not null,                  -- nonce || ChaCha20-Poly1305 ciphertext
    confirmed_at     timestamptz,                           -- NULL until first code verified = "enabled"
    failed_attempts  int         not null default 0,
    locked_until     timestamptz,
    created_at       timestamptz not null default now(),
    unique (user_id, type)
);

create table auth_mfa_recovery_code (
    id         bigserial primary key,
    user_id    bigint      not null references auth_user (id),
    code_hash  text        not null,   -- bcrypt, single-use
    used_at    timestamptz,
    created_at timestamptz not null default now()
);
create index auth_mfa_recovery_code_user_idx on auth_mfa_recovery_code (user_id);
```

- A factor row is created **unconfirmed** at setup; `confirmed_at` set on first verified
  code makes MFA "enabled". Login only enforces confirmed factors (abandoned setup never
  locks anyone out).
- `secret_encrypted` holds `nonce‖ciphertext`; plaintext base32 secret never persisted.
- `failed_attempts`/`locked_until` implement the per-factor brute-force cap.
- Latest existing migration is 0006.

---

## 4. Config + crypto

New `platform::config::AuthSettings` fields (loaded via the existing `APP__AUTH__*`
mechanism):
- **`mfa_policy: String`** parsed into `MfaPolicy { Off, Optional, Required }`. **Code
  default (unset) = `required`.** `.env.example` sets `MFA_POLICY=off` (documented local
  dev convenience, like `AUTO_MIGRATE=true`).
- **`mfa_encryption_key_file` / `mfa_encryption_key_base64`** — resolved with the same
  file-wins precedence as the JWT keys (`resolve_key`). A 32-byte key.

**Startup validation:** if `mfa_policy != off`, the encryption key must resolve to 32
bytes or the app fails to boot (same rigor as the JWT keys). When `off`, no key needed
and MFA endpoints are disabled.

**Crypto:**
- **AEAD: `chacha20poly1305`** (RustCrypto, pure-Rust). Encrypt the base32 secret with a
  random 96-bit nonce; store `nonce‖ciphertext`.
- **TOTP: `totp-rs`** — RFC 6238, 30s step, 6 digits, ±1 step skew; provides the
  `otpauth://` provisioning URI for the SPA's QR.
- **Recovery codes: bcrypt** (reuse `auth/password.rs`).

New deps: `chacha20poly1305`, `totp-rs` (+ `base32` if not transitive). `make gen-keys`
also emits a dev MFA key into `secrets/`.

---

## 5. Login / enrollment state machine

`POST /auth/login`: after `check_credentials` succeeds, let
`enabled = mfa.confirmed_factor(user).is_some()` and branch:

| `mfa_policy` | enabled? | Outcome |
|---|---|---|
| `off` | — | full tokens, `amr=["pwd"]` |
| any | yes | `mfa_required` challenge, `purpose: "verify"` |
| `optional` | no | full tokens, `amr=["pwd"]` |
| `required` | no | `mfa_required` challenge, `purpose: "enroll"` |

**`LoginResponse`** is a tagged enum (clean for the OpenAPI-typed client):
```
{ status: "authenticated", tokens: AuthTokens }
{ status: "mfa_required", purpose: "verify" | "enroll", mfa_token: String, factor_types: ["totp"] }
```

**Limited credentials (Approach A — short-lived JWTs):** `mfa_token` is a
`JwtIssuer::issue_mfa_token`-minted JWT with `token_type` `"mfa_pending"` (verify) or
`"mfa_enroll"` (enroll), ~5-min TTL, `sub=user`, no scopes. The setup/confirm/verify
handlers use a small decode helper that accepts only the matching `token_type` (they do
not go through the normal scoped `Authenticated` extractor). Brute-force is bounded by
the tower-governor `/auth/*` rate limit **plus** the per-factor `failed_attempts` /
`locked_until` cap. On success, full tokens are minted with the appropriate `amr`.

---

## 6. Ports, `AuthState`, token issuance

- **`FactorVerifier` trait** — `verify(secret_plaintext, code, now) -> bool`,
  `generate_secret() -> String`, `provisioning_uri(secret, account, issuer) -> String`.
  v1 impl `TotpVerifier` (wraps `totp-rs`). `Arc<dyn FactorVerifier>` so tests inject a
  deterministic verifier.
- **`MfaRepository` trait** (Postgres impl co-located on `PostgresUserRepository`):
  `confirmed_factor(user_id)`, `get_factor(user_id, type)`,
  `upsert_unconfirmed_factor(user_id, type, secret_encrypted)`,
  `confirm_factor(user_id, type)`, `delete_factors(user_id)`,
  `record_failed_attempt(factor_id)`, `reset_attempts(factor_id)`,
  `store_recovery_codes(user_id, hashes)`, `consume_recovery_code(user_id, code) -> bool`,
  `delete_recovery_codes(user_id)`.
- **`MfaCipher`** — `chacha20poly1305` wrapper; `encrypt`/`decrypt`. Bundled into
  **`MfaConfig { policy: MfaPolicy, cipher: Option<MfaCipher> }`** (cipher `None` only
  when `policy=off`).
- **`AuthState` gains** `mfa: Arc<dyn MfaRepository>`, `mfa_verifier: Arc<dyn
  FactorVerifier>`, `mfa_config: MfaConfig`. Ripples to the **8 `AuthState` construction
  sites** (composition root + 7 test helpers).
- **`JwtIssuer` gains** `issue_mfa_token(user_id, purpose, now)` and an `amr:
  Vec<String>` param on `issue_access`. **`platform::AccessClaims` gains `amr:
  Vec<String>`**, populated centrally in `issue_access`.

---

## 7. Endpoints

All under `/auth` in `domain-auth`, added to the OpenAPI doc (utoipa):
- **`POST /auth/login`** → tagged `LoginResponse` (§5).
- **`POST /auth/mfa/setup`** — auth: `mfa_enroll` token (forced) or a normal access
  token (voluntary self-enroll). Generates + encrypts a secret, upserts an unconfirmed
  factor, returns `{ provisioning_uri, secret }`.
- **`POST /auth/mfa/confirm`** — same auth; body `{ code }`. Verifies first code → sets
  `confirmed_at` → generates 10 single-use recovery codes (returned plaintext **once**). If called
  with an `mfa_enroll` token, also returns full `AuthTokens` (`amr=["pwd","totp"]`);
  self-enroll returns just the codes.
- **`POST /auth/mfa/verify`** — auth: `mfa_pending` token; body `{ code }` (TOTP or
  recovery). Success → full tokens (`amr=["pwd","totp"]` or `["pwd","recovery"]`).
  Failure → `record_failed_attempt` + lockout when the cap is hit.
- **`POST /auth/mfa/recovery-codes`** — auth: full token; regenerate (invalidate old,
  return new plaintext once).
- **`DELETE /auth/mfa`** — self-disable; allowed only when `policy != required`.
- **`POST /admin/users/:id/mfa/reset`** — `admin` scope; clears factors + recovery
  codes, emits an outbox **`user.mfa_reset`** event (admin id, target id, cid) as the
  audit trail, logs it. Under `required`, the user re-enrolls on next login.

Config edges: when `policy=off`, the MFA endpoints return `404`/`409` (disabled). The
per-attempt cap and rate limit protect `verify`.

---

## 8. Observability / audit

- Structured logs (with correlation id) on every MFA state change: setup, confirm,
  verify success/failure, recovery-code use, self-disable, admin reset.
- Admin reset emits the `user.mfa_reset` outbox event (the durable audit record).

---

## 9. Testing strategy

**Pure unit (no DB):**
- TOTP generate→verify roundtrip; ±1-step skew accepted; wrong code + out-of-window
  rejected.
- `MfaCipher` encrypt→decrypt roundtrip; tampered ciphertext fails to decrypt.
- Recovery-code hash→verify; wrong code rejected.
- `MfaPolicy` parsing (off/optional/required + default).

**`#[sqlx::test]` integration** (mirroring `domain-auth/tests/http.rs`, new
`tests/mfa.rs`):
- Login returns the correct `LoginResponse` for each `(policy, enabled)` cell in §5.
- Forced-enroll flow end-to-end: login `required`+no factor → `mfa_enroll` token →
  setup → confirm → full tokens carrying `amr=["pwd","totp"]`.
- Verify happy path; wrong code increments `failed_attempts`; lockout after the cap.
- Recovery code is single-use (second use rejected).
- Self-disable rejected under `required`, allowed under `optional`.
- Admin reset clears the factor + recovery codes and emits `user.mfa_reset`.
- An `mfa_enroll`/`mfa_pending` token is rejected by the normal scoped routes (can't be
  used as an access token).

---

## 10. Files touched (anticipated)

- **Create:** `migrations/0007_mfa.sql`; `crates/domain-auth/src/auth/totp.rs`,
  `auth/mfa_crypto.rs` (`MfaCipher`); MFA handlers (in `ports/http.rs` or a new
  `ports/mfa_http.rs`); `crates/domain-auth/tests/mfa.rs`.
- **Modify:** `platform/src/config.rs` (`AuthSettings` + `MfaPolicy`),
  `platform/src/auth/mod.rs` (`AccessClaims.amr`), `domain-auth/src/auth/jwt.rs`
  (`issue_mfa_token`, `amr` on `issue_access`), `ports/repository.rs` (`MfaRepository`,
  `FactorVerifier`), `ports/postgres.rs` (impl), `ports/http.rs` (`AuthState`, login
  branch, router), `ports/dto.rs` (`LoginResponse`, MFA request/response DTOs),
  `crates/app/src/state.rs` + the 7 test `AuthState` builders, root `Cargo.toml` +
  `domain-auth/Cargo.toml` (deps), `.env.example` (MFA config), `Makefile` (`gen-keys`
  emits an MFA key), `crates/app/src/openapi.rs` (register new paths/schemas).
