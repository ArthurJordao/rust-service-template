# MFA by Default (backend) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add TOTP + recovery-code MFA to `domain-auth`, enforced by a configurable `mfa_policy` (default `required`), with secrets encrypted at rest, a two-step login state machine, an `amr` access-token claim, and an audited admin reset.

**Architecture:** New `FactorVerifier`/`MfaRepository` ports (existing trait+`Arc<dyn>` pattern) + an `MfaCipher` (ChaCha20-Poly1305) and `TotpVerifier` (totp-rs). Login branches on `(policy, has_confirmed_factor)` and, when a factor step is needed, returns a short-lived `mfa_pending`/`mfa_enroll` JWT instead of tokens; setup/confirm/verify complete the flow and mint full tokens carrying `amr`. TOTP secrets are AEAD-encrypted; recovery codes are bcrypt-hashed and single-use.

**Tech Stack:** Rust (stable), axum 0.7, sqlx (runtime query API), jsonwebtoken (RS256), `totp-rs`, `chacha20poly1305`, `bcrypt`, utoipa.

## Global Constraints

- **sqlx runtime query API only** (`sqlx::query`/`query_as`/`.bind`), NEVER `query!` macros.
- **One table per concern**; new tables `auth_mfa_factor` + `auth_mfa_recovery_code` (migration `0007_mfa.sql`; latest existing is 0006).
- **TOTP secret AEAD-encrypted at rest** (ChaCha20-Poly1305, random 96-bit nonce, stored `nonce‖ciphertext` in `bytea`); **recovery codes bcrypt-hashed, single-use** (10 per enrollment).
- **`mfa_policy` default `required`** (code/prod); `.env.example` sets `off`. **Fail-fast at startup:** if `mfa_policy != off`, a 32-byte `mfa_encryption_key` must resolve.
- **Limited credentials are short-lived JWTs** (`token_type` `"mfa_pending"` / `"mfa_enroll"`, ~5-min TTL, no scopes). Full tokens minted only after the factor step, with `amr` set.
- **Ports as traits, DI via `Arc<dyn Port>`**, hexagonal (pure logic testable without axum/DB).
- **Before every commit:** `cargo fmt --all` and `cargo clippy --all-targets --locked -- -D warnings` clean.
- DB-backed tests use `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres` and `#[sqlx::test(migrations = "../../migrations")]`.

---

## File Structure

- `crates/platform/src/config.rs` — `MfaPolicy` enum + `AuthSettings` MFA fields + `resolve_key`-style key resolution + startup validation hook.
- `crates/platform/src/auth/mod.rs` — `AccessClaims.amr`.
- `crates/domain-auth/src/auth/jwt.rs` — `amr` param on `issue_access`; `issue_mfa_token` + `MfaTokenClaims`.
- `crates/domain-auth/src/auth/mfa_crypto.rs` (new) — `MfaCipher`.
- `crates/domain-auth/src/auth/totp.rs` (new) — `TotpVerifier` + `FactorVerifier` trait.
- `crates/domain-auth/src/auth/recovery.rs` (new) — recovery-code generation + hashing.
- `crates/domain-auth/src/ports/repository.rs` — `MfaRepository` trait + `MfaFactor` struct.
- `crates/domain-auth/src/ports/postgres.rs` — `MfaRepository` impl.
- `crates/domain-auth/src/ports/dto.rs` — `LoginResponse` (tagged) + MFA DTOs.
- `crates/domain-auth/src/ports/http.rs` — `AuthState` fields, `MfaConfig`, login branch, MFA handlers, router.
- `crates/app/src/state.rs` + `crates/app/src/openapi.rs` — wiring + OpenAPI registration.
- `migrations/0007_mfa.sql`, `.env.example`, `Makefile`, root + `domain-auth` `Cargo.toml`.
- Tests: `crates/domain-auth/tests/mfa.rs` (new) + the 7 `AuthState` builders updated.

---

## Task 1: Deps + MfaPolicy + config

**Files:**
- Modify: root `Cargo.toml`, `crates/domain-auth/Cargo.toml`, `crates/platform/src/config.rs`

**Interfaces:**
- Produces: `platform::config::MfaPolicy { Off, Optional, Required }` (`FromStr`/`Default=Required`); `AuthSettings` fields `mfa_policy: String`, `mfa_encryption_key_file: String`, `mfa_encryption_key_base64: String`; `AuthSettings::mfa_policy() -> MfaPolicy`; `AuthSettings::mfa_encryption_key() -> anyhow::Result<Option<[u8;32]>>` (Ok(None) only when policy==Off).

- [ ] **Step 1: Add dependencies.** Root `Cargo.toml` `[workspace.dependencies]`:
```toml
totp-rs = { version = "5", features = ["gen_secret", "otpauth"] }
chacha20poly1305 = "0.10"
base32 = "0.5"
```
`crates/domain-auth/Cargo.toml` `[dependencies]` (add):
```toml
totp-rs.workspace = true
chacha20poly1305.workspace = true
base32.workspace = true
```
> NOTE (external-API fragility): pin these majors. If a resolved patch changes the `totp-rs` (`TOTP::new`, `Secret`, `check`/`get_url`) or `chacha20poly1305` (`ChaCha20Poly1305`, `generate_nonce`, `encrypt`/`decrypt`) API shown in Tasks 3–4, adapt the calls to the resolved signatures — behavior is fixed (encrypt/decrypt roundtrip; RFC-6238 verify). Do not change majors.

- [ ] **Step 2: Write the failing test** — append to `crates/platform/src/config.rs` `mod tests`:
```rust
    #[test]
    fn mfa_policy_parses_and_defaults_required() {
        assert!(matches!("off".parse::<MfaPolicy>().unwrap(), MfaPolicy::Off));
        assert!(matches!("optional".parse::<MfaPolicy>().unwrap(), MfaPolicy::Optional));
        assert!(matches!("required".parse::<MfaPolicy>().unwrap(), MfaPolicy::Required));
        assert!(matches!(MfaPolicy::default(), MfaPolicy::Required));
        assert!("bogus".parse::<MfaPolicy>().is_err());
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p platform --lib mfa_policy_parses_and_defaults_required`
Expected: FAIL — `MfaPolicy` undefined.

- [ ] **Step 4: Implement** in `crates/platform/src/config.rs`.

Add near the top:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfaPolicy {
    Off,
    Optional,
    Required,
}

impl Default for MfaPolicy {
    fn default() -> Self {
        MfaPolicy::Required
    }
}

impl std::str::FromStr for MfaPolicy {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<MfaPolicy> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(MfaPolicy::Off),
            "optional" => Ok(MfaPolicy::Optional),
            "required" => Ok(MfaPolicy::Required),
            other => anyhow::bail!("invalid mfa_policy '{other}' (off|optional|required)"),
        }
    }
}

fn default_mfa_policy() -> String {
    "required".to_string()
}
```

Add fields to `AuthSettings` (after `admin_emails`):
```rust
    #[serde(default = "default_mfa_policy")]
    pub mfa_policy: String,
    #[serde(default)]
    pub mfa_encryption_key_file: String,
    #[serde(default)]
    pub mfa_encryption_key_base64: String,
```

Add methods to `impl AuthSettings`:
```rust
    pub fn mfa_policy(&self) -> MfaPolicy {
        self.mfa_policy.parse().unwrap_or(MfaPolicy::Required)
    }

    /// Resolve the 32-byte MFA encryption key. `Ok(None)` only when policy is Off.
    /// File path wins over inline base64; errors if policy != Off and no key resolves
    /// or the decoded key is not 32 bytes.
    pub fn mfa_encryption_key(&self) -> anyhow::Result<Option<[u8; 32]>> {
        if self.mfa_policy() == MfaPolicy::Off {
            return Ok(None);
        }
        let b64 = if !self.mfa_encryption_key_file.is_empty() {
            std::fs::read_to_string(&self.mfa_encryption_key_file)
                .with_context(|| format!("reading MFA key file '{}'", self.mfa_encryption_key_file))?
        } else if !self.mfa_encryption_key_base64.is_empty() {
            self.mfa_encryption_key_base64.clone()
        } else {
            anyhow::bail!(
                "mfa_policy != off but no MFA encryption key (set APP__AUTH__MFA_ENCRYPTION_KEY_FILE or _BASE64)"
            );
        };
        let bytes = base64_decode(b64.trim())?;
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("MFA encryption key must decode to exactly 32 bytes"))?;
        Ok(Some(arr))
    }
