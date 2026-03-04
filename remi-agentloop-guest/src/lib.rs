//! Guest SDK for building WASM agents compatible with remi-agentloop.
//!
//! This crate provides:
//! - **Types** (`LoopInput`, `ProtocolEvent`, `AgentConfig`, `ApiVersion`, etc.)
//! - **`GuestAgent`** trait for typed agent implementation.
//! - **`export_agent!()`** macro to wire up a `GuestAgent` as a WASM component export.
//!
//! # What changed from the string-based v0 ABI
//!
//! The WIT boundary is now **fully typed**: `LoopInput` and `ProtocolEvent` are WIT
//! records/variants instead of JSON strings.  Only intentionally open-ended fields
//! (e.g. `user_state`, `metadata`, `extra`) pass through as JSON sub-strings.
//!
//! In addition the world now has two new WIT interfaces:
//! - `remi:agentloop/config` — host-provided config pulled on demand via
//!   `get_config()` (injected into scope by [`export_agent!`]).
//! - `remi:agentloop/agent-info` — guest-advertised API version, used by the
//!   runner for compatibility checks.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use remi_agentloop_guest::prelude::*;
//!
//! #[derive(Default)]
//! struct EchoAgent;
//!
//! impl GuestAgent for EchoAgent {
//!     async fn chat(&self, input: LoopInput) -> Result<Vec<ProtocolEvent>, String> {
//!         let cfg = get_config(); // pull runtime config from host
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
/// - `&self` receiver (unit/zero-sized structs are the norm for pure guests)
/// - `async fn` support via `futures::executor::block_on` inside [`export_agent!`]
/// - `Default` supertrait so [`export_agent!`] can construct the instance
///
/// # Example
///
/// ```rust,ignore
/// #[derive(Default)]
/// struct EchoAgent;
///
/// impl GuestAgent for EchoAgent {
///     async fn chat(&self, input: LoopInput) -> Result<Vec<ProtocolEvent>, String> {
///         Ok(vec![
///             ProtocolEvent::Delta { content: "hi".into(), role: None },
///             ProtocolEvent::Done,
///         ])
///     }
/// }
///
/// remi_agentloop_guest::export_agent!(EchoAgent);
/// ```
pub trait GuestAgent: Default {
    fn chat(
        &self,
        input: LoopInput,
    ) -> impl std::future::Future<Output = Result<Vec<ProtocolEvent>, String>>;
}

// ── Canonical WIT world definition ──────────────────────────────────────────

/// Typed WIT world shared verbatim by the guest ([`export_agent!`]) and the host
/// (`wasmtime::component::bindgen!` in `remi-agentloop-wasm`).
///
/// Keeping this as a Rust `&str` constant lets both sides reference a single
/// source of truth without requiring the `wit/` directory to be on the include
/// path at compile time.
#[doc(hidden)]
pub const WIT_INLINE: &str = "
package remi:agentloop;

interface config {
    record agent-config {
        api-key: option<string>,
        model: option<string>,
        base-url: option<string>,
        temperature: option<f64>,
        max-tokens: option<u32>,
        timeout-ms: option<u64>,
        headers-json: option<string>,
        extra-json: option<string>,
    }
    get-config: func() -> agent-config;
}

interface agent-info {
    record api-version {
        major: u32,
        minor: u32,
        patch: u32,
    }
    get-api-version: func() -> api-version;
    get-min-host-version: func() -> api-version;
}

