//! WASM component runner for remi-agentloop.
//!
//! This crate provides [`WasmAgent`], which loads a WASM component implementing
//! the typed `remi:agentloop/agent-world` WIT world and exposes it as an [`Agent`].
//!
//! # Changes from the string-based v0 ABI
//!
//! - **Typed events**: `chat()` now passes a typed `LoopInput` variant and
//!   `next()` returns an `Option<WitProtocolEvent>` — no per-event JSON.
//! - **Config injection**: the host supplies an [`AgentConfig`] via the
//!   `remi:agentloop/config` import.  Use [`WasmAgent::with_config`] or
//!   [`WasmAgent::with_dynamic_config`] to customise.
//! - **Version check**: on first instantiation the runner reads the guest's
//!   `agent-info` exports and validates compatibility before calling `chat()`.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │  Host (WasmAgent)                                        │
//! │  Agent::chat(LoopInput)                                  │
//! │    ↓ convert Rust→WIT (typed, zero JSON at boundary)     │
//! │    ↓ call guest chat(typed input)                        │
//! │    ← event-stream resource                               │
//! │    loop {                                                │
//! │      next() → Some(WitProtocolEvent) → convert → yield  │
//! │      next() → None                   → end              │
//! │    }                                                     │
//! │    → Stream<ProtocolEvent>                               │
//! └──────────────────────────────────────────────────────────┘
//!            ↕ Component Model ABI (typed, no JSON)
//! ┌──────────────────────────────────────────────────────────┐
//! │  Guest (.wasm component)                                 │
//! │  config import  → calls host get_config() on demand      │
//! │  chat(typed input) → event-stream resource               │
//! │  next() → Some(typed event) | None                       │
//! └──────────────────────────────────────────────────────────┘
//! ```

#[cfg(feature = "http-host")]
mod http_host;
#[cfg(feature = "http-host")]
pub use http_host::WasmAgentWithHttp;

use std::path::Path;
use std::sync::{Arc, RwLock};

use remi_agentloop::agent::Agent;
use remi_agentloop::protocol::{ProtocolError, ProtocolEvent};
use remi_agentloop::types::LoopInput;
use remi_core::config::AgentConfig;

use wasmtime::component::{Component, Linker};
use wasmtime::{Config as WasmtimeConfig, Engine, Store};

// ── Host API version ─────────────────────────────────────────────────────────

/// The API version this host runner implements.
///
/// Compatibility rules enforced before the first `chat()` call:
/// - `guest.api_version.major == HOST_API_VERSION.major`
/// - `guest.min_host_version <= HOST_API_VERSION`
pub const HOST_API_VERSION: (u32, u32, u32) = (0, 1, 0);

// ── Host-side WIT bindings ──────────────────────────────────────────────────

wasmtime::component::bindgen!({
    path: "../wit",
    world: "agent-world",
});

// ── Config source trait ──────────────────────────────────────────────────────

/// Provides `AgentConfig` to the WASM guest's `config` import.
///
/// Implement this trait to feed configuration from any source (env vars,
/// config files, a live database, etc.) to the guest at runtime.
pub trait WasmConfigSource: Send + Sync + 'static {
    fn get_config(&self) -> AgentConfig;
}

/// Config source backed by a static, cloned `AgentConfig`.
pub struct StaticConfigSource(pub AgentConfig);

impl WasmConfigSource for StaticConfigSource {
    fn get_config(&self) -> AgentConfig {
        self.0.clone()
    }
}

/// Config source backed by a shared `RwLock<AgentConfig>`, enabling live updates.
///
/// Call `source.update(new_config)` from any thread to change the config
/// returned on the guest's next `get-config` call.
pub struct DynamicConfigSource(pub Arc<RwLock<AgentConfig>>);

impl DynamicConfigSource {
    pub fn new(initial: AgentConfig) -> Self {
        Self(Arc::new(RwLock::new(initial)))
    }

    pub fn update(&self, config: AgentConfig) {
        if let Ok(mut guard) = self.0.write() {
            *guard = config;
        }
    }

    pub fn handle(&self) -> Arc<RwLock<AgentConfig>> {
        self.0.clone()
    }
}

