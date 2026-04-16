//! WASM agent runner with host-provided **streaming** HTTP transport.
//!
//! [`WasmAgentWithHttp`] loads a WASM component that uses the extended WIT
//! world with `http-transport` import. The host provides HTTP capability
//! so the guest agent can make outbound requests (e.g. to LLM APIs).
//!
//! # Streaming design
//!
//! The `response-body` WIT resource enables true chunk-by-chunk streaming
//! across the WASM component boundary. The host only waits for HTTP headers
//! before returning; the response body is streamed lazily via `next-chunk()`.
//!
//! Requires the `http-host` feature (enabled by default).

use std::path::Path;

use remi_agentloop::agent::Agent;
use remi_agentloop::protocol::{ProtocolError, ProtocolEvent};
use remi_agentloop::types::LoopInput;

use wasmtime::component::{Component, Linker, Resource, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

// -- Host-side WIT bindings (streaming HTTP transport) ------------------------

wasmtime::component::bindgen!({
    inline: "
        package remi:agentloop;

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
    with: {
        "remi:agentloop/http-transport.response-body": ResponseBodyData,
    },
});

// -- Response body resource ---------------------------------------------------

/// Host-side data backing the `response-body` WIT resource.
///
/// Each instance holds an in-progress `reqwest::Response`. When the guest
/// calls `next-chunk()`, the host pulls the next chunk from the response
/// body stream via `response.chunk().await`.
pub struct ResponseBodyData {
    response: reqwest::Response,
}

// -- Host state ---------------------------------------------------------------

/// State held by the WASM store during guest execution.
struct HttpHostState {
    wasi: WasiCtx,
    table: ResourceTable,
}

impl WasiView for HttpHostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

// -- HTTP transport Host trait impls ------------------------------------------

impl remi::agentloop::http_transport::HostResponseBody for HttpHostState {
    fn next_chunk(&mut self, self_: Resource<ResponseBodyData>) -> Result<Option<Vec<u8>>, String> {
        let data = self.table.get_mut(&self_).map_err(|e| e.to_string())?;
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                match data.response.chunk().await {
                    Ok(Some(bytes)) => Ok(Some(bytes.to_vec())),
                    Ok(None) => Ok(None),
                    Err(e) => Err(format!("chunk error: {e}")),
                }
            })
        })
    }

    fn drop(&mut self, rep: Resource<ResponseBodyData>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl remi::agentloop::http_transport::Host for HttpHostState {
    fn post_streaming(
        &mut self,
        url: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Result<(u16, Resource<ResponseBodyData>), String> {
        // Send the HTTP request -- only waits for headers, NOT the full body.
        let response = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let client = reqwest::Client::new();
                let mut req = client.post(&url).body(body);
                for (key, value) in headers {
                    req = req.header(key, value);
                }
                req.send().await.map_err(|e| format!("HTTP error: {e}"))
            })
        })?;

        let status = response.status().as_u16();

        // Store the response in the resource table -- body stream stays open.
        // Each next_chunk() call will pull the next chunk lazily.
        let resource = self
            .table
            .push(ResponseBodyData { response })
            .map_err(|e| e.to_string())?;

        Ok((status, resource))
    }
}

/// Designator type for linking the HTTP transport interface.
struct HttpDesignator;

impl wasmtime::component::HasData for HttpDesignator {
    type Data<'a> = &'a mut HttpHostState;
}

// -- WasmAgentWithHttp --------------------------------------------------------

/// A WASM component runner that provides **streaming** HTTP transport to the
/// guest.
///
/// HTTP responses are streamed chunk-by-chunk through the component boundary
/// via the `response-body` WIT resource. The guest pulls chunks incrementally
/// so SSE events from LLM APIs arrive with minimal latency.
pub struct WasmAgentWithHttp {
    engine: Engine,
    component: Component,
}

