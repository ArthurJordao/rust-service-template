use crate::observability::CORRELATION_ID_HEADER;
use serde::de::DeserializeOwned;

#[derive(Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    pub fn new() -> HttpClient {
        HttpClient { inner: reqwest::Client::new() }
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

    #[test]
    fn constructs_client() {
        let _c = HttpClient::new();
    }
}