```

Add a tiny standard-base64 decoder helper (avoid a new dep — reuse if the `base64` crate is present, else a small impl). Since `base32` is added for TOTP, use base32 for the key too for consistency: replace `base64_decode` usage with base32 and rename the field/method to `_base32`. **Decision: use base32 for the key** (matches TOTP secret encoding, and `base32` is already a dep). So: fields `mfa_encryption_key_base32`, and:
```rust
        let key_str = /* file or inline as above */;
        let bytes = base32::decode(base32::Alphabet::Rfc4648 { padding: false }, key_str.trim())
            .ok_or_else(|| anyhow::anyhow!("MFA encryption key is not valid base32"))?;
```
(Update the field names in the struct + the `.env.example` in Task 11 to `MFA_ENCRYPTION_KEY_BASE32` / `_FILE`.) `use anyhow::Context;` is already imported at the top of config.rs.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p platform --lib mfa_policy_parses_and_defaults_required`
Expected: PASS.

- [ ] **Step 6: Update the `config.rs` test `AuthSettings` builder + `loads_settings_from_env`** — the `auth(...)` test helper constructs `AuthSettings` with a literal; add the three new fields (`mfa_policy: "required".into()`, `mfa_encryption_key_file: String::new()`, `mfa_encryption_key_base32: String::new()`). Run `cargo test -p platform --lib` → all green.

- [ ] **Step 7: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets --locked -- -D warnings
git add Cargo.toml Cargo.lock crates/domain-auth/Cargo.toml crates/platform/src/config.rs
git commit -m "feat(config): MfaPolicy + MFA encryption-key settings"
```

---

## Task 2: `AccessClaims.amr` + `issue_access` amr + `issue_mfa_token`

**Files:**
- Modify: `crates/platform/src/auth/mod.rs`, `crates/domain-auth/src/auth/jwt.rs`, `crates/domain-auth/src/ports/http.rs`

**Interfaces:**
- Produces: `AccessClaims.amr: Vec<String>`; `JwtIssuer::issue_access(user_id, email, scopes, amr: Vec<String>, now)`; `JwtIssuer::issue_mfa_token(user_id, purpose: MfaPurpose, now) -> anyhow::Result<String>`; `MfaPurpose { Pending, Enroll }`; `MfaTokenClaims { sub, exp, iat, token_type }` where `token_type` is `"mfa_pending"`/`"mfa_enroll"`.

- [ ] **Step 1: Add `amr` to `AccessClaims`** in `crates/platform/src/auth/mod.rs` (after `token_type`):
```rust
    #[serde(default)]
    pub amr: Vec<String>,
```
Update the test `claims(...)` helper in that file to add `amr: vec![]`.

- [ ] **Step 2: Write the failing test** — append to `crates/domain-auth/src/auth/jwt.rs` `mod tests`:
```rust
    #[test]
    fn access_token_carries_amr() {
        let issuer = JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap();
        let (_t, claims) = issuer
            .issue_access(1, "a@b.c", vec![], vec!["pwd".into(), "totp".into()], chrono::Utc::now())
            .unwrap();
        assert_eq!(claims.amr, vec!["pwd".to_string(), "totp".to_string()]);
    }

    #[test]
    fn mfa_token_has_purpose_type() {
        let issuer = JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap();
        let token = issuer.issue_mfa_token(1, MfaPurpose::Enroll, chrono::Utc::now()).unwrap();
        let verifier = JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap();
        let claims: MfaTokenClaims = verifier.decode(&token).unwrap();
        assert_eq!(claims.token_type, "mfa_enroll");
        assert_eq!(claims.sub, "user-1");
    }
```

- [ ] **Step 2b: Run to verify it fails**

Run: `cargo test -p domain-auth --lib jwt`
Expected: FAIL — `issue_access` arity / `issue_mfa_token`/`MfaPurpose`/`MfaTokenClaims` undefined.

- [ ] **Step 3: Implement** in `crates/domain-auth/src/auth/jwt.rs`.

Add the `amr` param to `issue_access` (insert before `now`) and set it in the struct:
```rust
    pub fn issue_access(
        &self,
        user_id: i64,
        email: &str,
        scopes: Vec<String>,
        amr: Vec<String>,
        now: DateTime<Utc>,
    ) -> anyhow::Result<(String, AccessClaims)> {
        let exp = (now + Duration::seconds(self.access_ttl_seconds)).timestamp() as usize;
        let claims = AccessClaims {
            sub: format!("user-{user_id}"),
            scopes,
            exp,
            iat: now.timestamp() as usize,
            jti: uuid::Uuid::new_v4().to_string(),
            email: Some(email.to_string()),
            token_type: "user".to_string(),
            amr,
        };
        let token = encode(&Header::new(Algorithm::RS256), &claims, &self.key)?;
        Ok((token, claims))
    }
```

Add the MFA-token types + issuer (after `issue_refresh`), 5-min TTL:
```rust
#[derive(Debug, Clone, Copy)]
pub enum MfaPurpose {
    Pending,
    Enroll,
}

