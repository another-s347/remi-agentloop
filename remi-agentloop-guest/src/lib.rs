//! Guest SDK for building WASM agents compatible with remi-agentloop.
//!
//! This crate provides:
//! - **Types** (`LoopInput`, `ProtocolEvent`, etc.) that are serde-compatible
//!   with the host-side `remi-agentloop` crate.
//! - **`GuestAgent`** trait for typed agent implementation.
//! - **`generate!()`** macro to create WIT component-model bindings.
//! - **`export_agent!()`** macro to wire up a `GuestAgent` as a WASM export.
//!
//! Uses the Component Model **resource** pattern for streaming:
//! `chat()` returns an `event-stream` resource, and the host pulls
//! events by calling `next()` until it returns `None`.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use remi_agentloop_guest::prelude::*;
//!
//! struct EchoAgent;
//!
//! impl GuestAgent for EchoAgent {
//!     fn chat(input: LoopInput) -> Result<Vec<ProtocolEvent>, String> {
//!         let text = match &input {
//!             LoopInput::Start { content, .. } => content.text_content(),
//!             _ => return Err("Resume not supported".into()),
//!         };
//!         Ok(vec![
//!             ProtocolEvent::Delta { content: format!("Echo: {text}"), role: None },
//!             ProtocolEvent::Done,
//!         ])
//!     }
//! }
//!
//! remi_agentloop_guest::export_agent!(EchoAgent);
//! ```

pub mod types;

// Re-export key dependencies for use in macros.
#[doc(hidden)]
pub use futures as _futures;
#[doc(hidden)]
pub use serde_json as _serde_json;
#[doc(hidden)]
pub use wit_bindgen as _wit_bindgen;

pub use types::*;

// ── GuestAgent trait ────────────────────────────────────────────────────────

/// Trait for implementing a WASM guest agent.
///
/// The interface mirrors the host-side `Agent` trait:
/// - `&self` receiver (unit/zero-sized structs are the norm for pure guests)
/// - `async fn` return (bridged to synchronous WIT via `futures::executor::block_on`
///   inside [`export_agent!`]; works because guest code never touches real I/O)
/// - `Default` supertrait so [`export_agent!`] can construct an instance
///
/// # Example
///
/// ```rust,ignore
/// #[derive(Default)]
/// struct EchoAgent;
///
/// impl GuestAgent for EchoAgent {
///     async fn chat(&self, input: LoopInput) -> Result<Vec<ProtocolEvent>, String> {
///         let text = match &input {
///             LoopInput::Start { content, .. } => content.text_content(),
///             _ => return Err("Resume not supported".into()),
///         };
///         Ok(vec![
///             ProtocolEvent::Delta { content: format!("Echo: {text}"), role: None },
///             ProtocolEvent::Done,
///         ])
///     }
/// }
///
/// remi_agentloop_guest::export_agent!(EchoAgent);
/// ```
pub trait GuestAgent: Default {
    /// Handle a chat request and return a batch of protocol events.
    ///
    /// May be `async` — the runtime inside a WASM guest is provided by
    /// `futures::executor::block_on`, which is sufficient for pure computation
    /// (no real I/O). State that must survive across `NeedToolExecution`
    /// round-trips should be serialized into `AgentState::user_state`.
    fn chat(
        &self,
        input: LoopInput,
    ) -> impl std::future::Future<Output = Result<Vec<ProtocolEvent>, String>>;
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Encode a [`ProtocolEvent`] to a JSON string.
pub fn encode_event(event: &ProtocolEvent) -> String {
    serde_json::to_string(event).expect("ProtocolEvent serialization failed")
}

/// Decode a [`LoopInput`] from a JSON string.
pub fn decode_input(json: &str) -> Result<LoopInput, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}

// ── WIT binding generation + export macro ────────────────────────────────────

/// Generate WIT component-model bindings for the remi agent interface.
///
/// Most users should prefer [`export_agent!`] which calls this internally.
#[macro_export]
macro_rules! generate {
    () => {
        $crate::_wit_bindgen::generate!({
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
    };
}

/// Wire up a [`GuestAgent`] implementation as a WASM component export.
///
/// This macro:
/// 1. Generates WIT component-model bindings (via [`generate!`]).
/// 2. Creates a `RemiEventStream` struct backed by `Vec<String>` + cursor.
/// 3. Implements the generated `GuestEventStream` trait for pull-based iteration.
/// 4. Implements the generated `Guest` trait, bridging JSON ↔ typed API.
/// 5. Calls the generated `export!` to emit ABI entry points.
///
/// # Example
///
/// ```rust,ignore
/// use remi_agentloop_guest::prelude::*;
///
/// struct MyAgent;
///
/// impl GuestAgent for MyAgent {
///     fn chat(input: LoopInput) -> Result<Vec<ProtocolEvent>, String> {
///         Ok(vec![
///             ProtocolEvent::Delta { content: "hi".into(), role: None },
///             ProtocolEvent::Done,
///         ])
///     }
/// }
///
/// remi_agentloop_guest::export_agent!(MyAgent);
/// ```
#[macro_export]
macro_rules! export_agent {
    ($agent:ident) => {
        // 1. Generate WIT bindings.
        //    Produces:
        //      - `exports::remi::agentloop::agent::Guest` trait (has `chat`)
        //      - `exports::remi::agentloop::agent::GuestEventStream` trait (has `next`)
        //      - `export!(...)` macro
        $crate::generate!();

        // 2. Event stream resource — wraps a Vec of JSON-encoded events.
        pub struct RemiEventStream {
            events: Vec<String>,
            cursor: std::cell::Cell<usize>,
        }

        // 3. Implement `GuestEventStream` (pull-based iteration).
        impl exports::remi::agentloop::agent::GuestEventStream for RemiEventStream {
            fn next(&self) -> Option<String> {
                let i = self.cursor.get();
                if i < self.events.len() {
                    self.cursor.set(i + 1);
                    Some(self.events[i].clone())
                } else {
                    None
                }
            }
        }

        // 4. Implement `Guest` (entry point).
        impl exports::remi::agentloop::agent::Guest for $agent {
            type EventStream = RemiEventStream;

            fn chat(
                input_json: String,
            ) -> Result<exports::remi::agentloop::agent::EventStream, String> {
                // Deserialize input.
                let input: $crate::LoopInput =
                    $crate::_serde_json::from_str(&input_json).map_err(|e| e.to_string())?;

                // Construct a transient agent instance and drive the async
                // future to completion.  The future is always immediately
                // ready for pure-computation guests (no real I/O), so
                // block_on never actually parks the thread.
                let agent = <$agent as std::default::Default>::default();
                let events =
                    $crate::_futures::executor::block_on($crate::GuestAgent::chat(&agent, input))?;

                // Serialize events to JSON.
                let json_events: Vec<String> = events
                    .iter()
                    .map(|e| {
                        $crate::_serde_json::to_string(e)
                            .expect("ProtocolEvent serialization failed")
                    })
                    .collect();

                // Wrap in resource.
                Ok(exports::remi::agentloop::agent::EventStream::new(
                    RemiEventStream {
                        events: json_events,
                        cursor: std::cell::Cell::new(0),
                    },
                ))
            }
        }

        // 5. Emit component-model ABI exports.
        export!($agent);
    };
}

// ── Prelude ─────────────────────────────────────────────────────────────────

pub mod prelude {
    pub use crate::types::*;
    pub use crate::{decode_input, encode_event, GuestAgent};
    // Bring futures::StreamExt into scope so async chat impls can call .collect()
    pub use crate::_futures::stream::StreamExt as FuturesStreamExt;
}
