# Spec 1a: Workspace + Platform Foundations — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the cargo workspace and the cross-cutting `platform` crate (config, observability + correlation IDs, db pool, JSON error handling, JWT verification, metrics, http client) as a compiling library with unit tests.

**Architecture:** A cargo workspace with three crates — `platform` (shared lib), `domain-account` (added in Plan 1c), and `app` (binary, added in Plan 1c). This plan only builds `platform` plus an empty `app`/`domain-account` so the workspace compiles. Cross-cutting concerns each live in their own `platform` module. Dependency injection is via concrete `AppState` (built in Plan 1c) holding `Arc<dyn Port>` trait objects; this plan defines the port traits and the resource builders.

**Tech Stack:** Rust (stable), tokio, axum + tower-http, sqlx (Postgres), tracing + tracing-subscriber, config, serde, thiserror, anyhow, reqwest, jsonwebtoken, prometheus, uuid, chrono.

## Global Constraints

- Rust edition **2021**, resolver **2**.
- Dependency versions (pin in each `Cargo.toml`, copied verbatim): `tokio = { version = "1", features = ["full"] }`, `axum = "0.7"`, `tower = "0.5"`, `tower-http = { version = "0.6", features = ["cors", "trace"] }`, `sqlx = { version = "0.8", features = ["runtime-tokio", "tls-rustls", "postgres", "macros", "chrono", "uuid", "json", "migrate"] }`, `tracing = "0.1"`, `tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }`, `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`, `config = "0.14"`, `thiserror = "1"`, `anyhow = "1"`, `reqwest = { version = "0.12", features = ["json"] }`, `jsonwebtoken = "9"`, `prometheus = "0.13"`, `uuid = { version = "1", features = ["v4", "serde"] }`, `chrono = { version = "0.4", features = ["serde"] }`, `async-trait = "0.1"`, `http = "1"`.
- axum **0.7** path-param syntax is `:id` (not `{id}`). Do not upgrade to 0.8 in this plan.
- All public functions return `Result<T, E>` with explicit error types — no `.unwrap()`/`.expect()` outside tests and `main` startup.
- Correlation-id header name is **`X-Correlation-Id`** everywhere.
- JWT subject convention: `user-{id}`; scopes are a JSON array claim named `scopes`.
- Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings` before each commit; both must pass.

---

### Task 1: Workspace scaffold

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/platform/Cargo.toml`
- Create: `crates/platform/src/lib.rs`
- Create: `crates/domain-account/Cargo.toml`
- Create: `crates/domain-account/src/lib.rs`
- Create: `crates/app/Cargo.toml`
- Create: `crates/app/src/main.rs`
- Create: `rust-toolchain.toml`

**Interfaces:**
- Produces: a compiling 3-crate workspace. `platform` exposes nothing yet; `app` is a placeholder `main`.

- [ ] **Step 1: Create the toolchain file**

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

- [ ] **Step 2: Create the workspace manifest**

`Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = ["crates/platform", "crates/domain-account", "crates/app"]

[workspace.package]
edition = "2021"
version = "0.1.0"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
axum = "0.7"
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "trace"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "tls-rustls", "postgres", "macros", "chrono", "uuid", "json", "migrate"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
config = "0.14"
thiserror = "1"
anyhow = "1"
reqwest = { version = "0.12", features = ["json"] }
jsonwebtoken = "9"
prometheus = "0.13"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
async-trait = "0.1"
http = "1"
```

- [ ] **Step 3: Create the `platform` crate manifest**

`crates/platform/Cargo.toml`:
```toml
[package]
name = "platform"
edition.workspace = true
version.workspace = true

[dependencies]
tokio.workspace = true
axum.workspace = true
tower.workspace = true
tower-http.workspace = true
sqlx.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
serde.workspace = true
serde_json.workspace = true
config.workspace = true
thiserror.workspace = true
anyhow.workspace = true
reqwest.workspace = true
jsonwebtoken.workspace = true
prometheus.workspace = true
uuid.workspace = true
chrono.workspace = true
async-trait.workspace = true
http.workspace = true
```

- [ ] **Step 4: Create placeholder crate sources**

`crates/platform/src/lib.rs`:
```rust
//! Cross-cutting platform concerns shared by all domains.
```

