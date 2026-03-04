//! LangSmith tracing backend.
//!
//! Posts a run tree to the [LangSmith](https://docs.smith.langchain.com/) REST
//! API.  All HTTP calls are dispatched through an internal mpsc channel to a
//! background Tokio task so tracing **never blocks** the agent loop.
//!
//! Requires the `tracing-langsmith` feature flag.
//!
//! # Example
//! ```no_run
//! use remi_agentloop::tracing::LangSmithTracer;
//!
//! let tracer = LangSmithTracer::new(std::env::var("LANGSMITH_API_KEY").unwrap())
//!     .with_project("my-agent");
//! ```

use std::future::Future;

use serde::Serialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::config::AgentConfig;
use crate::tracing::{
    InterruptTrace, ModelEndTrace, ModelStartTrace, ResumeTrace, RunEndTrace, RunStartTrace,
    RunStatus, ToolEndTrace, ToolStartTrace, Tracer, TurnStartTrace,
};

const DEFAULT_API_URL: &str = "https://api.smith.langchain.com";

// ── Internal message passed to the background sender ─────────────────────────

enum LangSmithMessage {
    /// Create a new run (HTTP POST /runs).
    Post {
        run: LangSmithRun,
        api_url: String,
        api_key: String,
    },
    /// Update an existing run (HTTP PATCH /runs/{id}).
    Patch {
        id: String,
        patch: serde_json::Value,
        api_url: String,
        api_key: String,
    },
}

// ── LangSmith run payload ─────────────────────────────────────────────────────

/// Wire format for POST /runs.
#[derive(Debug, Serialize)]
struct LangSmithRun {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_run_id: Option<String>,
    run_type: String,
    name: String,
    inputs: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    outputs: Option<serde_json::Value>,
    start_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra: Option<serde_json::Value>,
    session_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_metadata: Option<serde_json::Value>,
}

// ── LangSmithTracer ───────────────────────────────────────────────────────────

/// LangSmith tracing backend.
///
/// Maps the remi-agentloop trace events onto LangSmith's run-tree model:
///
/// ```text
/// Chain Run  (AgentLoop run, run_id)
///   ├── LLM Run   (model call, turn 0)
///   ├── Tool Run  (tool execution)
///   ├── LLM Run   (model call, turn 1)
///   └── ...
/// ```
///
/// Interrupt/resume is tracked as a **single** Chain Run — `on_interrupt`
/// patches the run status, `on_resume` clears `end_time` so the run shows
/// as still in-progress, and subsequent events keep appending to the same
/// chain.
pub struct LangSmithTracer {
    api_key: String,
    api_url: String,
    project_name: String,
    tx: mpsc::UnboundedSender<LangSmithMessage>,
}

impl LangSmithTracer {
    /// Create a new tracer.  Spawns a background Tokio task immediately.
    pub fn new(api_key: impl Into<String>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(Self::background_sender(rx));
        Self {
            api_key: api_key.into(),
            api_url: DEFAULT_API_URL.to_string(),
            project_name: "default".to_string(),
            tx,
        }
    }

    /// Set the LangSmith project (session) name.  Default: `"default"`.
    pub fn with_project(mut self, name: impl Into<String>) -> Self {
        self.project_name = name.into();
        self
    }

