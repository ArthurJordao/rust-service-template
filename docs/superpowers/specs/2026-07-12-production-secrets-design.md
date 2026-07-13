# Production Secrets & Config — Design

**Date:** 2026-07-12
**Status:** Approved design, ready for implementation planning
**Scope:** A production-readiness guide for secrets/config **plus** a boot-time guard
that refuses to start in production with the committed dev/test keys. Small code
change + docs; no new runtime features.

---

## 1. Goal & context

The template ships with **committed dev/test secrets** so it builds and runs with zero
setup: `.env.example` points `AUTH__*` at the RSA test fixtures under
`crates/*/tests/fixtures/`, and there is a default/test MFA encryption key. That
convenience is also the single worst production footgun — deploying with those keys
means anyone can forge JWTs and decrypt MFA secrets. This spec makes the safe path
explicit (docs) and makes the unsafe path fail loudly (guard).

Principles:
- **Frictionless in dev, guard-railed in prod.** Don't remove the committed dev
  fixtures (they keep the template turnkey); instead refuse to *use* them when
  `ENVIRONMENT=production`.
- **Fail fast, fail clear.** The guard runs at startup, before serving, with an error
  that names exactly which secret is still the dev value and how to fix it.

---

## 2. Decisions

1. **Boot-time guard, production-only.** When `APP__SERVER__ENVIRONMENT=production`,
   the app validates its secrets during startup (in the composition root, before the
   server binds) and **panics/exits non-zero** if any is a known dev value.
2. **Detection by fingerprint, not by path.** Compare a hash (e.g. SHA-256) of the
   loaded JWT public key and the MFA key against the fingerprints of the committed
   fixtures. Path-based checks are too easy to defeat by copying; fingerprints catch
   "pointed prod at the test key" regardless of location.
3. **Docs live in the repo** as `docs/production-readiness.md`, linked from the README
   (the README is added by this spec too, since none exists) and referenced from
   `CLAUDE.md`.
4. **Host-agnostic.** The guide covers the secret/config contract; deployment-target
   specifics (Fly / VPS / Neon) are noted but not prescribed — the earlier
   deploy-target decision is still open.

---

## 3. The guard (code)

- A `platform` function, e.g. `config::assert_production_secrets(&settings)`, called
  from the `app` composition root right after config load and before building the
  router. No-op unless `environment == "production"`.
- Checks:
  - **JWT key:** fingerprint of the configured RS256 public key ≠ the committed
    test-fixture public key. (Public key is sufficient — if the public key matches the
    test fixture, the private key is the known committed one.)
  - **MFA key:** the resolved MFA encryption key ≠ the default/test key.
  - (Optional, same guard) refuse an obviously-empty/short key.
- On failure: log an error naming the offending secret and the remediation
  (`make gen-keys`, set it as a host secret), then exit non-zero. On success: a single
  info line "production secret checks passed".
- The committed fixture fingerprints are embedded as constants (computed from the
  files at build time or pinned literals with a test that recomputes and asserts them,
  so they can't silently drift).

## 4. The guide (docs)

`docs/production-readiness.md` — a checklist a developer runs before the first real
deploy:

1. **Generate real secrets:** `make gen-keys` → RSA JWT keypair + MFA key into
   `secrets/` (gitignored). Never commit `secrets/`.
2. **Provide them to the host** as secrets/env (not baked into the image):
   `AUTH__*` key paths/values, the MFA key, `DATABASE_URL`.
3. **Set `APP__SERVER__ENVIRONMENT=production`** (activates the guard) and confirm the
   dev JWT banner is gone.
4. **Lock CORS** to the real origin(s).
5. **Point `DATABASE_URL`** at managed Postgres; `APP__DATABASE__AUTO_MIGRATE=false`
   (migrations run via the `migrate` bin / release command).
6. **Rotate** guidance: how to roll the JWT key (kid/rollover is out of scope; note the
   manual path — rotate key, all sessions invalidate).
7. A short **secrets inventory** table: name, purpose, where it comes from, blast
   radius if leaked.

Add a top-level **`README.md`** (human-facing — distinct from the agent-facing
`CLAUDE.md`): what the template is, quickstart (`make up`, run, test), and a link to
the production-readiness guide and the design docs.

## 5. Testing strategy

- **Guard unit tests (`platform`):** with `environment=production` and the committed
  test key → the assert returns an error naming the JWT key; with a freshly generated
  (different) key → passes; same for the MFA key; with `environment` non-production →
  always passes (no-op). A test recomputes the pinned fixture fingerprints from the
  fixture files so they can't drift silently.
- **Composition-root wiring:** an `app` test (or a focused unit) that the guard is
  invoked and a production build with test keys fails to construct the app. (Exact
  harness in the plan — likely testing the `assert_*` function directly rather than a
  full boot.)
- No frontend changes.

## 6. Files touched

- **Code:** `crates/platform/src/config.rs` (or a new `config/secrets_guard.rs`) —
  `assert_production_secrets` + fingerprint constants + tests; `crates/app/src/`
  (composition root) — call the guard before serving.
- **Docs:** `docs/production-readiness.md` (create), `README.md` (create), a pointer
  from `CLAUDE.md`.
