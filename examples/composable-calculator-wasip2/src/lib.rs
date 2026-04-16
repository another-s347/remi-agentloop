//! wasip2 entry point for the composable calculator agent.
//!
//! This crate compiles the **same agent logic** from
//! `composable-calculator-agent` into a WASM component that:
//!
//! 1. **Imports** `remi:agentloop/http-transport` — the host provides HTTP
//! 2. **Exports** `remi:agentloop/agent` — the standard agent interface
//!
//! # Streaming
//!
//! HTTP responses are streamed chunk-by-chunk through the component boundary
//! via the `response-body` WIT resource. The guest pulls chunks incrementally
//! so SSE events from LLM APIs arrive with minimal latency — identical to
//! the native streaming behavior.
//!
//! Build:
//! ```sh
//! cargo build --target wasm32-wasip2 --release
//! ```

#![allow(unused)]

use futures::{Stream, StreamExt};
use remi_agentloop::http::{HttpStreamingResponse, HttpTransport, HttpTransportError};
use remi_agentloop::prelude::*;
use remi_agentloop::protocol::ProtocolEvent;
use std::pin::Pin;

// ── WIT bindings ──────────────────────────────────────────────────────────────

wit_bindgen::generate!({
    inline: "
        package remi:agentloop;

        /// Host-provided HTTP transport — the WASM component calls this
        /// to make outbound HTTP requests (e.g. to the LLM API).
        interface http-transport {
            /// Streaming response body. The host pushes chunks from the
            /// HTTP response; the guest pulls them via next-chunk().
            resource response-body {
                /// Read the next chunk. Returns none when fully consumed.
                next-chunk: func() -> result<option<list<u8>>, string>;
            }

            /// POST with streaming response.
            /// Returns the HTTP status code and a streaming body resource.
            post-streaming: func(
                url: string,
                headers: list<tuple<string, string>>,
                body: list<u8>,
            ) -> result<tuple<u16, response-body>, string>;
        }

        /// Agent interface — exported by the WASM component.
        interface agent {
            resource event-stream {
                next: func() -> option<string>;
            }
            chat: func(input-json: string) -> result<event-stream, string>;
        }

        world agent-world {
            import http-transport;
            export agent;
        }
    ",
});

// ── WitHttpTransport ──────────────────────────────────────────────────────────

/// [`HttpTransport`] implementation backed by WIT host imports.
///
/// When the WASM component calls `transport.post_streaming(...)`, it
/// invokes the host's `remi:agentloop/http-transport.post-streaming()` import.
/// The host sends the HTTP request and returns the status + a `response-body`
/// resource handle. The guest then pulls chunks lazily via `next-chunk()`.
///
/// The `response-body` resource is wrapped into an async stream, so
/// `sse_lines()` and the rest of the streaming SSE pipeline work exactly
/// as on native.
#[derive(Clone)]
struct WitHttpTransport;

impl HttpTransport for WitHttpTransport {
    fn post_streaming(
        &self,
        url: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> impl std::future::Future<Output = Result<HttpStreamingResponse, HttpTransportError>>
    {
        async move {
            // Call the WIT host import — synchronous from the component's perspective
            let (status, response_body) =
                remi::agentloop::http_transport::post_streaming(&url, &headers, &body)
                    .map_err(|e| HttpTransportError::new(e))?;

            // Wrap the resource handle as an async stream that pulls chunks
            // via next_chunk() until the response body is fully consumed.
            let stream = futures::stream::unfold(response_body, |body| async move {
                match body.next_chunk() {
                    Ok(Some(chunk)) => Some((Ok(chunk), body)),
                    Ok(None) => None, // fully consumed
                    Err(e) => Some((Err(HttpTransportError::new(e)), body)),
                }
            });

            Ok(HttpStreamingResponse {
                status,
                headers: Vec::new(),
                body: Box::pin(stream),
            })
        }
    }
}

// ── WIT export implementation ─────────────────────────────────────────────────

/// Event stream resource — backed by a **lazy** async stream.
///
/// Each `next()` call drives the underlying agent loop forward by exactly
/// one event via a minimal `block_on`. The host pulls events at its own
/// pace — no upfront collection, no blocking the entire agent loop.
pub struct RemiEventStream {
    stream: std::cell::RefCell<Pin<Box<dyn Stream<Item = String>>>>,
}

impl exports::remi::agentloop::agent::GuestEventStream for RemiEventStream {
    fn next(&self) -> Option<String> {
        let mut stream = self.stream.borrow_mut();
        // Drive ONE step — all I/O goes through WIT imports (synchronous),
        // so this completes as soon as one event is produced.
        futures::executor::block_on(stream.as_mut().next())
    }
}

struct ComposableCalculatorAgent;

impl exports::remi::agentloop::agent::Guest for ComposableCalculatorAgent {
    type EventStream = RemiEventStream;

    fn chat(
        input_json: String,
    ) -> Result<exports::remi::agentloop::agent::EventStream, String> {
        // 1. Deserialize LoopInput
        let input: LoopInput =
            serde_json::from_str(&input_json).map_err(|e| e.to_string())?;

        // 2. Extract API config from input metadata
        let (api_key, base_url, model) = extract_config(&input);

        // 3. Construct OpenAIClient with host-injected HTTP transport
        let mut oai = OpenAIClient::with_transport(WitHttpTransport, &api_key);
        if !base_url.is_empty() {
            oai = oai.with_base_url(&base_url);
        }
        if !model.is_empty() {
            oai = oai.with_model(&model);
        }

        // 4. Build the agent — exact same code path as native!
        let agent = composable_calculator_agent::build_agent(oai);

        // 5. Create a lazy stream that OWNS the agent.
        //    No block_on here — the async machinery is deferred entirely
        //    into per-event next() calls driven by the host.
        let event_stream = async_stream::stream! {
            match agent.chat(ChatCtx::default(), input).await {
                Ok(inner) => {
                    futures::pin_mut!(inner);
                    while let Some(event) = inner.next().await {
                        let proto: ProtocolEvent = event.into();
                        yield serde_json::to_string(&proto)
                            .expect("ProtocolEvent serialization failed");
                    }
                }
                Err(e) => {
                    let proto: ProtocolEvent = AgentEvent::Error(e).into();
                    yield serde_json::to_string(&proto)
                        .expect("ProtocolEvent serialization failed");
                }
            }
        };

        // 6. Wrap in resource — the stream is lazy, nothing runs until next()
        Ok(exports::remi::agentloop::agent::EventStream::new(
            RemiEventStream {
                stream: std::cell::RefCell::new(Box::pin(event_stream)),
            },
        ))
    }
}

export!(ComposableCalculatorAgent);

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Extract LLM API configuration from LoopInput metadata.
///
/// Expected metadata JSON:
/// ```json
/// {
///   "api_key": "sk-...",
///   "base_url": "https://api.openai.com/v1",
///   "model": "gpt-4o"
/// }
/// ```
fn extract_config(input: &LoopInput) -> (String, String, String) {
    let meta = match input {
        LoopInput::Start { metadata, .. } => metadata.as_ref(),
        LoopInput::Resume { state, .. } => state.config.metadata.as_ref(),
    };

    let empty = serde_json::Value::Null;
    let meta = meta.unwrap_or(&empty);

    let api_key = meta["api_key"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let base_url = meta["base_url"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let model = meta["model"]
        .as_str()
        .unwrap_or("")
        .to_string();

    (api_key, base_url, model)
}
