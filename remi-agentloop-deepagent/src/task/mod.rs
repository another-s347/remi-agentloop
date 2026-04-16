//! `SubAgentTaskTool` — delegates tasks to a spawned inner agent.
//!
//! The tool accepts a `task` string, creates a fresh `AgentLoop` with bash
//! and filesystem tools, runs the task to completion, and returns the agent's
//! final text response.  This lets the outer agent offload focused subtasks
//! to a "worker" without polluting its own context.
//!
//! ## Claude Code inspiration
//!
//! Claude Code's "subagent" pattern: the orchestrator agent plans and breaks
//! down work, then delegates self-contained subtasks to isolated agents that
//! each get a clean context.  This prevents context bloat from large
//! intermediate outputs and allows independent retry of failed subtasks.

use async_stream::stream;
use futures::{Future, Stream, StreamExt};
use remi_core::agent::Agent;
use remi_core::builder::AgentBuilder;
use remi_core::checkpoint::Checkpoint;
use remi_core::error::AgentError;
use remi_core::model::ChatModel;
use remi_core::state::AgentState;
use remi_core::tool::{Tool, ToolOutput, ToolResult};
use remi_core::types::{
    AgentEvent, ChatCtx, Content, InterruptInfo, LoopInput, ParsedToolCall, ResumePayload,
    SubSessionEvent, SubSessionEventPayload, ToolCallOutcome,
};
use remi_tool::{
    BashTool, LocalFsCreateTool, LocalFsLsTool, LocalFsReadTool, LocalFsRemoveTool,
    LocalFsWriteTool,
};
use serde_json::json;
use std::pin::Pin;
use std::sync::Arc;

// ── Type alias ────────────────────────────────────────────────────────────────

/// Object-safe runner type: takes an owned task string, returns a boxed future.
pub type SubagentEventStream = Pin<Box<dyn Stream<Item = AgentEvent>>>;
pub type RunnerFn = dyn Fn(
        ChatCtx,
        String,
    ) -> Pin<Box<dyn Future<Output = Result<SubagentEventStream, AgentError>>>>
    + Send
    + Sync;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SubagentForwardEvent {
    RunStart {
        thread_id: String,
        run_id: String,
        metadata: Option<serde_json::Value>,
    },
    TextDelta {
        content: String,
    },
    ThinkingStart,
    ThinkingEnd {
        content: String,
    },
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        id: String,
        delta: String,
    },
    ToolDelta {
        id: String,
        name: String,
        delta: String,
    },
    ToolResult {
        id: String,
        name: String,
        result: String,
    },
    Interrupt {
        interrupts: Vec<InterruptInfo>,
    },
    TurnStart {
        turn: usize,
    },
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
    },
    Done,
    Cancelled,
    Error {
        message: String,
    },
    Checkpoint {
        checkpoint: Checkpoint,
    },
    NeedToolExecution {
        state: AgentState,
        tool_calls: Vec<ParsedToolCall>,
        completed_results: Vec<ToolCallOutcome>,
    },
}

impl From<AgentEvent> for SubagentForwardEvent {
    fn from(event: AgentEvent) -> Self {
        match event {
            AgentEvent::RunStart { thread_id, run_id, metadata } => Self::RunStart {
                thread_id: thread_id.0,
                run_id: run_id.0,
                metadata,
            },
            AgentEvent::TextDelta(content) => Self::TextDelta { content },
            AgentEvent::ThinkingStart => Self::ThinkingStart,
            AgentEvent::ThinkingEnd { content } => Self::ThinkingEnd { content },
            AgentEvent::ToolCallStart { id, name } => Self::ToolCallStart { id, name },
            AgentEvent::ToolCallArgumentsDelta { id, delta } => {
                Self::ToolCallArgumentsDelta { id, delta }
            }
            AgentEvent::ToolDelta { id, name, delta } => Self::ToolDelta { id, name, delta },
            AgentEvent::ToolResult { id, name, result } => Self::ToolResult { id, name, result },
            AgentEvent::Interrupt { interrupts } => Self::Interrupt { interrupts },
            AgentEvent::TurnStart { turn } => Self::TurnStart { turn },
            AgentEvent::Usage { prompt_tokens, completion_tokens } => Self::Usage {
                prompt_tokens,
                completion_tokens,
            },
            AgentEvent::Done => Self::Done,
            AgentEvent::Cancelled => Self::Cancelled,
            AgentEvent::Error(error) => Self::Error {
                message: error.to_string(),
            },
            AgentEvent::Checkpoint(checkpoint) => Self::Checkpoint { checkpoint },
            AgentEvent::NeedToolExecution {
                state,
                tool_calls,
                completed_results,
            } => Self::NeedToolExecution {
                state,
                tool_calls,
                completed_results,
            },
            AgentEvent::SubSession(event) => Self::Error {
                message: format!("nested sub-session forwarded separately: {:?}", event.payload),
            },
            AgentEvent::Custom { event_type, extra } => Self::Error {
                message: format!("nested custom event forwarded separately: {event_type} {extra}"),
            },
        }
    }
}