impl WasmConfigSource for DynamicConfigSource {
    fn get_config(&self) -> AgentConfig {
        self.0.read().map(|g| g.clone()).unwrap_or_default()
    }
}

// ── Host state ───────────────────────────────────────────────────────────────

/// State held by the WASM store during guest execution.
///
/// The `config_source` field is called by the WIT `config` import handler
/// every time the guest calls `get-config`.
struct HostState {
    config_source: Box<dyn WasmConfigSource>,
}

// Implement the generated trait for the `remi:agentloop/config` import.
impl remi::agentloop::config::Host for HostState {
    fn get_config(&mut self) -> remi::agentloop::config::AgentConfig {
        let cfg = self.config_source.get_config();
        rust_config_to_wit(cfg)
    }
}

/// Designator for linking the `remi:agentloop/config` import.
struct ConfigDesignator;
impl wasmtime::component::HasData for ConfigDesignator {
    type Data<'a> = &'a mut HostState;
}

// ── Rust ↔ WIT conversions ───────────────────────────────────────────────────
//
// These free functions convert between remi-core Rust types and the WIT-
// generated types produced by `wasmtime::component::bindgen!`.
//
// Convention:
//  - `rust_*_to_wit` : Rust (host) → WIT (passed to / received from guest)
//  - `wit_*_to_rust` : WIT (received from guest) → Rust (host protocol types)

fn rust_config_to_wit(c: AgentConfig) -> remi::agentloop::config::AgentConfig {
    remi::agentloop::config::AgentConfig {
        api_key: c.api_key,
        model: c.model,
        base_url: c.base_url,
        temperature: c.temperature,
        max_tokens: c.max_tokens,
        timeout_ms: c.timeout_ms,
        headers_json: if c.headers.is_empty() {
            None
        } else {
            serde_json::to_string(&c.headers).ok()
        },
        extra_json: if c.extra.is_null() {
            None
        } else {
            serde_json::to_string(&c.extra).ok()
        },
    }
}

// ── Content ──────────────────────────────────────────────────────────────────

use exports::remi::agentloop::agent as wit;
use remi_agentloop::types::{Content, ContentPart};

fn rust_content_part_to_wit(p: ContentPart) -> wit::ContentPart {
    match p {
        ContentPart::Text { text } => wit::ContentPart::Text(text),
        ContentPart::ImageUrl { image_url } => wit::ContentPart::ImageUrl(wit::ImageUrlDetail {
            url: image_url.url,
            detail: image_url.detail,
        }),
        ContentPart::ImageBase64 { media_type, data } => {
            wit::ContentPart::ImageBase64((media_type, data))
        }
        ContentPart::Audio { input_audio } => wit::ContentPart::Audio(wit::AudioDetail {
            data: input_audio.data,
            format: input_audio.format,
        }),
        other => wit::ContentPart::FileJson(serde_json::to_string(&other).unwrap_or_default()),
    }
}

fn rust_content_to_wit(c: Content) -> wit::Content {
    match c {
        Content::Text(s) => wit::Content::Text(s),
        Content::Parts(ps) => {
            wit::Content::Parts(ps.into_iter().map(rust_content_part_to_wit).collect())
        }
    }
}

fn role_str(r: &remi_agentloop::types::Role) -> String {
    match r {
        remi_agentloop::types::Role::System => "system".into(),
        remi_agentloop::types::Role::User => "user".into(),
        remi_agentloop::types::Role::Assistant => "assistant".into(),
        remi_agentloop::types::Role::Tool => "tool".into(),
    }
}

fn rust_msg_to_wit(m: remi_agentloop::types::Message) -> wit::Message {
    wit::Message {
        id: m.id.0.clone(),
        role: role_str(&m.role),
        content: rust_content_to_wit(m.content),
        tool_calls: m.tool_calls.map(|tcs| {
            tcs.into_iter()
                .map(|tc| wit::ToolCallMessage {
                    id: tc.id,
                    call_type: tc.call_type,
                    name: tc.function.name,
                    arguments: tc.function.arguments,
                })
                .collect()
        }),
        tool_call_id: m.tool_call_id,
    }
}

