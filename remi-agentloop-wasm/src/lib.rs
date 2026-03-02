//! WASM component runner for remi-agentloop.
//!
//! This crate provides [`WasmAgent`], which loads a WASM component implementing
//! the `remi:agentloop/agent` WIT world and exposes it as an [`Agent`] that
//! can be composed with the rest of the remi-agentloop framework.
//!
//! # Architecture — Resource-based streaming (Component Model idiom)
//!
//! ```text
//! ┌──────────────────────────────────┐
//! │  Host (WasmAgent)                │
//! │  Agent::chat(LoopInput)          │
//! │    ↓ serialize to JSON           │
//! │    ↓ call guest chat()           │
//! │    ← event-stream resource       │
//! │    loop {                        │
//! │      next() → Some(json) → yield │
//! │      next() → None       → end  │
//! │    }                             │
//! │    → Stream<ProtocolEvent>       │
//! └──────────────────────────────────┘
//! │          ↕ Component Model ABI
//! ┌──────────────────────────────────┐
//! │  Guest (.wasm component)         │
//! │  chat(input_json)                │
//! │    → event-stream { events[] }   │
//! │  next() → some(events[i])        │
//! │  next() → none                   │
//! └──────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use remi_agentloop::prelude::*;
//! use remi_agentloop_wasm::WasmAgent;
//!
//! let agent = WasmAgent::from_file("my_agent.wasm").unwrap();
//! let stream = agent.chat("Hello from host!".into()).await?;
//! ```

#[cfg(feature = "http-host")]
mod http_host;
#[cfg(feature = "http-host")]
pub use http_host::WasmAgentWithHttp;

use std::path::Path;

use remi_agentloop::agent::Agent;
use remi_agentloop::protocol::{ProtocolError, ProtocolEvent};
use remi_agentloop::types::LoopInput;

use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

// ── Host-side WIT bindings ──────────────────────────────────────────────────

wasmtime::component::bindgen!({
    inline: "
        package remi:agentloop;

        interface agent {
            resource event-stream {
                next: func() -> option<string>;
            }
            chat: func(input-json: string) -> result<event-stream, string>;
        }

        world agent-world {
            export agent;
        }
    ",
});

// ── Host state ──────────────────────────────────────────────────────────────

/// State held by the WASM store during guest execution.
struct HostState;

// ── WasmAgent ───────────────────────────────────────────────────────────────

/// A WASM component that implements the `remi:agentloop/agent-world` WIT world,
/// wrapped as an [`Agent`].
///
/// Each call to [`Agent::chat`] instantiates the component, calls `chat`
/// to obtain an `event-stream` resource, then pulls events via `next()`
/// until the stream is exhausted.
pub struct WasmAgent {
    engine: Engine,
    component: Component,
}

impl WasmAgent {
    /// Load a WASM component from a file path.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ProtocolError> {
        let mut config = Config::new();
        config.wasm_component_model(true);

        let engine = Engine::new(&config).map_err(|e| ProtocolError {
            code: "engine_error".into(),
            message: e.to_string(),
        })?;

        let component = Component::from_file(&engine, path).map_err(|e| ProtocolError {
            code: "component_error".into(),
            message: e.to_string(),
        })?;

        Ok(Self { engine, component })
    }

    /// Load a WASM component from in-memory bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut config = Config::new();
        config.wasm_component_model(true);

        let engine = Engine::new(&config).map_err(|e| ProtocolError {
            code: "engine_error".into(),
            message: e.to_string(),
        })?;

        let component = Component::new(&engine, bytes).map_err(|e| ProtocolError {
            code: "component_error".into(),
            message: e.to_string(),
        })?;

        Ok(Self { engine, component })
    }

    /// Run the guest component: call `chat`, pull all events from the
    /// returned `event-stream` resource via `next()`.
    fn run_guest(&self, input_json: &str) -> Result<Vec<ProtocolEvent>, ProtocolError> {
        let mut store = Store::new(&self.engine, HostState);

        let linker: Linker<HostState> = Linker::new(&self.engine);

        // Instantiate using the generated bindings.
        let bindings =
            AgentWorld::instantiate(&mut store, &self.component, &linker).map_err(|e| {
                ProtocolError {
                    code: "instantiate_error".into(),
                    message: e.to_string(),
                }
            })?;

        // Call `chat(input_json)` → Result<ResourceAny, String>
        let agent_iface = bindings.remi_agentloop_agent();
        let stream_resource =
            agent_iface
                .call_chat(&mut store, input_json)
                .map_err(|e| ProtocolError {
                    code: "call_error".into(),
                    message: e.to_string(),
                })?
                .map_err(|e| ProtocolError {
                    code: "guest_error".into(),
                    message: e.to_string(),
                })?;

        // Pull events via `next()` until `None`.
        let mut events = Vec::new();
        loop {
            let maybe_json = agent_iface
                .event_stream()
                .call_next(&mut store, stream_resource)
                .map_err(|e| ProtocolError {
                    code: "next_error".into(),
                    message: e.to_string(),
                })?;

            match maybe_json {
                Some(json) => {
                    if let Ok(event) = serde_json::from_str::<ProtocolEvent>(&json) {
                        events.push(event);
                    }
                }
                None => break,
            }
        }

        Ok(events)
    }
}

impl Agent for WasmAgent {
    type Request = LoopInput;
    type Response = ProtocolEvent;
    type Error = ProtocolError;

    fn chat(
        &self,
        req: Self::Request,
    ) -> impl std::future::Future<
        Output = Result<impl futures::Stream<Item = Self::Response>, Self::Error>,
    > {
        async move {
            let input_json = serde_json::to_string(&req).map_err(|e| ProtocolError {
                code: "serialize_error".into(),
                message: e.to_string(),
            })?;

            let events = self.run_guest(&input_json)?;

            Ok(futures::stream::iter(events))
        }
    }
}