impl WasmAgentWithHttp {
    fn make_engine(config: &Config) -> Result<Engine, ProtocolError> {
        Engine::new(config).map_err(|e| ProtocolError {
            code: "engine_error".into(),
            message: e.to_string(),
        })
    }

    /// Load a WASM component from a file path (JIT-compiles at load time).
    ///
    /// Requires the `compiler` feature (Cranelift). Not available on Android.
    #[cfg(feature = "compiler")]
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ProtocolError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Self::make_engine(&config)?;
        let component = Component::from_file(&engine, path).map_err(|e| ProtocolError {
            code: "component_error".into(),
            message: e.to_string(),
        })?;
        Ok(Self { engine, component })
    }

    /// Load a WASM component from in-memory bytes.
    ///
    /// Auto-detects the artifact format from the magic bytes:
    /// - Raw WASM (`\0asm` header) → JIT-compiled via Cranelift. Requires the
    ///   `compiler` feature; not available on Android.
    /// - Everything else → treated as a precompiled `.cwasm` artifact and loaded
    ///   via `Component::deserialize`. No compiler needed; works on Android.
    ///
    /// This means you can embed the right artifact for each platform and call
    /// this uniformly — the runtime picks the correct loading path on demand.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if !bytes.starts_with(b"\0asm") {
            // Precompiled artifact — no JIT needed.
            return Self::from_precompiled_bytes(bytes);
        }
        // Raw WASM — needs Cranelift JIT.
        #[cfg(not(feature = "compiler"))]
        return Err(ProtocolError {
            code: "no_compiler".into(),
            message: "Raw WASM bytes require the `compiler` feature (Cranelift JIT). \
                      Provide a precompiled .cwasm artifact for this platform."
                .into(),
        });
        #[cfg(feature = "compiler")]
        {
            let mut config = Config::new();
            config.wasm_component_model(true);
            let engine = Self::make_engine(&config)?;
            let component = Component::new(&engine, bytes).map_err(|e| ProtocolError {
                code: "component_error".into(),
                message: e.to_string(),
            })?;
            Ok(Self { engine, component })
        }
    }

    /// Load from a pre-AOT-compiled `.cwasm` blob produced by
    /// [`WasmAgentWithHttp::precompile_for_target`].
    ///
    /// # Safety
    /// The bytes must have been produced by a trusted invocation of
    /// `precompile_for_target` (or `Engine::precompile_component`). Loading
    /// an untrusted or mismatched blob is undefined behaviour.
    ///
    /// Available without the `compiler` feature — suitable for Android where
    /// JIT compilation is not permitted.
    pub fn from_precompiled_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Self::make_engine(&config)?;
        // SAFETY: caller guarantees bytes are a valid, trusted wasmtime artifact.
        let component =
            unsafe { Component::deserialize(&engine, bytes) }.map_err(|e| ProtocolError {
                code: "component_error".into(),
                message: format!("Failed to deserialize precompiled WASM: {e}"),
            })?;
        Ok(Self { engine, component })
    }

    /// Cross-compile a `.wasm` component bytes into a precompiled `.cwasm`
    /// blob for `target_triple` (e.g. `"aarch64-linux-android"`).
    ///
    /// The resulting bytes can be saved to disk and later loaded on the target
    /// device via [`from_precompiled_bytes`].
    ///
    /// Requires the `compiler` feature (Cranelift cross-compilation).
    #[cfg(feature = "compiler")]
    pub fn precompile_for_target(
        wasm_bytes: &[u8],
        target_triple: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.target(target_triple).map_err(|e| ProtocolError {
            code: "target_error".into(),
            message: format!("Unknown target triple '{target_triple}': {e}"),
        })?;
        let engine = Self::make_engine(&config)?;
        engine
            .precompile_component(wasm_bytes)
            .map_err(|e| ProtocolError {
                code: "precompile_error".into(),
                message: format!("Precompile failed for target '{target_triple}': {e}"),
            })
    }

    /// Set up the guest component and call `chat()`, returning the
    /// store, bindings, and event-stream resource for lazy iteration.
    fn init_guest(
        &self,
        input_json: &str,
    ) -> Result<
        (
            Store<HttpHostState>,
            AgentWorld,
            wasmtime::component::ResourceAny,
        ),
        ProtocolError,
    > {
        let wasi = WasiCtxBuilder::new().inherit_stdio().build();
        let mut store = Store::new(
            &self.engine,
            HttpHostState {
                wasi,
                table: ResourceTable::new(),
            },
        );

        let mut linker: Linker<HttpHostState> = Linker::new(&self.engine);

        // Link WASI p2
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| ProtocolError {
            code: "linker_error".into(),
            message: format!("Failed to link WASI: {e}"),
        })?;

        // Link streaming http-transport interface
        remi::agentloop::http_transport::add_to_linker::<HttpHostState, HttpDesignator>(
            &mut linker,
            |state| state,
        )
        .map_err(|e| ProtocolError {
            code: "linker_error".into(),
            message: format!("Failed to link http-transport: {e}"),
        })?;

        // Instantiate the component
        let bindings =
            AgentWorld::instantiate(&mut store, &self.component, &linker).map_err(|e| {
                ProtocolError {
                    code: "instantiate_error".into(),
                    message: e.to_string(),
                }
            })?;

        // Call chat(input_json) — the guest sets up the agent and returns
        // a lazy event-stream resource (no work done yet).
        let stream_resource = {
            let agent_iface = bindings.remi_agentloop_agent();
            agent_iface
                .call_chat(&mut store, input_json)
                .map_err(|e| ProtocolError {
                    code: "call_error".into(),
                    message: e.to_string(),
                })?
                .map_err(|e| ProtocolError {
                    code: "guest_error".into(),
                    message: e,
                })?
        };

        Ok((store, bindings, stream_resource))
    }
}

