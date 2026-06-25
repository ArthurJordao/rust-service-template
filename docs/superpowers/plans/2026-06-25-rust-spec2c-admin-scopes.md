# Spec 2c: Admin Scope Management — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the admin-gated scope-management surface — list the scope catalog, list users with their scopes, read and replace a user's scopes — with replacement bumping `tokens_valid_from` so the per-user revocation path (built in 2b) actually fires.

**Architecture:** A `ScopeRepository` port + Postgres adapter (reusing `PostgresUserRepository`). Four routes on the `domain-auth` router protected by the `Authenticated` extractor + `require_scope("admin")`. `AuthState` gains the `FromRef` impls the extractor needs. A seed migration populates the scope catalog.

**Tech Stack:** axum 0.7, sqlx (runtime API), jsonwebtoken 9, serde.

## Global Constraints

- Same as Spec 2a/2b. Depends on 2a + 2b being complete (uses `Authenticated`, `RevocationChecker`, `AuthState` with `verifier`/`revocation`).
- Admin authorization: `Authenticated(claims)` then `platform::auth::require_scope(&claims, "admin")`.
- Replacing a user's scopes MUST bump `auth_user.tokens_valid_from = now()` in the same transaction.
- axum 0.7 path syntax `:id`.
- Run `cargo fmt --all` + `cargo clippy --all-targets -- -D warnings` before each commit.

---

### Task 1: Seed the scope catalog

**Files:**
- Create: `migrations/0005_seed_scopes.sql`

**Interfaces:**
- Produces: baseline rows in `scope` so `GET /scopes` returns the known catalog.

- [ ] **Step 1: Write the seed migration**

`migrations/0005_seed_scopes.sql`:
```sql
insert into scope (name, description) values
    ('admin', 'Full administrative access'),
    ('read:accounts:own', 'Read your own account')
on conflict (name) do nothing;
```

- [ ] **Step 2: Verify well-formed**

Run: `cargo build -p platform`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add migrations/0005_seed_scopes.sql
git commit -m "feat(auth): seed scope catalog (admin, read:accounts:own)"
```

---

### Task 2: `ScopeRepository` port + Postgres adapter

**Files:**
- Modify: `crates/domain-auth/src/ports/repository.rs` (add `ScopeRepository`)
- Modify: `crates/domain-auth/src/ports/postgres.rs` (impl on `PostgresUserRepository`)
- Modify: `crates/domain-auth/src/ports/mod.rs` (export)
- Test: `crates/domain-auth/tests/scopes.rs`

**Interfaces:**
- Consumes: `User`, `ScopeRow` (2a); `platform::db::Db`.
- Produces:
  - `#[async_trait] pub trait ScopeRepository: Send + Sync { async fn list_catalog(&self) -> anyhow::Result<Vec<ScopeRow>>; async fn list_users_with_scopes(&self) -> anyhow::Result<Vec<(User, Vec<String>)>>; async fn replace_user_scopes(&self, user_id: i64, scopes: &[String]) -> anyhow::Result<()>; }`
  - `impl ScopeRepository for PostgresUserRepository`.

- [ ] **Step 1: Write the failing test**

`crates/domain-auth/tests/scopes.rs`:
```rust
use domain_auth::ports::postgres::PostgresUserRepository;
use domain_auth::ports::{ScopeRepository, UserRepository};

async fn seed_user(pool: &sqlx::PgPool, email: &str) -> i64 {
    sqlx::query_scalar(
        "insert into auth_user (email, password_hash, created_by_cid) values ($1, 'h', 'cid') returning id",
    )
    .bind(email)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn lists_catalog_and_replaces_scopes(pool: sqlx::PgPool) {
    let repo = PostgresUserRepository::new(pool.clone());

    // Catalog is seeded by migration 0005.
    let catalog = repo.list_catalog().await.unwrap();
    assert!(catalog.iter().any(|s| s.name == "admin"));

    let uid = seed_user(&pool, "u@x.y").await;
    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("select tokens_valid_from from auth_user where id = $1")
            .bind(uid)
            .fetch_one(&pool)
            .await
            .unwrap();

    repo.replace_user_scopes(uid, &["admin".into(), "read:accounts:own".into()])
        .await
        .unwrap();
    assert_eq!(repo.scope_names(uid).await.unwrap().len(), 2);

    // Replacing bumps tokens_valid_from.
    let after: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("select tokens_valid_from from auth_user where id = $1")
            .bind(uid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(after >= before);

    // Replacing again with fewer scopes overwrites.
    repo.replace_user_scopes(uid, &["read:accounts:own".into()]).await.unwrap();
    assert_eq!(repo.scope_names(uid).await.unwrap(), vec!["read:accounts:own".to_string()]);

    let users = repo.list_users_with_scopes().await.unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].0.email, "u@x.y");
    assert_eq!(users[0].1, vec!["read:accounts:own".to_string()]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-auth --test scopes`
