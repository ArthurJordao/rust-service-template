use crate::domain::authorize;
use crate::models::Account;
use crate::ports::AccountRepository;
use axum::extract::{FromRef, Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use http::StatusCode;
use platform::auth::{Authenticated, JwtVerifier};
use platform::db::Db;
use platform::events::{EventPublisher, NewEvent};
use platform::metrics::Metrics;
use platform::observability::CorrelationId;
use platform::server::{status_handler, AppError};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
pub struct AccountState {
    pub pool: Db,
    pub repo: Arc<dyn AccountRepository>,
    pub publisher: Arc<dyn EventPublisher>,
    pub jwt: Arc<JwtVerifier>,
    pub metrics: Metrics,
}

impl FromRef<AccountState> for Arc<JwtVerifier> {
    fn from_ref(state: &AccountState) -> Self {
        state.jwt.clone()
    }
}

pub fn router(state: AccountState) -> Router {
    Router::new()
        .route("/status", get(status_handler))
        .route("/accounts", get(list_accounts))
        .route("/accounts/:id", get(get_account))
        .route("/metrics", get(metrics_handler))
        .route("/dev/register", post(dev_register))
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

#[derive(Deserialize)]
struct DevRegister {
    auth_user_id: i64,
    email: String,
}

/// DEV-ONLY: publish a `user.registered` event to exercise the outbox loop.
/// Replaced by the real auth domain in Spec 2.
async fn dev_register(
    State(state): State<AccountState>,
    CorrelationId(cid): CorrelationId,
    Json(body): Json<DevRegister>,
) -> Result<StatusCode, AppError> {
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    state
        .publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "user.registered".into(),
                aggregate_id: body.auth_user_id.to_string(),
                payload: serde_json::json!({
                    "auth_user_id": body.auth_user_id,
                    "email": body.email,
                }),
                correlation_id: cid,
            },
        )
        .await
        .map_err(AppError::Internal)?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(StatusCode::ACCEPTED)
}