impl Agent for WasmAgentWithHttp {
    type Request = LoopInput;
    type Response = ProtocolEvent;
    type Error = ProtocolError;

    fn chat(
        &self,
        _ctx: remi_core::types::ChatCtx,
        req: Self::Request,
    ) -> impl std::future::Future<
        Output = Result<impl futures::Stream<Item = Self::Response>, Self::Error>,
    > {
        async move {
            let input_json = serde_json::to_string(&req).map_err(|e| ProtocolError {
                code: "serialize_error".into(),
                message: e.to_string(),
            })?;

            let (mut store, bindings, stream_resource) = self.init_guest(&input_json)?;

            // Return a lazy stream — each poll calls guest next() once,
            // which may trigger HTTP calls back through the host.
            let stream = async_stream::stream! {
                loop {
                    let agent_iface = bindings.remi_agentloop_agent();
                    match agent_iface
                        .event_stream()
                        .call_next(&mut store, stream_resource)
                    {
                        Ok(Some(json)) => {
                            match serde_json::from_str::<ProtocolEvent>(&json) {
                                Ok(event) => yield event,
                                Err(error) => {
                                    let event_type = serde_json::from_str::<serde_json::Value>(&json)
                                        .ok()
                                        .and_then(|value| value.get("type").and_then(|field| field.as_str()).map(ToString::to_string));
                                    let message = match event_type {
                                        Some(event_type) => format!(
                                            "Failed to parse guest protocol event of type '{}': {}",
                                            event_type, error
                                        ),
                                        None => format!("Failed to parse guest protocol event: {}", error),
                                    };
                                    yield ProtocolEvent::Error {
                                        message,
                                        code: Some("guest_event_parse_error".into()),
                                    };
                                    break;
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            yield ProtocolEvent::Error {
                                message: format!("Guest event stream failed: {}", error),
                                code: Some("guest_event_stream_error".into()),
                            };
                            break;
                        }
                    }
                }
            };

            Ok(stream)
        }
    }
}