fn rust_tool_def_to_wit(td: remi_agentloop::tool::ToolDefinition) -> wit::ToolDefinition {
    wit::ToolDefinition {
        name: td.function.name,
        description: td.function.description,
        parameters_schema_json: serde_json::to_string(&td.function.parameters).unwrap_or_default(),
    }
}

fn rust_outcome_to_wit(o: remi_agentloop::types::ToolCallOutcome) -> wit::ToolCallOutcome {
    match o {
        remi_agentloop::types::ToolCallOutcome::Result {
            tool_call_id,
            tool_name,
            content,
        } => wit::ToolCallOutcome::Result(wit::ToolCallResult {
            tool_call_id,
            tool_name,
            content: rust_content_to_wit(content),
        }),
        remi_agentloop::types::ToolCallOutcome::Error {
            tool_call_id,
            tool_name,
            error,
        } => wit::ToolCallOutcome::Error(wit::ToolCallError {
            tool_call_id,
            tool_name,
            err_msg: error,
        }),
    }
}

fn rust_step_cfg_to_wit(c: remi_agentloop::state::StepConfig) -> wit::StepConfig {
    wit::StepConfig {
        model: c.model,
        temperature: c.temperature,
        max_tokens: c.max_tokens,
        metadata_json: c
            .metadata
            .map(|v| serde_json::to_string(&v).unwrap_or_default()),
    }
}

fn rust_state_to_wit(s: remi_agentloop::state::AgentState) -> wit::AgentState {
    wit::AgentState {
        messages: s.messages.into_iter().map(rust_msg_to_wit).collect(),
        system_prompt: s.system_prompt,
        tool_definitions: s
            .tool_definitions
            .into_iter()
            .map(rust_tool_def_to_wit)
            .collect(),
        config: rust_step_cfg_to_wit(s.config),
        thread_id: s.thread_id.0.clone(),
        run_id: s.run_id.0.clone(),
        turn: s.turn as u32,
        phase_json: serde_json::to_string(&s.phase).unwrap_or_default(),
        user_state_json: serde_json::to_string(&s.user_state).unwrap_or_default(),
    }
}

/// Convert a host-side `LoopInput` to the WIT-generated type for passing to
/// the guest's `chat()` export.
fn rust_loop_input_to_wit(input: LoopInput) -> wit::LoopInput {
    match input {
        LoopInput::Start {
            content,
            history,
            extra_tools,
            model,
            temperature,
            max_tokens,
            metadata,
        } => wit::LoopInput::Start(wit::LoopInputStart {
            content: rust_content_to_wit(content),
            history: history.into_iter().map(rust_msg_to_wit).collect(),
            extra_tools: extra_tools.into_iter().map(rust_tool_def_to_wit).collect(),
            model,
            temperature,
            max_tokens,
            metadata_json: metadata.map(|v| serde_json::to_string(&v).unwrap_or_default()),
        }),
        LoopInput::Resume { state, results } => wit::LoopInput::Resume(wit::LoopInputResume {
            state: rust_state_to_wit(state),
            results: results.into_iter().map(rust_outcome_to_wit).collect(),
        }),
        // Cancel has no WIT counterpart; handled by run_guest before this call.
        LoopInput::Cancel { .. } => {
            unreachable!("Cancel must be intercepted before rust_loop_input_to_wit")
        }
    }
}

// ── WIT → Rust (ProtocolEvent returned from guest) ──────────────────────────

fn wit_interrupt_to_rust(i: wit::InterruptInfo) -> remi_agentloop::types::InterruptInfo {
    remi_agentloop::types::InterruptInfo {
        interrupt_id: remi_agentloop::types::InterruptId(i.interrupt_id),
        tool_call_id: i.tool_call_id,
        tool_name: i.tool_name,
        kind: i.kind,
        data: serde_json::from_str(&i.data_json).unwrap_or(serde_json::Value::Null),
    }
}

fn wit_outcome_to_rust(o: wit::ToolCallOutcome) -> remi_agentloop::types::ToolCallOutcome {
    match o {
        wit::ToolCallOutcome::Result(r) => remi_agentloop::types::ToolCallOutcome::Result {
            tool_call_id: r.tool_call_id,
            tool_name: r.tool_name,
            content: wit_content_to_rust(r.content),
        },
        wit::ToolCallOutcome::Error(e) => remi_agentloop::types::ToolCallOutcome::Error {
            tool_call_id: e.tool_call_id,
            tool_name: e.tool_name,
            error: e.err_msg,
        },
    }
}