Expected: FAIL — `ScopeRepository` not found.

- [ ] **Step 3: Add the trait**

Append to `crates/domain-auth/src/ports/repository.rs`:
```rust
use crate::models::{ScopeRow, User};

#[async_trait::async_trait]
pub trait ScopeRepository: Send + Sync {
    async fn list_catalog(&self) -> anyhow::Result<Vec<ScopeRow>>;
    async fn list_users_with_scopes(&self) -> anyhow::Result<Vec<(User, Vec<String>)>>;
    async fn replace_user_scopes(&self, user_id: i64, scopes: &[String]) -> anyhow::Result<()>;
}
```
> If `use crate::models::User;` is already imported at the top of `repository.rs`, merge `ScopeRow` into the existing `use` rather than duplicating.

- [ ] **Step 4: Implement on `PostgresUserRepository`**

Append to `crates/domain-auth/src/ports/postgres.rs`:
```rust
use crate::models::ScopeRow;
use crate::ports::repository::ScopeRepository;

#[async_trait::async_trait]
impl ScopeRepository for PostgresUserRepository {
    async fn list_catalog(&self) -> anyhow::Result<Vec<ScopeRow>> {
        let rows = sqlx::query_as::<_, ScopeRow>(
            "select id, name, description from scope order by name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn list_users_with_scopes(&self) -> anyhow::Result<Vec<(User, Vec<String>)>> {
        let users = sqlx::query_as::<_, User>(&format!(
            "select {USER_COLS} from auth_user order by id"
        ))
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(users.len());
        for user in users {
            let scopes: Vec<(String,)> =
                sqlx::query_as("select scope from user_scope where user_id = $1 order by scope")
                    .bind(user.id)
                    .fetch_all(&self.pool)
                    .await?;
            out.push((user, scopes.into_iter().map(|(s,)| s).collect()));
        }
        Ok(out)
    }

    async fn replace_user_scopes(&self, user_id: i64, scopes: &[String]) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("delete from user_scope where user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        for scope in scopes {
            sqlx::query("insert into user_scope (user_id, scope) values ($1, $2)")
                .bind(user_id)
                .bind(scope)
                .execute(&mut *tx)
                .await?;
        }
        // Invalidate the user's existing access tokens (per-user revocation epoch).
        sqlx::query("update auth_user set tokens_valid_from = now() where id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
```
> `USER_COLS` and `PostgresUserRepository.pool` are defined in this file (Task 5 of 2a / Task 3 of 2b) and are accessible here.

- [ ] **Step 5: Export the trait**

`crates/domain-auth/src/ports/mod.rs` (update the `pub use`):
```rust
pub use repository::{RefreshTokenRepository, ScopeRepository, UserRepository};
```

- [ ] **Step 6: Run test to verify it passes**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-auth --test scopes`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/domain-auth
git commit -m "feat(auth): ScopeRepository (catalog, users-with-scopes, replace + tokens_valid_from bump)"
```

---

### Task 3: Admin endpoints + `AuthState` FromRef impls

**Files:**
- Modify: `crates/domain-auth/src/ports/dto.rs` (`UserWithScopes`, `SetScopesRequest`)
- Modify: `crates/domain-auth/src/ports/http.rs` (FromRef impls, routes, handlers; `scopes: Arc<dyn ScopeRepository>` on `AuthState`)
- Test: `crates/domain-auth/tests/admin.rs`

**Interfaces:**
- Consumes: `ScopeRepository`, `UserRepository`, `ScopeRow`, `platform::auth::{Authenticated, JwtVerifier, RevocationChecker, require_scope}`.
- Produces:
  - DTOs: `UserWithScopes { id: i64, email: String, scopes: Vec<String> }`, `SetScopesRequest { scopes: Vec<String> }`.
  - `AuthState` gains `pub scopes: Arc<dyn ScopeRepository>`.
  - `impl FromRef<AuthState> for Arc<JwtVerifier>` and `impl FromRef<AuthState> for Arc<dyn RevocationChecker>`.
  - Routes: `GET /scopes`, `GET /users`, `GET /users/:id/scopes`, `PUT /users/:id/scopes` (all admin).

- [ ] **Step 1: Write the failing test**

`crates/domain-auth/tests/admin.rs`:
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
const TEST_PUB_PEM: &str = include_str!("fixtures/test_pub.pem");

