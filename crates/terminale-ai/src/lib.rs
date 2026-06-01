//! AI provider abstraction for `terminale`.
//!
//! Goal: every provider (Claude, OpenAI, Ollama, …) implements the same
//! [`AiProvider`] trait so the rest of the workspace stays unaware of
//! which backend is in use. New providers add one file in this crate.
//!
//! Streaming is first-class — `AiProvider::stream` returns an async
//! channel of incremental text chunks so the UI can display tokens as
//! they arrive instead of waiting for the full response.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod claude;
pub mod ollama;
pub mod openai;
pub mod suggestion;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;

pub use claude::ClaudeProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAiProvider;
pub use suggestion::{extract_suggested_command, suggestion_messages};

/// Build a boxed provider from a provider name. `secret` is the API key
/// for cloud providers (ignored by Ollama); `ollama_url` is the daemon
/// endpoint (ignored by cloud providers). Unknown names default to
/// Claude. Keeps provider construction in one place so callers don't
/// match on concrete types.
#[must_use]
pub fn build_provider(provider: &str, secret: String, ollama_url: String) -> Box<dyn AiProvider> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "openai" => Box::new(OpenAiProvider::new(secret)),
        "ollama" => Box::new(OllamaProvider::new().with_base_url(ollama_url)),
        _ => Box::new(ClaudeProvider::new(secret)),
    }
}

/// One message in a multi-turn conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiMessage {
    /// `"user"` / `"assistant"` / `"system"`.
    pub role: String,
    /// Free-form text content. Multi-modal payloads belong in a future
    /// `parts: Vec<Part>` field.
    pub content: String,
}

impl AiMessage {
    /// Build a user-role message.
    #[must_use]
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: text.into(),
        }
    }
    /// Build an assistant-role message.
    #[must_use]
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: text.into(),
        }
    }
    /// Build a system-role message.
    #[must_use]
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: text.into(),
        }
    }
}

/// What the host sends to a provider for inference.
#[derive(Debug, Clone)]
pub struct AiRequest {
    /// Provider-specific model name (e.g. `"claude-opus-4-7"`, `"gpt-4o"`).
    pub model: String,
    /// Conversation so far (oldest first). Provider adapters may rewrite
    /// or drop messages to fit their schema (e.g. fold `system` into a
    /// top-level field).
    pub messages: Vec<AiMessage>,
    /// Soft cap on output tokens. `None` = let the provider decide.
    pub max_tokens: Option<u32>,
    /// Sampling temperature in `[0.0, 2.0]`. `None` = provider default.
    pub temperature: Option<f32>,
}

impl AiRequest {
    /// Quick one-shot user-message request.
    #[must_use]
    pub fn one_shot(model: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            messages: vec![AiMessage::user(prompt)],
            max_tokens: None,
            temperature: None,
        }
    }
}

/// One increment in a streamed response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamChunk {
    /// A piece of generated text. Concatenate to form the full reply.
    Text(String),
    /// The stream ended cleanly.
    Done,
    /// An error occurred mid-stream. The channel will also be closed.
    Error(String),
}

/// Errors a provider can fail with.
#[derive(Debug, Error)]
pub enum AiError {
    /// HTTP layer failed (timeout, DNS, TLS, …).
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    /// Provider returned a non-2xx status with the given body.
    #[error("provider HTTP {status}: {body}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Response body (truncated to 4 KiB).
        body: String,
    },
    /// JSON decoding of the response failed.
    #[error("decode error: {0}")]
    Decode(#[from] serde_json::Error),
    /// Provider configuration is missing or invalid (e.g. no API key).
    #[error("misconfigured: {0}")]
    Misconfigured(String),
}

/// Common surface every provider implements.
#[async_trait]
pub trait AiProvider: Send + Sync + 'static {
    /// Short, human-readable provider name (`"claude"`, `"openai"`, …).
    fn name(&self) -> &'static str;

    /// One-shot non-streaming inference. Default impl drives [`Self::stream`]
    /// and concatenates the chunks, so providers only need to implement
    /// streaming.
    async fn complete(&self, req: AiRequest) -> Result<String, AiError> {
        let mut rx = self.stream(req).await?;
        let mut out = String::new();
        while let Some(chunk) = rx.recv().await {
            match chunk {
                StreamChunk::Text(t) => out.push_str(&t),
                StreamChunk::Done => break,
                StreamChunk::Error(e) => return Err(AiError::Misconfigured(e)),
            }
        }
        Ok(out)
    }

    /// Stream-based inference. The returned receiver is closed when the
    /// provider sends [`StreamChunk::Done`] or [`StreamChunk::Error`].
    async fn stream(&self, req: AiRequest) -> Result<mpsc::Receiver<StreamChunk>, AiError>;
}
