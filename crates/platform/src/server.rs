use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;
use http::StatusCode;
use serde_json::json;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::db::Db;

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
    #[error("{0}")]
    Conflict(String),
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
            AppError::Conflict(m) => (StatusCode::CONFLICT, m),
            AppError::Internal(e) => {
                tracing::error!(error = %e, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

/// Build a CORS layer from a list of allowed origins.
pub fn cors_layer(origins: &[String]) -> CorsLayer {
    let parsed: Vec<http::HeaderValue> = origins.iter().filter_map(|o| o.parse().ok()).collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(parsed))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
}

pub async fn status_handler() -> &'static str {
    "OK"
}

/// Readiness probe: succeeds only if a DB connection can run `select 1`. Returns
/// 503 otherwise so the platform pulls the instance from rotation. Distinct from
/// `/status` (liveness), which is a static 200.
pub async fn readyz_handler(State(pool): State<Db>) -> Response {
    match sqlx::query("select 1").execute(&pool).await {
        Ok(_) => (StatusCode::OK, "ready").into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "readiness check failed");
            (StatusCode::SERVICE_UNAVAILABLE, "not ready").into_response()
        }
    }
}

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

    #[tokio::test]
    async fn readyz_returns_503_when_db_unreachable() {
        // Lazy pool pointed at a closed port: construction succeeds, query fails.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(300))
            .connect_lazy("postgres://127.0.0.1:1/none")
            .expect("lazy pool");
        let res = super::readyz_handler(axum::extract::State(pool))
            .await
            .into_response();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