`crates/domain-account/Cargo.toml`:
```toml
[package]
name = "domain-account"
edition.workspace = true
version.workspace = true

[dependencies]
platform = { path = "../platform" }
```

`crates/domain-account/src/lib.rs`:
```rust
//! Account domain. Implemented in Plan 1c.
```

`crates/app/Cargo.toml`:
```toml
[package]
name = "app"
edition.workspace = true
version.workspace = true

[dependencies]
platform = { path = "../platform" }
domain-account = { path = "../domain-account" }
tokio.workspace = true
```

`crates/app/src/main.rs`:
```rust
fn main() {
    println!("rust-service-template: app placeholder");
}
```

- [ ] **Step 5: Verify the workspace compiles**

Run: `cargo build`
Expected: PASS — all three crates compile.

- [ ] **Step 6: Commit**

```bash
git add .
git commit -m "chore: scaffold cargo workspace (platform, domain-account, app)"
```

---

### Task 2: `platform::config` — typed settings from env

**Files:**
- Create: `crates/platform/src/config.rs`
- Modify: `crates/platform/src/lib.rs`
- Test: inline `#[cfg(test)]` module in `config.rs`

**Interfaces:**
- Produces:
  - `pub struct Settings { pub server: ServerSettings, pub database: DatabaseSettings, pub auth: AuthSettings, pub cors_allowed_origins: Vec<String> }`
  - `pub struct ServerSettings { pub port: u16, pub environment: String }`
  - `pub struct DatabaseSettings { pub url: String, pub max_connections: u32, pub auto_migrate: bool }`
  - `pub struct AuthSettings { pub jwt_public_key_pem: String }`
  - `pub fn Settings::load() -> Result<Settings, config::ConfigError>`

- [ ] **Step 1: Write the failing test**

Add to `crates/platform/src/config.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_settings_from_env() {
        std::env::set_var("APP__SERVER__PORT", "9999");
        std::env::set_var("APP__SERVER__ENVIRONMENT", "test");
        std::env::set_var("APP__DATABASE__URL", "postgres://localhost/x");
        std::env::set_var("APP__DATABASE__MAX_CONNECTIONS", "3");
        std::env::set_var("APP__DATABASE__AUTO_MIGRATE", "true");
        std::env::set_var("APP__AUTH__JWT_PUBLIC_KEY_PEM", "PEM");
        std::env::set_var("APP__CORS_ALLOWED_ORIGINS", "http://localhost:5173");

        let s = Settings::load().expect("settings load");
        assert_eq!(s.server.port, 9999);
        assert_eq!(s.database.max_connections, 3);
        assert!(s.database.auto_migrate);
        assert_eq!(s.cors_allowed_origins, vec!["http://localhost:5173".to_string()]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p platform config::`
Expected: FAIL — `Settings` not found.

- [ ] **Step 3: Write the implementation**

Top of `crates/platform/src/config.rs`:
```rust
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSettings {
    pub port: u16,
    pub environment: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseSettings {
    pub url: String,
    pub max_connections: u32,
    pub auto_migrate: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthSettings {
    pub jwt_public_key_pem: String,
}

fn default_cors() -> Vec<String> {
    vec!["http://localhost:5173".to_string()]
}

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    pub server: ServerSettings,
    pub database: DatabaseSettings,
    pub auth: AuthSettings,
    #[serde(default = "default_cors")]
    pub cors_allowed_origins: Vec<String>,
}

impl Settings {
    /// Load settings from environment variables prefixed `APP__`,
    /// nested with `__` (e.g. `APP__SERVER__PORT`).
    /// Comma-separated lists (cors origins) are split via the list parser.
    pub fn load() -> Result<Settings, config::ConfigError> {
        config::Config::builder()
            .add_source(
                config::Environment::with_prefix("APP")
                    .separator("__")
                    .list_separator(",")
                    .with_list_parse_key("cors_allowed_origins")
                    .try_parsing(true),
            )
            .build()?
            .try_deserialize()
    }
}
```

Add to `crates/platform/src/lib.rs`:
```rust
pub mod config;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p platform config::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/platform
git commit -m "feat(platform): typed settings loaded from env (config)"
```

---

### Task 3: `platform::observability` — tracing init + correlation-id layer

