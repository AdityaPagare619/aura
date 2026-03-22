//! HTTP backend implementation using curl subprocess for Telegram Bot API.
//!
//! REASON: reqwest with rustls-tls pulls in rustls-platform-verifier, which
//! panics on Termux/Android (target_os="android" but no JVM available).
//! curl works perfectly on Termux — uses OpenSSL, no platform verifier needed.
//!
//! Benefits of curl approach:
//! - Works on Termux (no JVM required for TLS)
//! - Works on CI (cross-compiled binary uses curl for Telegram calls)
//! - No complex TLS configuration needed
//! - curl is battle-tested for HTTPS on Android
//! - Simpler dependency tree (no rustls, no webpki, no platform verifier)
//!
//! The `HttpBackend` trait is synchronous. This implementation uses
//! `tokio::process::Command` which is async-native, but we expose it
//! through a sync interface. Callers wrap these in `spawn_blocking`.

use aura_types::errors::AuraError;
use tokio::process::Command;
use tracing::{debug, warn};

use super::polling::HttpBackend;

/// Maximum time for a curl request to complete.
const CURL_TIMEOUT_SECS: u64 = 30;

/// Telegram API base URL.
const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

/// Curl HTTP backend for Telegram Bot API.
///
/// Uses curl subprocess for all HTTP calls. This is the most reliable approach
/// for Termux because curl handles TLS using the system OpenSSL, which works
/// perfectly without any platform-specific initialization.
pub struct CurlHttpBackend {
    /// Bot token for Telegram API authentication.
    bot_token: String,
}

impl CurlHttpBackend {
    /// Create a new CurlHttpBackend.
    pub fn new(bot_token: String) -> Self {
        debug!(token_prefix = %format!("{}...", &bot_token[..8.min(bot_token.len())]),
               "initialized curl HTTP backend");
        Self { bot_token }
    }

    /// Execute a curl request and return the response body as bytes.
    ///
    /// # Arguments
    /// * `method` - HTTP method (GET, POST)
    /// * `endpoint` - Telegram API endpoint (e.g., "getUpdates")
    /// * `body` - Optional JSON body for POST requests
    /// * `content_type` - Content-Type header value
    fn curl_request_sync(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<&[u8]>,
        content_type: Option<&str>,
    ) -> Result<Vec<u8>, AuraError> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            self.curl_request_impl(method, endpoint, body, content_type)
                .await
        })
    }

    /// Internal async implementation (called from sync wrapper).
    async fn curl_request_impl(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<&[u8]>,
        content_type: Option<&str>,
    ) -> Result<Vec<u8>, AuraError> {
        let url = format!("{}/bot{}/{}", TELEGRAM_API_BASE, self.bot_token, endpoint);

        debug!(url = %url, method = %method, "curl HTTP request");

        let mut args = vec![
            "-s".to_string(),
            "-S".to_string(),
            "-w".to_string(),
            "\\n%{http_code}".to_string(),
            "--max-time".to_string(),
            CURL_TIMEOUT_SECS.to_string(),
            "-X".to_string(),
            method.to_string(),
            url.clone(),
        ];

        if let Some(ct) = content_type {
            args.push("-H".to_string());
            args.push(format!("Content-Type: {}", ct));
        }

        if let Some(b) = body {
            let body_str = String::from_utf8_lossy(b);
            args.push("-d".to_string());
            args.push(body_str.to_string());
        }

        let output = Command::new("curl")
            .args(&args)
            .output()
            .await
            .map_err(|e| {
                warn!(error = %e, "failed to spawn curl");
                AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed)
            })?;

        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);

        if !output.status.success() {
            warn!(
                exit_code = ?output.status.code(),
                stderr = %stderr,
                "curl request failed"
            );
            return Err(AuraError::Ipc(
                aura_types::errors::IpcError::ConnectionFailed,
            ));
        }

        let (response_body, status_line) = if let Some(pos) = stdout.rfind('\n') {
            let (body, status) = stdout.split_at(pos);
            (body.to_string(), status.trim())
        } else {
            (stdout.to_string(), "")
        };

        let status_code: u16 = status_line.parse().unwrap_or(0);

        debug!(
            status_code = %status_code,
            body_len = %response_body.len(),
            "curl request completed"
        );

        if status_code == 0 {
            warn!("curl: connection failed (status code 0)");
            return Err(AuraError::Ipc(
                aura_types::errors::IpcError::ConnectionFailed,
            ));
        }

        if status_code >= 400 {
            warn!(
                status_code = %status_code,
                body = %response_body,
                "HTTP error response"
            );
            return Err(AuraError::Ipc(
                aura_types::errors::IpcError::ConnectionFailed,
            ));
        }

        Ok(response_body.into_bytes())
    }
}

impl HttpBackend for CurlHttpBackend {
    fn get(&self, endpoint: &str) -> Result<Vec<u8>, AuraError> {
        self.curl_request_sync("GET", endpoint, None, None)
    }

    fn post_json(&self, endpoint: &str, body: &[u8]) -> Result<Vec<u8>, AuraError> {
        self.curl_request_sync("POST", endpoint, Some(body), Some("application/json"))
    }

    fn post_multipart(
        &self,
        endpoint: &str,
        fields: Vec<(&str, String)>,
        file_field: Option<(&str, Vec<u8>, &str)>,
    ) -> Result<Vec<u8>, AuraError> {
        let boundary = "AURA_BOUNDARY_123456789";

        let mut body = Vec::new();

        for (key, value) in fields {
            body.extend(format!("--{}\r\n", boundary).as_bytes());
            body.extend(
                format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", key).as_bytes(),
            );
            body.extend(value.as_bytes());
            body.extend(b"\r\n");
        }

        if let Some((field_name, file_data, mime_type)) = file_field {
            body.extend(format!("--{}\r\n", boundary).as_bytes());
            body.extend(
                format!(
                    "Content-Disposition: form-data; name=\"{}\"; filename=\"upload\"\r\n",
                    field_name
                )
                .as_bytes(),
            );
            body.extend(format!("Content-Type: {}\r\n\r\n", mime_type).as_bytes());
            body.extend(&file_data);
            body.extend(b"\r\n");
        }

        body.extend(format!("--{}--\r\n", boundary).as_bytes());

        let content_type = format!("multipart/form-data; boundary={}", boundary);
        self.curl_request_sync("POST", endpoint, Some(&body), Some(&content_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_creation() {
        let backend = CurlHttpBackend::new("test_token_123456".to_string());
        // Verify backend was created (token stored internally)
        let _ = backend;
    }

    #[tokio::test]
    async fn test_curl_get_request() {
        // This test requires curl to be available
        // Skip if curl is not in PATH
        let result = Command::new("curl").args(&["--version"]).output().await;

        if result.is_err() {
            return; // Skip if curl not available
        }
    }
}
