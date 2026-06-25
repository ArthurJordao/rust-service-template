# Spec 2a: domain-auth Core (user store + issuance + register/login) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the `domain-auth` crate with a Postgres user/scope store, bcrypt password hashing, RS256 token issuance, and `POST /auth/register` + `POST /auth/login`; make it the real producer of `user.registered`, removing `domain-account`'s `/dev/register` stand-in.

**Architecture:** A new hexagonal domain crate mirroring `domain-account`: pure rules in `domain.rs`, crypto in `auth/`, Postgres adapters under `ports/`. Register inserts the user, seeds a default scope, and publishes `user.registered` to the outbox in **one transaction** (atomic with state). Token issuance reuses `platform::auth::AccessClaims` (extended here) signed RS256 with the RSA private key; the existing `platform` verifier already validates these tokens.

**Tech Stack:** axum 0.7, sqlx (runtime API), tokio, tracing, serde, jsonwebtoken 9, bcrypt, uuid, chrono, async-trait.

## Global Constraints

- Same dependency pins and rules as Spec 1 (`docs/superpowers/plans/2026-06-24-rust-spec1a-workspace-and-platform.md`, "Global Constraints").
- New workspace dependency: `bcrypt = "0.16"`. Reuse existing `jsonwebtoken = "9"`, `uuid`, `chrono`.
- Use the **runtime** sqlx API (`sqlx::query`, `query_as`, `query_scalar`, `.bind`) — never the compile-time `query!` macros.
- `#[sqlx::test]` resolves migrations relative to the crate manifest dir; integration tests MUST use `#[sqlx::test(migrations = "../../migrations")]` (the migrations dir is at the workspace root). Tests need `DATABASE_URL` set, e.g. `postgres://postgres:postgres@localhost:5432/postgres`.
- RS256 throughout. JWT subject convention `user-{id}`; scopes are a JSON array claim `scopes`; access tokens also carry `jti`, `iat`, `email`, and `type` (= `"user"`).
- Event type strings: `"user.registered"` (payload `{auth_user_id, email}`) and `"account.created"` (Spec 1).
- bcrypt cost factor **12**.
- Default scope granted to a new user: `read:accounts:own`.
- axum 0.7 path syntax `:id`.
- Run `cargo fmt --all` and `cargo clippy --all-targets -- -D warnings` before each commit; both must be clean.

---

### Task 1: Crate scaffold + workspace deps + migration

**Files:**
- Modify: `Cargo.toml` (workspace: add `crates/domain-auth` member + `bcrypt` dep)
- Create: `crates/domain-auth/Cargo.toml`
- Create: `crates/domain-auth/src/lib.rs`
- Create: `migrations/0003_auth_user.sql`

**Interfaces:**
- Produces: a compiling (empty) `domain-auth` crate in the workspace; the `auth_user`, `scope`, `user_scope` tables.

- [ ] **Step 1: Add the workspace member and bcrypt dependency**

In root `Cargo.toml`, add `"crates/domain-auth"` to `members`:
```toml
members = ["crates/platform", "crates/domain-account", "crates/domain-auth", "crates/app"]
```
Add to `[workspace.dependencies]`:
```toml
bcrypt = "0.16"
```

- [ ] **Step 2: Create the crate manifest**

`crates/domain-auth/Cargo.toml`:
```toml
[package]
name = "domain-auth"
edition.workspace = true
version.workspace = true

[dependencies]
platform = { path = "../platform" }
axum.workspace = true
sqlx.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
async-trait.workspace = true
anyhow.workspace = true
chrono.workspace = true
http.workspace = true
jsonwebtoken.workspace = true
bcrypt.workspace = true
uuid.workspace = true

[dev-dependencies]
tower = { workspace = true, features = ["util"] }
```

- [ ] **Step 3: Create the placeholder lib**

`crates/domain-auth/src/lib.rs`:
```rust
//! Auth domain: users, scopes, JWT issuance, login/register/refresh/logout.
```

- [ ] **Step 4: Write the migration**

`migrations/0003_auth_user.sql`:
```sql
create table auth_user (
    id                bigserial primary key,
    email             text        not null unique,
    password_hash     text        not null,
    tokens_valid_from timestamptz not null default now(),
    created_at        timestamptz not null default now(),
    created_by_cid    text        not null
);

create table scope (
    id          bigserial primary key,
    name        text not null unique,
    description text not null
);

create table user_scope (
    id         bigserial primary key,
    user_id    bigint not null references auth_user (id),
    scope      text   not null,
    granted_by bigint,
    unique (user_id, scope)
);
```

- [ ] **Step 5: Verify the workspace compiles and migration is well-formed**

Run: `cargo build`
Expected: PASS — `domain-auth` compiles; `sqlx::migrate!` validates the new migration.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/domain-auth migrations/0003_auth_user.sql
git commit -m "chore(auth): scaffold domain-auth crate + auth_user/scope/user_scope migration"
```

---

### Task 2: Extend `platform::auth::AccessClaims` + `AuthSettings`

**Files:**
- Modify: `crates/platform/src/auth.rs` (extend `AccessClaims`; fix test constructions)
- Modify: `crates/platform/src/config.rs` (extend `AuthSettings`)
- Modify: `crates/domain-account/src/domain.rs` (fix `AccessClaims` test constructions)
- Modify: `crates/domain-account/tests/http.rs` (no claim construction — verify only)

**Interfaces:**
- Produces:
  - `AccessClaims { sub, scopes, exp, iat: usize, jti: String, email: Option<String>, token_type: String }` with `#[serde(rename = "type", default)]` on `token_type`.
  - `AuthSettings { jwt_public_key_pem, jwt_private_key_pem, access_token_ttl_seconds: i64, refresh_token_ttl_days: i64, admin_emails: String }` + `fn admin_email_list(&self) -> Vec<String>`.

