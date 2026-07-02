use crate::observability::CORRELATION_ID_HEADER;
use serde::de::DeserializeOwned;
use std::time::Duration;

#[derive(Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    pub fn new() -> HttpClient {
        HttpClient::with_timeouts(CONNECT_TIMEOUT, REQUEST_TIMEOUT)
    }

    pub fn with_timeouts(connect: Duration, total: Duration) -> HttpClient {
        let inner = reqwest::Client::builder()
            .connect_timeout(connect)
            .timeout(total)
            .build()
            .expect("build reqwest client");
        HttpClient { inner }
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        url: &str,
        cid: Option<&str>,
    ) -> anyhow::Result<T> {
        let mut req = self.inner.get(url);
        if let Some(cid) = cid {
            req = req.header(CORRELATION_ID_HEADER, cid);
        }
        let resp = req.send().await?.error_for_status()?;
        Ok(resp.json::<T>().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn constructs_client() {
        let _c = HttpClient::new();
    }

    #[tokio::test]
    async fn request_times_out_against_a_hung_server() {
        // A server that accepts the connection but never responds.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _accepted = listener.accept().await;
            tokio::time::sleep(Duration::from_secs(30)).await;
        });

        let client =
            HttpClient::with_timeouts(Duration::from_millis(200), Duration::from_millis(200));
        let url = format!("http://{addr}/");
        let result: anyhow::Result<serde_json::Value> = client.get_json(&url, None).await;
        assert!(result.is_err(), "expected a timeout error, got Ok");
    }
}
