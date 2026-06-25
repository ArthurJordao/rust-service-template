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
    let db_scopes = state
        .users
        .scope_names(user.id)
        .await
        .map_err(AppError::Internal)?;
    let scopes = effective_scopes(&user.email, db_scopes, &state.admin_emails);
    let now = chrono::Utc::now();
    let (access_token, _claims) = state
        .issuer
        .issue_access(user.id, &user.email, scopes, now)
        .map_err(AppError::Internal)?;
    let (_jti, refresh_token, _exp) = state
        .issuer
        .issue_refresh(user.id, now)
        .map_err(AppError::Internal)?;
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
        NewUser {
            email: body.email,
            password_hash,
        },
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