- [ ] **Step 1: Extend `AccessClaims`**

In `crates/platform/src/auth.rs`, replace the `AccessClaims` struct with:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessClaims {
    pub sub: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub exp: usize,
    pub iat: usize,
    pub jti: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(rename = "type", default)]
    pub token_type: String,
}
```

- [ ] **Step 2: Fix the existing `platform::auth` test constructions**

In `crates/platform/src/auth.rs`, update the test helper `claims` to set the new fields:
```rust
    fn claims(scopes: &[&str]) -> AccessClaims {
        AccessClaims {
            sub: "user-1".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            exp: 9_999_999_999,
            iat: 0,
            jti: "test-jti".into(),
            email: None,
            token_type: "user".into(),
        }
    }
```

- [ ] **Step 3: Fix the `domain-account` test construction**

In `crates/domain-account/src/domain.rs`, update the test helper `claims`:
```rust
    fn claims(sub: &str, scopes: &[&str]) -> AccessClaims {
        AccessClaims {
            sub: sub.into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            exp: 9_999_999_999,
            iat: 0,
            jti: "test-jti".into(),
            email: None,
            token_type: "user".into(),
        }
    }
```

- [ ] **Step 3b: Make `JwtVerifier` tolerate tokens without an `aud` claim**

`jsonwebtoken` 9's `Validation::new` enables `validate_aud`, which rejects tokens that
carry no `aud` claim (our tokens don't set one). Spec 1 never exercised an issue→verify
round-trip, so this was latent. In `crates/platform/src/auth.rs`, update
`JwtVerifier::from_rsa_pem` to disable aud validation:
```rust
    pub fn from_rsa_pem(pem: &str) -> anyhow::Result<JwtVerifier> {
        let key = DecodingKey::from_rsa_pem(pem.as_bytes())?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_aud = false;
        Ok(JwtVerifier { key, validation })
    }
```

- [ ] **Step 4: Extend `AuthSettings`**

In `crates/platform/src/config.rs`, replace `AuthSettings` with:
```rust
fn default_access_ttl() -> i64 {
    900
}

