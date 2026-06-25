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