fn wit_content_part_to_rust(p: wit::ContentPart) -> ContentPart {
    match p {
        wit::ContentPart::Text(s) => ContentPart::Text { text: s },
        wit::ContentPart::ImageUrl(d) => ContentPart::ImageUrl {
            image_url: remi_agentloop::types::ImageUrlDetail {
                url: d.url,
                detail: d.detail,
            },
        },
        wit::ContentPart::ImageBase64((mt, data)) => ContentPart::ImageBase64 {
            media_type: mt,
            data,
        },
        wit::ContentPart::Audio(d) => ContentPart::Audio {
            input_audio: remi_agentloop::types::AudioDetail {
                data: d.data,
                format: d.format,
            },
        },
        wit::ContentPart::FileJson(j) => {
            serde_json::from_str(&j).unwrap_or(ContentPart::Text { text: j })
        }
    }
}

fn wit_content_to_rust(c: wit::Content) -> Content {
    match c {
        wit::Content::Text(s) => Content::Text(s),
        wit::Content::Parts(ps) => {
            Content::Parts(ps.into_iter().map(wit_content_part_to_rust).collect())
        }
    }
}

fn role_from_str(s: &str) -> remi_agentloop::types::Role {
    match s {
        "system" => remi_agentloop::types::Role::System,
        "user" => remi_agentloop::types::Role::User,
        "assistant" => remi_agentloop::types::Role::Assistant,
        "tool" => remi_agentloop::types::Role::Tool,
        _ => remi_agentloop::types::Role::User,
    }
}

fn wit_msg_to_rust(m: wit::Message) -> remi_agentloop::types::Message {
    remi_agentloop::types::Message {
        id: remi_agentloop::types::MessageId(m.id),
        role: role_from_str(&m.role),
        content: wit_content_to_rust(m.content),
        tool_calls: m.tool_calls.map(|tcs| {
            tcs.into_iter()
                .map(|tc| remi_agentloop::types::ToolCallMessage {
                    id: tc.id,
                    call_type: tc.call_type,
                    function: remi_agentloop::types::FunctionCall {
                        name: tc.name,
                        arguments: tc.arguments,
                    },
                })
                .collect()
        }),
        tool_call_id: m.tool_call_id,
        reasoning_content: None,
    }
}

fn wit_tool_def_to_rust(td: wit::ToolDefinition) -> remi_agentloop::tool::ToolDefinition {
    remi_agentloop::tool::ToolDefinition {
        tool_type: "function".into(),
        function: remi_agentloop::tool::FunctionDefinition {
            name: td.name,
            description: td.description,
            parameters: serde_json::from_str(&td.parameters_schema_json)
                .unwrap_or(serde_json::Value::Null),
        },
    }
}

fn wit_step_cfg_to_rust(c: wit::StepConfig) -> remi_agentloop::state::StepConfig {
    remi_agentloop::state::StepConfig {
        model: c.model,
        temperature: c.temperature,
        max_tokens: c.max_tokens,
        metadata: c.metadata_json.and_then(|j| serde_json::from_str(&j).ok()),
    }
}

fn wit_state_to_rust(s: wit::AgentState) -> remi_agentloop::state::AgentState {
    remi_agentloop::state::AgentState {
        messages: s.messages.into_iter().map(wit_msg_to_rust).collect(),
        system_prompt: s.system_prompt,
        tool_definitions: s
            .tool_definitions
            .into_iter()
            .map(wit_tool_def_to_rust)
            .collect(),
        config: wit_step_cfg_to_rust(s.config),
        thread_id: remi_agentloop::types::ThreadId(s.thread_id),
        run_id: remi_agentloop::types::RunId(s.run_id),
        turn: s.turn as usize,
        phase: serde_json::from_str(&s.phase_json)
            .unwrap_or(remi_agentloop::state::AgentPhase::Error),
        user_state: serde_json::from_str(&s.user_state_json).unwrap_or(serde_json::Value::Null),
    }
}

