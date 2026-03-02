//! Browser entry point for the composable calculator agent.
//!
//! Compiles the **same agent logic** from `composable-calculator-agent` to
//! `wasm32-unknown-unknown` via `wasm-bindgen`. HTTP transport uses the
//! browser's native `fetch()` API with streaming `ReadableStream` for
//! true chunk-by-chunk SSE parsing.
//!
//! Build:
//! ```sh
//! wasm-pack build --target web examples/composable-calculator-browser
//! ```

use futures::{Stream, StreamExt};
use remi_agentloop::http::{HttpStreamingResponse, HttpTransport, HttpTransportError};
use remi_agentloop::prelude::*;
use remi_agentloop::protocol::ProtocolEvent;
use std::pin::Pin;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

// ── FetchTransport ───────────────────────────────────────────────────────────

/// [`HttpTransport`] backed by the browser `fetch()` API.
///
/// Uses `ReadableStream` to stream the response body chunk-by-chunk,
/// so SSE events from LLM APIs arrive with minimal latency.
#[derive(Clone)]
struct FetchTransport;

impl HttpTransport for FetchTransport {
    fn post_streaming(
        &self,
        url: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> impl std::future::Future<Output = Result<HttpStreamingResponse, HttpTransportError>>
    {
        async move {
            let window = web_sys::window()
                .ok_or_else(|| HttpTransportError::new("no global window"))?;

            // Build Headers
            let js_headers = web_sys::Headers::new()
                .map_err(|e| HttpTransportError::new(format!("{e:?}")))?;
            for (k, v) in &headers {
                js_headers
                    .set(k, v)
                    .map_err(|e| HttpTransportError::new(format!("{e:?}")))?;
            }

            // Build Request
            let mut opts = web_sys::RequestInit::new();
            opts.set_method("POST");
            opts.set_headers(&js_headers);

            // Body as Uint8Array
            let body_array = js_sys::Uint8Array::from(body.as_slice());
            opts.set_body(&body_array.into());

            let request = web_sys::Request::new_with_str_and_init(&url, &opts)
                .map_err(|e| HttpTransportError::new(format!("{e:?}")))?;

            // fetch() → Response
            let resp_value = JsFuture::from(window.fetch_with_request(&request))
                .await
                .map_err(|e| HttpTransportError::new(format!("fetch error: {e:?}")))?;

            let resp: web_sys::Response = resp_value
                .dyn_into()
                .map_err(|e| HttpTransportError::new(format!("{e:?}")))?;

            let status = resp.status();

            // Get ReadableStream from response body
            let readable_stream = resp
                .body()
                .ok_or_else(|| HttpTransportError::new("response has no body"))?;

            let reader = readable_stream
                .get_reader()
                .dyn_into::<web_sys::ReadableStreamDefaultReader>()
                .map_err(|e| HttpTransportError::new(format!("{e:?}")))?;

            // Create an async stream that pulls chunks from the ReadableStream
            let body_stream = futures::stream::unfold(reader, |reader| async move {
                let result = JsFuture::from(reader.read()).await;
                match result {
                    Ok(js_val) => {
                        let done = js_sys::Reflect::get(&js_val, &JsValue::from_str("done"))
                            .unwrap_or(JsValue::TRUE)
                            .as_bool()
                            .unwrap_or(true);

                        if done {
                            return None;
                        }

                        let value =
                            js_sys::Reflect::get(&js_val, &JsValue::from_str("value")).ok()?;
                        let array = js_sys::Uint8Array::new(&value);
                        let chunk = array.to_vec();
                        Some((Ok(chunk), reader))
                    }
                    Err(e) => Some((
                        Err(HttpTransportError::new(format!("read error: {e:?}"))),
                        reader,
                    )),
                }
            });

            Ok(HttpStreamingResponse {
                status,
                body: Box::pin(body_stream),
            })
        }
    }
}

// ── wasm-bindgen exports ─────────────────────────────────────────────────────

/// JS-visible event stream handle. Call `next()` to pull events one at a time.
///
/// In the browser, `next()` is **async** — each call returns a `Promise<string | undefined>`.
/// The event loop drives the fetch/SSE parsing forward one event per call.
#[wasm_bindgen]
pub struct EventStream {
    stream: std::cell::RefCell<Pin<Box<dyn Stream<Item = String>>>>,
}

#[wasm_bindgen]
impl EventStream {
    /// Pull the next event as a JSON string. Returns `undefined` when done.
    ///
    /// This is async — awaits the next event from the underlying agent stream,
    /// which may involve in-flight fetch() calls to the LLM API.
    pub async fn next(&self) -> JsValue {
        // SAFETY: wasm32-unknown-unknown is single-threaded, so RefCell borrow
        // across an await point is safe — no concurrent access possible.
        let mut stream = self.stream.borrow_mut();
        match stream.as_mut().next().await {
            Some(json) => JsValue::from_str(&json),
            None => JsValue::UNDEFINED,
        }
    }
}

/// Start the calculator agent. Returns a Promise that resolves to an `EventStream`.
///
/// `input_json` is a JSON-serialized `LoopInput`. Must include metadata
/// with `api_key`, `base_url`, and optionally `model`.
///
/// Usage from JS:
/// ```js
/// const stream = await chat(JSON.stringify({
///   type: "start",
///   content: "What is 12 + 8?",
///   metadata: { api_key: "sk-...", base_url: "https://...", model: "..." }
/// }));
/// let ev;
/// while ((ev = await stream.next()) !== undefined) {
///   console.log(JSON.parse(ev));
/// }
/// ```
#[wasm_bindgen]
pub fn chat(input_json: &str) -> Result<EventStream, JsValue> {
    console_error_panic_hook::set_once();

    let input: LoopInput = serde_json::from_str(input_json)
        .map_err(|e| JsValue::from_str(&format!("parse error: {e}")))?;

    let (api_key, base_url, model) = extract_config(&input);

    let mut oai = OpenAIClient::with_transport(FetchTransport, &api_key);
    if !base_url.is_empty() {
        oai = oai.with_base_url(&base_url);
    }
    if !model.is_empty() {
        oai = oai.with_model(&model);
    }

    let agent = composable_calculator_agent::build_agent(oai);

    let event_stream = async_stream::stream! {
        match agent.chat(input).await {
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

    Ok(EventStream {
        stream: std::cell::RefCell::new(Box::pin(event_stream)),
    })
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn extract_config(input: &LoopInput) -> (String, String, String) {
    let meta = match input {
        LoopInput::Start { metadata, .. } => metadata.as_ref(),
        LoopInput::Resume { state, .. } => state.config.metadata.as_ref(),
    };

    let empty = serde_json::Value::Null;
    let meta = meta.unwrap_or(&empty);

    let api_key = meta["api_key"].as_str().unwrap_or("").to_string();
    let base_url = meta["base_url"].as_str().unwrap_or("").to_string();
    let model = meta["model"].as_str().unwrap_or("").to_string();

    (api_key, base_url, model)
}
