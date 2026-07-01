use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry, TextEncoder,
};

#[derive(Clone)]
pub struct Metrics {
    pub http_requests: IntCounterVec,
    http_duration: HistogramVec,
    registry: Registry,
}

impl Metrics {
    pub fn new() -> anyhow::Result<Metrics> {
        let registry = Registry::new();
        let http_requests = IntCounterVec::new(
            Opts::new("http_requests_total", "Total HTTP requests"),
            &["method", "path", "status"],
        )?;
        let http_duration = HistogramVec::new(
            HistogramOpts::new(
                "http_request_duration_seconds",
                "HTTP request latency in seconds",
            ),
            &["method", "path", "status"],
        )?;
        registry.register(Box::new(http_requests.clone()))?;
        registry.register(Box::new(http_duration.clone()))?;
        Ok(Metrics {
            http_requests,
            http_duration,
            registry,
        })
    }

    pub fn record_http(&self, method: &str, path: &str, status: u16) {
        self.http_requests
            .with_label_values(&[method, path, &status.to_string()])
            .inc();
    }

    pub fn observe_http(&self, method: &str, path: &str, status: u16, secs: f64) {
        self.http_duration
            .with_label_values(&[method, path, &status.to_string()])
            .observe(secs);
    }

    pub fn render(&self) -> String {
        let encoder = TextEncoder::new();
        let mut buf = Vec::new();
        let families = self.registry.gather();
        let _ = encoder.encode(&families, &mut buf);
        String::from_utf8(buf).unwrap_or_default()
    }
}

/// axum middleware: record the request count + latency, labeling `path` with the
/// matched route template (e.g. `/items/:id`) to bound label cardinality. Apply
/// with `route_layer` (or `layer`) so `MatchedPath` is populated by routing.
pub async fn track_metrics(State(metrics): State<Metrics>, req: Request, next: Next) -> Response {
    let method = req.method().as_str().to_owned();
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());
    let start = std::time::Instant::now();
    let res = next.run(req).await;
    let status = res.status().as_u16();
    metrics.record_http(&method, &path, status);
    metrics.observe_http(&method, &path, status, start.elapsed().as_secs_f64());
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[test]
    fn records_and_renders() {
        let m = Metrics::new().unwrap();
        m.record_http("GET", "/accounts", 200);
        let out = m.render();
        assert!(out.contains("http_requests_total"));
        assert!(out.contains("/accounts"));
    }

    #[tokio::test]
    async fn middleware_labels_with_matched_path_template() {
        let metrics = Metrics::new().unwrap();
        let app = Router::new()
            .route("/items/:id", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                metrics.clone(),
                track_metrics,
            ));

        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/items/42")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let _ = res.into_body().collect().await;

        let out = metrics.render();
        assert!(
            out.contains("/items/:id"),
            "want matched-path template: {out}"
        );
        assert!(
            !out.contains("/items/42"),
            "raw path must not be a label: {out}"
        );
        assert!(out.contains("http_request_duration_seconds"));
    }
}