/// Convert a WIT-typed `ProtocolEvent` (returned from `next()`) to the
/// host-side Rust `ProtocolEvent`.
fn wit_event_to_rust(event: wit::ProtocolEvent) -> ProtocolEvent {
    match event {
        wit::ProtocolEvent::RunStart(e) => ProtocolEvent::RunStart {
            thread_id: e.thread_id,
            run_id: e.run_id,
            metadata: e.metadata_json.and_then(|j| serde_json::from_str(&j).ok()),
        },
        wit::ProtocolEvent::Delta(e) => ProtocolEvent::Delta {
            content: e.content,
            role: e.role,
        },
        wit::ProtocolEvent::ToolCallStart(e) => ProtocolEvent::ToolCallStart {
            id: e.id,
            name: e.name,
        },
        wit::ProtocolEvent::ToolCallDelta(e) => ProtocolEvent::ToolCallDelta {
            id: e.id,
            arguments_delta: e.arguments_delta,
        },
        wit::ProtocolEvent::ToolDelta(e) => ProtocolEvent::ToolDelta {
            id: e.id,
            name: e.name,
            delta: e.delta,
        },
        wit::ProtocolEvent::ToolResult(e) => ProtocolEvent::ToolResult {
            id: e.id,
            name: e.name,
            result: e.value,
        },
        wit::ProtocolEvent::Interrupt(e) => ProtocolEvent::Interrupt {
            interrupts: e
                .interrupts
                .into_iter()
                .map(wit_interrupt_to_rust)
                .collect(),
        },
        wit::ProtocolEvent::TurnStart(e) => ProtocolEvent::TurnStart {
            turn: e.turn as usize,
        },
        wit::ProtocolEvent::Usage(e) => ProtocolEvent::Usage {
            prompt_tokens: e.prompt_tokens,
            completion_tokens: e.completion_tokens,
        },
        wit::ProtocolEvent::Error(e) => ProtocolEvent::Error {
            message: e.message,
            code: e.code,
        },
        wit::ProtocolEvent::Done => ProtocolEvent::Done,
        wit::ProtocolEvent::NeedToolExecution(e) => ProtocolEvent::NeedToolExecution {
            state: wit_state_to_rust(e.state),
            tool_calls: e
                .tool_calls
                .into_iter()
                .map(|tc| remi_agentloop::types::ParsedToolCall {
                    id: tc.id,
                    name: tc.name,
                    arguments: serde_json::from_str(&tc.arguments_json)
                        .unwrap_or(serde_json::Value::Null),
                })
                .collect(),
            completed_results: e
                .completed_results
                .into_iter()
                .map(wit_outcome_to_rust)
                .collect(),
        },
    }
}

// ── WasmAgent ────────────────────────────────────────────────────────────────

/// A WASM component implementing `remi:agentloop/agent-world`, wrapped as an
/// [`Agent`].
///
/// # Example
///
/// ```rust,ignore
/// use remi_agentloop::prelude::*;
/// use remi_agentloop_wasm::WasmAgent;
///
/// let agent = WasmAgent::from_file("my_agent.wasm")?;
/// let stream = agent.chat("Hello!".into()).await?;
/// ```
pub struct WasmAgent {
    engine: Engine,
    component: Component,
    config_source: Arc<dyn WasmConfigSource>,
}

impl WasmAgent {
    fn make_engine() -> Result<Engine, ProtocolError> {
        let mut cfg = WasmtimeConfig::new();
        cfg.wasm_component_model(true);
        Engine::new(&cfg).map_err(|e| ProtocolError {
            code: "engine_error".into(),
            message: e.to_string(),
        })
    }