impl MfaPurpose {
    fn token_type(self) -> &'static str {
        match self {
            MfaPurpose::Pending => "mfa_pending",
            MfaPurpose::Enroll => "mfa_enroll",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MfaTokenClaims {
    pub sub: String,
    pub iat: usize,
    pub exp: usize,
    #[serde(rename = "type")]
    pub token_type: String,
}
```
Add to `impl JwtIssuer`:
```rust
    /// Short-lived (5 min) single-purpose token gating the next MFA step.
    pub fn issue_mfa_token(
        &self,
        user_id: i64,
        purpose: MfaPurpose,
        now: DateTime<Utc>,
    ) -> anyhow::Result<String> {
        let claims = MfaTokenClaims {
            sub: format!("user-{user_id}"),
            iat: now.timestamp() as usize,
            exp: (now + Duration::minutes(5)).timestamp() as usize,
            token_type: purpose.token_type().to_string(),
        };
        Ok(encode(&Header::new(Algorithm::RS256), &claims, &self.key)?)
    }
```

- [ ] **Step 4: Fix the two existing `issue_access` call sites** in `crates/domain-auth/src/ports/http.rs` — `issue_token_pair` (~line 76) and `refresh` (~line 190). Pass an `amr`: `issue_token_pair` should accept the amr from its caller — change its signature to `issue_token_pair(state: &AuthState, user: &User, amr: Vec<String>)` and pass `amr` into `issue_access`. Update the `register` call to `issue_token_pair(&state, &user, vec!["pwd".into()])`. In `refresh`, the token is re-minted from a refresh token (MFA already satisfied earlier), so pass `vec!["pwd".into()]` (or carry forward — v1: `vec!["pwd".into()]`). The `login` caller is rewritten in Task 7; for now update it to `issue_token_pair(&state, &user, vec!["pwd".into()])` to keep it compiling.

- [ ] **Step 5: Run tests**

Run: `cargo test -p platform --lib && cargo test -p domain-auth --lib jwt`
Expected: PASS (amr + mfa-token tests green; platform claims test updated).

- [ ] **Step 6: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets --locked -- -D warnings
git add crates/platform/src/auth/mod.rs crates/domain-auth/src/auth/jwt.rs crates/domain-auth/src/ports/http.rs
git commit -m "feat(auth): amr claim + short-lived MFA-purpose tokens"
```

---

## Task 3: `MfaCipher` (ChaCha20-Poly1305)

**Files:**
- Create: `crates/domain-auth/src/auth/mfa_crypto.rs`
- Modify: `crates/domain-auth/src/auth/mod.rs` (module decl)

**Interfaces:**
- Produces: `MfaCipher::new(key: [u8;32]) -> MfaCipher`; `encrypt(&self, plaintext: &str) -> anyhow::Result<Vec<u8>>` (returns `nonce‖ciphertext`); `decrypt(&self, blob: &[u8]) -> anyhow::Result<String>`.

- [ ] **Step 1: Declare the module** — in `crates/domain-auth/src/auth/mod.rs` add `pub mod mfa_crypto;`.

- [ ] **Step 2: Write the failing test** — create `crates/domain-auth/src/auth/mfa_crypto.rs` with only tests first:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_then_decrypt_roundtrips() {
        let cipher = MfaCipher::new([7u8; 32]);
        let blob = cipher.encrypt("JBSWY3DPEHPK3PXP").unwrap();
        assert_ne!(blob, b"JBSWY3DPEHPK3PXP"); // not plaintext
        assert_eq!(cipher.decrypt(&blob).unwrap(), "JBSWY3DPEHPK3PXP");
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let cipher = MfaCipher::new([7u8; 32]);
        let mut blob = cipher.encrypt("secret").unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0xFF;
        assert!(cipher.decrypt(&blob).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let a = MfaCipher::new([1u8; 32]);
        let b = MfaCipher::new([2u8; 32]);
        let blob = a.encrypt("secret").unwrap();
        assert!(b.decrypt(&blob).is_err());
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p domain-auth --lib mfa_crypto`
Expected: FAIL — `MfaCipher` undefined.

- [ ] **Step 4: Implement** (prepend to the file, above the tests):
```rust
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

const NONCE_LEN: usize = 12;

/// AEAD wrapper for encrypting TOTP secrets at rest. Stored blob is nonce‖ciphertext.
#[derive(Clone)]
pub struct MfaCipher {
    cipher: ChaCha20Poly1305,
}

impl MfaCipher {
    pub fn new(key: [u8; 32]) -> MfaCipher {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
        MfaCipher { cipher }
    }

    pub fn encrypt(&self, plaintext: &str) -> anyhow::Result<Vec<u8>> {
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ct = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| anyhow::anyhow!("mfa encrypt: {e}"))?;
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(nonce.as_slice());
        out.extend_from_slice(&ct);
        Ok(out)
    }

    pub fn decrypt(&self, blob: &[u8]) -> anyhow::Result<String> {
        if blob.len() < NONCE_LEN {
            anyhow::bail!("mfa ciphertext too short");
        }
        let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);
        let pt = self
            .cipher
            .decrypt(nonce, ct)
            .map_err(|e| anyhow::anyhow!("mfa decrypt: {e}"))?;
        Ok(String::from_utf8(pt)?)
    }
}
```
> If the resolved `chacha20poly1305` API differs (e.g. `generate_nonce`/`Key::from_slice`), adapt; behavior (random-nonce AEAD, nonce‖ct blob) is fixed.

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p domain-auth --lib mfa_crypto`
Expected: PASS (3 tests).

- [ ] **Step 6: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets --locked -- -D warnings
git add crates/domain-auth/src/auth/mfa_crypto.rs crates/domain-auth/src/auth/mod.rs
git commit -m "feat(auth): MfaCipher (ChaCha20-Poly1305) for TOTP secret at rest"
```

---

## Task 4: `TotpVerifier` + `FactorVerifier` trait

**Files:**
- Create: `crates/domain-auth/src/auth/totp.rs`
- Modify: `crates/domain-auth/src/auth/mod.rs` (module decl)

**Interfaces:**
- Produces: trait `FactorVerifier: Send + Sync` with `generate_secret(&self) -> String` (base32), `provisioning_uri(&self, secret: &str, account: &str) -> anyhow::Result<String>`, `verify(&self, secret: &str, code: &str, now: DateTime<Utc>) -> bool`; impl `TotpVerifier::new(issuer: String)`.

- [ ] **Step 1: Declare module** — `crates/domain-auth/src/auth/mod.rs` add `pub mod totp;`.

- [ ] **Step 2: Write the failing test** — create `crates/domain-auth/src/auth/totp.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn generated_secret_verifies_current_code() {
        let v = TotpVerifier::new("rust-service".into());
        let secret = v.generate_secret();
        let now = Utc::now();
        // derive the current code via a fresh TOTP over the same secret
        let code = v.current_code(&secret, now).unwrap();
        assert!(v.verify(&secret, &code, now));
        assert!(!v.verify(&secret, "000000", now));
    }

    #[test]
    fn provisioning_uri_is_otpauth() {
        let v = TotpVerifier::new("rust-service".into());
        let uri = v.provisioning_uri(&v.generate_secret(), "a@b.c").unwrap();
        assert!(uri.starts_with("otpauth://totp/"));
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p domain-auth --lib totp`
Expected: FAIL — `TotpVerifier`/`FactorVerifier` undefined.

- [ ] **Step 4: Implement** (prepend, above tests):
```rust
use chrono::{DateTime, Utc};
use totp_rs::{Algorithm, Secret, TOTP};

/// A pluggable second-factor verifier. v1 impl is TOTP; WebAuthn/email-OTP can add
/// impls later keyed by the `auth_mfa_factor.type` column.
pub trait FactorVerifier: Send + Sync {
    fn generate_secret(&self) -> String;
    fn provisioning_uri(&self, secret_base32: &str, account: &str) -> anyhow::Result<String>;
    fn verify(&self, secret_base32: &str, code: &str, now: DateTime<Utc>) -> bool;
}

pub struct TotpVerifier {
    issuer: String,
}

impl TotpVerifier {
    pub fn new(issuer: String) -> TotpVerifier {
        TotpVerifier { issuer }
    }

    fn totp(&self, secret_base32: &str, account: &str) -> anyhow::Result<TOTP> {
        let bytes = Secret::Encoded(secret_base32.to_string())
            .to_bytes()
            .map_err(|e| anyhow::anyhow!("bad totp secret: {e:?}"))?;
        TOTP::new(
            Algorithm::SHA1,
            6,
            1, // skew: accept ±1 step for clock drift
            30,
            bytes,
            Some(self.issuer.clone()),
            account.to_string(),
        )
        .map_err(|e| anyhow::anyhow!("totp init: {e}"))
    }

    #[cfg(test)]
    pub fn current_code(&self, secret_base32: &str, now: DateTime<Utc>) -> anyhow::Result<String> {
        let totp = self.totp(secret_base32, "test")?;
        Ok(totp.generate(now.timestamp() as u64))
    }
}

impl FactorVerifier for TotpVerifier {
    fn generate_secret(&self) -> String {
        Secret::generate_secret().to_encoded().to_string()
    }

    fn provisioning_uri(&self, secret_base32: &str, account: &str) -> anyhow::Result<String> {
        Ok(self.totp(secret_base32, account)?.get_url())
    }

    fn verify(&self, secret_base32: &str, code: &str, now: DateTime<Utc>) -> bool {
        match self.totp(secret_base32, "verify") {
            Ok(totp) => totp.check(code, now.timestamp() as u64),
            Err(_) => false,
        }
    }
}
```
> If the resolved `totp-rs` API differs (`TOTP::new` arity, `Secret::to_bytes`/`to_encoded`, `check`/`generate`/`get_url`), adapt to the resolved signatures. Behavior fixed: RFC-6238 SHA1/6-digit/30s/±1-skew, base32 secret, `otpauth://` URI.

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p domain-auth --lib totp`
Expected: PASS.

- [ ] **Step 6: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets --locked -- -D warnings
git add crates/domain-auth/src/auth/totp.rs crates/domain-auth/src/auth/mod.rs
git commit -m "feat(auth): TotpVerifier + FactorVerifier trait"
```

---

## Task 5: Recovery-code generation + hashing

**Files:**
- Create: `crates/domain-auth/src/auth/recovery.rs`
- Modify: `crates/domain-auth/src/auth/mod.rs`

**Interfaces:**
- Produces: `generate_recovery_codes() -> Vec<String>` (10 codes, `xxxxx-xxxxx` lowercase base32-ish); `hash_recovery_code(code: &str) -> anyhow::Result<String>`; `verify_recovery_code(hash: &str, code: &str) -> bool` (reuse `password.rs`).

- [ ] **Step 1: Declare module** — `mod.rs` add `pub mod recovery;`.

- [ ] **Step 2: Write the failing test** — `crates/domain-auth/src/auth/recovery.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_ten_unique_codes() {
        let codes = generate_recovery_codes();
        assert_eq!(codes.len(), 10);
        let uniq: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(uniq.len(), 10);
        assert!(codes.iter().all(|c| c.len() == 11 && c.contains('-')));
    }

    #[test]
    fn hash_then_verify() {
        let code = "abcde-fghij".to_string();
        let hash = hash_recovery_code(&code).unwrap();
        assert!(verify_recovery_code(&hash, &code));
        assert!(!verify_recovery_code(&hash, "wrong-code0"));
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p domain-auth --lib recovery`
Expected: FAIL — undefined.

- [ ] **Step 4: Implement** (prepend):
```rust
use crate::auth::password::{hash_password, verify_password};
use totp_rs::Secret;

/// 10 single-use recovery codes formatted `xxxxx-xxxxx` (lowercased base32, no padding).
pub fn generate_recovery_codes() -> Vec<String> {
    (0..10)
        .map(|_| {
            let raw = Secret::generate_secret().to_encoded().to_string().to_lowercase();
            let s: String = raw.chars().filter(|c| c.is_ascii_alphanumeric()).take(10).collect();
            format!("{}-{}", &s[0..5], &s[5..10])
        })
        .collect()
}

pub fn hash_recovery_code(code: &str) -> anyhow::Result<String> {
    hash_password(code)
}

pub fn verify_recovery_code(hash: &str, code: &str) -> bool {
    verify_password(hash, code)
}
```
(If `Secret::generate_secret().to_encoded()` yields fewer than 10 alphanumerics, loop/pad — in practice a base32 secret is ≥16 chars, so `take(10)` is safe.)

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p domain-auth --lib recovery`
Expected: PASS.

- [ ] **Step 6: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets --locked -- -D warnings
git add crates/domain-auth/src/auth/recovery.rs crates/domain-auth/src/auth/mod.rs
git commit -m "feat(auth): recovery-code generation + hashing"
```

---

## Task 6: Migration + `MfaRepository` (trait + Postgres impl)

**Files:**
- Create: `migrations/0007_mfa.sql`
- Modify: `crates/domain-auth/src/ports/repository.rs`, `crates/domain-auth/src/ports/postgres.rs`
- Test: `crates/domain-auth/tests/mfa_repo.rs`

**Interfaces:**
- Produces: `MfaFactor { id, user_id, factor_type, secret_encrypted: Vec<u8>, confirmed_at: Option<DateTime<Utc>>, failed_attempts: i32, locked_until: Option<DateTime<Utc>> }`; trait `MfaRepository` (methods listed below); impl on `PostgresUserRepository`.

- [ ] **Step 1: Write the migration** — `migrations/0007_mfa.sql`:
```sql
create table auth_mfa_factor (
    id               bigserial primary key,
    user_id          bigint      not null references auth_user (id),
    type             text        not null default 'totp',
    secret_encrypted bytea       not null,
    confirmed_at     timestamptz,
    failed_attempts  int         not null default 0,
    locked_until     timestamptz,
    created_at       timestamptz not null default now(),
    unique (user_id, type)
);

create table auth_mfa_recovery_code (
    id         bigserial primary key,
    user_id    bigint      not null references auth_user (id),
    code_hash  text        not null,
    used_at    timestamptz,
    created_at timestamptz not null default now()
);
create index auth_mfa_recovery_code_user_idx on auth_mfa_recovery_code (user_id);
```

- [ ] **Step 2: Add the trait + struct** in `crates/domain-auth/src/ports/repository.rs`:
```rust
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MfaFactor {
    pub id: i64,
    pub user_id: i64,
    #[sqlx(rename = "type")]
    pub factor_type: String,
    pub secret_encrypted: Vec<u8>,
    pub confirmed_at: Option<DateTime<Utc>>,
    pub failed_attempts: i32,
    pub locked_until: Option<DateTime<Utc>>,
}

#[async_trait::async_trait]
pub trait MfaRepository: Send + Sync {
    async fn confirmed_factor(&self, user_id: i64) -> anyhow::Result<Option<MfaFactor>>;
    async fn get_factor(&self, user_id: i64, factor_type: &str) -> anyhow::Result<Option<MfaFactor>>;
    async fn upsert_unconfirmed_factor(
        &self,
        user_id: i64,
        factor_type: &str,
        secret_encrypted: &[u8],
    ) -> anyhow::Result<()>;
    async fn confirm_factor(&self, user_id: i64, factor_type: &str) -> anyhow::Result<()>;
    async fn delete_factors(&self, user_id: i64) -> anyhow::Result<()>;
    async fn record_failed_attempt(&self, factor_id: i64, locked_until: Option<DateTime<Utc>>) -> anyhow::Result<()>;
    async fn reset_attempts(&self, factor_id: i64) -> anyhow::Result<()>;
    async fn store_recovery_codes(&self, user_id: i64, hashes: &[String]) -> anyhow::Result<()>;
    /// Returns true and marks used iff an unused code matching `code` exists.
    async fn consume_recovery_code(&self, user_id: i64, code: &str) -> anyhow::Result<bool>;
    async fn delete_recovery_codes(&self, user_id: i64) -> anyhow::Result<()>;
}
```
(Add `use crate::models` imports as needed; `DateTime, Utc` already imported.)

- [ ] **Step 3: Write the failing test** — `crates/domain-auth/tests/mfa_repo.rs`:
```rust
use domain_auth::ports::postgres::PostgresUserRepository;
use domain_auth::ports::MfaRepository;

async fn seed_user(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar(
        "insert into auth_user (email, password_hash, created_by_cid) \
         values ('a@b.c', 'x', 'cid') returning id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn factor_lifecycle(pool: sqlx::PgPool) {
    let repo = PostgresUserRepository::new(pool.clone());
    let uid = seed_user(&pool).await;

    assert!(repo.confirmed_factor(uid).await.unwrap().is_none());
    repo.upsert_unconfirmed_factor(uid, "totp", b"enc").await.unwrap();
    assert!(repo.confirmed_factor(uid).await.unwrap().is_none()); // unconfirmed
    let f = repo.get_factor(uid, "totp").await.unwrap().unwrap();
    assert_eq!(f.secret_encrypted, b"enc");

    repo.confirm_factor(uid, "totp").await.unwrap();
    assert!(repo.confirmed_factor(uid).await.unwrap().is_some());

    repo.store_recovery_codes(uid, &["h1".into(), "h2".into()]).await.unwrap();
    // consume matching a hash we can verify is impossible here (hashes are bcrypt);
    // instead test delete path:
    repo.delete_factors(uid).await.unwrap();
    repo.delete_recovery_codes(uid).await.unwrap();
    assert!(repo.confirmed_factor(uid).await.unwrap().is_none());
}
```

- [ ] **Step 4: Run to verify it fails**

Run: `DATABASE_URL=postgres://arthurjordao@localhost:5432/postgres cargo test -p domain-auth --test mfa_repo`
Expected: FAIL — `MfaRepository` not impl'd on `PostgresUserRepository`.

- [ ] **Step 5: Implement** the `MfaRepository` impl in `crates/domain-auth/src/ports/postgres.rs` (new `impl` block, runtime query API). Key queries:
```rust
#[async_trait::async_trait]
impl MfaRepository for PostgresUserRepository {
    async fn confirmed_factor(&self, user_id: i64) -> anyhow::Result<Option<MfaFactor>> {
        let f = sqlx::query_as::<_, MfaFactor>(
            "select id, user_id, type, secret_encrypted, confirmed_at, failed_attempts, locked_until \
             from auth_mfa_factor where user_id = $1 and confirmed_at is not null",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(f)
    }

    async fn get_factor(&self, user_id: i64, factor_type: &str) -> anyhow::Result<Option<MfaFactor>> {
        let f = sqlx::query_as::<_, MfaFactor>(
            "select id, user_id, type, secret_encrypted, confirmed_at, failed_attempts, locked_until \
             from auth_mfa_factor where user_id = $1 and type = $2",
        )
        .bind(user_id).bind(factor_type)
        .fetch_optional(&self.pool).await?;
        Ok(f)
    }

    async fn upsert_unconfirmed_factor(&self, user_id: i64, factor_type: &str, secret_encrypted: &[u8]) -> anyhow::Result<()> {
        sqlx::query(
            "insert into auth_mfa_factor (user_id, type, secret_encrypted) values ($1, $2, $3) \
             on conflict (user_id, type) do update set secret_encrypted = excluded.secret_encrypted, \
                 confirmed_at = null, failed_attempts = 0, locked_until = null",
        )
        .bind(user_id).bind(factor_type).bind(secret_encrypted)
        .execute(&self.pool).await?;
        Ok(())
    }

    async fn confirm_factor(&self, user_id: i64, factor_type: &str) -> anyhow::Result<()> {
        sqlx::query("update auth_mfa_factor set confirmed_at = now() where user_id = $1 and type = $2")
            .bind(user_id).bind(factor_type).execute(&self.pool).await?;
        Ok(())
    }

    async fn delete_factors(&self, user_id: i64) -> anyhow::Result<()> {
        sqlx::query("delete from auth_mfa_factor where user_id = $1").bind(user_id).execute(&self.pool).await?;
        Ok(())
    }

    async fn record_failed_attempt(&self, factor_id: i64, locked_until: Option<DateTime<Utc>>) -> anyhow::Result<()> {
        sqlx::query("update auth_mfa_factor set failed_attempts = failed_attempts + 1, locked_until = $2 where id = $1")
            .bind(factor_id).bind(locked_until).execute(&self.pool).await?;
        Ok(())
    }

    async fn reset_attempts(&self, factor_id: i64) -> anyhow::Result<()> {
        sqlx::query("update auth_mfa_factor set failed_attempts = 0, locked_until = null where id = $1")
            .bind(factor_id).execute(&self.pool).await?;
        Ok(())
    }

    async fn store_recovery_codes(&self, user_id: i64, hashes: &[String]) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("delete from auth_mfa_recovery_code where user_id = $1").bind(user_id).execute(&mut *tx).await?;
        for h in hashes {
            sqlx::query("insert into auth_mfa_recovery_code (user_id, code_hash) values ($1, $2)")
                .bind(user_id).bind(h).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn consume_recovery_code(&self, user_id: i64, code: &str) -> anyhow::Result<bool> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "select id, code_hash from auth_mfa_recovery_code where user_id = $1 and used_at is null",
        )
        .bind(user_id).fetch_all(&self.pool).await?;
        for (id, hash) in rows {
            if crate::auth::recovery::verify_recovery_code(&hash, code) {
                sqlx::query("update auth_mfa_recovery_code set used_at = now() where id = $1")
                    .bind(id).execute(&self.pool).await?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn delete_recovery_codes(&self, user_id: i64) -> anyhow::Result<()> {
        sqlx::query("delete from auth_mfa_recovery_code where user_id = $1").bind(user_id).execute(&self.pool).await?;
        Ok(())
    }
}
```
Ensure `MfaRepository`, `MfaFactor` are re-exported from `ports/mod.rs` (add to the existing `pub use repository::{...}`).

- [ ] **Step 6: Run to verify it passes**

Run: `DATABASE_URL=... cargo test -p domain-auth --test mfa_repo`
Expected: PASS.

- [ ] **Step 7: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets --locked -- -D warnings
git add migrations/0007_mfa.sql crates/domain-auth/src/ports/repository.rs crates/domain-auth/src/ports/postgres.rs crates/domain-auth/src/ports/mod.rs crates/domain-auth/tests/mfa_repo.rs
git commit -m "feat(auth): MFA schema + MfaRepository (factors + recovery codes)"
```

---

## Task 7: `AuthState` wiring + `MfaConfig` (compile the crate + all call sites)

**Files:**
- Modify: `crates/domain-auth/src/ports/http.rs` (`AuthState`, `MfaConfig`, `MfaPolicy` re-export), `crates/app/src/state.rs`, and the 7 test `AuthState` builders.

**Interfaces:**
- Produces: `MfaConfig { policy: MfaPolicy, cipher: Option<MfaCipher> }`; `AuthState` gains `mfa: Arc<dyn MfaRepository>`, `mfa_verifier: Arc<dyn FactorVerifier>`, `mfa_config: MfaConfig`. This task only threads the fields (no behavior yet) so everything compiles.

- [ ] **Step 1: Add the fields + `MfaConfig`** in `crates/domain-auth/src/ports/http.rs`:
```rust
use crate::auth::mfa_crypto::MfaCipher;
use crate::auth::totp::FactorVerifier;
use crate::ports::MfaRepository;
use platform::config::MfaPolicy;

#[derive(Clone)]
pub struct MfaConfig {
    pub policy: MfaPolicy,
    pub cipher: Option<std::sync::Arc<MfaCipher>>,
}
```
Add to `AuthState`:
```rust
    pub mfa: Arc<dyn MfaRepository>,
    pub mfa_verifier: Arc<dyn FactorVerifier>,
    pub mfa_config: MfaConfig,
```

- [ ] **Step 2: Wire the composition root** — `crates/app/src/state.rs` `auth_state(...)` (and `build_resources` if the cipher/verifier are built there). Build from settings:
```rust
    let mfa_policy = res.settings.auth.mfa_policy();
    let mfa_cipher = res.settings.auth.mfa_encryption_key()
        .expect("MFA key")  // build_resources already returns Result — use `?` there instead of expect
        .map(|k| Arc::new(domain_auth::auth::mfa_crypto::MfaCipher::new(k)));
```
Set on `AuthState`:
```rust
    mfa: Arc::new(PostgresUserRepository::new(res.pool.clone())),
    mfa_verifier: Arc::new(domain_auth::auth::totp::TotpVerifier::new("rust-service".into())),
    mfa_config: MfaConfig { policy: mfa_policy, cipher: mfa_cipher },
```
Resolve the key in `build_resources` (which returns `anyhow::Result`) with `?` and stash on `Resources`, or compute in `auth_state`. Prefer resolving in `build_resources` so startup fail-fast happens at boot: add `res.mfa_cipher: Option<Arc<MfaCipher>>` + `res.mfa_policy` to `Resources`.

- [ ] **Step 3: Update the 7 test `AuthState` builders** — run `grep -rn "AuthState {" crates` and add to each builder:
```rust
        mfa: repo.clone(),
        mfa_verifier: Arc::new(domain_auth::auth::totp::TotpVerifier::new("test".into())),
        mfa_config: domain_auth::ports::http::MfaConfig {
            policy: platform::config::MfaPolicy::Off,
            cipher: None,
        },
```
(Where the builder already has a `repo` var it's `repo.clone()`; the app-crate e2e builders may need `use` paths. Default the test policy to `Off` unless a specific test needs otherwise — MFA tests in Task 8+ build their own state with a cipher + policy.)

- [ ] **Step 4: Build the workspace**

Run: `DATABASE_URL=... cargo build --workspace --locked`
Expected: compiles (fields threaded everywhere; no behavior change yet).

- [ ] **Step 5: Run the existing suites (no regressions)**

Run: `DATABASE_URL=... cargo test -p domain-auth -p app`
Expected: PASS (existing auth/e2e tests unaffected — policy Off = today's behavior).

- [ ] **Step 6: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets --locked -- -D warnings
git add -A
git commit -m "feat(auth): thread MfaConfig/mfa repo/verifier through AuthState + call sites"
```

---

## Task 8: Login state machine + `LoginResponse`

**Files:**
- Modify: `crates/domain-auth/src/ports/dto.rs`, `crates/domain-auth/src/ports/http.rs`
- Test: `crates/domain-auth/tests/mfa.rs` (new)

**Interfaces:**
- Consumes: `issue_mfa_token`/`MfaPurpose` (Task 2), `mfa.confirmed_factor` (Task 6), `MfaConfig` (Task 7).
- Produces: `LoginResponse` tagged enum; `login` returns `Json<LoginResponse>`.

- [ ] **Step 1: Add DTOs** in `crates/domain-auth/src/ports/dto.rs`:
```rust
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LoginResponse {
    Authenticated { tokens: AuthTokens },
    MfaRequired { purpose: String, mfa_token: String, factor_types: Vec<String> },
}
```

- [ ] **Step 2: Write the failing test** — `crates/domain-auth/tests/mfa.rs` (state builder with a real cipher + a chosen policy; helper to register a user):
```rust
// ... imports mirroring tests/http.rs, plus a `state_with(pool, policy)` builder that
// sets mfa_config.policy and a cipher (MfaCipher::new([9u8;32])), mfa_verifier = TotpVerifier.
#[sqlx::test(migrations = "../../migrations")]
async fn login_required_no_factor_returns_enroll_challenge(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Required));
    register(&app, "a@b.c", "pw").await;
    let (status, body) = post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "mfa_required");
    assert_eq!(body["purpose"], "enroll");
    assert!(body["mfa_token"].as_str().unwrap().len() > 10);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_optional_no_factor_authenticates(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Optional));
    register(&app, "a@b.c", "pw").await;
    let (status, body) = post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "authenticated");
    assert!(body["tokens"]["access_token"].as_str().unwrap().len() > 10);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_off_authenticates(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Off));
    register(&app, "a@b.c", "pw").await;
    let (status, body) = post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "authenticated");
}
```
(Include the `state_with`, `register`, `post_json` helpers at the top of the file — model them on `tests/http.rs`'s `state` + oneshot pattern; `register` posts `/auth/register`.)

- [ ] **Step 3: Run to verify it fails**

Run: `DATABASE_URL=... cargo test -p domain-auth --test mfa`
Expected: FAIL — `login` still returns `AuthTokens`, not `LoginResponse`.

- [ ] **Step 4: Rewrite `login`** in `crates/domain-auth/src/ports/http.rs`:
```rust
#[utoipa::path(post, path = "/auth/login", request_body = LoginRequest,
    responses((status = 200, body = LoginResponse), (status = 401)), tag = "auth")]
pub(crate) async fn login(
    State(state): State<AuthState>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, AppError> {
    let found = state.users.find_by_email(&body.email).await.map_err(AppError::Internal)?;
    let user = match check_credentials(found.as_ref(), &body.password) {
        Ok(u) => u.clone(),
        Err(e) => { tracing::warn!(email = %body.email, "login failed"); return Err(e); }
    };

    let enabled = state.mfa.confirmed_factor(user.id).await.map_err(AppError::Internal)?.is_some();
    let now = chrono::Utc::now();
    use platform::config::MfaPolicy;
    let response = match (state.mfa_config.policy, enabled) {
        (MfaPolicy::Off, _) | (MfaPolicy::Optional, false) => {
            let tokens = issue_token_pair(&state, &user, vec!["pwd".into()]).await?;
            LoginResponse::Authenticated { tokens }
        }
        (_, true) => {
            let mfa_token = state.issuer.issue_mfa_token(user.id, crate::auth::jwt::MfaPurpose::Pending, now).map_err(AppError::Internal)?;
            LoginResponse::MfaRequired { purpose: "verify".into(), mfa_token, factor_types: vec!["totp".into()] }
        }
        (MfaPolicy::Required, false) => {
            let mfa_token = state.issuer.issue_mfa_token(user.id, crate::auth::jwt::MfaPurpose::Enroll, now).map_err(AppError::Internal)?;
            LoginResponse::MfaRequired { purpose: "enroll".into(), mfa_token, factor_types: vec!["totp".into()] }
        }
    };
    tracing::info!(email = %user.email, "login processed");
    Ok(Json(response))
}
```
Add `LoginResponse` to the `dto` import list.

- [ ] **Step 5: Run to verify it passes**

Run: `DATABASE_URL=... cargo test -p domain-auth --test mfa`
Expected: PASS (3 login-branch tests). Also `cargo test -p app` — the e2e tests that log in use policy Off in their builders, so they still get `AuthTokens` under `tokens` — **update those e2e assertions** if they parse the login body directly (they now get `{status:"authenticated", tokens:{...}}`). Fix any that break.

- [ ] **Step 6: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets --locked -- -D warnings
git add -A
git commit -m "feat(auth): login state machine returns tagged LoginResponse"
```

---

## Task 9: Enrollment endpoints (setup + confirm)

**Files:**
- Modify: `crates/domain-auth/src/ports/dto.rs`, `crates/domain-auth/src/ports/http.rs`
- Test: `crates/domain-auth/tests/mfa.rs`

**Interfaces:**
- Consumes: `mfa_config.cipher`, `mfa_verifier`, `mfa` repo, `issue_mfa_token`/`MfaTokenClaims`, recovery helpers.
- Produces: `POST /auth/mfa/setup`, `POST /auth/mfa/confirm`; DTOs `MfaSetupResponse { provisioning_uri, secret }`, `MfaConfirmRequest { code }`, `MfaConfirmResponse { recovery_codes: Vec<String>, tokens: Option<AuthTokens> }`; a helper `mfa_user_id(&state, headers, allowed_types) -> Result<i64, AppError>` that accepts either a valid access token OR an mfa-token of an allowed `token_type`.

- [ ] **Step 1: Add DTOs** (dto.rs):
```rust
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MfaSetupResponse { pub provisioning_uri: String, pub secret: String }

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MfaConfirmRequest { pub code: String }

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MfaConfirmResponse {
    pub recovery_codes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<AuthTokens>,
}
```

- [ ] **Step 2: Write the failing test** (mfa.rs) — full forced-enroll flow:
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn forced_enroll_flow_issues_tokens_with_amr(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Required));
    register(&app, "a@b.c", "pw").await;
    let (_s, login) = post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    let mfa_token = login["mfa_token"].as_str().unwrap().to_string();

    // setup with the enroll token
    let (s1, setup) = post_bearer(&app, "/auth/mfa/setup", &mfa_token, "{}").await;
    assert_eq!(s1, 200);
    let secret = setup["secret"].as_str().unwrap().to_string();

    // compute a valid code for `secret` (test TotpVerifier) and confirm
    let code = current_totp_code(&secret);
    let (s2, confirm) = post_bearer(&app, "/auth/mfa/confirm", &mfa_token,
        &format!(r#"{{"code":"{code}"}}"#)).await;
    assert_eq!(s2, 200);
    assert_eq!(confirm["recovery_codes"].as_array().unwrap().len(), 10);
    assert!(confirm["tokens"]["access_token"].as_str().unwrap().len() > 10);
    // amr present in the issued access token
    // (decode with the test verifier and assert amr contains "totp")
}
```
(Add `current_totp_code` helper using `TotpVerifier::new("test").current_code(secret, Utc::now())`.)

- [ ] **Step 3: Run to verify it fails** (`--test mfa`) → FAIL (routes 404 / handlers missing).

- [ ] **Step 4: Implement** the `mfa_user_id` auth helper + handlers in `http.rs`, register routes in `router`:
```rust
// in router():
    .route("/auth/mfa/setup", post(mfa_setup))
    .route("/auth/mfa/confirm", post(mfa_confirm))
```
```rust
use crate::auth::jwt::MfaTokenClaims;

/// Resolve the acting user from either a normal access token or an mfa-token whose
/// token_type is in `allowed` (e.g. ["mfa_enroll"]). Returns (user_id, from_mfa_token).
fn mfa_user_id(state: &AuthState, headers: &http::HeaderMap, allowed: &[&str]) -> Result<(i64, bool), AppError> {
    let token = headers.get(http::header::AUTHORIZATION).and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or_else(|| AppError::Unauthorized("missing Bearer token".into()))?;
    // Try an access token first.
    if let Ok(claims) = state.verifier.verify(token) {
        if claims.token_type == "user" {
            let id = claims.sub.strip_prefix("user-").and_then(|s| s.parse().ok())
                .ok_or_else(|| AppError::Unauthorized("bad sub".into()))?;
            return Ok((id, false));
        }
    }
    // Else an mfa-token of an allowed type.
    let claims: MfaTokenClaims = state.verifier.decode(token)?;
    if !allowed.contains(&claims.token_type.as_str()) {
        return Err(AppError::Unauthorized("wrong token for this step".into()));
    }
    let id = claims.sub.strip_prefix("user-").and_then(|s| s.parse().ok())
        .ok_or_else(|| AppError::Unauthorized("bad sub".into()))?;
    Ok((id, true))
}

fn cipher(state: &AuthState) -> Result<&MfaCipher, AppError> {
    state.mfa_config.cipher.as_deref()
        .ok_or_else(|| AppError::Conflict("MFA is disabled".into()))
}

#[utoipa::path(post, path = "/auth/mfa/setup", responses((status = 200, body = MfaSetupResponse)), tag = "auth")]
pub(crate) async fn mfa_setup(
    State(state): State<AuthState>,
    headers: http::HeaderMap,
) -> Result<Json<MfaSetupResponse>, AppError> {
    let (user_id, _) = mfa_user_id(&state, &headers, &["mfa_enroll"])?;
    let user = state.users.find_by_id(user_id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::Unauthorized("user not found".into()))?;
    let secret = state.mfa_verifier.generate_secret();
    let uri = state.mfa_verifier.provisioning_uri(&secret, &user.email).map_err(AppError::Internal)?;
    let enc = cipher(&state)?.encrypt(&secret).map_err(AppError::Internal)?;
    state.mfa.upsert_unconfirmed_factor(user_id, "totp", &enc).await.map_err(AppError::Internal)?;
    tracing::info!(user_id, "mfa setup initiated");
    Ok(Json(MfaSetupResponse { provisioning_uri: uri, secret }))
}

#[utoipa::path(post, path = "/auth/mfa/confirm", request_body = MfaConfirmRequest,
    responses((status = 200, body = MfaConfirmResponse)), tag = "auth")]
pub(crate) async fn mfa_confirm(
    State(state): State<AuthState>,
    headers: http::HeaderMap,
    Json(body): Json<MfaConfirmRequest>,
) -> Result<Json<MfaConfirmResponse>, AppError> {
    let (user_id, from_mfa_token) = mfa_user_id(&state, &headers, &["mfa_enroll"])?;
    let factor = state.mfa.get_factor(user_id, "totp").await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("no pending factor; call setup first".into()))?;
    let secret = cipher(&state)?.decrypt(&factor.secret_encrypted).map_err(AppError::Internal)?;
    if !state.mfa_verifier.verify(&secret, &body.code, chrono::Utc::now()) {
        return Err(AppError::Unauthorized("invalid code".into()));
    }
    state.mfa.confirm_factor(user_id, "totp").await.map_err(AppError::Internal)?;

    // fresh recovery codes (shown once)
    let codes = crate::auth::recovery::generate_recovery_codes();
    let hashes: Vec<String> = codes.iter()
        .map(|c| crate::auth::recovery::hash_recovery_code(c)).collect::<anyhow::Result<_>>()
        .map_err(AppError::Internal)?;
    state.mfa.store_recovery_codes(user_id, &hashes).await.map_err(AppError::Internal)?;

    let tokens = if from_mfa_token {
        let user = state.users.find_by_id(user_id).await.map_err(AppError::Internal)?
            .ok_or_else(|| AppError::Unauthorized("user not found".into()))?;
        Some(issue_token_pair(&state, &user, vec!["pwd".into(), "totp".into()]).await?)
    } else {
        None // self-enroll: caller already holds a valid access token
    };
    tracing::info!(user_id, "mfa confirmed");
    Ok(Json(MfaConfirmResponse { recovery_codes: codes, tokens }))
}
```

- [ ] **Step 5: Run to verify it passes** (`--test mfa`) → PASS.

- [ ] **Step 6: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets --locked -- -D warnings
git add -A
git commit -m "feat(auth): MFA setup + confirm (enrollment) endpoints"
```

---

## Task 10: `verify` endpoint (TOTP + recovery, attempt cap)

**Files:**
- Modify: `crates/domain-auth/src/ports/dto.rs`, `crates/domain-auth/src/ports/http.rs`
- Test: `crates/domain-auth/tests/mfa.rs`

**Interfaces:**
- Produces: `POST /auth/mfa/verify`; DTO `MfaVerifyRequest { code }` → `Json<AuthTokens>`. Lockout constant `MFA_MAX_ATTEMPTS = 5`, `MFA_LOCK_MINUTES = 15`.

- [ ] **Step 1: Add DTO** — `MfaVerifyRequest { pub code: String }`.

- [ ] **Step 2: Write the failing test** (mfa.rs) — enable a factor (reuse the enroll flow), then login → verify:
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn verify_completes_login_and_wrong_code_locks_out(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Required));
    register(&app, "a@b.c", "pw").await;
    let secret = enroll(&app, "a@b.c", "pw").await; // helper: login->setup->confirm, returns secret

    // subsequent login now requires verify
    let (_s, login) = post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    assert_eq!(login["purpose"], "verify");
    let pending = login["mfa_token"].as_str().unwrap().to_string();

    // wrong code fails
    let (bad, _) = post_bearer(&app, "/auth/mfa/verify", &pending, r#"{"code":"000000"}"#).await;
    assert_eq!(bad, 401);

    // correct code succeeds
    let code = current_totp_code(&secret);
    let (ok, tokens) = post_bearer(&app, "/auth/mfa/verify", &pending, &format!(r#"{{"code":"{code}"}}"#)).await;
    assert_eq!(ok, 200);
    assert!(tokens["access_token"].as_str().unwrap().len() > 10);
}
```

- [ ] **Step 3: Run → FAIL** (`--test mfa`).

- [ ] **Step 4: Implement** `mfa_verify` + register route `.route("/auth/mfa/verify", post(mfa_verify))`:
```rust
const MFA_MAX_ATTEMPTS: i32 = 5;

pub(crate) async fn mfa_verify(
    State(state): State<AuthState>,
    headers: http::HeaderMap,
    Json(body): Json<MfaVerifyRequest>,
) -> Result<Json<AuthTokens>, AppError> {
    let (user_id, _) = mfa_user_id(&state, &headers, &["mfa_pending"])?;
    let factor = state.mfa.confirmed_factor(user_id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::BadRequest("no confirmed factor".into()))?;
    let now = chrono::Utc::now();
    if factor.locked_until.map(|t| t > now).unwrap_or(false) {
        return Err(AppError::Unauthorized("too many attempts; try later".into()));
    }
    let secret = cipher(&state)?.decrypt(&factor.secret_encrypted).map_err(AppError::Internal)?;

    let (ok, amr_factor) = if state.mfa_verifier.verify(&secret, &body.code, now) {
        (true, "totp")
    } else if state.mfa.consume_recovery_code(user_id, &body.code).await.map_err(AppError::Internal)? {
        (true, "recovery")
    } else {
        (false, "")
    };

    if !ok {
        let next = factor.failed_attempts + 1;
        let lock = (next >= MFA_MAX_ATTEMPTS).then(|| now + chrono::Duration::minutes(15));
        state.mfa.record_failed_attempt(factor.id, lock).await.map_err(AppError::Internal)?;
        tracing::warn!(user_id, "mfa verify failed");
        return Err(AppError::Unauthorized("invalid code".into()));
    }

    state.mfa.reset_attempts(factor.id).await.map_err(AppError::Internal)?;
    let user = state.users.find_by_id(user_id).await.map_err(AppError::Internal)?
        .ok_or_else(|| AppError::Unauthorized("user not found".into()))?;
    tracing::info!(user_id, amr = amr_factor, "mfa verified");
    let tokens = issue_token_pair(&state, &user, vec!["pwd".into(), amr_factor.into()]).await?;
    Ok(Json(tokens))
}
```

- [ ] **Step 5: Run → PASS** (`--test mfa`). Add a second test for recovery-code single-use if time permits (verify with a recovery code, then assert reuse fails).

- [ ] **Step 6: fmt + clippy + commit**
```bash
cargo fmt --all && cargo clippy --all-targets --locked -- -D warnings
git add -A
git commit -m "feat(auth): MFA verify endpoint (TOTP + recovery, attempt lockout)"
```

---

## Task 11: recovery regen + self-disable + admin reset (audited) + OpenAPI + docs

**Files:**
- Modify: `crates/domain-auth/src/ports/http.rs`, `crates/app/src/openapi.rs`, `.env.example`, `Makefile`
- Test: `crates/domain-auth/tests/mfa.rs`

**Interfaces:**
- Produces: `POST /auth/mfa/recovery-codes`, `DELETE /auth/mfa`, `POST /admin/users/:id/mfa/reset` (emits `user.mfa_reset` outbox event).

- [ ] **Step 1: Write the failing tests** (mfa.rs) — admin reset clears the factor + emits an event; self-disable rejected under `required`:
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn admin_reset_clears_factor_and_emits_event(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Required));
    register(&app, "admin@x.y", "pw").await; // admin via admin_emails in state_with
    let _ = enroll(&app, "admin@x.y", "pw").await;
    // find the user's id + an admin access token
    let admin_token = admin_access_token(&app, "admin@x.y", "pw").await; // enroll->verify->token
    let uid: i64 = sqlx::query_scalar("select id from auth_user where email='admin@x.y'")
        .fetch_one(&pool).await.unwrap();
    let (s, _) = post_bearer(&app, &format!("/admin/users/{uid}/mfa/reset"), &admin_token, "{}").await;
    assert_eq!(s, 204);
    let cnt: i64 = sqlx::query_scalar("select count(*) from auth_mfa_factor where user_id=$1")
        .bind(uid).fetch_one(&pool).await.unwrap();
    assert_eq!(cnt, 0);
    let ev: i64 = sqlx::query_scalar("select count(*) from outbox_event where event_type='user.mfa_reset'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(ev, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn self_disable_rejected_when_required(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Required));
    register(&app, "a@b.c", "pw").await;
    let token = access_token_via_enroll(&app, "a@b.c", "pw").await;
    let (s, _) = delete_bearer(&app, "/auth/mfa", &token).await;
    assert_eq!(s, 409);
}
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** the three handlers + routes in `http.rs`:
```rust
// router():
    .route("/auth/mfa/recovery-codes", post(mfa_regen_recovery))
    .route("/auth/mfa", axum::routing::delete(mfa_self_disable))
    .route("/admin/users/:id/mfa/reset", post(admin_mfa_reset))
```
```rust
pub(crate) async fn mfa_regen_recovery(
    State(state): State<AuthState>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<String>>, AppError> {
    let uid = user_id_from_sub(&claims.sub)?;
    let codes = crate::auth::recovery::generate_recovery_codes();
    let hashes: Vec<String> = codes.iter().map(|c| crate::auth::recovery::hash_recovery_code(c))
        .collect::<anyhow::Result<_>>().map_err(AppError::Internal)?;
    state.mfa.store_recovery_codes(uid, &hashes).await.map_err(AppError::Internal)?;
    Ok(Json(codes))
}

pub(crate) async fn mfa_self_disable(
    State(state): State<AuthState>,
    Authenticated(claims): Authenticated,
) -> Result<StatusCode, AppError> {
    if state.mfa_config.policy == platform::config::MfaPolicy::Required {
        return Err(AppError::Conflict("cannot disable MFA under required policy".into()));
    }
    let uid = user_id_from_sub(&claims.sub)?;
    state.mfa.delete_factors(uid).await.map_err(AppError::Internal)?;
    state.mfa.delete_recovery_codes(uid).await.map_err(AppError::Internal)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn admin_mfa_reset(
    State(state): State<AuthState>,
    Authenticated(claims): Authenticated,
    CorrelationId(cid): CorrelationId,
    Path(target): Path<i64>,
) -> Result<StatusCode, AppError> {
    require_scope(&claims, "admin")?;
    let mut tx = state.pool.begin().await.map_err(|e| AppError::Internal(e.into()))?;
    sqlx::query("delete from auth_mfa_factor where user_id = $1").bind(target).execute(&mut *tx).await.map_err(|e| AppError::Internal(e.into()))?;
    sqlx::query("delete from auth_mfa_recovery_code where user_id = $1").bind(target).execute(&mut *tx).await.map_err(|e| AppError::Internal(e.into()))?;
    let admin_id = user_id_from_sub(&claims.sub)?;
    state.publisher.publish(&mut tx, platform::events::NewEvent {
        event_type: "user.mfa_reset".into(),
        aggregate_id: target.to_string(),
        payload: serde_json::json!({ "admin_user_id": admin_id, "target_user_id": target }),
        correlation_id: cid.clone(),
    }).await.map_err(AppError::Internal)?;
    tx.commit().await.map_err(|e| AppError::Internal(e.into()))?;
    tracing::warn!(admin_user_id = admin_id, target_user_id = target, "mfa reset by admin");
    Ok(StatusCode::NO_CONTENT)
}

fn user_id_from_sub(sub: &str) -> Result<i64, AppError> {
    sub.strip_prefix("user-").and_then(|s| s.parse().ok())
        .ok_or_else(|| AppError::Unauthorized("bad sub".into()))
}
```
(`use crate::ports::MfaRepository` and the DTO/imports as needed. The admin reset uses the caller's tx to publish so the delete + audit event commit atomically.)

- [ ] **Step 4: Register OpenAPI paths + schemas** in `crates/app/src/openapi.rs` — add the new handler paths (`login` response type changed to `LoginResponse`; `mfa_setup`/`mfa_confirm`/`mfa_verify`/`mfa_regen_recovery`/`mfa_self_disable`/`admin_mfa_reset`) and the new schemas (`LoginResponse`, `MfaSetupResponse`, `MfaConfirmRequest`, `MfaConfirmResponse`, `MfaVerifyRequest`) to the `#[derive(OpenApi)] paths(...)/components(schemas(...))`. Then run `make gen-api` and commit the regenerated `web/src/api/schema.d.ts` (the openapi-drift CI job enforces this).

- [ ] **Step 5: `.env.example` + `make gen-keys`** —
  - `.env.example`: under a new MFA block, `APP__AUTH__MFA_POLICY=off` (dev), and document `APP__AUTH__MFA_ENCRYPTION_KEY_FILE=secrets/mfa_key.b32` (+ a comment: prod uses `required` and must set the key).
  - `Makefile` `gen-keys`: also emit an MFA key: `head -c 20 /dev/urandom | base32 | tr -d '=' > secrets/mfa_key.b32` (20 random bytes → 32 base32 chars = the 32-byte-decoded... note: base32 of 20 bytes = 32 chars decoding back to 20 bytes; the key must be 32 BYTES, so generate 32 bytes: `head -c 32 /dev/urandom | base32 | tr -d '=' > secrets/mfa_key.b32`). Use 32 bytes.

- [ ] **Step 6: Run tests + full gate**

Run: `DATABASE_URL=... cargo test -p domain-auth -p app && cargo fmt --all --check && cargo clippy --all-targets --locked -- -D warnings`
Expected: PASS/clean. `make gen-api` shows no further drift.

- [ ] **Step 7: Commit**
```bash
git add -A
git commit -m "feat(auth): recovery regen, self-disable, audited admin reset + OpenAPI/docs"
```

---

## Self-Review Notes (coverage vs. spec)

- **§3 schema (0007, generic `type`, confirmed_at, failed_attempts/locked_until, hashed recovery):** Task 6. ✅
- **§4 config (MfaPolicy default required, key fail-fast, chacha20poly1305/totp-rs/bcrypt):** Tasks 1 (+ startup fail-fast wired in Task 7's `build_resources` `?`), 3, 4, 5. ✅ (Key encoded as **base32** for consistency with the TOTP secret + the already-added `base32` dep — a deliberate refinement of the spec's "base64"; noted in Task 1.)
- **§5 login state machine + tagged LoginResponse + short-lived mfa tokens + amr:** Tasks 2, 8. ✅
- **§6 ports (FactorVerifier/MfaRepository/MfaCipher), AuthState, JwtIssuer additions, 8 sites:** Tasks 2, 3, 4, 6, 7. ✅
- **§7 endpoints (setup/confirm/verify/recovery/self-disable/admin-reset), amr, audit event:** Tasks 9, 10, 11. ✅
- **§8 observability/audit (logs + user.mfa_reset outbox event):** Tasks 9–11 (structured logs) + Task 11 (event). ✅
- **§9 testing:** unit (Tasks 3–5), integration (Tasks 6, 8, 9, 10, 11). ✅
- **Refinement flagged:** MFA key uses base32 encoding (not base64) to reuse the `base32` dep and match TOTP secret encoding — surfaced to the human at plan handoff.
- **Blast-radius note:** Tasks 7 & 8 touch the 7 test `AuthState` builders and any e2e test that parses the login body (now `LoginResponse`-shaped) — the plan calls these out explicitly so the ripple isn't a surprise mid-execution.