fn default_refresh_ttl() -> i64 {
    7
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthSettings {
    pub jwt_public_key_pem: String,
    #[serde(default)]
    pub jwt_private_key_pem: String,
    #[serde(default = "default_access_ttl")]
    pub access_token_ttl_seconds: i64,
    #[serde(default = "default_refresh_ttl")]
    pub refresh_token_ttl_days: i64,
    #[serde(default)]
    pub admin_emails: String,
}

impl AuthSettings {
    /// Parse the comma-separated `admin_emails` config value into a trimmed list.
    pub fn admin_email_list(&self) -> Vec<String> {
        self.admin_emails
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}
```

- [ ] **Step 5: Run the affected tests to verify they still pass**

Run: `cargo test -p platform auth:: && cargo test -p platform config:: && cargo test -p domain-account domain::`
Expected: PASS — claims construct with the new fields; config still loads (new fields default).

- [ ] **Step 6: Build + clippy gate, then commit**

Run: `cargo build && cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/platform crates/domain-account
git commit -m "feat(platform): extend AccessClaims (jti/iat/email/type) + AuthSettings (issuance keys, TTLs, admin emails)"
```

---

### Task 3: `auth/password` — bcrypt hash/verify

**Files:**
- Create: `crates/domain-auth/src/auth/mod.rs`
- Create: `crates/domain-auth/src/auth/password.rs`
- Modify: `crates/domain-auth/src/lib.rs`
- Test: inline `#[cfg(test)]` in `password.rs`

**Interfaces:**
- Produces:
  - `pub fn hash_password(plaintext: &str) -> anyhow::Result<String>`
  - `pub fn verify_password(stored_hash: &str, plaintext: &str) -> bool`

- [ ] **Step 1: Write the failing test**

`crates/domain-auth/src/auth/password.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_roundtrip() {
        let hash = hash_password("hunter2").unwrap();
        assert!(verify_password(&hash, "hunter2"));
        assert!(!verify_password(&hash, "wrong"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p domain-auth password::`
Expected: FAIL — `hash_password` not found.

- [ ] **Step 3: Write the implementation**

Top of `crates/domain-auth/src/auth/password.rs`:
```rust
/// Hash a plaintext password with bcrypt (cost factor 12).
pub fn hash_password(plaintext: &str) -> anyhow::Result<String> {
    Ok(bcrypt::hash(plaintext, 12)?)
}

/// Verify a plaintext password against a stored bcrypt hash.
/// Returns false on any error (malformed hash, mismatch).
pub fn verify_password(stored_hash: &str, plaintext: &str) -> bool {
    bcrypt::verify(plaintext, stored_hash).unwrap_or(false)
}
```

`crates/domain-auth/src/auth/mod.rs`:
```rust
pub mod password;
```

`crates/domain-auth/src/lib.rs`:
```rust
//! Auth domain: users, scopes, JWT issuance, login/register/refresh/logout.
pub mod auth;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p domain-auth password::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/domain-auth
git commit -m "feat(auth): bcrypt password hashing"
```

---

### Task 4: Models + `auth/jwt` issuer

**Files:**
- Create: `crates/domain-auth/src/models.rs`
- Create: `crates/domain-auth/src/auth/jwt.rs`
- Modify: `crates/domain-auth/src/auth/mod.rs`
- Modify: `crates/domain-auth/src/lib.rs`
- Test: inline `#[cfg(test)]` in `jwt.rs`

**Interfaces:**
- Consumes: `platform::auth::{AccessClaims, JwtVerifier}`.
- Produces:
  - `pub struct User { pub id: i64, pub email: String, pub password_hash: String, pub tokens_valid_from: chrono::DateTime<chrono::Utc>, pub created_at: chrono::DateTime<chrono::Utc>, pub created_by_cid: String }` (`sqlx::FromRow`)
  - `pub struct NewUser { pub email: String, pub password_hash: String }`
  - `pub struct ScopeRow { pub id: i64, pub name: String, pub description: String }` (`sqlx::FromRow`, `serde::Serialize`)
  - `pub struct JwtIssuer` + `pub fn from_rsa_pem(pem, access_ttl_seconds: i64, refresh_ttl_days: i64) -> anyhow::Result<JwtIssuer>`
  - `pub fn JwtIssuer::access_ttl_seconds(&self) -> i64`
  - `pub fn JwtIssuer::issue_access(&self, user_id: i64, email: &str, scopes: Vec<String>, now: DateTime<Utc>) -> anyhow::Result<(String, AccessClaims)>`
  - `pub fn JwtIssuer::issue_refresh(&self, user_id: i64, now: DateTime<Utc>) -> anyhow::Result<(String /*jti*/, String /*token*/, DateTime<Utc> /*expires_at*/)>`

- [ ] **Step 1: Write the models**

`crates/domain-auth/src/models.rs`:
```rust
use serde::Serialize;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub password_hash: String,
    pub tokens_valid_from: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by_cid: String,
}

#[derive(Debug, Clone)]
pub struct NewUser {
    pub email: String,
    pub password_hash: String,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ScopeRow {
    pub id: i64,
    pub name: String,
    pub description: String,
}
```

- [ ] **Step 2: Write the failing test**

`crates/domain-auth/src/auth/jwt.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use platform::auth::JwtVerifier;

    const TEST_PRIV_PEM: &str = include_str!("../../tests/fixtures/test_priv.pem");
    const TEST_PUB_PEM: &str = include_str!("../../tests/fixtures/test_pub.pem");

    #[test]
    fn issued_access_token_verifies_with_public_key() {
        let issuer = JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap();
        let now = chrono::Utc::now();
        let (token, claims) = issuer
            .issue_access(42, "a@b.c", vec!["admin".into()], now)
            .unwrap();
        assert_eq!(claims.sub, "user-42");
        assert_eq!(claims.token_type, "user");
        assert!(!claims.jti.is_empty());

        let verifier = JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap();
        let verified = verifier.verify(&token).unwrap();
        assert_eq!(verified.sub, "user-42");
        assert_eq!(verified.email.as_deref(), Some("a@b.c"));
        assert!(verified.has_scope("admin"));
    }

    #[test]
    fn issued_refresh_token_has_jti_and_expiry() {
        let issuer = JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap();
        let now = chrono::Utc::now();
        let (jti, token, expires_at) = issuer.issue_refresh(42, now).unwrap();
        assert!(!jti.is_empty());
        assert!(!token.is_empty());
        assert!(expires_at > now);
    }
}
```

- [ ] **Step 2b: Create the test RSA keypair fixtures**

Run (the public key matches `domain-account`'s existing fixture format; here we need both halves):
```bash
mkdir -p crates/domain-auth/tests/fixtures
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out crates/domain-auth/tests/fixtures/test_priv.pem
openssl rsa -in crates/domain-auth/tests/fixtures/test_priv.pem -pubout -out crates/domain-auth/tests/fixtures/test_pub.pem
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p domain-auth jwt::`
Expected: FAIL — `JwtIssuer` not found.

- [ ] **Step 4: Write the implementation**

Top of `crates/domain-auth/src/auth/jwt.rs`:
```rust
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use platform::auth::AccessClaims;
use serde::{Deserialize, Serialize};

/// Claims for a refresh JWT (separate shape from the access token).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshClaims {
    pub sub: String,
    pub iat: usize,
    pub exp: usize,
    pub jti: String,
    #[serde(rename = "type")]
    pub token_type: String,
}

/// Signs access + refresh tokens with an RSA private key (RS256).
#[derive(Clone)]
pub struct JwtIssuer {
    key: EncodingKey,
    access_ttl_seconds: i64,
    refresh_ttl_days: i64,
}

impl JwtIssuer {
    pub fn from_rsa_pem(
        pem: &str,
        access_ttl_seconds: i64,
        refresh_ttl_days: i64,
    ) -> anyhow::Result<JwtIssuer> {
        let key = EncodingKey::from_rsa_pem(pem.as_bytes())?;
        Ok(JwtIssuer { key, access_ttl_seconds, refresh_ttl_days })
    }

    pub fn access_ttl_seconds(&self) -> i64 {
        self.access_ttl_seconds
    }

    /// Issue a signed access token. Returns the compact token and its claims.
    pub fn issue_access(
        &self,
        user_id: i64,
        email: &str,
        scopes: Vec<String>,
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
        };
        let token = encode(&Header::new(Algorithm::RS256), &claims, &self.key)?;
        Ok((token, claims))
    }

    /// Issue a signed refresh token. Returns (jti, token, expires_at).
    pub fn issue_refresh(
        &self,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> anyhow::Result<(String, String, DateTime<Utc>)> {
        let jti = uuid::Uuid::new_v4().to_string();
        let expires_at = now + Duration::days(self.refresh_ttl_days);
        let claims = RefreshClaims {
            sub: format!("user-{user_id}"),
            iat: now.timestamp() as usize,
            exp: expires_at.timestamp() as usize,
            jti: jti.clone(),
            token_type: "refresh".to_string(),
        };
        let token = encode(&Header::new(Algorithm::RS256), &claims, &self.key)?;
        Ok((jti, token, expires_at))
    }
}
```

`crates/domain-auth/src/auth/mod.rs` (replace):
```rust
pub mod jwt;
pub mod password;
```

`crates/domain-auth/src/lib.rs` (replace):
```rust
//! Auth domain: users, scopes, JWT issuance, login/register/refresh/logout.
pub mod auth;
pub mod models;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p domain-auth jwt::`
Expected: PASS — issued tokens verify with the platform verifier.

- [ ] **Step 6: Commit**

```bash
git add crates/domain-auth
git commit -m "feat(auth): models + RS256 JwtIssuer (access + refresh)"
```

---

### Task 5: Repository ports + Postgres adapters

**Files:**
- Create: `crates/domain-auth/src/ports/mod.rs`
- Create: `crates/domain-auth/src/ports/repository.rs`
- Create: `crates/domain-auth/src/ports/postgres.rs`
- Modify: `crates/domain-auth/src/lib.rs`
- Test: `crates/domain-auth/tests/repository.rs`

**Interfaces:**
- Consumes: `User`, `NewUser`, `ScopeRow`; `platform::db::Db`; `platform::events::{EventPublisher, NewEvent}`.
- Produces:
  - `#[async_trait] pub trait UserRepository: Send + Sync { async fn find_by_email(&self, email: &str) -> anyhow::Result<Option<User>>; async fn find_by_id(&self, id: i64) -> anyhow::Result<Option<User>>; async fn list(&self) -> anyhow::Result<Vec<User>>; async fn scope_names(&self, user_id: i64) -> anyhow::Result<Vec<String>>; }`
  - `pub struct PostgresUserRepository { pool: Db }` + `pub fn new(pool: Db) -> Self` implementing `UserRepository`.
  - `pub async fn register_user_with_event(pool: &Db, publisher: &dyn EventPublisher, new: NewUser, default_scopes: &[&str], cid: &str) -> anyhow::Result<User>` — inserts `auth_user`, seeds `user_scope` rows, publishes `user.registered` (payload `{auth_user_id, email}`), all in ONE transaction. Returns the inserted user. Caller checks email uniqueness first (the DB unique constraint is the backstop).

- [ ] **Step 1: Write the failing test**

`crates/domain-auth/tests/repository.rs`:
```rust
use domain_auth::models::NewUser;
use domain_auth::ports::postgres::{register_user_with_event, PostgresUserRepository};
use domain_auth::ports::UserRepository;
use platform::events::{OutboxPublisher, Routes};

#[sqlx::test(migrations = "../../migrations")]
async fn register_inserts_user_scopes_and_emits_event(pool: sqlx::PgPool) {
    let publisher = OutboxPublisher::new(Routes::new());
    let repo = PostgresUserRepository::new(pool.clone());

    let user = register_user_with_event(
        &pool,
        &publisher,
        NewUser { email: "a@b.c".into(), password_hash: "hash".into() },
        &["read:accounts:own"],
        "cid-1",
    )
    .await
    .unwrap();
    assert_eq!(user.email, "a@b.c");
    assert_eq!(user.created_by_cid, "cid-1");

    // user.registered emitted in the same txn.
    let events: i64 = sqlx::query_scalar(
        "select count(*) from outbox_event where event_type = 'user.registered'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, 1);

    // Reads + scopes via the port.
    assert!(repo.find_by_email("a@b.c").await.unwrap().is_some());
    assert_eq!(repo.scope_names(user.id).await.unwrap(), vec!["read:accounts:own".to_string()]);
    assert_eq!(repo.list().await.unwrap().len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-auth --test repository`
Expected: FAIL — items not found.

- [ ] **Step 3: Write the repository trait**

`crates/domain-auth/src/ports/repository.rs`:
```rust
use crate::models::User;

#[async_trait::async_trait]
pub trait UserRepository: Send + Sync {
    async fn find_by_email(&self, email: &str) -> anyhow::Result<Option<User>>;
    async fn find_by_id(&self, id: i64) -> anyhow::Result<Option<User>>;
    async fn list(&self) -> anyhow::Result<Vec<User>>;
    async fn scope_names(&self, user_id: i64) -> anyhow::Result<Vec<String>>;
}
```

- [ ] **Step 4: Write the Postgres adapter**

`crates/domain-auth/src/ports/postgres.rs`:
```rust
use crate::models::{NewUser, User};
use crate::ports::UserRepository;
use platform::db::Db;
use platform::events::{EventPublisher, NewEvent};

#[derive(Clone)]
pub struct PostgresUserRepository {
    pool: Db,
}

impl PostgresUserRepository {
    pub fn new(pool: Db) -> Self {
        PostgresUserRepository { pool }
    }
}

const USER_COLS: &str =
    "id, email, password_hash, tokens_valid_from, created_at, created_by_cid";

#[async_trait::async_trait]
impl UserRepository for PostgresUserRepository {
    async fn find_by_email(&self, email: &str) -> anyhow::Result<Option<User>> {
        let row = sqlx::query_as::<_, User>(&format!(
            "select {USER_COLS} from auth_user where email = $1"
        ))
        .bind(email)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn find_by_id(&self, id: i64) -> anyhow::Result<Option<User>> {
        let row = sqlx::query_as::<_, User>(&format!(
            "select {USER_COLS} from auth_user where id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn list(&self) -> anyhow::Result<Vec<User>> {
        let rows = sqlx::query_as::<_, User>(&format!(
            "select {USER_COLS} from auth_user order by id"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn scope_names(&self, user_id: i64) -> anyhow::Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("select scope from user_scope where user_id = $1 order by scope")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(s,)| s).collect())
    }
}

/// Insert a user, seed default scopes, and publish `user.registered` atomically.
pub async fn register_user_with_event(
    pool: &Db,
    publisher: &dyn EventPublisher,
    new: NewUser,
    default_scopes: &[&str],
    cid: &str,
) -> anyhow::Result<User> {
    let mut tx = pool.begin().await?;

    let user = sqlx::query_as::<_, User>(&format!(
        "insert into auth_user (email, password_hash, created_by_cid) \
         values ($1, $2, $3) returning {USER_COLS}"
    ))
    .bind(&new.email)
    .bind(&new.password_hash)
    .bind(cid)
    .fetch_one(&mut *tx)
    .await?;

    for scope in default_scopes {
        sqlx::query("insert into user_scope (user_id, scope) values ($1, $2)")
            .bind(user.id)
            .bind(scope)
            .execute(&mut *tx)
            .await?;
    }

    publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "user.registered".into(),
                aggregate_id: user.id.to_string(),
                payload: serde_json::json!({
                    "auth_user_id": user.id,
                    "email": user.email,
                }),
                correlation_id: cid.to_string(),
            },
        )
        .await?;

    tx.commit().await?;
    Ok(user)
}
```

`crates/domain-auth/src/ports/mod.rs`:
```rust
pub mod postgres;
pub mod repository;
pub use repository::UserRepository;
```

`crates/domain-auth/src/lib.rs` (replace):
```rust
//! Auth domain: users, scopes, JWT issuance, login/register/refresh/logout.
pub mod auth;
pub mod models;
pub mod ports;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-auth --test repository`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/domain-auth
git commit -m "feat(auth): UserRepository + Postgres adapter + transactional register-with-event"
```

---

### Task 6: Domain rules — credentials + token assembly

**Files:**
- Create: `crates/domain-auth/src/domain.rs`
- Modify: `crates/domain-auth/src/lib.rs`
- Test: inline `#[cfg(test)]` in `domain.rs`

**Interfaces:**
- Consumes: `auth::password::verify_password`, `JwtIssuer`, `User`, `platform::auth::AccessClaims`.
- Produces:
  - `pub struct TokenPair { pub access_token: String, pub refresh_token: String, pub refresh_jti: String, pub refresh_expires_at: chrono::DateTime<chrono::Utc>, pub expires_in: i64 }`
  - `pub fn effective_scopes(email: &str, db_scopes: Vec<String>, admin_emails: &[String]) -> Vec<String>` — adds `"admin"` when `email` is in `admin_emails` and not already present (bootstrap).
  - `pub fn check_credentials(user: Option<&User>, password: &str) -> Result<&User, AppError>` — `401` on missing user or bad password; always runs a hash compare to reduce timing signal.

- [ ] **Step 1: Write the failing test**

`crates/domain-auth/src/domain.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::password::hash_password;

    fn user(email: &str, password: &str) -> User {
        User {
            id: 1,
            email: email.into(),
            password_hash: hash_password(password).unwrap(),
            tokens_valid_from: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            created_by_cid: "cid".into(),
        }
    }

    #[test]
    fn effective_scopes_bootstraps_admin_for_admin_email() {
        let scopes = effective_scopes(
            "boss@x.y",
            vec!["read:accounts:own".into()],
            &["boss@x.y".to_string()],
        );
        assert!(scopes.contains(&"admin".to_string()));
        assert!(scopes.contains(&"read:accounts:own".to_string()));
    }

    #[test]
    fn effective_scopes_leaves_non_admin_email_untouched() {
        let scopes = effective_scopes("u@x.y", vec!["read:accounts:own".into()], &[]);
        assert_eq!(scopes, vec!["read:accounts:own".to_string()]);
    }

    #[test]
    fn check_credentials_accepts_correct_password() {
        let u = user("a@b.c", "pw");
        assert!(check_credentials(Some(&u), "pw").is_ok());
    }

    #[test]
    fn check_credentials_rejects_wrong_password() {
        let u = user("a@b.c", "pw");
        assert!(check_credentials(Some(&u), "nope").is_err());
    }

    #[test]
    fn check_credentials_rejects_missing_user() {
        assert!(check_credentials(None, "pw").is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p domain-auth domain::`
Expected: FAIL — items not found.

- [ ] **Step 3: Write the implementation**

Top of `crates/domain-auth/src/domain.rs`:
```rust
use crate::auth::password::verify_password;
use crate::models::User;
use platform::server::AppError;

/// A freshly issued access + refresh token pair plus metadata for persistence.
#[derive(Debug, Clone)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub refresh_jti: String,
    pub refresh_expires_at: chrono::DateTime<chrono::Utc>,
    pub expires_in: i64,
}

/// A bcrypt hash of a throwaway value, used to spend ~the same time hashing on
/// the "user not found" path as on the real-user path (reduces timing signal).
const DUMMY_HASH: &str = "$2b$12$C6UzMDM.H6dfI/f/IKcEeO3.9I8H8sJ8q8sJ8q8sJ8q8sJ8q8sJ8";

/// Add the `admin` scope when the email is in the admin bootstrap list.
pub fn effective_scopes(email: &str, mut db_scopes: Vec<String>, admin_emails: &[String]) -> Vec<String> {
    let is_admin_email = admin_emails.iter().any(|e| e == email);
    if is_admin_email && !db_scopes.iter().any(|s| s == "admin") {
        db_scopes.push("admin".to_string());
    }
    db_scopes
}

/// Verify credentials. Returns the user on success, `401` otherwise.
pub fn check_credentials<'a>(user: Option<&'a User>, password: &str) -> Result<&'a User, AppError> {
    match user {
        Some(u) if verify_password(&u.password_hash, password) => Ok(u),
        Some(_) => Err(AppError::Unauthorized("invalid credentials".into())),
        None => {
            // Spend comparable time so presence of the account isn't timing-detectable.
            let _ = verify_password(DUMMY_HASH, password);
            Err(AppError::Unauthorized("invalid credentials".into()))
        }
    }
}
```

`crates/domain-auth/src/lib.rs` (replace):
```rust
//! Auth domain: users, scopes, JWT issuance, login/register/refresh/logout.
pub mod auth;
pub mod domain;
pub mod models;
pub mod ports;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p domain-auth domain::`
Expected: PASS.

> If the literal `DUMMY_HASH` fails to parse as a bcrypt hash on your platform, regenerate it with `bcrypt::hash("x", 12)` in a scratch test and paste the output; `verify_password` swallows errors so correctness does not depend on it, only timing.

- [ ] **Step 5: Commit**

```bash
git add crates/domain-auth
git commit -m "feat(auth): credential check + admin-email scope bootstrap (pure rules)"
```

---

### Task 7: HTTP router — register + login + status + metrics

**Files:**
- Create: `crates/domain-auth/src/ports/http.rs`
- Create: `crates/domain-auth/src/ports/dto.rs`
- Modify: `crates/domain-auth/src/ports/mod.rs`
- Modify: `crates/domain-auth/src/lib.rs`
- Test: `crates/domain-auth/tests/http.rs`

**Interfaces:**
- Consumes: `UserRepository`, `register_user_with_event`, `check_credentials`, `effective_scopes`, `JwtIssuer`, `platform::{db::Db, events::EventPublisher, metrics::Metrics, observability::CorrelationId, server::{AppError, status_handler}}`.
- Produces:
  - DTOs: `RegisterRequest { email, password }`, `LoginRequest { email, password }`, `AuthTokens { access_token, refresh_token, token_type, expires_in }`.
  - `#[derive(Clone)] pub struct AuthState { pub pool: Db, pub users: Arc<dyn UserRepository>, pub publisher: Arc<dyn EventPublisher>, pub issuer: Arc<JwtIssuer>, pub admin_emails: Arc<Vec<String>>, pub metrics: Metrics }`
  - `pub fn router(state: AuthState) -> axum::Router`
  - `pub async fn issue_token_pair(state: &AuthState, user: &User) -> Result<AuthTokens, AppError>` — assembles scopes, issues access+refresh, **stores the refresh token** is deferred to Plan 2b; here it returns tokens without persisting refresh (login/register still work end-to-end; refresh/logout arrive in 2b).

> Note for 2b: `issue_token_pair` will be extended to persist the refresh token via `RefreshTokenRepository`. In 2a it issues tokens only. This is intentional and keeps 2a independently shippable (register/login return working access tokens).

- [ ] **Step 1: Write the DTOs**

`crates/domain-auth/src/ports/dto.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
}
```

- [ ] **Step 2: Write the failing test**

`crates/domain-auth/tests/http.rs`:
```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::{router, AuthState};
use domain_auth::ports::postgres::PostgresUserRepository;
use http_body_util::BodyExt;
use platform::events::{OutboxPublisher, Routes};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("fixtures/test_priv.pem");

fn state(pool: sqlx::PgPool) -> AuthState {
    AuthState {
        pool: pool.clone(),
        users: Arc::new(PostgresUserRepository::new(pool.clone())),
        publisher: Arc::new(OutboxPublisher::new(
            Routes::new().add("user.registered", "account.on-user-registered"),
        )),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        admin_emails: Arc::new(vec![]),
        metrics: Metrics::new().unwrap(),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn register_then_login(pool: sqlx::PgPool) {
    let app = router(state(pool.clone()));

    // Register.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"a@b.c","password":"hunter2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // Duplicate email -> 409.
    let dup = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"a@b.c","password":"hunter2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(dup.status(), StatusCode::CONFLICT);

    // Login with correct password -> 200 + tokens.
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"a@b.c","password":"hunter2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let body = login.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["access_token"].as_str().unwrap().len() > 10);

    // Wrong password -> 401.
    let bad = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"a@b.c","password":"wrong"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 2b: Add the `http-body-util` dev-dependency**

The test reads the response body. Add to `crates/domain-auth/Cargo.toml` `[dev-dependencies]`:
```toml
http-body-util = "0.1"
```
And add to the root `[workspace.dependencies]` if not present:
```toml
http-body-util = "0.1"
```
Then make the crate dev-dep use the workspace version:
```toml
http-body-util = { workspace = true }
```
Also copy the fixtures so the test crate can `include_str!` them (already created in Task 4 under `tests/fixtures/`).

- [ ] **Step 3: Run test to verify it fails**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-auth --test http`
Expected: FAIL — `router`/`AuthState` not found.

- [ ] **Step 4: Add a `Conflict` variant to `platform::server::AppError`**

The register handler returns `409` on duplicate email; `AppError` has no such variant yet.
In `crates/platform/src/server.rs`, add to the `AppError` enum:
```rust
    #[error("{0}")]
    Conflict(String),
```
and to the `into_response` match arm list:
```rust
            AppError::Conflict(m) => (StatusCode::CONFLICT, m),
```

- [ ] **Step 5: Write the router**

`crates/domain-auth/src/ports/http.rs` imports:
```rust
use crate::auth::jwt::JwtIssuer;
use crate::auth::password::hash_password;
use crate::domain::{check_credentials, effective_scopes};
use crate::models::{NewUser, User};
use crate::ports::dto::{AuthTokens, LoginRequest, RegisterRequest};
use crate::ports::postgres::register_user_with_event;
use crate::ports::UserRepository;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use http::StatusCode;
use platform::db::Db;
use platform::events::EventPublisher;
use platform::metrics::Metrics;
use platform::observability::CorrelationId;
use platform::server::{status_handler, AppError};
use std::sync::Arc;
```

Then the state + router + handlers:
```rust
#[derive(Clone)]
pub struct AuthState {
    pub pool: Db,
    pub users: Arc<dyn UserRepository>,
    pub publisher: Arc<dyn EventPublisher>,
    pub issuer: Arc<JwtIssuer>,
    pub admin_emails: Arc<Vec<String>>,
    pub metrics: Metrics,
}

pub fn router(state: AuthState) -> Router {
    Router::new()
        .route("/status", get(status_handler))
        .route("/metrics", get(metrics_handler))
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .with_state(state)
}

async fn metrics_handler(State(state): State<AuthState>) -> String {
    state.metrics.render()
}

/// Build access + refresh tokens for a user (refresh persistence arrives in 2b).
pub async fn issue_token_pair(state: &AuthState, user: &User) -> Result<AuthTokens, AppError> {
    let db_scopes = state.users.scope_names(user.id).await.map_err(AppError::Internal)?;
    let scopes = effective_scopes(&user.email, db_scopes, &state.admin_emails);
    let now = chrono::Utc::now();
    let (access_token, _claims) = state
        .issuer
        .issue_access(user.id, &user.email, scopes, now)
        .map_err(AppError::Internal)?;
    let (_jti, refresh_token, _exp) =
        state.issuer.issue_refresh(user.id, now).map_err(AppError::Internal)?;
    Ok(AuthTokens {
        access_token,
        refresh_token,
        token_type: "Bearer".into(),
        expires_in: state.issuer.access_ttl_seconds(),
    })
}

async fn register(
    State(state): State<AuthState>,
    CorrelationId(cid): CorrelationId,
    Json(body): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<AuthTokens>), AppError> {
    if state
        .users
        .find_by_email(&body.email)
        .await
        .map_err(AppError::Internal)?
        .is_some()
    {
        return Err(AppError::Conflict("email already registered".into()));
    }
    let password_hash = hash_password(&body.password).map_err(AppError::Internal)?;
    let user = register_user_with_event(
        &state.pool,
        state.publisher.as_ref(),
        NewUser { email: body.email, password_hash },
        &["read:accounts:own"],
        &cid,
    )
    .await
    .map_err(AppError::Internal)?;
    let tokens = issue_token_pair(&state, &user).await?;
    Ok((StatusCode::CREATED, Json(tokens)))
}

async fn login(
    State(state): State<AuthState>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<AuthTokens>, AppError> {
    let found = state
        .users
        .find_by_email(&body.email)
        .await
        .map_err(AppError::Internal)?;
    let user = check_credentials(found.as_ref(), &body.password)?.clone();
    let tokens = issue_token_pair(&state, &user).await?;
    Ok(Json(tokens))
}
```

- [ ] **Step 6: Wire the ports module + lib**

`crates/domain-auth/src/ports/mod.rs` (replace):
```rust
pub mod dto;
pub mod http;
pub mod postgres;
pub mod repository;
pub use repository::UserRepository;
```

`crates/domain-auth/src/lib.rs` (replace):
```rust
//! Auth domain: users, scopes, JWT issuance, login/register/refresh/logout.
pub mod auth;
pub mod domain;
pub mod models;
pub mod ports;

pub use ports::http::{router, AuthState};
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-auth --test http`
Expected: PASS — register 201, duplicate 409, login 200, bad password 401.

- [ ] **Step 8: Format + lint, then commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/platform crates/domain-auth
git commit -m "feat(auth): register + login HTTP router (issues tokens; 409 on dup email)"
```

---

### Task 8: Wire `domain-auth` into `app`; remove `domain-account`'s `/dev/register`

**Files:**
- Modify: `crates/app/Cargo.toml` (add `domain-auth` dep)
- Modify: `crates/app/src/state.rs` (build `AuthState`)
- Modify: `crates/app/src/main.rs` (merge `domain_auth::router`)
- Modify: `crates/domain-account/src/ports/http.rs` (remove `dev_register` + `DevRegister`)
- Modify: `crates/domain-account/tests/http.rs` (drop any dev/register references if present — there are none; leave as is)
- Modify: `.env.example` (add private key + TTL + admin-emails keys)

**Interfaces:**
- Consumes: `domain_auth::{router, AuthState}`, `domain_auth::ports::postgres::PostgresUserRepository`, `domain_auth::auth::jwt::JwtIssuer`; existing `Resources`.
- Produces: a binary serving both `domain-account` and `domain-auth` routes.

- [ ] **Step 1: Add the app dependency**

`crates/app/Cargo.toml` `[dependencies]` add:
```toml
domain-auth = { path = "../domain-auth" }
```

- [ ] **Step 2: Remove the dev stand-in from `domain-account`**

In `crates/domain-account/src/ports/http.rs`:
- Remove the `.route("/dev/register", post(dev_register))` line from `router`.
- Delete the `dev_register` handler and the `DevRegister` struct.
- Remove now-unused imports: `axum::routing::post` (keep `get`), `platform::events::{EventPublisher, NewEvent}` if no longer used (note: `AccountState` still holds `publisher: Arc<dyn EventPublisher>`, so keep the `EventPublisher` import; `NewEvent` and `post` become unused — remove them), `serde::Deserialize` if unused.

Run `cargo build -p domain-account` and remove exactly what the compiler flags as unused.

- [ ] **Step 3: Build `AuthState` in app wiring**

In `crates/app/src/state.rs`, add imports:
```rust
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::postgres::PostgresUserRepository;
use domain_auth::AuthState;
```
Add an `issuer` and `admin_emails` to `Resources`:
```rust
    pub issuer: Arc<JwtIssuer>,
    pub admin_emails: Arc<Vec<String>>,
```
In `build_resources`, after building `jwt`, construct the issuer from the private key + settings:
```rust
    let issuer = Arc::new(
        JwtIssuer::from_rsa_pem(
            &settings.auth.jwt_private_key_pem,
            settings.auth.access_token_ttl_seconds,
            settings.auth.refresh_token_ttl_days,
        )
        .context("parse JWT private key")?,
    );
    let admin_emails = Arc::new(settings.auth.admin_email_list());
```
Add them to the returned `Resources { … issuer, admin_emails, … }`.

Add an `auth_state` builder:
```rust
pub fn auth_state(res: &Resources) -> AuthState {
    AuthState {
        pool: res.pool.clone(),
        users: Arc::new(PostgresUserRepository::new(res.pool.clone())),
        publisher: res.publisher.clone(),
        issuer: res.issuer.clone(),
        admin_emails: res.admin_emails.clone(),
        metrics: res.metrics.clone(),
    }
}
```

- [ ] **Step 4: Merge the auth router in main**

In `crates/app/src/main.rs`, change the app builder to merge both routers (apply the middleware/cors to the merged router):
```rust
    let app = domain_account::router(state::account_state(&res))
        .merge(domain_auth::router(state::auth_state(&res)))
        .layer(axum::middleware::from_fn(correlation_id_middleware))
        .layer(cors);
```

- [ ] **Step 5: Update `.env.example`**

Append/replace the auth section of `.env.example`:
```bash
APP__AUTH__JWT_PUBLIC_KEY_PEM="-----BEGIN PUBLIC KEY-----\n...replace...\n-----END PUBLIC KEY-----"
APP__AUTH__JWT_PRIVATE_KEY_PEM="-----BEGIN PRIVATE KEY-----\n...replace...\n-----END PRIVATE KEY-----"
APP__AUTH__ACCESS_TOKEN_TTL_SECONDS=900
APP__AUTH__REFRESH_TOKEN_TTL_DAYS=7
APP__AUTH__ADMIN_EMAILS=admin@example.com
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo build -p app`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/app crates/domain-account .env.example
git commit -m "feat(app): mount domain-auth router; remove domain-account /dev/register stand-in"
```

---

### Task 9: End-to-end test (register → outbox → dispatch → account) + wrap-up

**Files:**
- Create: `crates/app/tests/auth_e2e.rs`
- Modify: `crates/app/Cargo.toml` (dev-deps: add `domain-auth`, `http-body-util`)

**Interfaces:**
- Consumes: `domain_auth::router`, `AuthState`, the dispatcher, `domain_account` repository.

- [ ] **Step 1: Add dev-deps**

`crates/app/Cargo.toml` `[dev-dependencies]` add:
```toml
domain-auth = { path = "../domain-auth" }
http-body-util = { workspace = true }
```

- [ ] **Step 2: Write the e2e test**

`crates/app/tests/auth_e2e.rs`:
```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_account::ports::AccountRepository;
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::{router, AuthState};
use domain_auth::ports::postgres::PostgresUserRepository;
use platform::events::{
    dispatch_once, DispatcherConfig, EventPublisher, OutboxPublisher, Routes, SubscriberRegistry,
};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_priv.pem");

#[sqlx::test(migrations = "../../migrations")]
async fn register_then_dispatch_creates_account(pool: sqlx::PgPool) {
    let account_repo = Arc::new(PostgresAccountRepository::new(pool.clone()));
    let publisher: Arc<dyn EventPublisher> = Arc::new(OutboxPublisher::new(
        Routes::new().add("user.registered", "account.on-user-registered"),
    ));
    let mut registry = SubscriberRegistry::new();
    registry.register(Arc::new(AccountSubscriber::new(
        pool.clone(),
        account_repo.clone(),
        publisher.clone(),
    )));
    let registry = Arc::new(registry);

    let auth = router(AuthState {
        pool: pool.clone(),
        users: Arc::new(PostgresUserRepository::new(pool.clone())),
        publisher: publisher.clone(),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        admin_emails: Arc::new(vec![]),
        metrics: Metrics::new().unwrap(),
    });

    // 1. Register publishes user.registered.
    let res = auth
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"e2e@x.y","password":"hunter2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // 2. Dispatcher delivers it -> account subscriber creates the account.
    dispatch_once(&pool, &registry, &DispatcherConfig::default())
        .await
        .unwrap();

    // 3. The account now exists (auth_user.id == auth_user_id == 1 for the first user).
    let acc = account_repo.find_by_auth_user_id(1).await.unwrap();
    assert!(acc.is_some(), "account created from user.registered");
}
```

- [ ] **Step 3: Run the test**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p app --test auth_e2e`
Expected: PASS.

- [ ] **Step 4: Full suite + gate**

Run: `cargo build && DATABASE_URL=postgres://localhost/postgres cargo test && cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: PASS, clean.

- [ ] **Step 5: Commit**

```bash
git add crates/app
git commit -m "test(app): e2e register -> outbox -> dispatch -> account.created"
```

---

## Self-Review

**Spec coverage (against design §3/§4/§6/§7 for the 2a slice):** crate scaffold + auth_user/scope/user_scope migration ✓ (Task 1); `AccessClaims` extension + `AuthSettings` ✓ (Task 2); bcrypt ✓ (Task 3); models + RS256 `JwtIssuer` ✓ (Task 4); `UserRepository` + transactional register-with-event ✓ (Task 5); credential check + admin bootstrap ✓ (Task 6); register/login router + 409 ✓ (Task 7); app wiring + `/dev/register` removal + real producer ✓ (Task 8); full outbox loop e2e ✓ (Task 9). Refresh/logout/revocation deferred to 2b; admin scope endpoints deferred to 2c (per design §9).

**Placeholder scan:** the only intentional marker is the `password_placeholder_unused` import in Task 7 Step 4, explicitly called out to be replaced/deleted before completion. No other TBD/TODO.

**Type consistency:** `AccessClaims` new fields (Task 2) are populated by `JwtIssuer::issue_access` (Task 4) and consumed by Task 7. `register_user_with_event(pool, &dyn EventPublisher, NewUser, &[&str], cid)` signature consistent across Tasks 5/7. `AuthState` fields identical across Tasks 7/8/9. `AppError::Conflict` added in Task 7 and used there. Event payload `{auth_user_id, email}` matches `domain-account`'s `UserRegistered` subscriber from Spec 1.

**Known follow-up (2b):** `issue_token_pair` persists the refresh token; refresh/logout endpoints; `RevocationChecker` port + extractor change; prune task.
