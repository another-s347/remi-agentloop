#[cfg(feature = "http-client")]
use async_stream::stream;
use futures::Stream;
#[cfg(feature = "http-client")]
use serde::Deserialize;
use std::future::Future;

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::error::AgentError;
#[cfg(feature = "http-client")]
use crate::types::Role;
use crate::types::{ChatRequest, ChatResponseChunk};

// ── OpenAI wire types (internal, only used with http-client) ─────────────────
#[cfg(feature = "http-client")]
#[derive(Debug, Deserialize)]
struct OAIChatCompletionChunk {
    #[serde(default)]
    choices: Vec<OAIChoice>,
    usage: Option<OAIUsage>,
}

#[cfg(feature = "http-client")]
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct OAIChoice {
    #[serde(default)]
    index: usize,
    delta: OAIDelta,
    finish_reason: Option<String>,
}

#[cfg(feature = "http-client")]
#[derive(Debug, Deserialize)]
struct OAIDelta {
    role: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<OAIToolCallDelta>>,
}

#[cfg(feature = "http-client")]
#[derive(Debug, Deserialize)]
struct OAIToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<OAIFunctionDelta>,
}

#[cfg(feature = "http-client")]
#[derive(Debug, Deserialize)]
struct OAIFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[cfg(feature = "http-client")]
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

/// OpenAI-compatible streaming chat client
#[derive(Clone)]
pub struct OpenAIClient {
    #[cfg(feature = "http-client")]
    client: reqwest::Client,
    base_url: String,
    #[allow(dead_code)]
    api_key: String,
    model: String,
}

impl OpenAIClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            #[cfg(feature = "http-client")]
            client: reqwest::Client::new(),
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

    pub fn from_config(config: &AgentConfig) -> Self {
        Self {
            #[cfg(feature = "http-client")]
            client: reqwest::Client::new(),
            base_url: config
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com/v1".into()),
            api_key: config.api_key.clone().unwrap_or_default(),
            model: config.model.clone().unwrap_or_else(|| "gpt-4o".into()),
        }
    }
}

impl Agent for OpenAIClient {
    type Request = ChatRequest;
    type Response = ChatResponseChunk;
    type Error = AgentError;

    fn chat(
        &self,
        req: ChatRequest,
    ) -> impl Future<Output = Result<impl Stream<Item = ChatResponseChunk>, AgentError>> {
        #[cfg(feature = "http-client")]
        {
            let client = self.client.clone();
            let url = format!("{}/chat/completions", self.base_url);
            let api_key = self.api_key.clone();
            let model = self.model.clone();

            async move {
                let mut req = req;
                if req.model.is_empty() {
                    req.model = model;
                }
                req.stream = true;
                req.stream_options = Some(crate::types::StreamOptions {
                    include_usage: true,
                });

                let response = client
                    .post(&url)
                    .bearer_auth(&api_key)
                    .json(&req)
                    .send()
                    .await
                    .map_err(AgentError::Http)?;

                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    return Err(AgentError::model(format!("HTTP {}: {}", status, body)));
                }

                use futures::TryStreamExt;
                use tokio::io::AsyncBufReadExt;
                use tokio_util::io::StreamReader;

                let byte_stream = response
                    .bytes_stream()
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
                let reader = StreamReader::new(byte_stream);
                let lines = reader.lines();

                Ok(stream! {
                    let mut lines = lines;
                    loop {
                        match lines.next_line().await {
                            Err(_) => break,
                            Ok(None) => break,
                            Ok(Some(line)) => {
                                let line = line.trim().to_string();
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
                        }
                    }
                    yield ChatResponseChunk::Done;
                })
            }
        }

        #[cfg(not(feature = "http-client"))]
        {
            let _ = req;
            async move {
                Err::<futures::stream::Empty<ChatResponseChunk>, AgentError>(AgentError::model(
                    "http-client feature not enabled",
                ))
            }
        }
    }
}
