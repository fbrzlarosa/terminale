//! Anthropic Claude provider (Messages API + SSE streaming).

use crate::{AiError, AiMessage, AiProvider, AiRequest, StreamChunk};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic Messages API client.
#[derive(Debug, Clone)]
pub struct ClaudeProvider {
    api_key: String,
    http: reqwest::Client,
    /// Base URL — overridable for tests / proxies.
    base_url: String,
}

impl ClaudeProvider {
    /// Build a new client. `api_key` is read from
    /// `$ANTHROPIC_API_KEY` by the caller; we don't peek at env so the
    /// host has full control over secret handling.
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            http: reqwest::Client::new(),
            base_url: ANTHROPIC_URL.to_string(),
        }
    }

    /// Override the API endpoint. Mostly useful for testing against a
    /// mock server.
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }
}

#[derive(Serialize)]
struct MsgBody {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    messages: Vec<MsgBody>,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

/// Anthropic's SSE event payload. We only care about the `delta.text`
/// field for `content_block_delta` events.
#[derive(Deserialize)]
struct SseEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    delta: Option<SseDelta>,
}

#[derive(Deserialize)]
struct SseDelta {
    #[serde(default)]
    text: Option<String>,
}

#[async_trait]
impl AiProvider for ClaudeProvider {
    fn name(&self) -> &'static str {
        "claude"
    }

    async fn stream(&self, req: AiRequest) -> Result<mpsc::Receiver<StreamChunk>, AiError> {
        if self.api_key.is_empty() {
            return Err(AiError::Misconfigured("ANTHROPIC_API_KEY is empty".into()));
        }
        // Anthropic wants system separately; the rest stay in `messages`.
        let mut system: Option<String> = None;
        let mut messages: Vec<MsgBody> = Vec::with_capacity(req.messages.len());
        for AiMessage { role, content } in &req.messages {
            if role == "system" {
                // Concatenate multiple system prompts.
                system = Some(match system.take() {
                    Some(prev) => format!("{prev}\n\n{content}"),
                    None => content.clone(),
                });
                continue;
            }
            messages.push(MsgBody {
                role: role.clone(),
                content: content.clone(),
            });
        }

        let body = Request {
            model: &req.model,
            messages,
            max_tokens: req.max_tokens.unwrap_or(1024),
            stream: true,
            system,
            temperature: req.temperature,
        };

        let resp = self
            .http
            .post(&self.base_url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            let truncated = if body.len() > 4096 {
                body[..4096].to_string()
            } else {
                body
            };
            return Err(AiError::Status {
                status,
                body: truncated,
            });
        }

        let (tx, rx) = mpsc::channel::<StreamChunk>(64);

        tokio::spawn(async move {
            let mut stream = resp.bytes_stream();
            let mut buffer: Vec<u8> = Vec::new();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        buffer.extend_from_slice(&bytes);
                        // SSE frames are separated by blank lines.
                        while let Some(end) = find_double_newline(&buffer) {
                            let frame: Vec<u8> = buffer.drain(..end).collect();
                            // Skip the trailing \n\n.
                            buffer.drain(..frame.len().min(buffer.len()).min(0));
                            // Actually consume the separator now.
                            // (frame's `end` points to the start of the
                            // double-newline, but we already drained
                            // through `end`. Trim leftover terminator.)
                            while matches!(buffer.first(), Some(b'\r' | b'\n')) {
                                buffer.remove(0);
                            }
                            if let Some(text) = parse_sse_frame(&frame) {
                                if !text.is_empty()
                                    && tx.send(StreamChunk::Text(text)).await.is_err()
                                {
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(StreamChunk::Error(e.to_string())).await;
                        return;
                    }
                }
            }
            let _ = tx.send(StreamChunk::Done).await;
        });

        Ok(rx)
    }
}

/// Find the byte index where `\n\n` (or `\r\n\r\n`) starts in `buf`.
fn find_double_newline(buf: &[u8]) -> Option<usize> {
    for (i, w) in buf.windows(2).enumerate() {
        if w == b"\n\n" {
            return Some(i);
        }
    }
    for (i, w) in buf.windows(4).enumerate() {
        if w == b"\r\n\r\n" {
            return Some(i);
        }
    }
    None
}

/// Parse one SSE frame (lines starting with `data: …`) and return the
/// concatenated text delta, if any.
fn parse_sse_frame(frame: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(frame).ok()?;
    let mut out = String::new();
    for line in text.lines() {
        // Anthropic SSE blocks interleave an `event:` line before the
        // `data:` line. Skip non-data lines — the old `?` here aborted the
        // ENTIRE frame on the leading `event:` line, so every text delta
        // was dropped and Claude streamed nothing.
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload == "[DONE]" {
            continue;
        }
        if let Ok(ev) = serde_json::from_str::<SseEvent>(payload) {
            if ev.kind == "content_block_delta" {
                if let Some(d) = ev.delta {
                    if let Some(t) = d.text {
                        out.push_str(&t);
                    }
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_delta_after_event_line() {
        // Real Anthropic shape: an `event:` line precedes the `data:` line.
        // This is exactly the case the old `?`-on-strip_prefix dropped.
        let frame = b"event: content_block_delta\n\
                      data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}";
        assert_eq!(parse_sse_frame(frame).as_deref(), Some("Hi"));
    }

    #[test]
    fn non_delta_events_yield_no_text() {
        let frame = b"event: message_start\n\
                      data: {\"type\":\"message_start\"}";
        assert_eq!(parse_sse_frame(frame).as_deref(), Some(""));
        // [DONE] sentinel is ignored, never panics.
        assert_eq!(parse_sse_frame(b"data: [DONE]").as_deref(), Some(""));
    }

    #[test]
    fn finds_sse_frame_boundaries() {
        assert_eq!(find_double_newline(b"abc\n\ndef"), Some(3));
        assert_eq!(find_double_newline(b"abc\r\n\r\ndef"), Some(3));
        assert_eq!(find_double_newline(b"no boundary here"), None);
    }
}
