//! LangSmith tracing backend.
//!
//! Posts a run tree to the LangSmith REST API. All HTTP calls are dispatched
//! through an internal mpsc channel to a background Tokio task so tracing
//! never blocks the agent loop.

use std::collections::HashMap;
use std::future::Future;

use serde::Serialize;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::config::AgentConfig;
use crate::tracing::{
    ExternalToolResultTrace, InterruptTrace, ModelEndTrace, ModelStartTrace, ResumeTrace,
    RunEndTrace, RunStartTrace, RunStatus, ToolEndTrace, ToolExecutionHandoffTrace,
    ToolStartTrace, Tracer, TurnStartTrace,
};
use crate::types::{RunId, SpanKind, SpanNode};

const DEFAULT_API_URL: &str = "https://api.smith.langchain.com";

enum LangSmithMessage {
    Post {
        run: LangSmithRun,
        api_url: String,
        api_key: String,
    },
    Patch {
        id: String,
        patch: serde_json::Value,
        api_url: String,
        api_key: String,
    },
    Flush {
        close: bool,
        ack: oneshot::Sender<()>,
    },
}

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

pub struct LangSmithTracer {
    api_key: String,
    api_url: String,
    project_name: String,
    manage_root_run: bool,
    tx: std::sync::Mutex<Option<mpsc::UnboundedSender<LangSmithMessage>>>,
    handle: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    custom_event_counters: std::sync::Mutex<HashMap<String, usize>>,
}

impl LangSmithTracer {
    pub fn new(api_key: impl Into<String>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(Self::background_sender(rx));
        Self {
            api_key: api_key.into(),
            api_url: DEFAULT_API_URL.to_string(),
            project_name: "default".to_string(),
            manage_root_run: true,
            tx: std::sync::Mutex::new(Some(tx)),
            handle: std::sync::Mutex::new(Some(handle)),
            custom_event_counters: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn with_project(mut self, name: impl Into<String>) -> Self {
        self.project_name = name.into();
        self
    }

    pub fn with_api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = url.into();
        self
    }

    pub fn attach_to_existing_run(mut self) -> Self {
        self.manage_root_run = false;
        self
    }

    pub fn from_config(config: &AgentConfig) -> Option<Self> {
        let api_key = config.extra.get("langsmith_api_key")?.as_str()?;
        let mut tracer = Self::new(api_key);
        if let Some(project) = config
            .extra
            .get("langsmith_project")
            .and_then(|value| value.as_str())
        {
            tracer = tracer.with_project(project);
        }
        if let Some(url) = config
            .extra
            .get("langsmith_api_url")
            .and_then(|value| value.as_str())
        {
            tracer = tracer.with_api_url(url);
        }
        Some(tracer)
    }

    pub fn is_closed(&self) -> bool {
        self.tx
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(mpsc::UnboundedSender::is_closed))
            .unwrap_or(true)
    }

    pub async fn flush(&self) {
        self.send_flush_signal(false).await;
    }

    pub async fn close(&self) {
        self.send_flush_signal(true).await;
        if let Ok(mut guard) = self.tx.lock() {
            let _ = guard.take();
        }
        let handle = self.handle.lock().ok().and_then(|mut guard| guard.take());
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }

    fn post(&self, run: LangSmithRun) {
        let target = format!("run {} ({})", run.id, run.name);
        if let Ok(guard) = self.tx.lock() {
            if let Some(tx) = guard.as_ref() {
                if let Err(error) = tx.send(LangSmithMessage::Post {
                    run,
                    api_url: self.api_url.clone(),
                    api_key: self.api_key.clone(),
                }) {
                    Self::log_enqueue_error("POST /runs", &target, &error.to_string());
                }
            } else {
                Self::log_enqueue_error("POST /runs", &target, "sender channel already closed");
            }
        } else {
            Self::log_enqueue_error("POST /runs", &target, "sender mutex poisoned");
        }
    }