    /// Override the LangSmith API base URL.  Useful for self-hosted
    /// deployments.
    pub fn with_api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = url.into();
        self
    }

    /// Construct from an [`AgentConfig`], reading credentials from
    /// `config.extra`.
    ///
    /// Recognised keys:
    /// - `langsmith_api_key`  (required)
    /// - `langsmith_project`  (optional, default `"default"`)
    /// - `langsmith_api_url`  (optional)
    ///
    /// Returns `None` when `langsmith_api_key` is absent.
    pub fn from_config(config: &AgentConfig) -> Option<Self> {
        let api_key = config.extra.get("langsmith_api_key")?.as_str()?;
        let mut tracer = Self::new(api_key);
        if let Some(p) = config
            .extra
            .get("langsmith_project")
            .and_then(|v| v.as_str())
        {
            tracer = tracer.with_project(p);
        }
        if let Some(u) = config
            .extra
            .get("langsmith_api_url")
            .and_then(|v| v.as_str())
        {
            tracer = tracer.with_api_url(u);
        }
        Some(tracer)
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn post(&self, run: LangSmithRun) {
        let _ = self.tx.send(LangSmithMessage::Post {
            run,
            api_url: self.api_url.clone(),
            api_key: self.api_key.clone(),
        });
    }

    fn patch(&self, id: impl Into<String>, patch: serde_json::Value) {
        let _ = self.tx.send(LangSmithMessage::Patch {
            id: id.into(),
            patch,
            api_url: self.api_url.clone(),
            api_key: self.api_key.clone(),
        });
    }

    /// Derive a deterministic UUID for an LLM child run.
    ///
    /// Uses UUID v5 (namespace = parent run UUID, name = `"llm:<turn>"`) so
    /// `on_model_start` and `on_model_end` always produce the same child ID
    /// for the same run + turn without any shared mutable state.
    fn llm_run_id(run_id: &crate::types::RunId, turn: usize) -> String {
        let ns = Uuid::parse_str(&run_id.0).unwrap_or_else(|_| Uuid::nil());
        Uuid::new_v5(&ns, format!("llm:{turn}").as_bytes()).to_string()
    }

    /// Background task: drains the mpsc channel and performs HTTP calls.
    async fn background_sender(mut rx: mpsc::UnboundedReceiver<LangSmithMessage>) {
        let client = reqwest::Client::new();
        while let Some(msg) = rx.recv().await {
            match msg {
                LangSmithMessage::Post {
                    run,
                    api_url,
                    api_key,
                } => {
                    let url = format!("{api_url}/runs");
                    if let Err(e) = client
                        .post(&url)
                        .header("x-api-key", &api_key)
                        .json(&run)
                        .send()
                        .await
                    {
                        eprintln!("[langsmith] POST /runs error: {e}");
                    }
                }
                LangSmithMessage::Patch {
                    id,
                    patch,
                    api_url,
                    api_key,
                } => {
                    let url = format!("{api_url}/runs/{id}");
                    if let Err(e) = client
                        .patch(&url)
                        .header("x-api-key", &api_key)
                        .json(&patch)
                        .send()
                        .await
                    {
                        eprintln!("[langsmith] PATCH /runs/{id} error: {e}");
                    }
                }
            }
        }
    }
}

// ── Tracer impl ────────────────────────────────────────────────────────────────

impl Tracer for LangSmithTracer {
    /// POST a new Chain Run representing the full AgentLoop execution.
    fn on_run_start(&self, event: &RunStartTrace) -> impl Future<Output = ()> {
        self.post(LangSmithRun {
            id: event.run_id.0.clone(),
            parent_run_id: None,
            run_type: "chain".to_string(),
            name: "AgentLoop".to_string(),
            inputs: serde_json::json!({
                "messages": event.input_messages,
                "model": event.model,
            }),
            outputs: None,
            start_time: event.timestamp.to_rfc3339(),
            end_time: None,
            extra: Some(serde_json::json!({
                "system_prompt": event.system_prompt,
                "thread_id": event.thread_id,
            })),
            session_name: self.project_name.clone(),
            status: None,
            error: None,
            metadata: event.metadata.clone(),
            usage_metadata: None,
        });
        async {}
    }

    /// PATCH the Chain Run with final outputs, status, and token usage.
    fn on_run_end(&self, event: &RunEndTrace) -> impl Future<Output = ()> {
        let status = match &event.status {
            RunStatus::Completed => "success",
            RunStatus::Error | RunStatus::MaxTurnsExceeded => "error",
            // Interrupted = paused, not truly ended; see also on_resume.
            RunStatus::Interrupted => "interrupted",
        };
        self.patch(
            event.run_id.0.clone(),
            serde_json::json!({
                "outputs": {
                    "messages": event.output_messages,
                },
                "end_time": event.timestamp.to_rfc3339(),
                "status": status,
                "error": event.error,
                "usage_metadata": {
                    "prompt_tokens": event.total_prompt_tokens,
                    "completion_tokens": event.total_completion_tokens,
                    "total_tokens":
                        event.total_prompt_tokens + event.total_completion_tokens,
                },
            }),
        );
        async {}
    }