**Files:**
- Create: `crates/platform/src/observability.rs`
- Modify: `crates/platform/src/lib.rs`
- Test: inline `#[cfg(test)]` in `observability.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub const CORRELATION_ID_HEADER: &str = "x-correlation-id";`
  - `pub fn init_tracing(env_filter: &str)` — installs a JSON subscriber (idempotent; safe to skip if already set).
  - `pub fn correlation_id_layer() -> tower_http::trace::TraceLayer<...>` — NOT used directly; instead we expose a tower middleware function (below).
  - `pub async fn correlation_id_middleware(req: Request, next: Next) -> Response` — axum middleware that reads/generates the cid, stores it in request extensions, opens an `info_span!` with `cid`, and echoes the header on the response.
  - `pub struct CorrelationId(pub String)` — newtype stored in request extensions and used as an axum extractor.
  - `pub fn new_correlation_id() -> String` — generates a uuid v4 string.

- [ ] **Step 1: Write the failing test**

Add to `crates/platform/src/observability.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_non_empty_cid() {
        let cid = new_correlation_id();
        assert_eq!(cid.len(), 36); // uuid v4 hyphenated
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p platform observability::`
Expected: FAIL — `new_correlation_id` not found.

- [ ] **Step 3: Write the implementation**

`crates/platform/src/observability.rs` (top):
```rust
use axum::{
    extract::Request,
    middleware::Next,
    response::Response,
};
use http::{HeaderName, HeaderValue};
use tracing::Instrument;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub const CORRELATION_ID_HEADER: &str = "x-correlation-id";

/// A correlation id carried through a request and into spawned event handlers.
#[derive(Debug, Clone)]
pub struct CorrelationId(pub String);

pub fn new_correlation_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Install a JSON tracing subscriber. Idempotent: a second call is a no-op.
pub fn init_tracing(env_filter: &str) {
    let filter = EnvFilter::try_new(env_filter).unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().json().with_current_span(true).with_span_list(false))
        .try_init();
}

/// axum middleware: extract or mint a correlation id, attach it to the request
/// (extensions), run the rest of the stack inside a span carrying the cid, and
/// echo the cid on the response.
pub async fn correlation_id_middleware(mut req: Request, next: Next) -> Response {
    let cid = req
        .headers()
        .get(CORRELATION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(new_correlation_id);

    req.extensions_mut().insert(CorrelationId(cid.clone()));

    let span = tracing::info_span!("request", cid = %cid);
    let mut res = next.run(req).instrument(span).await;

    if let Ok(val) = HeaderValue::from_str(&cid) {
        res.headers_mut()
            .insert(HeaderName::from_static(CORRELATION_ID_HEADER), val);
    }
    res
}
```

Add an extractor so handlers can read the cid:
```rust
#[async_trait::async_trait]
impl<S> axum::extract::FromRequestParts<S> for CorrelationId
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<CorrelationId>()
            .cloned()
            .unwrap_or_else(|| CorrelationId(new_correlation_id())))
    }
}
```

Add to `crates/platform/src/lib.rs`:
```rust
pub mod observability;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p platform observability::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/platform
git commit -m "feat(platform): tracing JSON init + correlation-id middleware/extractor"
```

---

### Task 4: `platform::server` — JSON error type + shared router pieces

**Files:**
- Create: `crates/platform/src/server.rs`
- Modify: `crates/platform/src/lib.rs`
- Test: inline `#[cfg(test)]` in `server.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub enum AppError { NotFound(String), Forbidden(String), Unauthorized(String), BadRequest(String), Internal(anyhow::Error) }`
  - `impl axum::response::IntoResponse for AppError` — JSON body `{ "error": <message> }` with matching status.
  - `impl From<anyhow::Error> for AppError`
  - `pub fn cors_layer(origins: &[String]) -> tower_http::cors::CorsLayer`
  - `pub async fn status_handler() -> &'static str` — returns `"OK"`.

- [ ] **Step 1: Write the failing test**

Add to `crates/platform/src/server.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use http::StatusCode;

    #[test]
    fn not_found_maps_to_404() {
        let res = AppError::NotFound("nope".into()).into_response();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn forbidden_maps_to_403() {
        let res = AppError::Forbidden("no".into()).into_response();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p platform server::`
Expected: FAIL — `AppError` not found.

- [ ] **Step 3: Write the implementation**

