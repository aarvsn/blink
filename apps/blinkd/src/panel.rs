//! HTTP client for panel callbacks.
//!
//! The daemon phones home to:
//! * `GET  {appUrl}/api/servers/:id/config`   — fetch unit config (3 retries)
//! * `GET  {appUrl}/api/servers/:id/validate/:token` — validate browser tokens
//! * `GET  {appUrl}/api/servers/:id/cargo-files` — protected file metadata
//!
//! Unlike the original TypeScript daemon, every panel request carries the
//! node's `X-API-Key` so the panel can distinguish real nodes from strangers.

use crate::error::{AppError, AppResult};
use crate::models::{CargoFilesResponse, ServerConfig, ValidateResponse};
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct PanelClient {
    http: Client,
    app_url: String,
    api_key: String,
}

impl PanelClient {
    pub fn new(app_url: String, api_key: String) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        PanelClient { http, app_url, api_key }
    }

    fn auth_headers(&self, token: Option<&str>) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&self.api_key) {
            headers.insert("X-API-Key", v);
        }
        if let Some(token) = token {
            if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
                headers.insert(reqwest::header::AUTHORIZATION, v);
            }
        }
        headers
    }

    /// Fetch the full server configuration, retrying up to 3 times.
    pub async fn fetch_server_config(&self, server_id: &str) -> AppResult<ServerConfig> {
        let url = format!("{}/api/servers/{}/config", self.app_url, server_id);
        let mut last_error = String::new();

        for attempt in 1..=3u32 {
            match self
                .http
                .get(&url)
                .headers(self.auth_headers(None))
                .timeout(Duration::from_secs(10))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    return resp.json::<ServerConfig>().await.map_err(|e| {
                        AppError::BadGateway(format!("invalid server config from panel: {e}"))
                    });
                }
                Ok(resp) => {
                    last_error = format!("panel returned status {}", resp.status());
                }
                Err(e) => {
                    last_error = e.to_string();
                }
            }
            tracing::warn!(
                "failed to fetch server configuration (attempt {attempt}/3): {last_error}"
            );
            tokio::time::sleep(Duration::from_millis(1000 * attempt as u64)).await;
        }

        Err(AppError::BadGateway(format!(
            "failed to fetch server configuration after 3 attempts: {last_error}"
        )))
    }

    /// Validate a browser token with the panel. Returns `None` on any failure.
    pub async fn validate_token(&self, server_id: &str, token: &str) -> Option<ValidateResponse> {
        let url = format!("{}/api/servers/{}/validate/{}", self.app_url, server_id, token);
        let resp = self
            .http
            .get(&url)
            .headers(self.auth_headers(Some(token)))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json::<ValidateResponse>().await.ok()
    }

    /// Fetch protected cargo file metadata for a server (token-gated).
    pub async fn fetch_cargo_files(&self, server_id: &str, token: &str) -> CargoFilesResponse {
        let url = format!("{}/api/servers/{}/cargo-files?token={}", self.app_url, server_id, token);
        match self
            .http
            .get(&url)
            .headers(self.auth_headers(Some(token)))
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => resp.json().await.unwrap_or(CargoFilesResponse {
                cargo_files: vec![],
            }),
            _ => CargoFilesResponse { cargo_files: vec![] },
        }
    }

    /// Stream-download an arbitrary (usually signed cargo) URL to a file.
    pub async fn download_to(&self, url: &str, target: &std::path::Path) -> AppResult<u64> {
        use tokio::io::AsyncWriteExt;

        let resp = self
            .http
            .get(url)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| AppError::BadGateway(format!("failed to download cargo file: {e}")))?;

        if !resp.status().is_success() {
            return Err(AppError::BadGateway(format!(
                "cargo download returned status {}",
                resp.status()
            )));
        }

        let mut file = tokio::fs::File::create(target).await?;
        let mut written: u64 = 0;
        let mut stream = resp.bytes_stream();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| AppError::BadGateway(format!("cargo download failed: {e}")))?;
            file.write_all(&chunk).await?;
            written += chunk.len() as u64;
        }
        file.flush().await?;
        Ok(written)
    }
}

/// TTL cache for panel token validations, keyed by sha256(server:token).
pub struct ValidationCache {
    ttl: Duration,
    map: Mutex<HashMap<String, (ValidateResponse, Instant)>>,
}

impl ValidationCache {
    pub fn new(ttl_secs: u64) -> Self {
        ValidationCache {
            ttl: Duration::from_secs(ttl_secs),
            map: Mutex::new(HashMap::new()),
        }
    }

    fn key(server_id: &str, token: &str) -> String {
        let digest = Sha256::digest(format!("{server_id}:{token}").as_bytes());
        hex(&digest)
    }

    pub async fn get(&self, server_id: &str, token: &str) -> Option<ValidateResponse> {
        let map = self.map.lock().await;
        let key = Self::key(server_id, token);
        map.get(&key).and_then(|(v, at)| {
            if at.elapsed() < self.ttl {
                Some(v.clone())
            } else {
                None
            }
        })
    }

    pub async fn put(&self, server_id: &str, token: &str, validation: ValidateResponse) {
        let mut map = self.map.lock().await;
        let key = Self::key(server_id, token);
        if map.len() > 10_000 {
            map.clear(); // crude but effective upper bound
        }
        map.insert(key, (validation, Instant::now()));
    }

    pub async fn invalidate(&self, server_id: &str, token: &str) {
        let mut map = self.map.lock().await;
        map.remove(&Self::key(server_id, token));
    }

    /// Periodic cleanup task.
    pub fn spawn_cleanup(self: &Arc<Self>) {
        let cache = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let mut map = cache.map.lock().await;
                map.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(3600));
            }
        });
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}