    /// POST a new LLM child run.  The UUID is derived deterministically from
    /// `run_id + turn` via UUID v5 so `on_model_start` / `on_model_end` always
    /// agree on the same ID without shared state.
    fn on_model_start(&self, event: &ModelStartTrace) -> impl Future<Output = ()> {
        let llm_id = Self::llm_run_id(&event.run_id, event.turn);
        self.post(LangSmithRun {
            id: llm_id,
            parent_run_id: Some(event.run_id.0.clone()),
            run_type: "llm".to_string(),
            name: event.model.clone(),
            inputs: serde_json::json!({
                "messages": event.messages,
                "tools": event.tools,
            }),
            outputs: None,
            start_time: event.timestamp.to_rfc3339(),
            end_time: None,
            extra: None,
            session_name: self.project_name.clone(),
            status: None,
            error: None,
            metadata: None,
            usage_metadata: None,
        });
        async {}
    }

    /// PATCH the LLM child run with its outputs and token usage.
    fn on_model_end(&self, event: &ModelEndTrace) -> impl Future<Output = ()> {
        let llm_id = Self::llm_run_id(&event.run_id, event.turn);
        self.patch(
            llm_id,
            serde_json::json!({
                "outputs": {
                    "response": event.response_text,
                    "tool_calls": event.tool_calls,
                },
                "end_time": event.timestamp.to_rfc3339(),
                "status": "success",
                "usage_metadata": {
                    "prompt_tokens": event.prompt_tokens,
                    "completion_tokens": event.completion_tokens,
                    "total_tokens": event.prompt_tokens + event.completion_tokens,
                },
            }),
        );
        async {}
    }

    /// POST a new Tool child run.  Uses `tool_call_id` as the run UUID since
    /// it is already unique within a run and stable across pause/resume.
    fn on_tool_start(&self, event: &ToolStartTrace) -> impl Future<Output = ()> {
        self.post(LangSmithRun {
            id: event.tool_call_id.clone(),
            parent_run_id: Some(event.run_id.0.clone()),
            run_type: "tool".to_string(),
            name: event.tool_name.clone(),
            inputs: serde_json::json!({
                "arguments": event.arguments,
            }),
            outputs: None,
            start_time: event.timestamp.to_rfc3339(),
            end_time: None,
            extra: None,
            session_name: self.project_name.clone(),
            status: None,
            error: None,
            metadata: None,
            usage_metadata: None,
        });
        async {}
    }

    /// PATCH the Tool child run with its result, status, and any error.
    fn on_tool_end(&self, event: &ToolEndTrace) -> impl Future<Output = ()> {
        let status = if event.error.is_some() {
            "error"
        } else {
            "success"
        };
        self.patch(
            event.tool_call_id.clone(),
            serde_json::json!({
                "outputs": {
                    "result": event.result,
                    "interrupted": event.interrupted,
                },
                "end_time": event.timestamp.to_rfc3339(),
                "status": status,
                "error": event.error,
            }),
        );
        async {}
    }

    /// PATCH the Chain Run to record interrupt details.
    ///
    /// The run is **not** ended here — `on_run_end(Interrupted)` follows
    /// shortly to update `status` and `end_time`.
    fn on_interrupt(&self, event: &InterruptTrace) -> impl Future<Output = ()> {
        self.patch(
            event.run_id.0.clone(),
            serde_json::json!({
                "extra": {
                    "interrupts": event.interrupts,
                    "interrupt_time": event.timestamp.to_rfc3339(),
                },
            }),
        );
        async {}
    }

    /// PATCH the Chain Run to signal that execution is continuing.
    ///
    /// Clears `end_time` and `status` so LangSmith shows the run as
    /// still in-progress, and records `resume_time` in `extra`.
    fn on_resume(&self, event: &ResumeTrace) -> impl Future<Output = ()> {
        self.patch(
            event.run_id.0.clone(),
            serde_json::json!({
                "extra": {
                    "resume_time": event.timestamp.to_rfc3339(),
                    "resume_payloads_count": event.payloads_count,
                },
                // Null clears these fields — signals the run is active again.
                "end_time": serde_json::Value::Null,
                "status": serde_json::Value::Null,
            }),
        );
        async {}
    }

    /// No-op: turns are implicit in the LangSmith run tree via the sequence
    /// of LLM child runs.
    fn on_turn_start(&self, _event: &TurnStartTrace) -> impl Future<Output = ()> {
        async {}
    }
}
