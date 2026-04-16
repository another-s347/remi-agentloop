use std::sync::{Arc, Mutex};

use futures::stream;
use futures::{Stream, StreamExt};
use remi_agentloop_core::agent::{Agent, AgentExt};
use remi_agentloop_core::error::AgentError;
use remi_agentloop_core::state::{AgentState, StepConfig};
use remi_agentloop_core::tool::{
    registry::{DefaultToolRegistry, ToolRegistry}, Tool, ToolOutput, ToolResult,
};
use remi_agentloop_core::types::{
    AgentEvent, ChatCtx, LoopInput, ParsedToolCall, ToolCallOutcome,
};
use serde_json::json;

#[derive(Clone, Default)]
struct RecordingInnerAgent {
    starts: Arc<Mutex<Vec<LoopInput>>>,
}

impl RecordingInnerAgent {
    fn starts(&self) -> Vec<LoopInput> {
        self.starts.lock().unwrap().clone()
    }
}

impl Agent for RecordingInnerAgent {
    type Request = LoopInput;
    type Response = AgentEvent;
    type Error = AgentError;

    async fn chat(
        &self,
        _ctx: ChatCtx,
        req: Self::Request,
    ) -> Result<impl Stream<Item = Self::Response>, Self::Error> {
        self.starts.lock().unwrap().push(req.clone());

        match req {
            LoopInput::Start { .. } => Ok(stream::iter(vec![AgentEvent::NeedToolExecution {
                state: AgentState::new(StepConfig::new("test-model")),
                tool_calls: vec![ParsedToolCall {
                    id: "call-1".to_string(),
                    name: "outer_add".to_string(),
                    arguments: json!({ "value": 41 }),
                }],
                completed_results: vec![],
            }])),
            LoopInput::Resume { results, .. } => {
                let rendered = match results.first() {
                    Some(ToolCallOutcome::Result { content, .. }) => content.text_content(),
                    Some(ToolCallOutcome::Error { error, .. }) => error.clone(),
                    None => String::new(),
                };
                Ok(stream::iter(vec![
                    AgentEvent::TextDelta(format!("resumed:{rendered}")),
                    AgentEvent::Done,
                ]))
            }
        }
    }
}

struct OuterAddTool;

impl Tool for OuterAddTool {
    fn name(&self) -> &str {
        "outer_add"
    }

    fn description(&self) -> &str {
        "Adds one to the provided value."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "value": { "type": "integer" }
            },
            "required": ["value"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<remi_agentloop_core::types::ResumePayload>,
        _ctx: ChatCtx,
    ) -> Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError> {
        let value = arguments
            .get("value")
            .and_then(|value| value.as_i64())
            .unwrap_or_default();
        Ok(ToolResult::Output(stream::iter(vec![
            ToolOutput::Delta("running".to_string()),
            ToolOutput::text((value + 1).to_string()),
        ])))
    }
}

#[tokio::test]
async fn external_tool_layer_injects_executes_and_resumes() {
    let inner = RecordingInnerAgent::default();
    let agent = inner
        .clone()
        .layer(DefaultToolRegistry::new().tool(OuterAddTool).into_layer());

    let events = agent
        .chat(ChatCtx::default(), LoopInput::start("hello"))
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;

    let starts = inner.starts();
    assert_eq!(starts.len(), 2);
        
    match &starts[0] {
        LoopInput::Start { extra_tools, .. } => {
            assert_eq!(extra_tools.len(), 1);
            assert_eq!(extra_tools[0].function.name, "outer_add");
        }
        other => panic!("expected start input, got {other:?}"),
    }

    match &starts[1] {
        LoopInput::Resume { results, .. } => {
            assert!(matches!(
                results.first(),
                Some(ToolCallOutcome::Result { tool_name, .. }) if tool_name == "outer_add"
            ));
        }
        other => panic!("expected resume input, got {other:?}"),
    }

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolDelta { id, delta, .. } if id == "call-1" && delta == "running"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolResult { id, result, .. } if id == "call-1" && result == "42"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TextDelta(text) if text == "resumed:42"
    )));
    assert!(events.iter().any(|event| matches!(event, AgentEvent::Done)));
}