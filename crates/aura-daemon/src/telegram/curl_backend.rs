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
//! `std::process::Command` for fully synchronous curl calls.
//! No tokio runtime access needed — safe to call from spawn_blocking.
//!
//! IMPORTANT: TelegramPoller passes FULL URLs (https://api.telegram.org/botTOKEN/endpoint).
//! This backend handles both:
//!   - Full URLs (already include base + token): use as-is
//!   - Relative endpoints (getUpdates, sendMessage): prepend base + token

use std::process::Command;

use aura_types::errors::AuraError;
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
    /// This is a SYNCHRONOUS function. It uses `std::process::Command`
    /// directly, with NO tokio runtime access. This is safe to call from
    /// any context including `spawn_blocking` closures.
    ///
    /// Handles both full URLs (from TelegramPoller which already includes
    /// base + token) and relative endpoints (for direct API calls).
    ///
    /// # Arguments
    /// * `method` - HTTP method (GET, POST)
    /// * `endpoint` - Either a full URL or a relative endpoint path.
    ///   TelegramPoller always passes full URLs with base + token already.
    /// * `body` - Optional JSON body for POST requests
    /// * `content_type` - Content-Type header value
    fn curl_request_sync(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<&[u8]>,
        content_type: Option<&str>,
    ) -> Result<Vec<u8>, AuraError> {
        // If endpoint is already a full URL, use it as-is.
        // TelegramPoller::get_updates passes full URLs like:
        //   https://api.telegram.org/botTOKEN/getUpdates?offset=0&timeout=30...
        // In that case, self.bot_token is NOT prepended again.
        let url = if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
            endpoint.to_string()
        } else {
            format!("{}/bot{}/{}", TELEGRAM_API_BASE, self.bot_token, endpoint)
        };

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
            url,
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

        // Use std::process::Command — fully synchronous, no tokio runtime needed.
        // This is safe to call from spawn_blocking or any thread context.
        let output = Command::new("curl").args(&args).output().map_err(|e| {
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
        fields: Vec<(String, String)>,
        file_field: Option<(String, Vec<u8>, String)>,
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
        use std::process::Command as StdCommand;
        let result = StdCommand::new("curl").args(&["--version"]).output();

        if result.is_err() {
            return; // Skip if curl not available
        }
    }
}
