use axum::{extract::Request, middleware::Next, response::Response};
use http::{HeaderName, HeaderValue};
use tracing::Instrument;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub const CORRELATION_ID_HEADER: &str = "x-correlation-id";

/// A correlation id carried through a request and into spawned event handlers.
#[derive(Debug, Clone)]
pub struct CorrelationId(pub String);

pub fn new_correlation_id() -> String {
    uuid::Uuid::new_v4().to_string()
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
        .unwrap_or_else(new_correlation_id);

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
            .unwrap_or_else(|| CorrelationId(new_correlation_id())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_non_empty_cid() {
        let cid = new_correlation_id();
        assert_eq!(cid.len(), 36); // uuid v4 hyphenated
    }
}
