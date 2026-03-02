use async_stream::stream;
use futures::{Stream, StreamExt};
use serde::Deserialize;
use std::future::Future;

use remi_core::agent::Agent;
#[cfg(feature = "http-client")]
use remi_core::config::AgentConfig;
use remi_core::error::AgentError;
use remi_transport::HttpTransport;
use remi_core::types::{ChatRequest, ChatResponseChunk, Role};

// ── OpenAI wire types (SSE payload parsing) ──────────────────────────────────

#[derive(Debug, Deserialize)]
struct OAIChatCompletionChunk {
    #[serde(default)]
    choices: Vec<OAIChoice>,
    usage: Option<OAIUsage>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct OAIChoice {
    #[serde(default)]
    index: usize,
    delta: OAIDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAIDelta {
    role: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<OAIToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct OAIToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<OAIFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct OAIFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAIUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(alias = "output_tokens", default)]
    completion_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

// ── OpenAIClient ──────────────────────────────────────────────────────────────

/// OpenAI-compatible streaming chat client, generic over [`HttpTransport`].
///
/// On native targets with `http-client` feature, use [`OpenAIClient::new()`]
/// which automatically uses [`ReqwestTransport`](crate::http::ReqwestTransport).
///
/// For other targets (e.g. `wasm32-wasip2`), use [`OpenAIClient::with_transport()`]
/// to inject a host-provided transport.
pub struct OpenAIClient<T: HttpTransport> {
    transport: T,
    base_url: String,
    api_key: String,
    model: String,
}

impl<T: HttpTransport> Clone for OpenAIClient<T> {
    fn clone(&self) -> Self {
        Self {
            transport: self.transport.clone(),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            model: self.model.clone(),
        }
    }
}

/// Convenience constructor for native targets (uses [`ReqwestTransport`]).
///
/// This preserves the original `OpenAIClient::new(api_key)` API so existing
/// code continues to work without changes.
#[cfg(feature = "http-client")]
impl OpenAIClient<remi_transport::ReqwestTransport> {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_transport(remi_transport::ReqwestTransport::new(), api_key)
    }

    pub fn from_config(config: &AgentConfig) -> Self {
        let mut client = Self::new(config.api_key.clone().unwrap_or_default());
        if let Some(url) = &config.base_url {
            client.base_url = url.clone();
        }
        if let Some(model) = &config.model {
            client.model = model.clone();
        }
        client
    }
}

impl<T: HttpTransport> OpenAIClient<T> {
    /// Create an OpenAI client with a custom [`HttpTransport`].
    ///
    /// Use this on `wasm32-wasip2` or other targets where reqwest is unavailable:
    ///
    /// ```ignore
    /// let transport = MyWasiHttpTransport::new();
    /// let client = OpenAIClient::with_transport(transport, "sk-...");
    /// ```
    pub fn with_transport(transport: T, api_key: impl Into<String>) -> Self {
        Self {
            transport,
            base_url: "https://api.openai.com/v1".into(),
            api_key: api_key.into(),
            model: "gpt-4o".into(),
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
}

impl<T: HttpTransport> Agent for OpenAIClient<T> {
    type Request = ChatRequest;
    type Response = ChatResponseChunk;
    type Error = AgentError;

    fn chat(
        &self,
        req: ChatRequest,
    ) -> impl Future<Output = Result<impl Stream<Item = ChatResponseChunk>, AgentError>> {
        let transport = self.transport.clone();
        let url = format!("{}/chat/completions", self.base_url);
        let api_key = self.api_key.clone();
        let model = self.model.clone();

        async move {
            let mut req = req;
            if req.model.is_empty() {
                req.model = model;
            }
            req.stream = true;
            req.stream_options = Some(remi_core::types::StreamOptions {
                include_usage: true,
            });

            let body =
                serde_json::to_vec(&req).map_err(|e| AgentError::model(e.to_string()))?;

            let headers = vec![
                ("Authorization".into(), format!("Bearer {api_key}")),
                ("Content-Type".into(), "application/json".into()),
            ];

            let response = transport
                .post_streaming(url, headers, body)
                .await
                .map_err(|e| AgentError::model(e.to_string()))?;

            if response.status < 200 || response.status >= 300 {
                // Read the error body
                let mut body_bytes = Vec::new();
                let mut body_stream = response.body;
                while let Some(chunk) = body_stream.next().await {
                    if let Ok(bytes) = chunk {
                        body_bytes.extend_from_slice(&bytes);
                    }
                }
                let body_text = String::from_utf8_lossy(&body_bytes);
                return Err(AgentError::model(format!(
                    "HTTP {}: {}",
                    response.status, body_text
                )));
            }

            // Parse SSE from the streaming body — transport-agnostic
            let lines = remi_transport::http::sse_lines(response.body);

            Ok(stream! {
                let mut lines = std::pin::pin!(lines);
                while let Some(line) = lines.next().await {
                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            // keep reading — some APIs send usage chunk after [DONE]
                            continue;
                        }
                        match serde_json::from_str::<OAIChatCompletionChunk>(data) {
                            Err(_) => continue,
                            Ok(chunk) => {
                                if let Some(usage) = chunk.usage {
                                    yield ChatResponseChunk::Usage {
                                        prompt_tokens: usage.prompt_tokens,
                                        completion_tokens: usage.completion_tokens,
                                        total_tokens: usage.total_tokens,
                                    };
                                }
                                for choice in &chunk.choices {
                                    if let Some(content) = &choice.delta.content {
                                        if !content.is_empty() {
                                            let role = choice.delta.role.as_deref().map(|r| match r {
                                                "user" => Role::User,
                                                "system" => Role::System,
                                                "tool" => Role::Tool,
                                                _ => Role::Assistant,
                                            });
                                            yield ChatResponseChunk::Delta { content: content.clone(), role };
                                        }
                                    }
                                    if let Some(tool_calls) = &choice.delta.tool_calls {
                                        for tc in tool_calls {
                                            if let Some(func) = &tc.function {
                                                if let (Some(id), Some(name)) = (&tc.id, &func.name) {
                                                    yield ChatResponseChunk::ToolCallStart {
                                                        index: tc.index,
                                                        id: id.clone(),
                                                        name: name.clone(),
                                                    };
                                                }
                                                if let Some(args_delta) = &func.arguments {
                                                    if !args_delta.is_empty() {
                                                        yield ChatResponseChunk::ToolCallDelta {
                                                            index: tc.index,
                                                            arguments_delta: args_delta.clone(),
                                                        };
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                yield ChatResponseChunk::Done;
            })
        }
    }
}