`crates/platform/src/server.rs` (top):
```rust
use axum::response::{IntoResponse, Response};
use axum::Json;
use http::StatusCode;
use serde_json::json;
use tower_http::cors::{AllowOrigin, CorsLayer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    BadRequest(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            AppError::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            AppError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AppError::Internal(e) => {
                tracing::error!(error = %e, "internal server error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error".to_string())
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

/// Build a CORS layer from a list of allowed origins.
pub fn cors_layer(origins: &[String]) -> CorsLayer {
    let parsed: Vec<http::HeaderValue> = origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(parsed))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
}

pub async fn status_handler() -> &'static str {
    "OK"
}
```

Add to `crates/platform/src/lib.rs`:
```rust
pub mod server;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p platform server::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/platform
git commit -m "feat(platform): AppError JSON responses + CORS + status handler"
```

---

### Task 5: `platform::auth` — JWT verification, claims, scope guard

**Files:**
- Create: `crates/platform/src/auth.rs`
- Modify: `crates/platform/src/lib.rs`
- Test: inline `#[cfg(test)]` in `auth.rs`

**Interfaces:**
- Consumes: `AppError` from `platform::server`.
- Produces:
  - `pub struct AccessClaims { pub sub: String, pub scopes: Vec<String>, pub exp: usize }`
  - `pub struct JwtVerifier { /* holds DecodingKey + Validation */ }`
  - `pub fn JwtVerifier::from_rsa_pem(pem: &str) -> anyhow::Result<JwtVerifier>`
  - `pub fn JwtVerifier::verify(&self, token: &str) -> Result<AccessClaims, AppError>`
  - `impl AccessClaims { pub fn has_scope(&self, scope: &str) -> bool }`
  - `pub fn require_scope(claims: &AccessClaims, scope: &str) -> Result<(), AppError>`

Note: the axum extractor that pulls `AccessClaims` out of the `Authorization` header is built in Plan 1c (Task: account http port) once `AppState` exists to hold the verifier. This task delivers the pure verifier + guard.

- [ ] **Step 1: Write the failing test**

Add to `crates/platform/src/auth.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn claims(scopes: &[&str]) -> AccessClaims {
        AccessClaims {
            sub: "user-1".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            exp: 9_999_999_999,
        }
    }

    #[test]
    fn has_scope_checks_membership() {
        let c = claims(&["admin", "read:accounts:own"]);
        assert!(c.has_scope("admin"));
        assert!(!c.has_scope("write:accounts"));
    }

    #[test]
    fn require_scope_rejects_missing() {
        let c = claims(&["read:accounts:own"]);
        assert!(require_scope(&c, "admin").is_err());
        assert!(require_scope(&c, "read:accounts:own").is_ok());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p platform auth::`
Expected: FAIL — `AccessClaims` not found.

- [ ] **Step 3: Write the implementation**

`crates/platform/src/auth.rs` (top):
```rust
use crate::server::AppError;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessClaims {
    pub sub: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub exp: usize,
}

impl AccessClaims {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

#[derive(Clone)]
pub struct JwtVerifier {
    key: DecodingKey,
    validation: Validation,
}

impl JwtVerifier {
    /// Build a verifier from an RSA public key in PEM form (RS256).
    pub fn from_rsa_pem(pem: &str) -> anyhow::Result<JwtVerifier> {
        let key = DecodingKey::from_rsa_pem(pem.as_bytes())?;
        let validation = Validation::new(Algorithm::RS256);
        Ok(JwtVerifier { key, validation })
    }

    pub fn verify(&self, token: &str) -> Result<AccessClaims, AppError> {
        decode::<AccessClaims>(token, &self.key, &self.validation)
            .map(|data| data.claims)
            .map_err(|e| AppError::Unauthorized(format!("invalid token: {e}")))
    }
}

pub fn require_scope(claims: &AccessClaims, scope: &str) -> Result<(), AppError> {
    if claims.has_scope(scope) {
        Ok(())
    } else {
        Err(AppError::Forbidden(format!("missing required scope: {scope}")))
    }
}
```

Add to `crates/platform/src/lib.rs`:
```rust
pub mod auth;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p platform auth::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/platform
git commit -m "feat(platform): JWT verifier, access claims, scope guard"
```

---

### Task 6: `platform::metrics` — Prometheus registry + handler