// ── SubAgentTaskTool ──────────────────────────────────────────────────────────

/// A tool that delegates a task to a sub-agent and returns its final response.
pub struct SubAgentTaskTool {
    runner: Arc<RunnerFn>,
    tool_description: String,
    agent_name: String,
}

impl SubAgentTaskTool {
    /// Build a `SubAgentTaskTool` backed by `model`.
    ///
    /// Each invocation constructs a temporary `AgentLoop<M>` with bash + fs
    /// tools, cloning `model` so the original remains usable.
    pub fn new<M>(model: M, system_prompt: impl Into<String>, max_turns: usize) -> Self
    where
        M: ChatModel + Clone + Send + Sync + 'static,
    {
        let system_prompt = system_prompt.into();
        let runner: Arc<RunnerFn> = Arc::new(move |ctx: ChatCtx, task: String| {
            let model = model.clone();
            let system_prompt = system_prompt.clone();
            Box::pin(async move {
                let agent = AgentBuilder::new()
                    .model(model)
                    .system(system_prompt)
                    .tool(BashTool)
                    .tool(LocalFsReadTool)
                    .tool(LocalFsWriteTool)
                    .tool(LocalFsCreateTool)
                    .tool(LocalFsRemoveTool)
                    .tool(LocalFsLsTool)
                    .max_turns(max_turns)
                    .build_loop();

                let subagent_stream = stream! {
                    match agent.chat(ctx, LoopInput::start(&task)).await {
                        Ok(inner_stream) => {
                            let mut inner_stream = std::pin::pin!(inner_stream);
                            while let Some(ev) = inner_stream.next().await {
                                yield ev;
                            }
                        }
                        Err(e) => yield AgentEvent::Error(e),
                    }
                };

                Ok(Box::pin(subagent_stream) as SubagentEventStream)
            })
        });

        Self {
            runner,
            tool_description: "Delegate a focused subtask to a worker sub-agent. \
                The sub-agent has access to bash and filesystem tools. \
                Use this for self-contained tasks (file operations, code generation, \
                research) that you want to keep isolated from the main context."
                .to_string(),
            agent_name: "worker".to_string(),
        }
    }
}

// ── Tool impl ─────────────────────────────────────────────────────────────────

