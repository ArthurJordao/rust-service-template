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

/// Install a JSON tracing subscriber. Idempotent: a second call is a no-op.
pub fn init_tracing(env_filter: &str) {
    let filter = EnvFilter::try_new(env_filter).unwrap_or_else(|_| EnvFilter::new("info"));
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

/// axum middleware: extract or mint a correlation id, attach it to the request
/// (extensions), run the rest of the stack inside a span carrying the cid, and
/// echo the cid on the response.
pub async fn correlation_id_middleware(mut req: Request, next: Next) -> Response {
    let cid = req
        .headers()
        .get(CORRELATION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(new_segment);

    req.extensions_mut().insert(CorrelationId(cid.clone()));

    let span = tracing::info_span!("request", cid = %cid);
    let mut res = next.run(req).instrument(span).await;

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