**Files:**
- Create: `crates/platform/src/metrics.rs`
- Modify: `crates/platform/src/lib.rs`
- Test: inline `#[cfg(test)]` in `metrics.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct Metrics { pub http_requests: prometheus::IntCounterVec, registry: prometheus::Registry }`
  - `pub fn Metrics::new() -> anyhow::Result<Metrics>`
  - `pub fn Metrics::record_http(&self, method: &str, path: &str, status: u16)`
  - `pub fn Metrics::render(&self) -> String` — Prometheus text exposition.

- [ ] **Step 1: Write the failing test**

Add to `crates/platform/src/metrics.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_renders() {
        let m = Metrics::new().unwrap();
        m.record_http("GET", "/accounts", 200);
        let out = m.render();
        assert!(out.contains("http_requests_total"));
        assert!(out.contains("/accounts"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p platform metrics::`
Expected: FAIL — `Metrics` not found.

- [ ] **Step 3: Write the implementation**

`crates/platform/src/metrics.rs` (top):
```rust
use prometheus::{Encoder, IntCounterVec, Opts, Registry, TextEncoder};

#[derive(Clone)]
pub struct Metrics {
    pub http_requests: IntCounterVec,
    registry: Registry,
}

impl Metrics {
    pub fn new() -> anyhow::Result<Metrics> {
        let registry = Registry::new();
        let http_requests = IntCounterVec::new(
            Opts::new("http_requests_total", "Total HTTP requests"),
            &["method", "path", "status"],
        )?;
        registry.register(Box::new(http_requests.clone()))?;
        Ok(Metrics { http_requests, registry })
    }

    pub fn record_http(&self, method: &str, path: &str, status: u16) {
        self.http_requests
            .with_label_values(&[method, path, &status.to_string()])
            .inc();
    }

    pub fn render(&self) -> String {
        let encoder = TextEncoder::new();
        let mut buf = Vec::new();
        let families = self.registry.gather();
        let _ = encoder.encode(&families, &mut buf);
        String::from_utf8(buf).unwrap_or_default()
    }
}
```

Add to `crates/platform/src/lib.rs`:
```rust
pub mod metrics;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p platform metrics::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/platform
git commit -m "feat(platform): Prometheus metrics registry + http counter"
```

---

### Task 7: `platform::http_client` — reqwest wrapper with cid propagation

**Files:**
- Create: `crates/platform/src/http_client.rs`
- Modify: `crates/platform/src/lib.rs`
- Test: inline `#[cfg(test)]` in `http_client.rs`

**Interfaces:**
- Consumes: `CORRELATION_ID_HEADER` from `platform::observability`.
- Produces:
  - `pub struct HttpClient { inner: reqwest::Client }`
  - `pub fn HttpClient::new() -> HttpClient`
  - `pub async fn HttpClient::get_json<T: DeserializeOwned>(&self, url: &str, cid: Option<&str>) -> anyhow::Result<T>` — issues GET, attaches the cid header when present, deserializes JSON.

- [ ] **Step 1: Write the failing test**

Add to `crates/platform/src/http_client.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_client() {
        let _c = HttpClient::new();
    }
}
```

(Network calls are covered by integration tests in Plan 1c; this unit test only guards construction.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p platform http_client::`
Expected: FAIL — `HttpClient` not found.

- [ ] **Step 3: Write the implementation**

`crates/platform/src/http_client.rs` (top):
```rust
use crate::observability::CORRELATION_ID_HEADER;
use serde::de::DeserializeOwned;

#[derive(Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    pub fn new() -> HttpClient {
        HttpClient { inner: reqwest::Client::new() }
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        url: &str,
        cid: Option<&str>,
    ) -> anyhow::Result<T> {
        let mut req = self.inner.get(url);
        if let Some(cid) = cid {
            req = req.header(CORRELATION_ID_HEADER, cid);
        }
        let resp = req.send().await?.error_for_status()?;
        Ok(resp.json::<T>().await?)
    }
}
```

Add to `crates/platform/src/lib.rs`:
```rust
pub mod http_client;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p platform http_client::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/platform
git commit -m "feat(platform): reqwest http client with correlation-id propagation"
```

---

### Task 8: `platform::db` — pool builder + transaction helper

**Files:**
- Create: `crates/platform/src/db.rs`
- Modify: `crates/platform/src/lib.rs`
- Create: `migrations/.gitkeep`
- Test: inline `#[cfg(test)]` in `db.rs` (compile-only; live DB tests live in Plan 1b/1c)