impl Tool for SubAgentTaskTool {
    fn name(&self) -> &str {
        "task__run"
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Complete, self-contained task description for the sub-agent. \
                        Include all necessary context since the sub-agent starts with a clean history."
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        ctx: ChatCtx,
    ) -> Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError> {
        let task = arguments["task"]
            .as_str()
            .ok_or_else(|| AgentError::tool("task__run", "missing 'task'"))?
            .to_string();

        let runner = Arc::clone(&self.runner);
        let agent_name = self.agent_name.clone();
        let title = Some(task.clone());

        Ok(ToolResult::Output(stream! {
            let mut text = String::new();
            let subagent_ctx = ctx.fork();
            let mut subagent_stream = match (runner)(subagent_ctx, task).await {
                Ok(stream) => stream,
                Err(e) => {
                    let msg = format!("sub-agent error: {e}");
                    yield ToolOutput::Delta(format!("[{msg}]\n"));
                    yield ToolOutput::Result(Content::text(msg));
                    return;
                }
            };

            let mut sub_thread_id = None;
            let mut sub_run_id = None;
            while let Some(event) = subagent_stream.next().await {
                let forwarded = SubagentForwardEvent::from(event.clone());
                yield ToolOutput::custom(
                    "subagent_event",
                    serde_json::to_value(forwarded).unwrap_or(serde_json::Value::Null),
                );

                match event {
                    AgentEvent::RunStart { thread_id, run_id, .. } => {
                        sub_thread_id = Some(thread_id.clone());
                        sub_run_id = Some(run_id.clone());
                        yield ToolOutput::SubSession(SubSessionEvent::new(
                            String::new(),
                            thread_id,
                            run_id,
                            agent_name.clone(),
                            title.clone(),
                            0,
                            SubSessionEventPayload::Start,
                        ));
                        yield ToolOutput::Delta("[sub-agent started]\n".to_string());
                    }
                    AgentEvent::TextDelta(content) => {
                        let delta = content.clone();
                        text.push_str(&content);
                        if let (Some(thread_id), Some(run_id)) = (&sub_thread_id, &sub_run_id) {
                            yield ToolOutput::SubSession(SubSessionEvent::new(
                                String::new(),
                                thread_id.clone(),
                                run_id.clone(),
                                agent_name.clone(),
                                title.clone(),
                                0,
                                SubSessionEventPayload::Delta { content },
                            ));
                        }
                        yield ToolOutput::Delta(delta);
                    }
                    AgentEvent::ThinkingStart => {
                        if let (Some(thread_id), Some(run_id)) = (&sub_thread_id, &sub_run_id) {
                            yield ToolOutput::SubSession(SubSessionEvent::new(
                                String::new(),
                                thread_id.clone(),
                                run_id.clone(),
                                agent_name.clone(),
                                title.clone(),
                                0,
                                SubSessionEventPayload::ThinkingStart,
                            ));
                        }
                        yield ToolOutput::Delta("[sub-agent thinking]\n".to_string());
                    }
                    AgentEvent::ThinkingEnd { content } => {
                        if let (Some(thread_id), Some(run_id)) = (&sub_thread_id, &sub_run_id) {
                            yield ToolOutput::SubSession(SubSessionEvent::new(
                                String::new(),
                                thread_id.clone(),
                                run_id.clone(),
                                agent_name.clone(),
                                title.clone(),
                                0,
                                SubSessionEventPayload::ThinkingEnd { content },
                            ));
                        }
                        yield ToolOutput::Delta("[sub-agent thinking complete]\n".to_string());
                    }
                    AgentEvent::ToolCallStart { id, name } => {
                        let sub_id = id.clone();
                        let sub_name = name.clone();
                        if let (Some(thread_id), Some(run_id)) = (&sub_thread_id, &sub_run_id) {
                            yield ToolOutput::SubSession(SubSessionEvent::new(
                                String::new(),
                                thread_id.clone(),
                                run_id.clone(),
                                agent_name.clone(),
                                title.clone(),
                                0,
                                SubSessionEventPayload::ToolCallStart {
                                    id: sub_id,
                                    name: sub_name,
                                },
                            ));
                        }
                        yield ToolOutput::Delta(format!("[sub-agent tool start {name}#{id}]\n"));
                    }
                    AgentEvent::ToolCallArgumentsDelta { id, delta } => {
                        if let (Some(thread_id), Some(run_id)) = (&sub_thread_id, &sub_run_id) {
                            yield ToolOutput::SubSession(SubSessionEvent::new(
                                String::new(),
                                thread_id.clone(),
                                run_id.clone(),
                                agent_name.clone(),
                                title.clone(),
                                0,
                                SubSessionEventPayload::ToolCallArgumentsDelta { id, delta },
                            ));
                        }
                    }
                    AgentEvent::ToolDelta { id, name, delta } => {
                        let sub_id = id.clone();
                        let sub_name = name.clone();
                        let sub_delta = delta.clone();
                        if let (Some(thread_id), Some(run_id)) = (&sub_thread_id, &sub_run_id) {
                            yield ToolOutput::SubSession(SubSessionEvent::new(
                                String::new(),
                                thread_id.clone(),
                                run_id.clone(),
                                agent_name.clone(),
                                title.clone(),
                                0,
                                SubSessionEventPayload::ToolDelta {
                                    id: sub_id,
                                    name: sub_name,
                                    delta: sub_delta,
                                },
                            ));
                        }
                        yield ToolOutput::Delta(format!("[sub-agent tool {name}#{id}] {delta}"));
                    }
                    AgentEvent::ToolResult { id, name, result } => {
                        let sub_id = id.clone();
                        let sub_name = name.clone();
                        let sub_result = result.clone();
                        if let (Some(thread_id), Some(run_id)) = (&sub_thread_id, &sub_run_id) {
                            yield ToolOutput::SubSession(SubSessionEvent::new(
                                String::new(),
                                thread_id.clone(),
                                run_id.clone(),
                                agent_name.clone(),
                                title.clone(),
                                0,
                                SubSessionEventPayload::ToolResult {
                                    id: sub_id,
                                    name: sub_name,
                                    result: sub_result,
                                },
                            ));
                        }
                        yield ToolOutput::Delta(format!("[sub-agent tool result {name}#{id}] {result}\n"));
                    }
                    AgentEvent::TurnStart { turn } => {
                        if let (Some(thread_id), Some(run_id)) = (&sub_thread_id, &sub_run_id) {
                            yield ToolOutput::SubSession(SubSessionEvent::new(
                                String::new(),
                                thread_id.clone(),
                                run_id.clone(),
                                agent_name.clone(),
                                title.clone(),
                                0,
                                SubSessionEventPayload::TurnStart { turn },
                            ));
                        }
                    }
                    AgentEvent::Done => {
                        if let (Some(thread_id), Some(run_id)) = (&sub_thread_id, &sub_run_id) {
                            yield ToolOutput::SubSession(SubSessionEvent::new(
                                String::new(),
                                thread_id.clone(),
                                run_id.clone(),
                                agent_name.clone(),
                                title.clone(),
                                0,
                                SubSessionEventPayload::Done {
                                    final_output: if text.trim().is_empty() {
                                        None
                                    } else {
                                        Some(text.clone())
                                    },
                                },
                            ));
                        }
                        break;
                    }
                    AgentEvent::Error(error) => {
                        let message = error.to_string();
                        if let (Some(thread_id), Some(run_id)) = (&sub_thread_id, &sub_run_id) {
                            yield ToolOutput::SubSession(SubSessionEvent::new(
                                String::new(),
                                thread_id.clone(),
                                run_id.clone(),
                                agent_name.clone(),
                                title.clone(),
                                0,
                                SubSessionEventPayload::Error {
                                    message: message.clone(),
                                },
                            ));
                        }
                        yield ToolOutput::Delta(format!("[sub-agent error: {message}]\n"));
                        if text.is_empty() {
                            text = format!("Sub-agent failed: {message}");
                        }
                        break;
                    }
                    AgentEvent::Interrupt { interrupts } => {
                        let message = format!("Sub-agent interrupted with {} pending action(s)", interrupts.len());
                        if let (Some(thread_id), Some(run_id)) = (&sub_thread_id, &sub_run_id) {
                            yield ToolOutput::SubSession(SubSessionEvent::new(
                                String::new(),
                                thread_id.clone(),
                                run_id.clone(),
                                agent_name.clone(),
                                title.clone(),
                                0,
                                SubSessionEventPayload::Error {
                                    message: message.clone(),
                                },
                            ));
                        }
                        yield ToolOutput::Delta(format!("[sub-agent interrupted: {} pending interrupt(s)]\n", interrupts.len()));
                        if text.is_empty() {
                            text = message;
                        }
                        break;
                    }
                    AgentEvent::Cancelled => {
                        let message = "sub-agent cancelled".to_string();
                        if let (Some(thread_id), Some(run_id)) = (&sub_thread_id, &sub_run_id) {
                            yield ToolOutput::SubSession(SubSessionEvent::new(
                                String::new(),
                                thread_id.clone(),
                                run_id.clone(),
                                agent_name.clone(),
                                title.clone(),
                                0,
                                SubSessionEventPayload::Error {
                                    message: message.clone(),
                                },
                            ));
                        }
                        yield ToolOutput::Delta(format!("[{message}]\n"));
                        if text.is_empty() {
                            text = message;
                        }
                        break;
                    }
                    AgentEvent::NeedToolExecution { tool_calls, .. } => {
                        yield ToolOutput::Delta(format!(
                            "[sub-agent delegated {} external tool call(s)]\n",
                            tool_calls.len()
                        ));
                    }
                    AgentEvent::Usage { .. } | AgentEvent::Checkpoint(_) | AgentEvent::SubSession(_) => {}
                    AgentEvent::Custom { .. } => {}
                }

            }

            if text.is_empty() {
                yield ToolOutput::Result(Content::text("[sub-agent completed]"));
            } else {
                yield ToolOutput::Result(Content::text(text));
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_stream::stream;
    use futures::stream;
    use futures::{Stream, StreamExt};
    use remi_core::tool::InterruptRequest;
    use remi_core::tracing::{
        InterruptTrace, ModelEndTrace, ModelStartTrace, RunEndTrace, RunStartTrace, ToolEndTrace,
        ToolStartTrace, Tracer, TurnStartTrace,
    };
    use remi_core::types::{ChatResponseChunk, Role, SpanKind};
    use serde_json::Value;
    use std::collections::{HashMap, HashSet};
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    type ModelStream = Pin<Box<dyn Stream<Item = ChatResponseChunk> + Send>>;
    type ModelResponder = dyn Fn(ChatCtx, remi_core::types::ModelRequest) -> ModelStream + Send + Sync;

    #[derive(Clone)]
    struct TestModel {
        requests: Arc<Mutex<Vec<remi_core::types::ModelRequest>>>,
        responder: Arc<ModelResponder>,
    }

    impl TestModel {
        fn new(
            responder: impl Fn(ChatCtx, remi_core::types::ModelRequest) -> ModelStream
                + Send
                + Sync
                + 'static,
        ) -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                responder: Arc::new(responder),
            }
        }

        fn requests(&self) -> Vec<remi_core::types::ModelRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Agent for TestModel {
        type Request = remi_core::types::ModelRequest;
        type Response = ChatResponseChunk;
        type Error = AgentError;

        async fn chat(
            &self,
            ctx: ChatCtx,
            req: Self::Request,
        ) -> Result<impl Stream<Item = Self::Response>, Self::Error> {
            self.requests.lock().unwrap().push(req.clone());
            Ok((self.responder)(ctx, req))
        }
    }

    #[allow(dead_code)]
    #[derive(Clone, Debug)]
    enum TraceRecord {
        RunStart(RunStartTrace),
        RunEnd(RunEndTrace),
        ModelStart(ModelStartTrace),
        ModelEnd(ModelEndTrace),
        ToolStart(ToolStartTrace),
        ToolEnd(ToolEndTrace),
        Interrupt(InterruptTrace),
        TurnStart(TurnStartTrace),
    }

    #[derive(Clone, Default)]
    struct RecordingTracer {
        records: Arc<Mutex<Vec<TraceRecord>>>,
    }

    impl RecordingTracer {
        fn snapshot(&self) -> Vec<TraceRecord> {
            self.records.lock().unwrap().clone()
        }
    }

    impl Tracer for RecordingTracer {
        async fn on_run_start(&self, event: &RunStartTrace) {
            self.records
                .lock()
                .unwrap()
                .push(TraceRecord::RunStart(event.clone()));
        }

        async fn on_run_end(&self, event: &RunEndTrace) {
            self.records
                .lock()
                .unwrap()
                .push(TraceRecord::RunEnd(event.clone()));
        }

        async fn on_model_start(&self, event: &ModelStartTrace) {
            self.records
                .lock()
                .unwrap()
                .push(TraceRecord::ModelStart(event.clone()));
        }

        async fn on_model_end(&self, event: &ModelEndTrace) {
            self.records
                .lock()
                .unwrap()
                .push(TraceRecord::ModelEnd(event.clone()));
        }

        async fn on_tool_start(&self, event: &ToolStartTrace) {
            self.records
                .lock()
                .unwrap()
                .push(TraceRecord::ToolStart(event.clone()));
        }

        async fn on_tool_end(&self, event: &ToolEndTrace) {
            self.records
                .lock()
                .unwrap()
                .push(TraceRecord::ToolEnd(event.clone()));
        }

        async fn on_interrupt(&self, event: &InterruptTrace) {
            self.records
                .lock()
                .unwrap()
                .push(TraceRecord::Interrupt(event.clone()));
        }

        async fn on_turn_start(&self, event: &TurnStartTrace) {
            self.records
                .lock()
                .unwrap()
                .push(TraceRecord::TurnStart(event.clone()));
        }
    }

    struct InnerEchoTool;

    impl Tool for InnerEchoTool {
        fn name(&self) -> &str {
            "inner_echo"
        }

        fn description(&self) -> &str {
            "Echo a payload from inside the subagent."
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"]
            })
        }

        async fn execute(
            &self,
            arguments: serde_json::Value,
            _resume: Option<ResumePayload>,
            _ctx: ChatCtx,
        ) -> Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError> {
            let text = arguments
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(ToolResult::Output(stream! {
                yield ToolOutput::Delta(format!("inner-tool:{text}"));
                yield ToolOutput::text(format!("echo:{text}"));
            }))
        }
    }

    struct InterruptingInnerTool;

    impl Tool for InterruptingInnerTool {
        fn name(&self) -> &str {
            "needs_approval"
        }

        fn description(&self) -> &str {
            "Interrupts from inside the subagent."
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {},
                "required": []
            })
        }

        async fn execute(
            &self,
            _arguments: serde_json::Value,
            _resume: Option<ResumePayload>,
            _ctx: ChatCtx,
        ) -> Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError> {
            let result: ToolResult<stream::Empty<ToolOutput>> = ToolResult::Interrupt(
                InterruptRequest::new("approval", json!({ "source": "subagent" })),
            );
            Ok(result)
        }
    }

