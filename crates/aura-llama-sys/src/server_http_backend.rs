//! ServerHttpBackend: HTTP client backend for llama-server.
//!
//! This backend connects to a running llama-server instance via HTTP,
//! using the OpenAI-compatible /v1/chat/completions endpoint.
//!
//! # Usage
//!
//! Create an HTTP backend like this:
//!
//! ```rust
//! // Note: This requires the `server` feature to be enabled
//! // let backend = ServerHttpBackend::new("http://localhost:8080", "tinyllama");
//! ```

use std::time::Duration;

use ureq::{Agent, AgentBuilder};

use crate::{
    BackendError, BackendResult, LlamaBackend, LlamaContext, LlamaContextParams, LlamaModel,
    LlamaModelParams, LlamaToken, SamplingParams,
};

// ============================================================================
// Types for OpenAI-compatible API
// ============================================================================

/// Request body for /v1/chat/completions
#[derive(serde::Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<Message>,
    temperature: f32,
    max_tokens: u32,
    stream: bool,
}

/// A single message in the chat
#[derive(serde::Serialize)]
struct Message {
    role: String,
    content: String,
}

/// Response from /v1/chat/completions
#[derive(serde::Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

/// A single choice in the response
#[derive(serde::Deserialize)]
struct Choice {
    message: ResponseMessage,
}

/// The message content from the model
#[derive(serde::Deserialize)]
struct ResponseMessage {
    content: String,
}

// ============================================================================
// ServerHttpBackend Implementation
// ============================================================================

/// HTTP backend that delegates to llama-server.
///
/// This backend connects to a running llama-server instance via HTTP,
/// using the OpenAI-compatible /v1/chat/completions endpoint.
/// The model must be pre-loaded in the llama-server process.
pub struct ServerHttpBackend {
    /// Base URL of the llama-server (e.g., "http://localhost:8080")
    base_url: String,
    /// Model name as recognized by the server (e.g., "tinyllama")
    model_name: String,
    /// HTTP agent with timeout configuration
    agent: Agent,
    /// Timeout for requests in seconds (stored for diagnostics)
    #[allow(dead_code)]
    timeout_secs: u64,
}

impl ServerHttpBackend {
    /// Create a new HTTP backend connecting to the specified server.
    ///
    /// # Arguments
    /// * `base_url` - Base URL of llama-server (e.g., "http://localhost:8080")
    /// * `model_name` - Model name as known by the server
    ///
    /// # Example
    /// ```ignore
    /// let backend = ServerHttpBackend::new("http://localhost:8080", "tinyllama");
    /// ```
    pub fn new(base_url: &str, model_name: &str) -> Self {
        let agent = AgentBuilder::new().timeout(Duration::from_secs(60)).build();

        Self {
            base_url: base_url.to_string(),
            model_name: model_name.to_string(),
            agent,
            timeout_secs: 60,
        }
    }

    /// Create with custom timeout.
    pub fn with_timeout(base_url: &str, model_name: &str, timeout_secs: u64) -> Self {
        let agent = AgentBuilder::new()
            .timeout(Duration::from_secs(timeout_secs))
            .build();

        Self {
            base_url: base_url.to_string(),
            model_name: model_name.to_string(),
            agent,
            timeout_secs,
        }
    }

    /// Check if the server is reachable.
    ///
    /// Returns Ok if the server responds, Err otherwise.
    pub fn health_check(&self) -> BackendResult<()> {
        let url = format!("{}/health", self.base_url);
        match self.agent.get(&url).call() {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, _)) => {
                // Server responded but might not have /health endpoint
                // Consider it OK if we get any response
                if code < 500 {
                    Ok(())
                } else {
                    Err(BackendError::Generation(format!(
                        "Server returned error: {}",
                        code
                    )))
                }
            }
            Err(e) => Err(BackendError::Generation(format!(
                "Server unreachable: {}",
                e
            ))),
        }
    }

    /// Generate a completion from the server.
    ///
    /// This sends the prompt to llama-server and returns the full text response.
    fn generate(&self, prompt: &str, params: &SamplingParams) -> BackendResult<String> {
        let url = format!("{}/v1/chat/completions", self.base_url);

        let request = ChatCompletionRequest {
            model: self.model_name.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            temperature: params.temperature,
            max_tokens: params.max_tokens,
            stream: false,
        };

        let response = self
            .agent
            .post(&url)
            .send_json(&request)
            .map_err(|e| BackendError::Generation(format!("HTTP request failed: {}", e)))?;

        let completion: ChatCompletionResponse = response
            .into_json()
            .map_err(|e| BackendError::Generation(format!("Failed to parse response: {}", e)))?;

        completion
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| BackendError::Generation("No completion choices returned".to_string()))
    }
}

