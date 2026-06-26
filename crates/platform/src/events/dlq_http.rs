use crate::auth::{require_scope, Authenticated, JwtVerifier, RevocationChecker};
use crate::db::Db;
use crate::events::{list_dead_letters, replay_dead_letter, DeadLetter};
use crate::server::AppError;
use axum::extract::{FromRef, Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::sync::Arc;

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ReplayResponse {
    pub replayed: bool,
}

#[derive(Clone)]
pub struct DlqState {
    pub pool: Db,
    pub jwt: Arc<JwtVerifier>,
    pub revocation: Arc<dyn RevocationChecker>,
}

impl FromRef<DlqState> for Arc<JwtVerifier> {
    fn from_ref(state: &DlqState) -> Self {
        state.jwt.clone()
    }
}

impl FromRef<DlqState> for Arc<dyn RevocationChecker> {
    fn from_ref(state: &DlqState) -> Self {
        state.revocation.clone()
    }
}

pub fn dlq_router(state: DlqState) -> Router {
    Router::new()
        .route("/admin/dlq", get(list_handler))
        .route("/admin/dlq/:delivery_id/replay", post(replay_handler))
        .with_state(state)
}

async fn list_handler(
    State(state): State<DlqState>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<DeadLetter>>, AppError> {
    require_scope(&claims, "admin")?;
    let rows = list_dead_letters(&state.pool)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(rows))
}

async fn replay_handler(
    State(state): State<DlqState>,
    Authenticated(claims): Authenticated,
    Path(delivery_id): Path<i64>,
) -> Result<Json<ReplayResponse>, AppError> {
    require_scope(&claims, "admin")?;
    let replayed = replay_dead_letter(&state.pool, delivery_id)
        .await
        .map_err(AppError::Internal)?;
    tracing::info!(delivery_id, "dlq delivery replayed");
    Ok(Json(ReplayResponse { replayed }))
}
