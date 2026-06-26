use crate::models::SentNotification;
use crate::ports::repository::SentNotificationRepository;
use axum::extract::{FromRef, State};
use axum::routing::get;
use axum::{Json, Router};
use platform::auth::{require_scope, Authenticated, JwtVerifier, RevocationChecker};
use platform::metrics::Metrics;
use platform::server::AppError;
use std::sync::Arc;

#[derive(Clone)]
pub struct NotificationState {
    pub repo: Arc<dyn SentNotificationRepository>,
    pub jwt: Arc<JwtVerifier>,
    pub revocation: Arc<dyn RevocationChecker>,
    pub metrics: Metrics,
}

impl FromRef<NotificationState> for Arc<JwtVerifier> {
    fn from_ref(state: &NotificationState) -> Self {
        state.jwt.clone()
    }
}

impl FromRef<NotificationState> for Arc<dyn RevocationChecker> {
    fn from_ref(state: &NotificationState) -> Self {
        state.revocation.clone()
    }
}

pub fn router(state: NotificationState) -> Router {
    Router::new()
        .route("/notifications", get(list_notifications))
        .with_state(state)
}

#[utoipa::path(get, path = "/notifications",
    responses((status = 200, body = [SentNotification]), (status = 401), (status = 403)),
    security(("bearer_auth" = [])), tag = "notifications")]
pub(crate) async fn list_notifications(
    State(state): State<NotificationState>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<SentNotification>>, AppError> {
    require_scope(&claims, "admin")?;
    let rows = state.repo.list().await.map_err(AppError::Internal)?;
    Ok(Json(rows))
}
