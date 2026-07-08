use crate::auth::jwt::JwtIssuer;
use crate::auth::jwt::RefreshClaims;
use crate::auth::mfa_crypto::MfaCipher;
use crate::auth::password::hash_password;
use crate::auth::totp::FactorVerifier;
use crate::domain::{check_credentials, effective_scopes};
use crate::models::{NewUser, ScopeRow, User};
use crate::ports::dto::{
    AuthTokens, LoginRequest, LogoutRequest, RefreshRequest, RegisterRequest, SetScopesRequest,
    UserWithScopes,
};
use crate::ports::postgres::register_user_with_event;
use crate::ports::MfaRepository;
use crate::ports::RefreshTokenRepository;
use crate::ports::ScopeRepository;
use crate::ports::UserRepository;
use axum::extract::{FromRef, Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use http::StatusCode;
use platform::auth::{require_scope, Authenticated, JwtVerifier, RevocationChecker};
use platform::config::MfaPolicy;
use platform::db::Db;
use platform::events::EventPublisher;
use platform::metrics::Metrics;
use platform::observability::CorrelationId;
use platform::server::AppError;
use std::sync::Arc;

#[derive(Clone)]
pub struct MfaConfig {
    pub policy: MfaPolicy,
    pub cipher: Option<Arc<MfaCipher>>,
}

#[derive(Clone)]
pub struct AuthState {
    pub pool: Db,
    pub users: Arc<dyn UserRepository>,
    pub refresh_tokens: Arc<dyn RefreshTokenRepository>,
    pub scopes: Arc<dyn ScopeRepository>,
    pub publisher: Arc<dyn EventPublisher>,
    pub issuer: Arc<JwtIssuer>,
    pub verifier: Arc<JwtVerifier>,
    pub revocation: Arc<dyn RevocationChecker>,
    pub admin_emails: Arc<Vec<String>>,
    pub metrics: Metrics,
    pub mfa: Arc<dyn MfaRepository>,
    pub mfa_verifier: Arc<dyn FactorVerifier>,
    pub mfa_config: MfaConfig,
}

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

pub fn router(state: AuthState) -> Router {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        .route("/scopes", get(list_scopes))
        .route("/users", get(list_users))
        .route(
            "/users/:id/scopes",
            get(get_user_scopes).put(set_user_scopes),
        )
        .with_state(state)
}

/// Build access + refresh tokens for a user (refresh persistence arrives in 2b).
pub async fn issue_token_pair(
    state: &AuthState,
    user: &User,
    amr: Vec<String>,
) -> Result<AuthTokens, AppError> {
    let db_scopes = state
        .users
        .scope_names(user.id)
        .await
        .map_err(AppError::Internal)?;
    let scopes = effective_scopes(&user.email, db_scopes, &state.admin_emails);
    let now = chrono::Utc::now();
    let (access_token, _claims) = state
        .issuer
        .issue_access(user.id, &user.email, scopes, amr, now)
        .map_err(AppError::Internal)?;
    let (jti, refresh_token, refresh_exp) = state
        .issuer
        .issue_refresh(user.id, now)
        .map_err(AppError::Internal)?;
    state
        .refresh_tokens
        .store(&jti, user.id, refresh_exp)
        .await
        .map_err(AppError::Internal)?;
    Ok(AuthTokens {
        access_token,
        refresh_token,
        token_type: "Bearer".into(),
        expires_in: state.issuer.access_ttl_seconds(),
    })
}

#[utoipa::path(post, path = "/auth/register", request_body = RegisterRequest,
    responses((status = 201, body = AuthTokens), (status = 409)), tag = "auth")]
pub(crate) async fn register(
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
        NewUser {
            email: body.email,
            password_hash,
        },
        &["read:accounts:own"],
        &cid,
    )
    .await
    .map_err(AppError::Internal)?;
    tracing::info!(email = %user.email, user_id = user.id, "user registered");
    let tokens = issue_token_pair(&state, &user, vec!["pwd".into()]).await?;
    Ok((StatusCode::CREATED, Json(tokens)))
}

#[utoipa::path(post, path = "/auth/login", request_body = LoginRequest,
    responses((status = 200, body = AuthTokens), (status = 401)), tag = "auth")]
pub(crate) async fn login(
    State(state): State<AuthState>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<AuthTokens>, AppError> {
    let found = state
        .users
        .find_by_email(&body.email)
        .await
        .map_err(AppError::Internal)?;
    let user = match check_credentials(found.as_ref(), &body.password) {
        Ok(u) => u.clone(),
        Err(e) => {
            tracing::warn!(email = %body.email, "login failed");
            return Err(e);
        }
    };
    tracing::info!(email = %user.email, "login succeeded");
    let tokens = issue_token_pair(&state, &user, vec!["pwd".into()]).await?;
    Ok(Json(tokens))
}

#[utoipa::path(post, path = "/auth/refresh", request_body = RefreshRequest,
    responses((status = 200, body = AuthTokens), (status = 401)), tag = "auth")]
