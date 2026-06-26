use axum::{extract::Request, middleware::Next, response::Response};
use http::{HeaderName, HeaderValue};
use tracing::Instrument;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub const CORRELATION_ID_HEADER: &str = "x-correlation-id";

/// A correlation id carried through a request and into spawned event handlers.
#[derive(Debug, Clone)]
pub struct CorrelationId(pub String);

/// A short correlation-id segment (6 hex chars derived from a uuid v4).
pub fn new_segment() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

/// Extend a correlation id with a fresh child segment: `parent` -> `parent.<seg>`.
pub fn append(cid: &str) -> String {
    format!("{cid}.{}", new_segment())
}

/// Install a JSON tracing subscriber. Level comes from `RUST_LOG` if set, else
/// `default_level`. Idempotent: a second call is a no-op.
pub fn init_tracing(default_level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(default_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .json()
                .with_current_span(true)
                .with_span_list(false),
        )
        .try_init();
}

/// axum middleware: derive this request's cid by appending a fresh segment to the
/// incoming `X-Correlation-Id` (or a new root), run the stack inside a cid span,
/// emit one access log on completion, and echo the cid on the response.
pub async fn correlation_id_middleware(mut req: Request, next: Next) -> Response {
    let incoming = req
        .headers()
        .get(CORRELATION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let cid = append(&incoming.unwrap_or_else(new_segment));

    req.extensions_mut().insert(CorrelationId(cid.clone()));

    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let infra = matches!(path.as_str(), "/status" | "/metrics");

    let span = tracing::info_span!("request", %cid);
    let mut res = async move {
        let start = std::time::Instant::now();
        let res = next.run(req).await;
        if !infra {
            tracing::info!(
                method = %method,
                path = %path,
                status = res.status().as_u16(),
                latency_ms = start.elapsed().as_millis() as u64,
                "request completed"
            );
        }
        res
    }
    .instrument(span)
    .await;

    if let Ok(val) = HeaderValue::from_str(&cid) {
        res.headers_mut()
            .insert(HeaderName::from_static(CORRELATION_ID_HEADER), val);
    }
    res
}

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
            .unwrap_or_else(|| CorrelationId(new_segment())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_is_short_and_append_grows_path() {
        let seg = new_segment();
        assert_eq!(seg.len(), 6);
        assert!(seg.chars().all(|c| c.is_ascii_alphanumeric()));

        let child = append("abc123");
        assert!(
            child.starts_with("abc123."),
            "child must extend the parent: {child}"
        );
        assert_eq!(child.matches('.').count(), 1);
        assert_eq!(child.len(), "abc123.".len() + 6);
    }
}
