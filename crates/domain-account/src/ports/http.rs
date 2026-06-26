use crate::domain::{auth_user_id_from_sub, authorize};
use crate::models::Account;
use crate::ports::AccountRepository;
use axum::extract::{FromRef, Path, State};
use axum::routing::get;
use axum::{Json, Router};
use platform::auth::{require_scope, Authenticated, JwtVerifier, RevocationChecker};
use platform::db::Db;
use platform::events::EventPublisher;
use platform::metrics::Metrics;
use platform::server::AppError;
use std::sync::Arc;

#[derive(Clone)]
pub struct AccountState {
    pub pool: Db,
    pub repo: Arc<dyn AccountRepository>,
    pub publisher: Arc<dyn EventPublisher>,
    pub jwt: Arc<JwtVerifier>,
    pub metrics: Metrics,
    pub revocation: Arc<dyn RevocationChecker>,
}

impl FromRef<AccountState> for Arc<JwtVerifier> {
    fn from_ref(state: &AccountState) -> Self {
        state.jwt.clone()
    }
}

impl FromRef<AccountState> for Arc<dyn RevocationChecker> {
    fn from_ref(state: &AccountState) -> Self {
        state.revocation.clone()
    }
}

pub fn router(state: AccountState) -> Router {
    Router::new()
        .route("/accounts", get(list_accounts))
        .route("/accounts/me", get(account_me))
        .route("/accounts/:id", get(get_account))
        .with_state(state)
}

#[utoipa::path(get, path = "/accounts",
    responses((status = 200, body = [Account]), (status = 401), (status = 403)),
    security(("bearer_auth" = [])), tag = "accounts")]
pub(crate) async fn list_accounts(
    State(state): State<AccountState>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<Account>>, AppError> {
    require_scope(&claims, "admin")?;
    let accounts = state.repo.list().await.map_err(AppError::Internal)?;
    Ok(Json(accounts))
}

#[utoipa::path(get, path = "/accounts/{id}",
    params(("id" = i64, Path,)),
    responses((status = 200, body = Account), (status = 401), (status = 403), (status = 404)),
    security(("bearer_auth" = [])), tag = "accounts")]
pub(crate) async fn get_account(
    State(state): State<AccountState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<i64>,
) -> Result<Json<Account>, AppError> {
    let account = state
        .repo
        .find_by_id(id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound("account not found".into()))?;
    authorize(&claims, &account)?;
    Ok(Json(account))
}

#[utoipa::path(get, path = "/accounts/me",
    responses((status = 200, body = Account), (status = 401), (status = 404)),
    security(("bearer_auth" = [])), tag = "accounts")]
pub(crate) async fn account_me(
    State(state): State<AccountState>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Account>, AppError> {
    let uid = auth_user_id_from_sub(&claims.sub)
        .ok_or_else(|| AppError::Unauthorized("invalid subject".into()))?;
    let account = state
        .repo
        .find_by_auth_user_id(uid)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound("no account for this user".into()))?;
    Ok(Json(account))
}