fn state(pool: sqlx::PgPool) -> (AuthState, JwtIssuer) {
    let repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    let issuer = JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap();
    let s = AuthState {
        pool: pool.clone(),
        users: repo.clone(),
        refresh_tokens: repo.clone(),
        scopes: repo.clone(),
        publisher: Arc::new(OutboxPublisher::new(Routes::new())),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        verifier: Arc::new(platform::auth::JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        revocation: Arc::new(platform::auth::NoopRevocationChecker),
        admin_emails: Arc::new(vec![]),
        metrics: Metrics::new().unwrap(),
    };
    (s, issuer)
}

fn bearer(issuer: &JwtIssuer, scopes: &[&str]) -> String {
    let (token, _) = issuer
        .issue_access(1, "admin@x.y", scopes.iter().map(|s| s.to_string()).collect(), chrono::Utc::now())
        .unwrap();
    format!("Bearer {token}")
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_lists_scope_catalog(pool: sqlx::PgPool) {
    let (s, issuer) = state(pool);
    let app = router(s);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/scopes")
                .header("authorization", bearer(&issuer, &["admin"]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.as_array().unwrap().iter().any(|s| s["name"] == "admin"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn non_admin_is_forbidden(pool: sqlx::PgPool) {
    let (s, issuer) = state(pool);
    let app = router(s);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/scopes")
                .header("authorization", bearer(&issuer, &["read:accounts:own"]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../migrations")]
async fn missing_token_is_unauthorized(pool: sqlx::PgPool) {
    let (s, _issuer) = state(pool);
    let app = router(s);
    let res = app
        .oneshot(Request::builder().uri("/users").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_sets_user_scopes(pool: sqlx::PgPool) {
    let uid: i64 = sqlx::query_scalar(
        "insert into auth_user (email, password_hash, created_by_cid) values ('u@x.y','h','cid') returning id",
    )
    .fetch_one(&pool).await.unwrap();
    let (s, issuer) = state(pool.clone());
    let app = router(s);

    let res = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/users/{uid}/scopes"))
                .header("authorization", bearer(&issuer, &["admin"]))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"scopes":["read:accounts:own"]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let n: i64 = sqlx::query_scalar("select count(*) from user_scope where user_id = $1")
        .bind(uid).fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-auth --test admin`
Expected: FAIL — `scopes` field / routes not found.

- [ ] **Step 3: Add the DTOs**

Append to `crates/domain-auth/src/ports/dto.rs`:
```rust
#[derive(Debug, Serialize)]
pub struct UserWithScopes {
    pub id: i64,
    pub email: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetScopesRequest {
    pub scopes: Vec<String>,
}
```

- [ ] **Step 4: Extend `AuthState` + add FromRef impls + routes + handlers**

In `crates/domain-auth/src/ports/http.rs`:
- Add imports:
  ```rust
  use crate::ports::dto::{SetScopesRequest, UserWithScopes};
  use crate::ports::ScopeRepository;
  use axum::extract::{FromRef, Path};
  use platform::auth::{require_scope, Authenticated};
  ```
- Add the field to `AuthState`:
  ```rust
      pub scopes: Arc<dyn ScopeRepository>,
  ```
- Add the `FromRef` impls the `Authenticated` extractor needs on this state:
  ```rust
  impl FromRef<AuthState> for Arc<JwtVerifier> {
      fn from_ref(state: &AuthState) -> Self {
          state.verifier.clone()
      }
  }

  impl FromRef<AuthState> for Arc<dyn RevocationChecker> {
      fn from_ref(state: &AuthState) -> Self {
          state.revocation.clone()
      }
  }
  ```
- Add routes to `router`:
  ```rust
          .route("/scopes", get(list_scopes))
          .route("/users", get(list_users))
          .route("/users/:id/scopes", get(get_user_scopes).put(set_user_scopes))
  ```
- Add handlers:
  ```rust
  async fn list_scopes(
      State(state): State<AuthState>,
      Authenticated(claims): Authenticated,
  ) -> Result<Json<Vec<crate::models::ScopeRow>>, AppError> {
      require_scope(&claims, "admin")?;
      let catalog = state.scopes.list_catalog().await.map_err(AppError::Internal)?;
      Ok(Json(catalog))
  }

  async fn list_users(
      State(state): State<AuthState>,
      Authenticated(claims): Authenticated,
  ) -> Result<Json<Vec<UserWithScopes>>, AppError> {
      require_scope(&claims, "admin")?;
      let rows = state.scopes.list_users_with_scopes().await.map_err(AppError::Internal)?;
      Ok(Json(
          rows.into_iter()
              .map(|(u, scopes)| UserWithScopes { id: u.id, email: u.email, scopes })
              .collect(),
      ))
  }

  async fn get_user_scopes(
      State(state): State<AuthState>,
      Authenticated(claims): Authenticated,
      Path(id): Path<i64>,
  ) -> Result<Json<Vec<String>>, AppError> {
      require_scope(&claims, "admin")?;
      let scopes = state.users.scope_names(id).await.map_err(AppError::Internal)?;
      Ok(Json(scopes))
  }

  async fn set_user_scopes(
      State(state): State<AuthState>,
      Authenticated(claims): Authenticated,
      Path(id): Path<i64>,
      Json(body): Json<SetScopesRequest>,
  ) -> Result<StatusCode, AppError> {
      require_scope(&claims, "admin")?;
      state
          .scopes
          .replace_user_scopes(id, &body.scopes)
          .await
          .map_err(AppError::Internal)?;
      Ok(StatusCode::NO_CONTENT)
  }
  ```

- [ ] **Step 5: Run tests to verify they pass**

Run: `DATABASE_URL=postgres://localhost/postgres cargo test -p domain-auth --test admin`
Expected: PASS — catalog 200, non-admin 403, no-token 401, set-scopes 204.

- [ ] **Step 6: Update other `AuthState` constructions for the new `scopes` field**

The `scopes` field is new on `AuthState`. Update every other constructor:
- `crates/domain-auth/tests/http.rs` `state(...)` — add `scopes: repo.clone(),`.
- `crates/domain-auth/tests/refresh_logout.rs` `state(...)` — add `scopes: repo.clone(),`.
- `crates/app/src/state.rs` `auth_state(...)` — add `scopes: repo.clone(),`.
- `crates/app/tests/auth_e2e.rs` — the `AuthState { … }` literal — add `scopes: <the user repo Arc>.clone(),`.

Run: `cargo build --tests` and fix any remaining missing-field errors the compiler reports.

- [ ] **Step 7: Full suite + gate, then commit**

Run: `cargo build && DATABASE_URL=postgres://localhost/postgres cargo test && cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: PASS, clean.

```bash
git add crates/domain-auth crates/app
git commit -m "feat(auth): admin scope endpoints (catalog, users, get/set user scopes)"
```

---

### Task 4: README + docker-compose note + Spec 2 wrap-up

**Files:**
- Modify: `README.md` (mention auth endpoints + that there is no Redis)
- Modify: `docs/superpowers/specs/2026-06-24-rust-service-template-design.md` (mark Spec 2 done in roadmap — optional housekeeping)

**Interfaces:**
- Produces: docs reflecting the new auth surface. No code dependencies.

- [ ] **Step 1: Update the README**

In `README.md`, under Architecture, add a bullet:
```markdown
- `crates/domain-auth` — register/login/refresh/logout, RS256 JWTs, **Postgres-backed**
  token revocation (no Redis), admin scope management
```

- [ ] **Step 2: Verify the whole workspace once more**

Run: `cargo build && DATABASE_URL=postgres://localhost/postgres cargo test && cargo clippy --all-targets -- -D warnings`
Expected: PASS, clean.

- [ ] **Step 3: Commit**

```bash
git add README.md docs
git commit -m "docs: document domain-auth surface (Spec 2 complete)"
```

---

## Self-Review

**Spec coverage (against design §6 admin rows + §5 tokens_valid_from writer):** scope catalog seed ✓ (Task 1); `ScopeRepository` (catalog, users-with-scopes, replace + `tokens_valid_from` bump) ✓ (Task 2); `GET /scopes`, `GET /users`, `GET /users/:id/scopes`, `PUT /users/:id/scopes` all admin-gated ✓ (Task 3); `AuthState` FromRef impls so `Authenticated` works on auth routes ✓ (Task 3); docs ✓ (Task 4). The scope-replace `tokens_valid_from` bump closes the loop with 2b's per-user revocation check.

**Placeholder scan:** none. The "merge into existing `use`" notes (Task 2 Step 3, and the import additions) are guidance against duplicate-import clippy warnings, not placeholders.

**Type consistency:** `ScopeRepository` methods identical across Tasks 2/3. `AuthState` final field set (pool, users, refresh_tokens, scopes, publisher, issuer, verifier, revocation, admin_emails, metrics) is consistent across Task 3 and every constructor updated in Task 3 Step 6. `ScopeRow` (2a) is the `GET /scopes` response body. `require_scope` + `Authenticated` are the Spec 1 / 2b platform exports. `UserWithScopes`/`SetScopesRequest` DTOs match the handlers.

**Spec 2 end state:** `domain-auth` provides the full auth surface; it is the real `user.registered` producer; revocation is Postgres-backed and enforced on every protected route; no Redis was introduced. Matches design §10 "out of scope" (service tokens, rotation, email-verify, rate limiting all deferred).