    fn check_version(engine: &Engine, component: &Component) -> Result<(), ProtocolError> {
        // Instantiate once just to read version info, then discard.
        let mut store = Store::new(
            engine,
            HostState {
                config_source: Box::new(StaticConfigSource(AgentConfig::default())),
            },
        );
        let mut linker: Linker<HostState> = Linker::new(engine);
        remi::agentloop::config::add_to_linker::<HostState, ConfigDesignator>(&mut linker, |s| s)
            .map_err(|e| ProtocolError {
            code: "linker_error".into(),
            message: e.to_string(),
        })?;

        let bindings =
            AgentWorld::instantiate(&mut store, component, &linker).map_err(|e| ProtocolError {
                code: "instantiate_error".into(),
                message: e.to_string(),
            })?;

        let info = bindings.remi_agentloop_agent_info();
        let guest_ver = info
            .call_get_api_version(&mut store)
            .map_err(|e| ProtocolError {
                code: "version_error".into(),
                message: e.to_string(),
            })?;
        let min_host = info
            .call_get_min_host_version(&mut store)
            .map_err(|e| ProtocolError {
                code: "version_error".into(),
                message: e.to_string(),
            })?;

        let (h_maj, h_min, h_patch) = HOST_API_VERSION;

        // Major version must match exactly.
        if guest_ver.major != h_maj {
            return Err(ProtocolError {
                code: "version_incompatible".into(),
                message: format!(
                    "Guest API version {}.{}.{} is incompatible with host {}.{}.{}",
                    guest_ver.major, guest_ver.minor, guest_ver.patch, h_maj, h_min, h_patch,
                ),
            });
        }

        // Host must be at least as new as the guest requires.
        let host_ver = (h_maj, h_min, h_patch);
        let required = (min_host.major, min_host.minor, min_host.patch);
        if required > host_ver {
            return Err(ProtocolError {
                code: "host_too_old".into(),
                message: format!(
                    "Guest requires host >= {}.{}.{}, but host is {}.{}.{}",
                    min_host.major, min_host.minor, min_host.patch, h_maj, h_min, h_patch,
                ),
            });
        }

        Ok(())
    }

    fn new_with_source(
        engine: Engine,
        component: Component,
        config_source: Arc<dyn WasmConfigSource>,
    ) -> Result<Self, ProtocolError> {
        Self::check_version(&engine, &component)?;
        Ok(Self {
            engine,
            component,
            config_source,
        })
    }

    /// Load a WASM component from a file path.
    ///
    /// Requires the `compiler` feature (Cranelift JIT). Not available on Android.
    #[cfg(feature = "compiler")]
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ProtocolError> {
        let engine = Self::make_engine()?;
        let component = Component::from_file(&engine, path).map_err(|e| ProtocolError {
            code: "component_error".into(),
            message: e.to_string(),
        })?;
        Self::new_with_source(
            engine,
            component,
            Arc::new(StaticConfigSource(AgentConfig::default())),
        )
    }