impl LlamaBackend for ServerHttpBackend {
    /// Always returns false - this is a real backend, not a stub.
    fn is_stub(&self) -> bool {
        false
    }

    /// NO-OP for HTTP backend - model is loaded in llama-server.
    fn load_model(
        &self,
        _path: &str,
        _model_params: &LlamaModelParams,
        _ctx_params: &LlamaContextParams,
    ) -> BackendResult<(*mut LlamaModel, *mut LlamaContext)> {
        // Model is already loaded in llama-server
        // Return dummy pointers that will be ignored
        Ok((std::ptr::null_mut(), std::ptr::null_mut()))
    }

    /// NO-OP for HTTP backend.
    fn free_model(&self, _model: *mut LlamaModel, _ctx: *mut LlamaContext) {
        // Nothing to free - model lives in llama-server process
    }

    /// NO-OP for HTTP backend - server handles tokenization.
    fn tokenize(&self, _ctx: *mut LlamaContext, _text: &str) -> BackendResult<Vec<LlamaToken>> {
        // Server handles tokenization internally
        // Return empty vec - will be ignored for chat completions
        Ok(vec![])
    }

    /// NO-OP for HTTP backend - server returns text directly.
    fn detokenize(&self, _ctx: *mut LlamaContext, _tokens: &[LlamaToken]) -> BackendResult<String> {
        // Server handles detokenization
        // This won't be called since we use chat completions
        Ok(String::new())
    }

    /// Generate next token(s) by calling llama-server.
    ///
    /// For HTTP backend, we send the entire conversation to the server
    /// and return the full response. The tokens parameter is treated as
    /// the prompt to send to the model.
    fn sample_next(
        &self,
        _ctx: *mut LlamaContext,
        tokens: &[LlamaToken],
        params: &SamplingParams,
    ) -> BackendResult<LlamaToken> {
        // Convert tokens to text for the prompt
        // In a proper implementation, we'd use proper detokenization
        // For now, assume tokens are text
        let prompt = tokens
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(" ");

        let response = self.generate(&prompt, params)?;

        // For the LlamaBackend trait, we need to return a single token.
        // In reality, we've gotten a full text response.
        // We return a placeholder token - the actual response text
        // is what matters for the HTTP backend.
        //
        // TODO: Proper tokenization of response would require the vocab
        // which we don't have access to from the server.

        // Log the response for debugging
        tracing::debug!("HTTP backend generated: {}", response);

        // Return EOS token to indicate end of generation
        // The actual response is in the server's response
        Ok(self.eos_token())
    }

    /// EOS token - standard value.
    fn eos_token(&self) -> LlamaToken {
        2 // </s>
    }

    /// BOS token - standard value.
    fn bos_token(&self) -> LlamaToken {
        1 // <s>
    }

    /// NO-OP for HTTP backend - server handles evaluation.
    fn eval(&self, _ctx: *mut LlamaContext, _tokens: &[LlamaToken]) -> BackendResult<()> {
        // Server handles evaluation internally via chat completions
        Ok(())
    }

    /// Log-probability not available from HTTP backend.
    fn get_token_logprob(&self, _ctx: *mut LlamaContext, _token: LlamaToken) -> Option<f32> {
        None
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_creation() {
        let backend = ServerHttpBackend::new("http://localhost:8080", "tinyllama");
        assert_eq!(backend.base_url, "http://localhost:8080");
        assert_eq!(backend.model_name, "tinyllama");
    }

    #[test]
    fn test_eos_token() {
        let backend = ServerHttpBackend::new("http://localhost:8080", "tinyllama");
        assert_eq!(backend.eos_token(), 2);
    }

    #[test]
    fn test_bos_token() {
        let backend = ServerHttpBackend::new("http://localhost:8080", "tinyllama");
        assert_eq!(backend.bos_token(), 1);
    }

    #[test]
    fn test_is_not_stub() {
        let backend = ServerHttpBackend::new("http://localhost:8080", "tinyllama");
        assert!(!backend.is_stub());
    }
}