    fn patch(&self, id: impl Into<String>, patch: serde_json::Value) {
        let id = id.into();
        let target = format!("run {id}");
        if let Ok(guard) = self.tx.lock() {
            if let Some(tx) = guard.as_ref() {
                if let Err(error) = tx.send(LangSmithMessage::Patch {
                    id,
                    patch,
                    api_url: self.api_url.clone(),
                    api_key: self.api_key.clone(),
                }) {
                    Self::log_enqueue_error("PATCH /runs/{id}", &target, &error.to_string());
                }
            } else {
                Self::log_enqueue_error("PATCH /runs/{id}", &target, "sender channel already closed");
            }
        } else {
            Self::log_enqueue_error("PATCH /runs/{id}", &target, "sender mutex poisoned");
        }
    }

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
                    let target = format!("run {} ({})", run.id, run.name);
                    match client
                        .post(&url)
                        .header("x-api-key", &api_key)
                        .json(&run)
                        .send()
                        .await
                    {
                        Ok(response) if !response.status().is_success() => {
                            Self::log_http_failure("POST /runs", &target, response).await;
                        }
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!("[langsmith] POST /runs transport error for {target}: {error}");
                        }
                    }
                }
                LangSmithMessage::Patch {
                    id,
                    patch,
                    api_url,
                    api_key,
                } => {
                    let url = format!("{api_url}/runs/{id}");
                    let target = format!("run {id}");
                    match client
                        .patch(&url)
                        .header("x-api-key", &api_key)
                        .json(&patch)
                        .send()
                        .await
                    {
                        Ok(response) if !response.status().is_success() => {
                            Self::log_http_failure("PATCH /runs/{id}", &target, response).await;
                        }
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!("[langsmith] PATCH /runs/{id} transport error for {target}: {error}");
                        }
                    }
                }
                LangSmithMessage::Flush { close, ack } => {
                    let _ = ack.send(());
                    if close {
                        break;
                    }
                }
            }
        }
    }

    async fn send_flush_signal(&self, close: bool) {
        let (ack_tx, ack_rx) = oneshot::channel();
        let send_result = if let Ok(guard) = self.tx.lock() {
            if let Some(tx) = guard.as_ref() {
                tx.send(LangSmithMessage::Flush { close, ack: ack_tx })
                    .map_err(|error| error.to_string())
            } else {
                Err("sender channel already closed".to_string())
            }
        } else {
            Err("sender mutex poisoned".to_string())
        };

        if let Err(error) = send_result {
            Self::log_enqueue_error(
                if close { "flush(close)" } else { "flush" },
                "tracer queue",
                &error,
            );
            return;
        }

        if ack_rx.await.is_err() {
            eprintln!(
                "[langsmith] background sender dropped before acknowledging {}",
                if close { "close" } else { "flush" }
            );
        }
    }

    fn log_enqueue_error(operation: &str, target: &str, error: &str) {
        eprintln!("[langsmith] failed to enqueue {operation} for {target}: {error}");
    }

    async fn log_http_failure(operation: &str, target: &str, response: reqwest::Response) {
        let status = response.status();
        let body = match response.text().await {
            Ok(text) if !text.trim().is_empty() => text,
            Ok(_) => "<empty body>".to_string(),
            Err(error) => format!("<failed to read body: {error}>"),
        };
        eprintln!("[langsmith] {operation} failed for {target}: status={status}, body={body}");
    }

    fn namespaced_uuid(namespace: &str, name: &str) -> String {
        let ns = Uuid::parse_str(namespace).unwrap_or_else(|_| Uuid::nil());
        Uuid::new_v5(&ns, name.as_bytes()).to_string()
    }

    fn tool_run_id(run_id: &RunId, tool_call_id: &str) -> String {
        Self::namespaced_uuid(&run_id.0, &format!("tool:{tool_call_id}"))
    }

    fn post_tool_run(
        &self,
        run_id: &RunId,
        tool_call_id: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) {
        self.post(LangSmithRun {
            id: Self::tool_run_id(run_id, tool_call_id),
            parent_run_id: Some(run_id.0.clone()),
            run_type: "tool".to_string(),
            name: tool_name.to_string(),
            inputs: serde_json::json!({ "arguments": arguments }),
            outputs: None,
            start_time: timestamp.to_rfc3339(),
            end_time: None,
            extra: None,
            session_name: self.project_name.clone(),
            status: None,
            error: None,
            metadata: None,
            usage_metadata: None,
        });
    }

    fn patch_tool_run(
        &self,
        run_id: &RunId,
        tool_call_id: &str,
        result: Option<&str>,
        interrupted: bool,
        error: Option<&str>,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) {
        self.patch(
            Self::tool_run_id(run_id, tool_call_id),
            serde_json::json!({
                "outputs": {
                    "result": result,
                    "interrupted": interrupted,
                },
                "end_time": timestamp.to_rfc3339(),
                "status": if error.is_some() { "error" } else { "success" },
                "error": error,
            }),
        );
    }

    fn langsmith_run_type(kind: &SpanKind) -> &'static str {
        match kind {
            SpanKind::Model => "llm",
            SpanKind::Tool => "tool",
            SpanKind::Run | SpanKind::Subagent | SpanKind::Custom { .. } => "chain",
        }
    }

    fn span_parent_id(span: &SpanNode) -> Option<String> {
        span.parent.as_ref().map(|parent| parent.span_id.0.clone())
    }

    fn span_name(span: &SpanNode, fallback: &str) -> String {
        match &span.kind {
            SpanKind::Run | SpanKind::Model | SpanKind::Tool => fallback.to_string(),
            SpanKind::Subagent => span
                .scope_key
                .clone()
                .unwrap_or_else(|| "subagent".to_string()),
            SpanKind::Custom { name } => name.clone(),
        }
    }

    fn next_custom_event_index(&self, run_id: &str) -> usize {
        self.custom_event_counters
            .lock()
            .ok()
            .map(|mut guard| {
                let counter = guard.entry(run_id.to_string()).or_insert(0);
                let index = *counter;
                *counter += 1;
                index
            })
            .unwrap_or(0)
    }

    fn clear_run_state(&self, run_id: &RunId) {
        if let Ok(mut guard) = self.custom_event_counters.lock() {
            guard.remove(&run_id.0);
        }
    }

    fn emit_custom_under(&self, parent_run_id: &str, name: &str, data: &serde_json::Value) {
        let index = self.next_custom_event_index(parent_run_id);
        let custom_id = Self::namespaced_uuid(parent_run_id, &format!("custom:{name}:{index}"));
        let timestamp = chrono::Utc::now().to_rfc3339();
        self.post(LangSmithRun {
            id: custom_id,
            parent_run_id: Some(parent_run_id.to_string()),
            run_type: "chain".to_string(),
            name: format!("custom:{name}"),
            inputs: data.clone(),
            outputs: Some(serde_json::json!({ "custom_event": name })),
            start_time: timestamp.clone(),
            end_time: Some(timestamp),
            extra: Some(serde_json::json!({ "custom_event_name": name })),
            session_name: self.project_name.clone(),
            status: Some("success".to_string()),
            error: None,
            metadata: None,
            usage_metadata: None,
        });
    }

    fn should_skip_root_run(&self, span: &SpanNode) -> bool {
        !self.manage_root_run && span.parent.is_none()
    }
}