    fn into_subagent_stream<A>(agent: A, ctx: ChatCtx, input: LoopInput) -> SubagentEventStream
    where
        A: Agent<Request = LoopInput, Response = AgentEvent, Error = AgentError> + 'static,
    {
        Box::pin(stream! {
            match agent.chat(ctx, input).await {
                Ok(inner_stream) => {
                    let mut inner_stream = std::pin::pin!(inner_stream);
                    while let Some(event) = inner_stream.next().await {
                        yield event;
                    }
                }
                Err(error) => {
                    yield AgentEvent::Error(error);
                }
            }
        })
    }

    fn latest_user_text(req: &remi_core::types::ModelRequest) -> String {
        req.messages
            .iter()
            .rev()
            .find(|message| matches!(message.role, Role::User))
            .map(|message| message.content.text_content())
            .unwrap_or_default()
    }

    fn has_tool_result(req: &remi_core::types::ModelRequest, tool_call_id: &str) -> bool {
        req.messages
            .iter()
            .any(|message| message.tool_call_id.as_deref() == Some(tool_call_id))
    }

    fn usage_chunk() -> ChatResponseChunk {
        ChatResponseChunk::Usage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 2,
        }
    }

    fn text_response(text: &str) -> ModelStream {
        Box::pin(stream::iter(vec![
            ChatResponseChunk::Delta {
                content: text.to_string(),
                role: Some(Role::Assistant),
            },
            usage_chunk(),
            ChatResponseChunk::Done,
        ]))
    }

    fn tool_call_response(calls: Vec<(usize, &str, &str, serde_json::Value)>) -> ModelStream {
        let mut chunks = Vec::new();
        for (index, id, name, arguments) in calls {
            chunks.push(ChatResponseChunk::ToolCallStart {
                index,
                id: id.to_string(),
                name: name.to_string(),
            });
            chunks.push(ChatResponseChunk::ToolCallDelta {
                index,
                arguments_delta: arguments.to_string(),
            });
        }
        chunks.push(usage_chunk());
        chunks.push(ChatResponseChunk::Done);
        Box::pin(stream::iter(chunks))
    }

    async fn collect_events(
        stream: impl Stream<Item = AgentEvent>,
    ) -> Vec<AgentEvent> {
        let mut stream = std::pin::pin!(stream);
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn subagent_tracing_records_parent_child_spans() {
        let model = TestModel::new(|_ctx, req| {
            let latest_user = latest_user_text(&req);

            if latest_user == "trace root" && !has_tool_result(&req, "outer-task") {
                return tool_call_response(vec![(
                    0,
                    "outer-task",
                    "task__run",
                    json!({ "task": "inner trace" }),
                )]);
            }

            if latest_user == "inner trace" && !has_tool_result(&req, "inner-echo") {
                return tool_call_response(vec![(
                    0,
                    "inner-echo",
                    "inner_echo",
                    json!({ "text": "trace-child" }),
                )]);
            }

            if has_tool_result(&req, "inner-echo") {
                return text_response("subagent finished");
            }

            if has_tool_result(&req, "outer-task") {
                return text_response("outer finished");
            }

            text_response("unexpected")
        });

        let tracer = RecordingTracer::default();
        let subagent_tool = SubAgentTaskTool {
            runner: Arc::new({
                let model = model.clone();
                move |ctx: ChatCtx, task: String| {
                    let model = model.clone();
                    Box::pin(async move {
                        let agent = AgentBuilder::new()
                            .model(model)
                            .system("inner")
                            .tool(InnerEchoTool)
                            .max_turns(4)
                            .build_loop();
                        Ok(into_subagent_stream(agent, ctx, LoopInput::start(task)))
                    })
                }
            }),
            tool_description: "test subagent".to_string(),
            agent_name: "worker".to_string(),
        };

        let agent = AgentBuilder::new()
            .model(model)
            .tool(subagent_tool)
            .tracer(tracer.clone())
            .max_turns(6)
            .build_loop();

        let stream = agent
            .chat(ChatCtx::default(), LoopInput::start("trace root"))
            .await
            .unwrap();
        let _events = collect_events(stream).await;

        let records = tracer.snapshot();
        let run_starts: Vec<_> = records
            .iter()
            .filter_map(|record| match record {
                TraceRecord::RunStart(event) => Some(event),
                _ => None,
            })
            .collect();
        let tool_starts: Vec<_> = records
            .iter()
            .filter_map(|record| match record {
                TraceRecord::ToolStart(event) => Some(event),
                _ => None,
            })
            .collect();

        assert_eq!(run_starts.len(), 2);
        assert!(tool_starts.len() >= 2);

        let main_run = run_starts
            .iter()
            .find(|event| event.span.parent.is_none())
            .unwrap();
        let main_tool = tool_starts
            .iter()
            .find(|event| event.tool_name == "task__run")
            .unwrap();
        let subagent_run = run_starts
            .iter()
            .find(|event| {
                event
                    .span
                    .parent
                    .as_ref()
                    .is_some_and(|parent| matches!(parent.kind, SpanKind::Subagent))
            })
            .unwrap();
        let subagent_tool = tool_starts
            .iter()
            .find(|event| event.tool_name == "inner_echo")
            .unwrap();

        assert_eq!(
            main_tool.span.parent.as_ref().unwrap().span_id,
            main_run.span.span_id
        );

        let subagent_span = subagent_run.span.parent.as_ref().unwrap();
        assert!(matches!(subagent_span.kind, SpanKind::Subagent));
        assert_eq!(
            subagent_span.parent.as_ref().unwrap().span_id,
            main_tool.span.span_id
        );
        assert_eq!(
            subagent_tool.span.parent.as_ref().unwrap().span_id,
            subagent_run.span.span_id
        );
    }

    #[tokio::test]
    async fn parallel_subagent_tools_stream_outputs_for_each_call() {
        let model = TestModel::new(|_ctx, req| {
            let latest_user = latest_user_text(&req);

            if latest_user == "parallel root"
                && !has_tool_result(&req, "task-a")
                && !has_tool_result(&req, "task-b")
            {
                return tool_call_response(vec![
                    (0, "task-a", "task__run", json!({ "task": "alpha" })),
                    (1, "task-b", "task__run", json!({ "task": "beta" })),
                ]);
            }

            if latest_user == "alpha" {
                return text_response("alpha-output");
            }

            if latest_user == "beta" {
                return text_response("beta-output");
            }

            if has_tool_result(&req, "task-a") && has_tool_result(&req, "task-b") {
                return text_response("parallel done");
            }

            text_response("unexpected")
        });

        let subagent_tool = SubAgentTaskTool::new(model.clone(), "inner", 3);
        let agent = AgentBuilder::new()
            .model(model)
            .tool(subagent_tool)
            .max_turns(6)
            .build_loop();

        let stream = agent
            .chat(ChatCtx::default(), LoopInput::start("parallel root"))
            .await
            .unwrap();
        let events = collect_events(stream).await;

        let mut per_tool_delta = HashMap::<String, String>::new();
        let mut tool_results = HashMap::<String, String>::new();
        let mut custom_tool_ids = HashSet::<String>::new();

        for event in events {
            match event {
                AgentEvent::ToolDelta { id, delta, .. } => {
                    per_tool_delta.entry(id).or_default().push_str(&delta);
                }
                AgentEvent::ToolResult { id, result, .. } => {
                    tool_results.insert(id, result);
                }
                AgentEvent::Custom { event_type, extra } if event_type == "subagent_event" => {
                    if let Some(tool_call_id) = extra.get("tool_call_id").and_then(Value::as_str) {
                        custom_tool_ids.insert(tool_call_id.to_string());
                    }
                }
                _ => {}
            }
        }

        assert!(
            per_tool_delta
                .get("task-a")
                .is_some_and(|delta| delta.contains("alpha-output"))
        );
        assert!(
            per_tool_delta
                .get("task-b")
                .is_some_and(|delta| delta.contains("beta-output"))
        );
        assert_eq!(tool_results.get("task-a"), Some(&"alpha-output".to_string()));
        assert_eq!(tool_results.get("task-b"), Some(&"beta-output".to_string()));
        assert_eq!(custom_tool_ids.len(), 2);
        assert!(custom_tool_ids.contains("task-a"));
        assert!(custom_tool_ids.contains("task-b"));
    }

    #[tokio::test]
    async fn cancel_propagates_into_subagent_and_cancels_outer_run() {
        let model = TestModel::new(|_ctx, req| {
            if latest_user_text(&req) == "cancel root" && !has_tool_result(&req, "cancel-task") {
                return tool_call_response(vec![(
                    0,
                    "cancel-task",
                    "task__run",
                    json!({ "task": "wait" }),
                )]);
            }

            text_response("cancel aftermath")
        });

        let subagent_tool = SubAgentTaskTool {
            runner: Arc::new(|ctx: ChatCtx, _task: String| {
                Box::pin(async move {
                    let stream = stream! {
                        yield AgentEvent::RunStart {
                            thread_id: ctx.thread_id(),
                            run_id: ctx.run_id(),
                            metadata: ctx.metadata(),
                        };
                        yield AgentEvent::TextDelta("still-running".to_string());
                        loop {
                            if ctx.is_cancelled() {
                                yield AgentEvent::Cancelled;
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    };
                    Ok(Box::pin(stream) as SubagentEventStream)
                })
            }),
            tool_description: "cancel subagent".to_string(),
            agent_name: "worker".to_string(),
        };

        let agent = AgentBuilder::new()
            .model(model)
            .tool(subagent_tool)
            .max_turns(4)
            .build_loop();

        let ctx = ChatCtx::default();
        let mut stream = std::pin::pin!(
            agent.chat(ctx.clone(), LoopInput::start("cancel root"))
                .await
                .unwrap()
        );

        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            if matches!(&event, AgentEvent::ToolDelta { id, .. } if id == "cancel-task") {
                ctx.cancel();
            }
            let is_cancelled = matches!(event, AgentEvent::Cancelled);
            events.push(event);
            if is_cancelled {
                break;
            }
        }

        assert!(events.iter().any(|event| matches!(event, AgentEvent::Cancelled)));
    }

    #[tokio::test]
    async fn subagent_interrupt_is_forwarded_and_traced_as_interrupted() {
        let model = TestModel::new(|_ctx, req| {
            let latest_user = latest_user_text(&req);

            if latest_user == "interrupt root" && !has_tool_result(&req, "interrupt-task") {
                return tool_call_response(vec![(
                    0,
                    "interrupt-task",
                    "task__run",
                    json!({ "task": "interrupt inner" }),
                )]);
            }

            if latest_user == "interrupt inner" && !has_tool_result(&req, "inner-interrupt") {
                return tool_call_response(vec![(
                    0,
                    "inner-interrupt",
                    "needs_approval",
                    json!({}),
                )]);
            }

            if has_tool_result(&req, "interrupt-task") {
                return text_response("outer after interrupt");
            }

            text_response("unexpected")
        });

        let tracer = RecordingTracer::default();
        let subagent_tool = SubAgentTaskTool {
            runner: Arc::new({
                let model = model.clone();
                move |ctx: ChatCtx, task: String| {
                    let model = model.clone();
                    Box::pin(async move {
                        let agent = AgentBuilder::new()
                            .model(model)
                            .system("inner")
                            .tool(InterruptingInnerTool)
                            .max_turns(4)
                            .build_loop();
                        Ok(into_subagent_stream(agent, ctx, LoopInput::start(task)))
                    })
                }
            }),
            tool_description: "interrupt subagent".to_string(),
            agent_name: "worker".to_string(),
        };

        let agent = AgentBuilder::new()
            .model(model)
            .tool(subagent_tool)
            .tracer(tracer.clone())
            .max_turns(6)
            .build_loop();

        let stream = agent
            .chat(ChatCtx::default(), LoopInput::start("interrupt root"))
            .await
            .unwrap();
        let events = collect_events(stream).await;

        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolDelta { id, delta, .. }
                if id == "interrupt-task"
                    && delta.contains("sub-agent interrupted: 1 pending interrupt(s)")
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::Custom { event_type, extra }
                if event_type == "subagent_event"
                    && extra.get("payload").and_then(|payload| payload.get("type")).and_then(Value::as_str)
                        == Some("interrupt")
        )));

        let records = tracer.snapshot();
        assert!(records.iter().any(|record| matches!(
            record,
            TraceRecord::Interrupt(event)
                if event
                    .span
                    .parent
                    .as_ref()
                    .is_some_and(|parent| matches!(parent.kind, SpanKind::Subagent))
        )));
        assert!(records.iter().any(|record| matches!(
            record,
            TraceRecord::RunEnd(event)
                if matches!(event.status, remi_core::tracing::RunStatus::Interrupted)
                    && event
                        .span
                        .parent
                        .as_ref()
                        .is_some_and(|parent| matches!(parent.kind, SpanKind::Subagent))
        )));
    }

    #[test]
    fn test_model_records_requests() {
        let model = TestModel::new(|_, _| text_response("noop"));
        assert!(model.requests().is_empty());
    }
}