    /// Load a WASM component from in-memory bytes.
    ///
    /// Auto-detects the artifact format from the magic bytes:
    /// - Raw WASM (`\0asm` header) → JIT-compiled via Cranelift. Requires the
    ///   `compiler` feature; not available on Android.
    /// - Everything else → treated as a precompiled `.cwasm` artifact and loaded
    ///   via `Component::deserialize`. No compiler needed; works on Android.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if !bytes.starts_with(b"\0asm") {
            return Self::from_precompiled_bytes(bytes);
        }
        #[cfg(not(feature = "compiler"))]
        return Err(ProtocolError {
            code: "no_compiler".into(),
            message: "Raw WASM bytes require the `compiler` feature (Cranelift JIT). \
                      Provide a precompiled .cwasm artifact for this platform."
                .into(),
        });
        #[cfg(feature = "compiler")]
        {
            let engine = Self::make_engine()?;
            let component = Component::new(&engine, bytes).map_err(|e| ProtocolError {
                code: "component_error".into(),
                message: e.to_string(),
            })?;
            Self::new_with_source(
                engine,
                component,
                Arc::new(StaticConfigSource(AgentConfig::default())),
            )
        }
    }

    /// Load from a pre-AOT-compiled `.cwasm` blob. No Cranelift needed at runtime.
    ///
    /// # Safety
    /// The bytes must be a trusted artifact produced by
    /// [`WasmAgent::precompile_for_target`] or `Engine::precompile_component`.
    pub fn from_precompiled_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let engine = Self::make_engine()?;
        // SAFETY: caller guarantees bytes are a valid, trusted wasmtime artifact.
        let component =
            unsafe { Component::deserialize(&engine, bytes) }.map_err(|e| ProtocolError {
                code: "component_error".into(),
                message: format!("Failed to deserialize precompiled WASM: {e}"),
            })?;
        Self::new_with_source(
            engine,
            component,
            Arc::new(StaticConfigSource(AgentConfig::default())),
        )
    }

    /// Cross-compile `.wasm` bytes to a precompiled `.cwasm` blob for `target_triple`.
    ///
    /// Requires the `compiler` feature (Cranelift cross-compilation).
    #[cfg(feature = "compiler")]
    pub fn precompile_for_target(
        wasm_bytes: &[u8],
        target_triple: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        let mut cfg = WasmtimeConfig::new();
        cfg.wasm_component_model(true);
        cfg.target(target_triple).map_err(|e| ProtocolError {
            code: "target_error".into(),
            message: format!("Unknown target triple '{target_triple}': {e}"),
        })?;
        let engine = Engine::new(&cfg).map_err(|e| ProtocolError {
            code: "engine_error".into(),
            message: e.to_string(),
        })?;
        engine
            .precompile_component(wasm_bytes)
            .map_err(|e| ProtocolError {
                code: "precompile_error".into(),
                message: format!("Precompile failed for '{target_triple}': {e}"),
            })
    }

    /// Override the config source with a static `AgentConfig`.
    pub fn with_config(self, config: AgentConfig) -> Self {
        Self {
            config_source: Arc::new(StaticConfigSource(config)),
            ..self
        }
    }

    /// Override the config source with a shared `DynamicConfigSource`.
    ///
    /// Updating the `DynamicConfigSource`'s inner value is immediately
    /// reflected on the guest's next `get-config` call.
    pub fn with_dynamic_config(self, source: Arc<RwLock<AgentConfig>>) -> Self {
        Self {
            config_source: Arc::new(DynamicConfigSource(source)),
            ..self
        }
    }

    /// Override the config source with any custom [`WasmConfigSource`].
    pub fn with_config_source(self, source: impl WasmConfigSource) -> Self {
        Self {
            config_source: Arc::new(source),
            ..self
        }
    }

    /// Instantiate the component and drive one full `chat` request.
    fn run_guest(&self, input: LoopInput) -> Result<Vec<ProtocolEvent>, ProtocolError> {
        // Cancel is a host-side concept; the guest has no Cancel variant.
        if let LoopInput::Cancel { .. } = &input {
            return Err(ProtocolError {
                code: "cancelled".into(),
                message: "Agent run was cancelled before reaching the guest".into(),
            });
        }
        // Clone the config snapshot for this invocation.
        let config_snapshot = self.config_source.get_config();

        let mut store = Store::new(
            &self.engine,
            HostState {
                config_source: Box::new(StaticConfigSource(config_snapshot)),
            },
        );

        let mut linker: Linker<HostState> = Linker::new(&self.engine);

        // Wire up the `config` import host implementation.
        remi::agentloop::config::add_to_linker::<HostState, ConfigDesignator>(&mut linker, |s| s)
            .map_err(|e| ProtocolError {
            code: "linker_error".into(),
            message: e.to_string(),
        })?;

        let bindings =
            AgentWorld::instantiate(&mut store, &self.component, &linker).map_err(|e| {
                ProtocolError {
                    code: "instantiate_error".into(),
                    message: e.to_string(),
                }
            })?;

        // Convert Rust LoopInput → WIT typed variant.
        let wit_input = rust_loop_input_to_wit(input);

        let agent_iface = bindings.remi_agentloop_agent();
        let stream_resource = agent_iface
            .call_chat(&mut store, &wit_input)
            .map_err(|e| ProtocolError {
                code: "call_error".into(),
                message: e.to_string(),
            })?
            .map_err(|e| ProtocolError {
                code: "guest_error".into(),
                message: e,
            })?;

        // Pull typed events via `next()`.
        let mut events = Vec::new();
        loop {
            let maybe_event = agent_iface
                .event_stream()
                .call_next(&mut store, stream_resource)
                .map_err(|e| ProtocolError {
                    code: "next_error".into(),
                    message: e.to_string(),
                })?;

            match maybe_event {
                Some(wit_event) => events.push(wit_event_to_rust(wit_event)),
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
            let events = self.run_guest(req)?;
            Ok(futures::stream::iter(events))
        }
    }
}