interface agent {
    record image-url-detail {
        url: string,
        detail: option<string>,
    }
    record audio-detail {
        data: string,
        format: string,
    }
    variant content-part {
        text(string),
        image-url(image-url-detail),
        image-base64(tuple<string, string>),
        audio(audio-detail),
        file-json(string),
    }
    variant content {
        text(string),
        parts(list<content-part>),
    }
    record tool-call-message {
        id: string,
        call-type: string,
        name: string,
        arguments: string,
    }
    record message {
        id: string,
        role: string,
        content: content,
        tool-calls: option<list<tool-call-message>>,
        tool-call-id: option<string>,
    }
    record tool-definition {
        name: string,
        description: string,
        parameters-schema-json: string,
    }
    record parsed-tool-call {
        id: string,
        name: string,
        arguments-json: string,
    }
    record tool-call-result {
        tool-call-id: string,
        tool-name: string,
        value: string,
    }
    record tool-call-error {
        tool-call-id: string,
        tool-name: string,
        err-msg: string,
    }
    variant tool-call-outcome {
        %result(tool-call-result),
        %error(tool-call-error),
    }
    record step-config {
        model: string,
        temperature: option<f64>,
        max-tokens: option<u32>,
        metadata-json: option<string>,
    }
    record agent-state {
        messages: list<message>,
        system-prompt: option<string>,
        tool-definitions: list<tool-definition>,
        config: step-config,
        thread-id: string,
        run-id: string,
        turn: u32,
        phase-json: string,
        user-state-json: string,
    }
    record interrupt-info {
        interrupt-id: string,
        tool-call-id: string,
        tool-name: string,
        kind: string,
        data-json: string,
    }
    record loop-input-start {
        content: content,
        history: list<message>,
        extra-tools: list<tool-definition>,
        model: option<string>,
        temperature: option<f64>,
        max-tokens: option<u32>,
        metadata-json: option<string>,
    }
    record loop-input-resume {
        state: agent-state,
        results: list<tool-call-outcome>,
    }
    variant loop-input {
        start(loop-input-start),
        resume(loop-input-resume),
    }
    record run-start-event { thread-id: string, run-id: string, metadata-json: option<string> }
    record delta-event { content: string, role: option<string> }
    record tool-call-start-event { id: string, name: string }
    record tool-call-delta-event { id: string, arguments-delta: string }
    record tool-delta-event { id: string, name: string, delta: string }
    record tool-result-event { id: string, name: string, value: string }
    record interrupt-event { interrupts: list<interrupt-info> }
    record turn-start-event { turn: u32 }
    record usage-event { prompt-tokens: u32, completion-tokens: u32 }
    record error-event { message: string, code: option<string> }
    record need-tool-execution-event {
        state: agent-state,
        tool-calls: list<parsed-tool-call>,
        completed-results: list<tool-call-outcome>,
    }
    variant protocol-event {
        run-start(run-start-event),
        delta(delta-event),
        tool-call-start(tool-call-start-event),
        tool-call-delta(tool-call-delta-event),
        tool-delta(tool-delta-event),
        tool-result(tool-result-event),
        interrupt(interrupt-event),
        turn-start(turn-start-event),
        usage(usage-event),
        %error(error-event),
        done,
        need-tool-execution(need-tool-execution-event),
    }
    resource event-stream {
        next: func() -> option<protocol-event>;
    }
    chat: func(input: loop-input) -> result<event-stream, string>;
}

world agent-world {
    import config;
    export agent;
    export agent-info;
}
";

// ── generate! macro ──────────────────────────────────────────────────────────

/// Generate WIT component-model bindings for the remi agent world.
///
/// Most users should prefer [`export_agent!`] which calls this internally.
#[macro_export]
macro_rules! generate {
    () => {
        $crate::_wit_bindgen::generate!({
            inline: $crate::WIT_INLINE,
        });
    };
}

// ── export_agent! macro ──────────────────────────────────────────────────────