impl Tracer for LangSmithTracer {
    fn on_run_start(&self, event: &RunStartTrace) -> impl Future<Output = ()> {
        if self.should_skip_root_run(&event.span) {
            return async {};
        }
        self.post(LangSmithRun {
            id: event.span.span_id.0.clone(),
            parent_run_id: Self::span_parent_id(&event.span),
            run_type: Self::langsmith_run_type(&event.span.kind).to_string(),
            name: Self::span_name(&event.span, "AgentLoop"),
            inputs: serde_json::json!({
                "messages": event.input_messages,
                "model": event.model,
                "run_id": event.run_id,
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

    fn on_run_end(&self, event: &RunEndTrace) -> impl Future<Output = ()> {
        self.clear_run_state(&event.run_id);
        if self.should_skip_root_run(&event.span) {
            return async {};
        }
        self.patch(
            event.span.span_id.0.clone(),
            serde_json::json!({
                "outputs": {
                    "messages": event.output_messages,
                    "run_id": event.run_id,
                },
                "end_time": event.timestamp.to_rfc3339(),
                "status": match &event.status {
                    RunStatus::Completed => "success",
                    RunStatus::Cancelled | RunStatus::Interrupted => "interrupted",
                    RunStatus::Error | RunStatus::MaxTurnsExceeded => "error",
                },
                "error": event.error,
                "usage_metadata": {
                    "prompt_tokens": event.total_prompt_tokens,
                    "completion_tokens": event.total_completion_tokens,
                    "total_tokens": event.total_prompt_tokens + event.total_completion_tokens,
                },
            }),
        );
        async {}
    }

    fn on_model_start(&self, event: &ModelStartTrace) -> impl Future<Output = ()> {
        self.post(LangSmithRun {
            id: event.span.span_id.0.clone(),
            parent_run_id: Self::span_parent_id(&event.span),
            run_type: Self::langsmith_run_type(&event.span.kind).to_string(),
            name: Self::span_name(&event.span, &event.model),
            inputs: serde_json::json!({
                "messages": event.messages,
                "tools": event.tools,
                "run_id": event.run_id,
                "turn": event.turn,
                "call_index": event.call_index,
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

    fn on_model_end(&self, event: &ModelEndTrace) -> impl Future<Output = ()> {
        let response_text = event.response_text.clone().unwrap_or_default();
        let tool_calls = event
            .tool_calls
            .iter()
            .map(|tool_call| {
                serde_json::json!({
                    "id": tool_call.id,
                    "type": "function",
                    "function": {
                        "name": tool_call.name,
                        "arguments": serde_json::to_string(&tool_call.arguments)
                            .unwrap_or_else(|_| "{}".to_string()),
                    },
                })
            })
            .collect::<Vec<_>>();
        self.patch(
            event.span.span_id.0.clone(),
            serde_json::json!({
                "outputs": {
                    "response": event.response_text,
                    "text": response_text,
                    "tool_calls": tool_calls,
                    "run_id": event.run_id,
                    "turn": event.turn,
                    "call_index": event.call_index,
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

    fn on_tool_start(&self, event: &ToolStartTrace) -> impl Future<Output = ()> {
        self.post(LangSmithRun {
            id: event.span.span_id.0.clone(),
            parent_run_id: Self::span_parent_id(&event.span),
            run_type: Self::langsmith_run_type(&event.span.kind).to_string(),
            name: Self::span_name(&event.span, &event.tool_name),
            inputs: serde_json::json!({
                "arguments": event.arguments,
                "run_id": event.run_id,
                "turn": event.turn,
                "tool_call_id": event.tool_call_id,
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

    fn on_tool_end(&self, event: &ToolEndTrace) -> impl Future<Output = ()> {
        self.patch(
            event.span.span_id.0.clone(),
            serde_json::json!({
                "outputs": {
                    "result": event.result,
                    "interrupted": event.interrupted,
                    "run_id": event.run_id,
                    "turn": event.turn,
                    "tool_call_id": event.tool_call_id,
                },
                "end_time": event.timestamp.to_rfc3339(),
                "status": if event.error.is_some() { "error" } else { "success" },
                "error": event.error,
            }),
        );
        async {}
    }

    fn on_tool_execution_handoff(&self, event: &ToolExecutionHandoffTrace) -> impl Future<Output = ()> {
        for tool_call in &event.tool_calls {
            self.post_tool_run(
                &event.run_id,
                &tool_call.id,
                &tool_call.name,
                &tool_call.arguments,
                event.timestamp,
            );
        }
        async {}
    }

    fn on_external_tool_result(&self, event: &ExternalToolResultTrace) -> impl Future<Output = ()> {
        self.patch_tool_run(
            &event.run_id,
            &event.tool_call_id,
            event.result.as_deref(),
            false,
            event.error.as_deref(),
            event.timestamp,
        );
        async {}
    }

    fn on_interrupt(&self, event: &InterruptTrace) -> impl Future<Output = ()> {
        if self.should_skip_root_run(&event.span) {
            return async {};
        }
        self.patch(
            event.span.span_id.0.clone(),
            serde_json::json!({
                "extra": {
                    "interrupts": event.interrupts,
                    "interrupt_time": event.timestamp.to_rfc3339(),
                    "run_id": event.run_id,
                },
            }),
        );
        async {}
    }

    fn on_resume(&self, event: &ResumeTrace) -> impl Future<Output = ()> {
        if self.should_skip_root_run(&event.span) {
            return async {};
        }
        self.patch(
            event.span.span_id.0.clone(),
            serde_json::json!({
                "extra": {
                    "resume_time": event.timestamp.to_rfc3339(),
                    "resume_payloads_count": event.payloads_count,
                    "resume_outcomes": event.outcomes,
                    "run_id": event.run_id,
                },
                "end_time": serde_json::Value::Null,
                "status": serde_json::Value::Null,
            }),
        );
        async {}
    }

    fn on_turn_start(&self, _event: &TurnStartTrace) -> impl Future<Output = ()> {
        async {}
    }

    fn on_custom(&self, name: &str, data: &serde_json::Value) -> impl Future<Output = ()> {
        let run_id = data
            .get("run_id")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .or_else(|| {
                data.get("payload")
                    .and_then(|payload| payload.get("run_id"))
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string())
            });
        if let Some(run_id) = run_id {
            self.emit_custom_under(&run_id, name, data);
        }
        async {}
    }

    fn on_flush(&self) -> impl Future<Output = ()> {
        LangSmithTracer::flush(self)
    }
}
