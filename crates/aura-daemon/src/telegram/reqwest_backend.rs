//! Real HTTP backend implementation using reqwest for Telegram Bot API.
//!
//! This replaces the stub `StubHttpBackend` for production use.
//!
//! The `HttpBackend` trait is synchronous. This implementation wraps reqwest's
//! async HTTP calls in `tokio::task::spawn_blocking` at the call sites
//! in `polling.rs`. Here we implement the sync trait directly.
//!
//! The reqwest client is async-native, so we spawn a dedicated tokio runtime
//! per request. This is correct and safe to call from spawn_blocking.

use std::time::Duration;

use aura_types::errors::AuraError;
use reqwest::multipart;
use tokio::runtime::Runtime;
use tracing::{debug, warn};

use super::polling::HttpBackend;

pub struct ReqwestHttpBackend {
    client: reqwest::Client,
    base_url: String,
}

impl ReqwestHttpBackend {
    pub fn new(bot_token: &str) -> Self {
        let base_url = format!("https://api.telegram.org/bot{}", bot_token);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client build");

        debug!(base_url = %base_url, "initialized reqwest HTTP backend");

        Self { client, base_url }
    }

    fn build_url(&self, url: &str) -> String {
        if url.starts_with("http") {
            url.to_string()
        } else {
            format!("{}/{}", self.base_url, url.trim_start_matches('/'))
        }
    }
}

impl HttpBackend for ReqwestHttpBackend {
    fn get(&self, url: &str) -> Result<Vec<u8>, AuraError> {
        let full_url = self.build_url(url);
        let client = self.client.clone();

        debug!(url = %full_url, "GET request");

        // Spawn a dedicated runtime per request. Safe to call from spawn_blocking.
        Runtime::new()
            .map_err(|_| AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed))?
            .block_on(async {
                let response = client.get(&full_url).send().await.map_err(|e| {
                    warn!(error = %e, url = %full_url, "HTTP GET failed");
                    AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed)
                })?;

                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    warn!(status = %status, body = %body, "HTTP GET returned error");
                    return Err(AuraError::Ipc(
                        aura_types::errors::IpcError::ConnectionFailed,
                    ));
                }

                let bytes = response.bytes().await.map_err(|e| {
                    warn!(error = %e, "failed to read response body");
                    AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed)
                })?;

                debug!(bytes = bytes.len(), "GET request successful");
                Ok(bytes.to_vec())
            })
    }

    fn post_json(&self, url: &str, body: &[u8]) -> Result<Vec<u8>, AuraError> {
        let full_url = self.build_url(url);
        let client = self.client.clone();

        debug!(url = %full_url, bytes = body.len(), "POST JSON request");

        // Spawn a dedicated runtime per request. Safe to call from spawn_blocking.
        Runtime::new()
            .map_err(|_| AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed))?
            .block_on(async {
                let response = client
                    .post(&full_url)
                    .header("Content-Type", "application/json")
                    .body(body.to_vec())
                    .send()
                    .await
                    .map_err(|e| {
                        warn!(error = %e, url = %full_url, "HTTP POST failed");
                        AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed)
                    })?;

                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    warn!(status = %status, body = %body, "HTTP POST returned error");
                    return Err(AuraError::Ipc(
                        aura_types::errors::IpcError::ConnectionFailed,
                    ));
                }

                let bytes = response.bytes().await.map_err(|e| {
                    warn!(error = %e, "failed to read response body");
                    AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed)
                })?;

                debug!(bytes = bytes.len(), "POST JSON request successful");
                Ok(bytes.to_vec())
            })
    }

    fn post_multipart(
        &self,
        url: &str,
        fields: Vec<(String, String)>,
        file_field: Option<(String, Vec<u8>, String)>,
    ) -> Result<Vec<u8>, AuraError> {
        let full_url = self.build_url(url);
        let client = self.client.clone();

        debug!(url = %full_url, fields = fields.len(), has_file = file_field.is_some(), "POST multipart request");

        // Spawn a dedicated runtime per request. Safe to call from spawn_blocking.
        Runtime::new()
            .map_err(|_| AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed))?
            .block_on(async {
                let mut form = multipart::Form::new();

                for (key, value) in fields {
                    form = form.text(key, value);
                }

                if let Some((field_name, file_data, mime_type)) = file_field {
                    let part = multipart::Part::bytes(file_data)
                        .mime_str(&mime_type)
                        .map_err(|e| {
                            warn!(error = %e, "failed to create multipart part");
                            AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed)
                        })?;
                    form = form.part(field_name, part);
                }

                let response = client
                    .post(&full_url)
                    .multipart(form)
                    .send()
                    .await
                    .map_err(|e| {
                        warn!(error = %e, url = %full_url, "HTTP multipart POST failed");
                        AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed)
                    })?;

                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    warn!(status = %status, body = %body, "HTTP multipart POST returned error");
                    return Err(AuraError::Ipc(
                        aura_types::errors::IpcError::ConnectionFailed,
                    ));
                }

                let bytes = response.bytes().await.map_err(|e| {
                    warn!(error = %e, "failed to read multipart response body");
                    AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed)
                })?;

                debug!(bytes = bytes.len(), "POST multipart request successful");
                Ok(bytes.to_vec())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_creation() {
        let backend = ReqwestHttpBackend::new("test_token");
        assert!(backend.base_url.contains("test_token"));
    }

    #[test]
    fn test_build_url_absolute() {
        let backend = ReqwestHttpBackend::new("test_token");
        let url = backend.build_url("https://other.com/api");
        assert_eq!(url, "https://other.com/api");
    }

    #[test]
    fn test_build_url_relative() {
        let backend = ReqwestHttpBackend::new("test_token");
        let url = backend.build_url("getUpdates");
        assert!(url.contains("getUpdates"));
    }

    #[test]
    fn test_build_url_trim_slash() {
        let backend = ReqwestHttpBackend::new("test_token");
        let url = backend.build_url("/getUpdates");
        assert!(url.contains("getUpdates"));
    }
}
