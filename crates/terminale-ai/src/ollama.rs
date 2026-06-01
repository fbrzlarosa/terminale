//! Ollama provider — talks to a local `ollama serve` over HTTP.
//!
//! Ollama's streaming uses newline-delimited JSON, one frame per line —
//! simpler than SSE.

use crate::{AiError, AiProvider, AiRequest, StreamChunk};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

const DEFAULT_URL: &str = "http://localhost:11434/api/chat";

/// Local Ollama daemon client.
#[derive(Debug, Clone)]
pub struct OllamaProvider {
    http: reqwest::Client,
    base_url: String,
}

impl OllamaProvider {
    /// Build a client pointed at the standard local daemon.
    #[must_use]
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: DEFAULT_URL.to_string(),
        }
    }

    /// Override the daemon endpoint — for a remote host or non-default
    /// port.
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    messages: Vec<Msg>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Options>,
}

#[derive(Serialize)]
struct Options {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
}

#[derive(Serialize)]
struct Msg {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct Frame {
    #[serde(default)]
    message: Option<FrameMsg>,
    #[serde(default)]
    done: bool,
}

#[derive(Deserialize)]
struct FrameMsg {
    #[serde(default)]
    content: String,
}

/// Parse one Ollama NDJSON line into `(text, done)`. Non-empty content
/// becomes `Some(text)`; an invalid/blank line is `(None, false)`.
fn parse_ollama_line(line: &str) -> (Option<String>, bool) {
    match serde_json::from_str::<Frame>(line.trim()) {
        Ok(f) => {
            let text = f.message.map(|m| m.content).filter(|c| !c.is_empty());
            (text, f.done)
        }
        Err(_) => (None, false),
    }
}

#[async_trait]
impl AiProvider for OllamaProvider {
    fn name(&self) -> &'static str {
        "ollama"
    }

    async fn stream(&self, req: AiRequest) -> Result<mpsc::Receiver<StreamChunk>, AiError> {
        let messages = req
            .messages
            .iter()
            .map(|m| Msg {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();
        let options = if req.temperature.is_some() || req.max_tokens.is_some() {
            Some(Options {
                temperature: req.temperature,
                num_predict: req.max_tokens,
            })
        } else {
            None
        };
        let body = Body {
            model: &req.model,
            messages,
            stream: true,
            options,
        };
        let resp = self.http.post(&self.base_url).json(&body).send().await?;
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
                while let Some(idx) = buf.find('\n') {
                    let line: String = buf.drain(..idx).collect();
                    buf.drain(..1);
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let (text, done) = parse_ollama_line(line);
                    if let Some(t) = text {
                        if tx.send(StreamChunk::Text(t)).await.is_err() {
                            return;
                        }
                    }
                    if done {
                        let _ = tx.send(StreamChunk::Done).await;
                        return;
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
    use crate::AiRequest;

    #[test]
    fn parses_ollama_ndjson_frames() {
        // Streaming text frame.
        assert_eq!(
            parse_ollama_line(r#"{"message":{"role":"assistant","content":"Hi"},"done":false}"#),
            (Some("Hi".to_string()), false)
        );
        // Final frame: done, empty content.
        assert_eq!(
            parse_ollama_line(r#"{"message":{"role":"assistant","content":""},"done":true}"#),
            (None, true)
        );
        // Blank / invalid lines never panic.
        assert_eq!(parse_ollama_line(""), (None, false));
        assert_eq!(parse_ollama_line("{bad json"), (None, false));
    }

    /// Live smoke test against a local Ollama daemon. Ignored by default
    /// (CI has no Ollama); run with:
    ///   cargo test -p terminale-ai --  --ignored ollama_live
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a local Ollama daemon; CI has none"]
    async fn ollama_live_streams_text() {
        let provider = OllamaProvider::new();
        let req = AiRequest::one_shot("gemma3:4b", "Reply with exactly the word: pong");
        let text = provider
            .complete(req)
            .await
            .expect("ollama should stream a completion");
        assert!(
            !text.trim().is_empty(),
            "expected non-empty reply, got {text:?}"
        );
        eprintln!("ollama replied: {text:?}");
    }
}