/// Wire up a [`GuestAgent`] as a WASM component export.
///
/// This macro:
/// 1. Generates WIT bindings for the typed `agent-world`.
/// 2. Creates `RemiEventStream` backed by `Vec<ProtocolEvent>` — converts each
///    event to a WIT variant on `next()`, eliminating per-event JSON encoding.
/// 3. Implements the `agent` and `agent-info` WIT exports on an internal adapter.
/// 4. Exposes a `get_config() -> AgentConfig` free function that pulls from the
///    host config import — callable from within `GuestAgent::chat`.
/// 5. Emits component-model ABI entry points via `export!(...)`.
///
/// # Version arguments (optional)
///
/// ```rust,ignore
/// remi_agentloop_guest::export_agent!(
///     MyAgent,
///     api_version: (0, 1, 0),   // API version this guest was built against
///     min_host: (0, 1, 0),      // minimum host version required
/// );
/// ```
#[macro_export]
macro_rules! export_agent {
    ($agent:ident) => {
        $crate::export_agent!($agent, api_version: (0, 1, 0), min_host: (0, 1, 0));
    };

    ($agent:ident, api_version: ($av_maj:expr, $av_min:expr, $av_patch:expr), min_host: ($mh_maj:expr, $mh_min:expr, $mh_patch:expr)) => {

        // ── 1. WIT bindings ──────────────────────────────────────────────────
        $crate::_wit_bindgen::generate!({
            inline: $crate::WIT_INLINE,
        });

        // ── 2. Event stream resource ─────────────────────────────────────────
        pub struct RemiEventStream {
            events: Vec<$crate::ProtocolEvent>,
            cursor: std::cell::Cell<usize>,
        }

        // ── 3. Type conversion helpers (Rust ↔ WIT-generated types) ─────────
        mod __remi_convert {
            use super::*;
            use exports::remi::agentloop::agent as wit;

            // ── content ──────────────────────────────────────────────────────

            pub fn rust_part_to_wit(p: $crate::ContentPart) -> wit::ContentPart {
                match p {
                    $crate::ContentPart::Text { text } => wit::ContentPart::Text(text),
                    $crate::ContentPart::ImageUrl { image_url } => {
                        wit::ContentPart::ImageUrl(wit::ImageUrlDetail {
                            url: image_url.url,
                            detail: image_url.detail,
                        })
                    }
                    $crate::ContentPart::ImageBase64 { media_type, data } => {
                        wit::ContentPart::ImageBase64((media_type, data))
                    }
                    $crate::ContentPart::Audio { input_audio } => {
                        wit::ContentPart::Audio(wit::AudioDetail {
                            data: input_audio.data,
                            format: input_audio.format,
                        })
                    }
                    other => wit::ContentPart::FileJson(
                        $crate::_serde_json::to_string(&other).unwrap_or_default(),
                    ),
                }
            }

            pub fn rust_content_to_wit(c: $crate::Content) -> wit::Content {
                match c {
                    $crate::Content::Text(s) => wit::Content::Text(s),
                    $crate::Content::Parts(ps) => {
                        wit::Content::Parts(ps.into_iter().map(rust_part_to_wit).collect())
                    }
                }
            }

            pub fn wit_part_to_rust(p: wit::ContentPart) -> $crate::ContentPart {
                match p {
                    wit::ContentPart::Text(s) => $crate::ContentPart::Text { text: s },
                    wit::ContentPart::ImageUrl(d) => $crate::ContentPart::ImageUrl {
                        image_url: $crate::ImageUrlDetail { url: d.url, detail: d.detail },
                    },
                    wit::ContentPart::ImageBase64((mt, data)) => {
                        $crate::ContentPart::ImageBase64 { media_type: mt, data }
                    }
                    wit::ContentPart::Audio(d) => $crate::ContentPart::Audio {
                        input_audio: $crate::AudioDetail { data: d.data, format: d.format },
                    },
                    wit::ContentPart::FileJson(j) => {
                        $crate::_serde_json::from_str(&j)
                            .unwrap_or($crate::ContentPart::Text { text: j })
                    }
                }
            }

            pub fn wit_content_to_rust(c: wit::Content) -> $crate::Content {
                match c {
                    wit::Content::Text(s) => $crate::Content::Text(s),
                    wit::Content::Parts(ps) => {
                        $crate::Content::Parts(ps.into_iter().map(wit_part_to_rust).collect())
                    }
                }
            }

            // ── message ──────────────────────────────────────────────────────

            fn role_str(r: &$crate::Role) -> &'static str {
                match r {
                    $crate::Role::System    => "system",
                    $crate::Role::User      => "user",
                    $crate::Role::Assistant => "assistant",
                    $crate::Role::Tool      => "tool",
                }
            }

            fn role_from_str(s: &str) -> $crate::Role {
                match s {
                    "system"    => $crate::Role::System,
                    "user"      => $crate::Role::User,
                    "assistant" => $crate::Role::Assistant,
                    "tool"      => $crate::Role::Tool,
                    _           => $crate::Role::User,
                }
            }

            pub fn wit_msg_to_rust(m: wit::Message) -> $crate::Message {
                $crate::Message {
                    id: $crate::MessageId(m.id),
                    role: role_from_str(&m.role),
                    content: wit_content_to_rust(m.content),
                    tool_calls: m.tool_calls.map(|tcs| {
                        tcs.into_iter().map(|tc| $crate::ToolCallMessage {
                            id: tc.id,
                            call_type: tc.call_type,
                            function: $crate::FunctionCall { name: tc.name, arguments: tc.arguments },
                        }).collect()
                    }),
                    tool_call_id: m.tool_call_id,
                }
            }

            pub fn rust_msg_to_wit(m: $crate::Message) -> wit::Message {
                wit::Message {
                    id: m.id.0,
                    role: role_str(&m.role).to_owned(),
                    content: rust_content_to_wit(m.content),
                    tool_calls: m.tool_calls.map(|tcs| {
                        tcs.into_iter().map(|tc| wit::ToolCallMessage {
                            id: tc.id,
                            call_type: tc.call_type,
                            name: tc.function.name,
                            arguments: tc.function.arguments,
                        }).collect()
                    }),
                    tool_call_id: m.tool_call_id,
                }
            }

            pub fn wit_tool_def_to_rust(td: wit::ToolDefinition) -> $crate::ToolDefinition {
                $crate::ToolDefinition {
                    tool_type: "function".into(),
                    function: $crate::FunctionDefinition {
                        name: td.name,
                        description: td.description,
                        parameters: $crate::_serde_json::from_str(&td.parameters_schema_json)
                            .unwrap_or($crate::_serde_json::Value::Null),
                    },
                }
            }

            pub fn rust_tool_def_to_wit(td: $crate::ToolDefinition) -> wit::ToolDefinition {
                wit::ToolDefinition {
                    name: td.function.name,
                    description: td.function.description,
                    parameters_schema_json: $crate::_serde_json::to_string(&td.function.parameters)
                        .unwrap_or_default(),
                }
            }

            pub fn wit_outcome_to_rust(o: wit::ToolCallOutcome) -> $crate::ToolCallOutcome {
                match o {
                    wit::ToolCallOutcome::Result(r) => $crate::ToolCallOutcome::Result {
                        tool_call_id: r.tool_call_id, tool_name: r.tool_name, result: r.value,
                    },
                    wit::ToolCallOutcome::Error(e) => $crate::ToolCallOutcome::Error {
                        tool_call_id: e.tool_call_id, tool_name: e.tool_name, error: e.err_msg,
                    },
                }
            }

            pub fn rust_outcome_to_wit(o: $crate::ToolCallOutcome) -> wit::ToolCallOutcome {
                match o {
                    $crate::ToolCallOutcome::Result { tool_call_id, tool_name, result } => {
                        wit::ToolCallOutcome::Result(wit::ToolCallResult { tool_call_id, tool_name, value: result })
                    }
                    $crate::ToolCallOutcome::Error { tool_call_id, tool_name, error } => {
                        wit::ToolCallOutcome::Error(wit::ToolCallError { tool_call_id, tool_name, err_msg: error })
                    }
                }
            }

            fn wit_step_cfg_to_rust(c: wit::StepConfig) -> $crate::StepConfig {
                $crate::StepConfig {
                    model: c.model,
                    temperature: c.temperature,
                    max_tokens: c.max_tokens,
                    metadata: c.metadata_json
                        .and_then(|j| $crate::_serde_json::from_str(&j).ok()),
                }
            }

            fn rust_step_cfg_to_wit(c: $crate::StepConfig) -> wit::StepConfig {
                wit::StepConfig {
                    model: c.model,
                    temperature: c.temperature,
                    max_tokens: c.max_tokens,
                    metadata_json: c.metadata
                        .map(|v| $crate::_serde_json::to_string(&v).unwrap_or_default()),
                }
            }

            fn wit_interrupt_to_rust(i: wit::InterruptInfo) -> $crate::InterruptInfo {
                $crate::InterruptInfo {
                    interrupt_id: $crate::InterruptId(i.interrupt_id),
                    tool_call_id: i.tool_call_id,
                    tool_name: i.tool_name,
                    kind: i.kind,
                    data: $crate::_serde_json::from_str(&i.data_json)
                        .unwrap_or($crate::_serde_json::Value::Null),
                }
            }

            fn rust_interrupt_to_wit(i: $crate::InterruptInfo) -> wit::InterruptInfo {
                wit::InterruptInfo {
                    interrupt_id: i.interrupt_id.0,
                    tool_call_id: i.tool_call_id,
                    tool_name: i.tool_name,
                    kind: i.kind,
                    data_json: $crate::_serde_json::to_string(&i.data).unwrap_or_default(),
                }
            }

            fn wit_state_to_rust(s: wit::AgentState) -> $crate::AgentState {
                $crate::AgentState {
                    messages: s.messages.into_iter().map(wit_msg_to_rust).collect(),
                    system_prompt: s.system_prompt,
                    tool_definitions: s.tool_definitions.into_iter().map(wit_tool_def_to_rust).collect(),
                    config: wit_step_cfg_to_rust(s.config),
                    thread_id: $crate::ThreadId(s.thread_id),
                    run_id: $crate::RunId(s.run_id),
                    turn: s.turn as usize,
                    phase: $crate::_serde_json::from_str(&s.phase_json)
                        .unwrap_or($crate::AgentPhase::Error),
                    user_state: $crate::_serde_json::from_str(&s.user_state_json)
                        .unwrap_or($crate::_serde_json::Value::Null),
                }
            }

            fn rust_state_to_wit(s: $crate::AgentState) -> wit::AgentState {
                wit::AgentState {
                    messages: s.messages.into_iter().map(rust_msg_to_wit).collect(),
                    system_prompt: s.system_prompt,
                    tool_definitions: s.tool_definitions.into_iter().map(rust_tool_def_to_wit).collect(),
                    config: rust_step_cfg_to_wit(s.config),
                    thread_id: s.thread_id.0,
                    run_id: s.run_id.0,
                    turn: s.turn as u32,
                    phase_json: $crate::_serde_json::to_string(&s.phase).unwrap_or_default(),
                    user_state_json: $crate::_serde_json::to_string(&s.user_state).unwrap_or_default(),
                }
            }

            // ── LoopInput: WIT → Rust ─────────────────────────────────────

            pub fn wit_input_to_rust(input: wit::LoopInput) -> $crate::LoopInput {
                match input {
                    wit::LoopInput::Start(s) => $crate::LoopInput::Start {
                        content: wit_content_to_rust(s.content),
                        history: s.history.into_iter().map(wit_msg_to_rust).collect(),
                        extra_tools: s.extra_tools.into_iter().map(wit_tool_def_to_rust).collect(),
                        model: s.model,
                        temperature: s.temperature,
                        max_tokens: s.max_tokens,
                        metadata: s.metadata_json
                            .and_then(|j| $crate::_serde_json::from_str(&j).ok()),
                    },
                    wit::LoopInput::Resume(r) => $crate::LoopInput::Resume {
                        state: wit_state_to_rust(r.state),
                        results: r.results.into_iter().map(wit_outcome_to_rust).collect(),
                    },
                }
            }

            // ── ProtocolEvent: Rust → WIT ─────────────────────────────────

            pub fn rust_event_to_wit(event: $crate::ProtocolEvent) -> wit::ProtocolEvent {
                match event {
                    $crate::ProtocolEvent::RunStart { thread_id, run_id, metadata } => {
                        wit::ProtocolEvent::RunStart(wit::RunStartEvent {
                            thread_id, run_id,
                            metadata_json: metadata.map(|v|
                                $crate::_serde_json::to_string(&v).unwrap_or_default()),
                        })
                    }
                    $crate::ProtocolEvent::Delta { content, role } => {
                        wit::ProtocolEvent::Delta(wit::DeltaEvent { content, role })
                    }
                    $crate::ProtocolEvent::ToolCallStart { id, name } => {
                        wit::ProtocolEvent::ToolCallStart(wit::ToolCallStartEvent { id, name })
                    }
                    $crate::ProtocolEvent::ToolCallDelta { id, arguments_delta } => {
                        wit::ProtocolEvent::ToolCallDelta(wit::ToolCallDeltaEvent { id, arguments_delta })
                    }
                    $crate::ProtocolEvent::ToolDelta { id, name, delta } => {
                        wit::ProtocolEvent::ToolDelta(wit::ToolDeltaEvent { id, name, delta })
                    }
                    $crate::ProtocolEvent::ToolResult { id, name, result } => {
                        wit::ProtocolEvent::ToolResult(wit::ToolResultEvent { id, name, value: result })
                    }
                    $crate::ProtocolEvent::Interrupt { interrupts } => {
                        wit::ProtocolEvent::Interrupt(wit::InterruptEvent {
                            interrupts: interrupts.into_iter().map(rust_interrupt_to_wit).collect(),
                        })
                    }
                    $crate::ProtocolEvent::TurnStart { turn } => {
                        wit::ProtocolEvent::TurnStart(wit::TurnStartEvent { turn: turn as u32 })
                    }
                    $crate::ProtocolEvent::Usage { prompt_tokens, completion_tokens } => {
                        wit::ProtocolEvent::Usage(wit::UsageEvent { prompt_tokens, completion_tokens })
                    }
                    $crate::ProtocolEvent::Error { message, code } => {
                        wit::ProtocolEvent::Error(wit::ErrorEvent { message, code })
                    }
                    $crate::ProtocolEvent::Done => wit::ProtocolEvent::Done,
                    $crate::ProtocolEvent::NeedToolExecution { state, tool_calls, completed_results } => {
                        wit::ProtocolEvent::NeedToolExecution(wit::NeedToolExecutionEvent {
                            state: rust_state_to_wit(state),
                            tool_calls: tool_calls.into_iter().map(|tc| wit::ParsedToolCall {
                                id: tc.id, name: tc.name,
                                arguments_json: $crate::_serde_json::to_string(&tc.arguments)
                                    .unwrap_or_default(),
                            }).collect(),
                            completed_results: completed_results.into_iter().map(rust_outcome_to_wit).collect(),
                        })
                    }
                }
            }

            // ── AgentConfig: WIT import → Rust ────────────────────────────

            pub fn wit_cfg_to_rust(c: remi::agentloop::config::AgentConfig) -> $crate::AgentConfig {
                $crate::AgentConfig {
                    api_key:     c.api_key,
                    model:       c.model,
                    base_url:    c.base_url,
                    temperature: c.temperature,
                    max_tokens:  c.max_tokens,
                    timeout_ms:  c.timeout_ms,
                    headers: c.headers_json
                        .and_then(|j| $crate::_serde_json::from_str(&j).ok())
                        .unwrap_or_default(),
                    extra: c.extra_json
                        .and_then(|j| $crate::_serde_json::from_str(&j).ok())
                        .unwrap_or($crate::_serde_json::Value::Null),
                }
            }
        } // mod __remi_convert

        // ── 4. GuestEventStream ──────────────────────────────────────────────

        impl exports::remi::agentloop::agent::GuestEventStream for RemiEventStream {
            fn next(&self) -> Option<exports::remi::agentloop::agent::ProtocolEvent> {
                let i = self.cursor.get();
                if i < self.events.len() {
                    self.cursor.set(i + 1);
                    Some(__remi_convert::rust_event_to_wit(self.events[i].clone()))
                } else {
                    None
                }
            }
        }

        // ── 5. WIT export adapter ────────────────────────────────────────────
        //
        // A single zero-sized struct implements all WIT export traits so the
        // user-facing `$agent` only needs to implement `GuestAgent`.

        struct __RemiExports;

        impl exports::remi::agentloop::agent::Guest for __RemiExports {
            type EventStream = RemiEventStream;

            fn chat(
                input: exports::remi::agentloop::agent::LoopInput,
            ) -> Result<exports::remi::agentloop::agent::EventStream, String> {
                let rust_input = __remi_convert::wit_input_to_rust(input);
                let agent = <$agent as std::default::Default>::default();
                let events = $crate::_futures::executor::block_on(
                    $crate::GuestAgent::chat(&agent, rust_input),
                )?;
                Ok(exports::remi::agentloop::agent::EventStream::new(RemiEventStream {
                    events,
                    cursor: std::cell::Cell::new(0),
                }))
            }
        }

        impl exports::remi::agentloop::agent_info::Guest for __RemiExports {
            fn get_api_version() -> exports::remi::agentloop::agent_info::ApiVersion {
                exports::remi::agentloop::agent_info::ApiVersion {
                    major: $av_maj, minor: $av_min, patch: $av_patch,
                }
            }
            fn get_min_host_version() -> exports::remi::agentloop::agent_info::ApiVersion {
                exports::remi::agentloop::agent_info::ApiVersion {
                    major: $mh_maj, minor: $mh_min, patch: $mh_patch,
                }
            }
        }

        // ── 6. get_config() free function ────────────────────────────────────
        //
        // Pulls runtime AgentConfig from the host's `config` import.
        // Available in the module where `export_agent!` is called.

        /// Pull the current agent configuration from the host.
        ///
        /// Calls the host-provided `remi:agentloop/config::get-config` WIT import.
        /// The host resolves this lazily, so repeated calls reflect live updates.
        pub fn get_config() -> $crate::AgentConfig {
            __remi_convert::wit_cfg_to_rust(remi::agentloop::config::get_config())
        }

        // ── 7. ABI entry points ──────────────────────────────────────────────

        export!(__RemiExports);
    };
}

// ── Prelude ─────────────────────────────────────────────────────────────────

pub mod prelude {
    pub use crate::types::*;
    pub use crate::GuestAgent;
    pub use crate::_futures::stream::StreamExt as FuturesStreamExt;
}