**Interfaces:**
- Consumes: `DatabaseSettings` from `platform::config`.
- Produces:
  - `pub type Db = sqlx::PgPool;`
  - `pub async fn make_pool(settings: &DatabaseSettings) -> anyhow::Result<Db>`
  - `pub async fn run_migrations(pool: &Db) -> anyhow::Result<()>` — runs `sqlx::migrate!("./migrations")`.

- [ ] **Step 1: Write the failing test**

Add to `crates/platform/src/db.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DatabaseSettings;

    #[test]
    fn builds_settings_struct() {
        // Compile-only guard: a real connection is exercised in integration tests.
        let _s = DatabaseSettings {
            url: "postgres://localhost/x".into(),
            max_connections: 5,
            auto_migrate: false,
        };
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p platform db::`
Expected: FAIL — `db` module not found / not declared.

- [ ] **Step 3: Write the implementation**

Create empty migrations dir marker: `migrations/.gitkeep` (empty file).

`crates/platform/src/db.rs` (top):
```rust
use crate::config::DatabaseSettings;
use sqlx::postgres::PgPoolOptions;

pub type Db = sqlx::PgPool;

pub async fn make_pool(settings: &DatabaseSettings) -> anyhow::Result<Db> {
    let pool = PgPoolOptions::new()
        .max_connections(settings.max_connections)
        .connect(&settings.url)
        .await?;
    Ok(pool)
}

pub async fn run_migrations(pool: &Db) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
```

Add to `crates/platform/src/lib.rs`:
```rust
pub mod db;
```

Note: `sqlx::migrate!("./migrations")` resolves relative to the workspace root at build time. The empty `migrations/` dir is valid (zero migrations). Real migrations are added in Plan 1b.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p platform db::`
Expected: PASS (compiles; the no-op test passes).

- [ ] **Step 5: Commit**

```bash
git add crates/platform migrations
git commit -m "feat(platform): Postgres pool builder + migration runner"
```

---

### Task 9: Plan 1a wrap-up — lib surface + clippy gate

**Files:**
- Modify: `crates/platform/src/lib.rs`

**Interfaces:**
- Produces: a clean `platform` public surface re-exporting the most-used items.

- [ ] **Step 1: Finalize the lib surface**

`crates/platform/src/lib.rs` (full):
```rust
//! Cross-cutting platform concerns shared by all domains.
pub mod auth;
pub mod config;
pub mod db;
pub mod http_client;
pub mod metrics;
pub mod observability;
pub mod server;

pub use config::Settings;
pub use server::AppError;
```

- [ ] **Step 2: Run the full test suite**

Run: `cargo test -p platform`
Expected: PASS — all unit tests across config/observability/server/auth/metrics/http_client/db.

- [ ] **Step 3: Format + lint gate**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: no warnings, no diffs.

- [ ] **Step 4: Commit**

```bash
git add crates/platform
git commit -m "chore(platform): finalize lib surface; fmt + clippy clean"
```

---

## Self-Review

**Spec coverage (against design §4):** config ✓ (Task 2), observability + cid ✓ (Task 3), server/AppError/CORS/status ✓ (Task 4), auth verify + scope ✓ (Task 5), metrics ✓ (Task 6), http_client ✓ (Task 7), db ✓ (Task 8). `events` and `metadata` modules are intentionally deferred: `events` is the whole of Plan 1b; `metadata` (created_at/created_by_cid injection) is folded into the account repository in Plan 1c where it is first needed (YAGNI — no insert exists yet).

**Placeholder scan:** no TBD/TODO; every code step shows complete code; the `.gitkeep` and empty `migrations/` are intentional and explained.

**Type consistency:** `Settings`/`DatabaseSettings` produced in Task 2 are consumed by Task 8 (`make_pool`) with matching field names (`url`, `max_connections`, `auto_migrate`). `AppError` from Task 4 is consumed by Task 5 (`verify`, `require_scope`). `CORRELATION_ID_HEADER` from Task 3 is consumed by Task 7. `AccessClaims`/`JwtVerifier` names are reused verbatim by Plan 1c's extractor.

**Deferred-to-Plan-1c note:** the axum extractor for `AccessClaims` needs `AppState` (which holds `JwtVerifier`), so it is built in 1c, not here.
