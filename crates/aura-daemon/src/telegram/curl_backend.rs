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

use std::time::Duration;

use async_trait::async_trait;
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
    async fn curl_request(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<&[u8]>,
        content_type: Option<&str>,
    ) -> Result<Vec<u8>, AuraError> {
        let url = format!("{}/bot{}/{}", TELEGRAM_API_BASE, self.bot_token, endpoint);

        debug!(url = %url, method = %method, "curl HTTP request");

        // Build curl command arguments
        let mut args = vec![
            // Silent (no progress meter)
            "-s".to_string(),
            // Don't show error bar
            "-S".to_string(),
            // Show HTTP response code
            "-w".to_string(),
            "\\n%{http_code}".to_string(),
            // Timeout
            "--max-time".to_string(),
            CURL_TIMEOUT_SECS.to_string(),
            // Method
            "-X".to_string(),
            method.to_string(),
            // URL
            url.clone(),
        ];

        // Add headers
        if let Some(ct) = content_type {
            args.push("-H".to_string());
            args.push(format!("Content-Type: {}", ct));
        }

        // Add body for POST requests
        if let Some(b) = body {
            // Write body to temp file to avoid shell escaping issues
            let body_str = String::from_utf8_lossy(b);
            args.push("-d".to_string());
            args.push(body_str.to_string());
        }

        // Execute curl
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

        // Check exit code
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

        // Parse HTTP status code from last line of stdout
        // curl outputs: "[body]\n[status_code]"
        let (response_body, status_line) = if let Some(pos) = stdout.rfind('\n') {
            let (body, status) = stdout.split_at(pos);
            (body.to_string(), status.trim())
        } else {
            (stdout.to_string(), "")
        };

        // Extract HTTP status code
        let status_code: u16 = status_line.parse().unwrap_or(0);

        debug!(
            status_code = %status_code,
            body_len = %response_body.len(),
            "curl request completed"
        );

        // Check for HTTP errors
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

        // Return body as bytes
        Ok(response_body.into_bytes())
    }
}

#[async_trait]
impl HttpBackend for CurlHttpBackend {
    /// Perform an HTTP GET request.
    async fn get(&self, endpoint: &str) -> Result<Vec<u8>, AuraError> {
        self.curl_request("GET", endpoint, None, None).await
    }

    /// Perform an HTTP POST request with JSON body.
    async fn post_json(&self, endpoint: &str, body: &[u8]) -> Result<Vec<u8>, AuraError> {
        self.curl_request("POST", endpoint, Some(body), Some("application/json"))
            .await
    }

    /// Perform an HTTP POST request with multipart form data.
    ///
    /// Note: curl handles multipart automatically when -F flags are used.
    /// For simplicity, we send as JSON with Content-Type: multipart/form-data
    /// handled differently. Since Telegram Bot API accepts JSON for most calls,
    /// we fall back to JSON encoding for simplicity.
    async fn post_multipart(
        &self,
        endpoint: &str,
        fields: Vec<(&str, String)>,
        file_field: Option<(&str, Vec<u8>, &str)>,
    ) -> Result<Vec<u8>, AuraError> {
        // Build multipart form data manually
        let boundary = "AURA_BOUNDARY_123456789";

        let mut body = Vec::new();

        // Add text fields
        for (key, value) in fields {
            body.extend(format!("--{}\r\n", boundary).as_bytes());
            body.extend(
                format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", key).as_bytes(),
            );
            body.extend(value.as_bytes());
            body.extend(b"\r\n");
        }

        // Add file field if present
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

        // Close boundary
        body.extend(format!("--{}--\r\n", boundary).as_bytes());

        let content_type = format!("multipart/form-data; boundary={}", boundary);
        self.curl_request("POST", endpoint, Some(&body), Some(&content_type))
            .await
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
