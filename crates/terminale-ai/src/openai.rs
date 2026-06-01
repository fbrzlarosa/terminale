//! OpenAI Chat Completions provider — minimal streaming implementation.

use crate::{AiError, AiProvider, AiRequest, StreamChunk};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

const OPENAI_URL: &str = "https://api.openai.com/v1/chat/completions";

/// OpenAI Chat Completions client.
#[derive(Debug, Clone)]
pub struct OpenAiProvider {
    api_key: String,
    http: reqwest::Client,
    base_url: String,
}

impl OpenAiProvider {
    /// Build a new client with the given API key.
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            http: reqwest::Client::new(),
            base_url: OPENAI_URL.to_string(),
        }
    }

    /// Override the endpoint for self-hosted gateways.
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }
}

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    messages: Vec<Msg>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Serialize)]
struct Msg {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct Frame {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Option<Delta>,
}

#[derive(Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
}

/// Parse one OpenAI SSE `data:` payload (the text after `data:`, trimmed)
/// into a stream chunk. Returns `None` for frames carrying no text delta
/// (role-only deltas, keep-alives, unknown shapes).
fn parse_openai_payload(payload: &str) -> Option<StreamChunk> {
    if payload == "[DONE]" {
        return Some(StreamChunk::Done);
    }
    let frame: Frame = serde_json::from_str(payload).ok()?;
    let text = frame.choices.into_iter().next()?.delta?.content?;
    if text.is_empty() {
        None
    } else {
        Some(StreamChunk::Text(text))
    }
}

#[async_trait]
impl AiProvider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn stream(&self, req: AiRequest) -> Result<mpsc::Receiver<StreamChunk>, AiError> {
        if self.api_key.is_empty() {
            return Err(AiError::Misconfigured("OPENAI_API_KEY is empty".into()));
        }
        let messages = req
            .messages
            .iter()
            .map(|m| Msg {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();
        let body = Body {
            model: &req.model,
            messages,
            stream: true,
            max_tokens: req.max_tokens,
            temperature: req.temperature,
        };
        let resp = self
            .http
            .post(&self.base_url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(AiError::Status { status, body });
        }
        let (tx, rx) = mpsc::channel::<StreamChunk>(64);
        tokio::spawn(async move {
            let mut s = resp.bytes_stream();
            let mut buf = String::new();
            while let Some(chunk) = s.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = tx.send(StreamChunk::Error(e.to_string())).await;
                        return;
                    }
                };
                buf.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(idx) = buf.find("\n\n") {
                    let frame: String = buf.drain(..idx).collect();
                    buf.drain(..2);
                    for line in frame.lines() {
                        let Some(payload) = line.strip_prefix("data:") else {
                            continue;
                        };
                        match parse_openai_payload(payload.trim()) {
                            Some(StreamChunk::Done) => {
                                let _ = tx.send(StreamChunk::Done).await;
                                return;
                            }
                            Some(chunk) => {
                                if tx.send(chunk).await.is_err() {
                                    return;
                                }
                            }
                            None => {}
                        }
                    }
                }
            }
            let _ = tx.send(StreamChunk::Done).await;
        });
        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_delta() {
        let p = r#"{"choices":[{"delta":{"content":"Hello"}}]}"#;
        assert_eq!(
            parse_openai_payload(p),
            Some(StreamChunk::Text("Hello".into()))
        );
    }

    #[test]
    fn done_marker_and_role_only_delta() {
        assert_eq!(parse_openai_payload("[DONE]"), Some(StreamChunk::Done));
        // Role-only opening delta carries no content → no chunk.
        assert_eq!(
            parse_openai_payload(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#),
            None
        );
        // Empty content is suppressed.
        assert_eq!(
            parse_openai_payload(r#"{"choices":[{"delta":{"content":""}}]}"#),
            None
        );
        // Garbage / keep-alive → None, never panics.
        assert_eq!(parse_openai_payload("{not json"), None);
        assert_eq!(parse_openai_payload("{}"), None);
    }
}