pub(crate) async fn refresh(
    State(state): State<AuthState>,
    Json(body): Json<RefreshRequest>,
) -> Result<Json<AuthTokens>, AppError> {
    let claims: RefreshClaims = state.verifier.decode(&body.refresh_token)?;
    if claims.token_type != "refresh" {
        return Err(AppError::Unauthorized(
            "access token cannot be used as refresh token".into(),
        ));
    }
    let stored = state
        .refresh_tokens
        .find_by_jti(&claims.jti)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::Unauthorized("refresh token not found".into()))?;
    if stored.revoked {
        return Err(AppError::Unauthorized("refresh token revoked".into()));
    }
    let user = state
        .users
        .find_by_id(stored.user_id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::Unauthorized("user not found".into()))?;

    // Issue a fresh access token; echo the same refresh token (no rotation).
    let db_scopes = state
        .users
        .scope_names(user.id)
        .await
        .map_err(AppError::Internal)?;
    let scopes = effective_scopes(&user.email, db_scopes, &state.admin_emails);
    let now = chrono::Utc::now();
    let (access_token, _claims) = state
        .issuer
        .issue_access(user.id, &user.email, scopes, vec!["pwd".into()], now)
        .map_err(AppError::Internal)?;
    Ok(Json(AuthTokens {
        access_token,
        refresh_token: body.refresh_token,
        token_type: "Bearer".into(),
        expires_in: state.issuer.access_ttl_seconds(),
    }))
}

#[utoipa::path(post, path = "/auth/logout", request_body = LogoutRequest,
    responses((status = 204)), tag = "auth")]
pub(crate) async fn logout(
    State(state): State<AuthState>,
    Json(body): Json<LogoutRequest>,
) -> Result<StatusCode, AppError> {
    tracing::info!("logout");
    // Revoke the refresh token if it parses and has the correct type (idempotent on garbage).
    if let Ok(claims) = state.verifier.decode::<RefreshClaims>(&body.refresh_token) {
        if claims.token_type == "refresh" {
            state
                .refresh_tokens
                .revoke(&claims.jti)
                .await
                .map_err(AppError::Internal)?;
        }
    }
    // Denylist the access token jti for its remaining lifetime, if supplied + valid.
    if let Some(at) = body.access_token {
        if let Ok(claims) = state.verifier.decode::<platform::auth::AccessClaims>(&at) {
            let expires_at = chrono::DateTime::<chrono::Utc>::from_timestamp(claims.exp as i64, 0)
                .unwrap_or_else(chrono::Utc::now);
            sqlx::query(
                "insert into revoked_access_token (jti, expires_at) values ($1, $2) \
                 on conflict (jti) do nothing",
            )
            .bind(&claims.jti)
            .bind(expires_at)
            .execute(&state.pool)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/scopes",
    responses((status = 200, body = [ScopeRow]), (status = 401), (status = 403)),
    security(("bearer_auth" = [])), tag = "admin")]
pub(crate) async fn list_scopes(
    State(state): State<AuthState>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<crate::models::ScopeRow>>, AppError> {
    require_scope(&claims, "admin")?;
    let catalog = state
        .scopes
        .list_catalog()
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(catalog))
}

#[utoipa::path(get, path = "/users",
    responses((status = 200, body = [UserWithScopes]), (status = 401), (status = 403)),
    security(("bearer_auth" = [])), tag = "admin")]
pub(crate) async fn list_users(
    State(state): State<AuthState>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<UserWithScopes>>, AppError> {
    require_scope(&claims, "admin")?;
    let rows = state
        .scopes
        .list_users_with_scopes()
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(
        rows.into_iter()
            .map(|(u, scopes)| UserWithScopes {
                id: u.id,
                email: u.email,
                scopes,
            })
            .collect(),
    ))
}

#[utoipa::path(get, path = "/users/{id}/scopes",
    params(("id" = i64, Path,)),
    responses((status = 200, body = [String]), (status = 401), (status = 403)),
    security(("bearer_auth" = [])), tag = "admin")]
pub(crate) async fn get_user_scopes(
    State(state): State<AuthState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<i64>,
) -> Result<Json<Vec<String>>, AppError> {
    require_scope(&claims, "admin")?;
    let scopes = state
        .users
        .scope_names(id)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(scopes))
}

#[utoipa::path(put, path = "/users/{id}/scopes",
    params(("id" = i64, Path,)), request_body = SetScopesRequest,
    responses((status = 204), (status = 401), (status = 403)),
    security(("bearer_auth" = [])), tag = "admin")]
pub(crate) async fn set_user_scopes(
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
    tracing::info!(target_user = id, "user scopes replaced");
    Ok(StatusCode::NO_CONTENT)
}
