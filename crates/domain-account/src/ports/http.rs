use crate::domain::authorize;
use crate::models::Account;
use crate::ports::AccountRepository;
use axum::extract::{FromRef, Path, State};
use axum::routing::get;
use axum::{Json, Router};
use platform::auth::{Authenticated, JwtVerifier, RevocationChecker};
use platform::db::Db;
use platform::events::EventPublisher;
use platform::metrics::Metrics;
use platform::server::{status_handler, AppError};
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
        .route("/status", get(status_handler))
        .route("/accounts", get(list_accounts))
        .route("/accounts/:id", get(get_account))
        .route("/metrics", get(metrics_handler))
        .with_state(state)
}

async fn list_accounts(State(state): State<AccountState>) -> Result<Json<Vec<Account>>, AppError> {
    let accounts = state.repo.list().await.map_err(AppError::Internal)?;
    Ok(Json(accounts))
}

async fn get_account(
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

async fn metrics_handler(State(state): State<AccountState>) -> String {
    state.metrics.render()
}
