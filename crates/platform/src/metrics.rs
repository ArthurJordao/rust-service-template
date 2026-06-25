use prometheus::{Encoder, IntCounterVec, Opts, Registry, TextEncoder};

#[derive(Clone)]
pub struct Metrics {
    pub http_requests: IntCounterVec,
    registry: Registry,
}

impl Metrics {
    pub fn new() -> anyhow::Result<Metrics> {
        let registry = Registry::new();
        let http_requests = IntCounterVec::new(
            Opts::new("http_requests_total", "Total HTTP requests"),
            &["method", "path", "status"],
        )?;
        registry.register(Box::new(http_requests.clone()))?;
        Ok(Metrics {
            http_requests,
            registry,
        })
    }

    pub fn record_http(&self, method: &str, path: &str, status: u16) {
        self.http_requests
            .with_label_values(&[method, path, &status.to_string()])
            .inc();
    }

    pub fn render(&self) -> String {
        let encoder = TextEncoder::new();
        let mut buf = Vec::new();
        let families = self.registry.gather();
        let _ = encoder.encode(&families, &mut buf);
        String::from_utf8(buf).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_renders() {
        let m = Metrics::new().unwrap();
        m.record_http("GET", "/accounts", 200);
        let out = m.render();
        assert!(out.contains("http_requests_total"));
        assert!(out.contains("/accounts"));
    }
}
