use futures::StreamExt;
use remi_core::agent::Agent;
use remi_core::error::AgentError;
use remi_core::types::{AgentEvent, ChatCtx, LoopInput, Message, Role};

use crate::capture::SessionCapture;
use crate::error::EvalError;
use crate::report::{
    EvaluationReport, EvaluationRunResult, ScoredEvaluationReport, ScoredRunResult,
    ToolResultRecord, UsageTotals,
};
use crate::variant::{ExperimentVariant, ToolOverrideMode};

pub trait Scorer {
    fn score(
        &self,
        run: &EvaluationRunResult,
    ) -> Result<Vec<crate::report::ScoreCard>, EvalError>;
}

pub fn build_replay_input(capture: &SessionCapture, variant: &ExperimentVariant) -> LoopInput {
    let history = apply_system_prompt(&capture.history, variant.system_prompt.as_deref());
    let extra_tools = match variant.tool_mode {
        ToolOverrideMode::Append => {
            let mut extra_tools = capture.extra_tools.clone();
            extra_tools.extend(variant.extra_tools.clone());
            extra_tools
        }
        ToolOverrideMode::Replace => variant.extra_tools.clone(),
    };

    let metadata = variant
        .metadata
        .clone()
        .or_else(|| capture.metadata.clone());

    let model = variant.model.clone().or_else(|| capture.model.clone());
    let temperature = variant.temperature.or(capture.temperature);
    let max_tokens = variant.max_tokens.or(capture.max_tokens);

    LoopInput::start_message(capture.message.clone())
        .history(history)
        .extra_tools(extra_tools)
        .metadata_opt(metadata)
        .model_opt(model)
        .temperature_opt(temperature)
        .max_tokens_opt(max_tokens)
}

fn apply_system_prompt(history: &[Message], system_prompt: Option<&str>) -> Vec<Message> {
    let Some(system_prompt) = system_prompt else {
        return history.to_vec();
    };

    let mut history = history.to_vec();
    if let Some(first_system) = history
        .iter_mut()
        .find(|message| matches!(message.role, Role::System))
    {
        first_system.content = remi_core::types::Content::text(system_prompt);
        return history;
    }

    history.insert(0, Message::system(system_prompt));
    history
}

pub struct ExperimentRunner<A> {
    agent: A,
}

impl<A> ExperimentRunner<A> {
    pub fn new(agent: A) -> Self {
        Self { agent }
    }
}

impl<A> ExperimentRunner<A>
where
    A: Agent<Request = LoopInput, Response = AgentEvent, Error = AgentError>,
{
    pub async fn run_variant(
        &self,
        capture: &SessionCapture,
        variant: &ExperimentVariant,
    ) -> Result<EvaluationRunResult, EvalError> {
        let input = build_replay_input(capture, variant);
        let mut stream = std::pin::pin!(self.agent.chat(ChatCtx::default(), input).await?);

        let mut final_text = String::new();
        let mut reasoning = None;
        let mut usage = UsageTotals::default();
        let mut tool_results = Vec::new();
        let mut events = Vec::new();
        let mut done = false;
        let mut cancelled = false;
        let mut interrupted = false;
        let mut error = None;

        while let Some(event) = stream.next().await {
            match &event {
                AgentEvent::TextDelta(delta) => final_text.push_str(delta),
                AgentEvent::ThinkingEnd { content } => reasoning = Some(content.clone()),
                AgentEvent::ToolResult { id, name, result } => {
                    tool_results.push(ToolResultRecord {
                        id: id.clone(),
                        name: name.clone(),
                        result: result.clone(),
                    });
                }
                AgentEvent::Usage {
                    prompt_tokens,
                    completion_tokens,
                } => {
                    usage.prompt_tokens = usage.prompt_tokens.saturating_add(*prompt_tokens);
                    usage.completion_tokens = usage
                        .completion_tokens
                        .saturating_add(*completion_tokens);
                }
                AgentEvent::Interrupt { .. } => interrupted = true,
                AgentEvent::Done => done = true,
                AgentEvent::Cancelled => cancelled = true,
                AgentEvent::Error(agent_error) => error = Some(agent_error.to_string()),
                _ => {}
            }
            events.push(event);
        }

        Ok(EvaluationRunResult {
            variant_id: variant.id.clone(),
            variant_label: variant.label.clone(),
            final_text,
            reasoning,
            usage,
            tool_results,
            events,
            done,
            cancelled,
            interrupted,
            error,
        })
    }

    pub async fn run_all(
        &self,
        capture: &SessionCapture,
        variants: &[ExperimentVariant],
    ) -> Result<EvaluationReport, EvalError> {
        let mut runs = Vec::with_capacity(variants.len());
        for variant in variants {
            runs.push(self.run_variant(capture, variant).await?);
        }
        Ok(EvaluationReport { runs })
    }

    pub async fn run_all_scored<S>(
        &self,
        capture: &SessionCapture,
        variants: &[ExperimentVariant],
        scorer: &S,
    ) -> Result<ScoredEvaluationReport, EvalError>
    where
        S: Scorer,
    {
        let mut runs = Vec::with_capacity(variants.len());
        for variant in variants {
            let run = self.run_variant(capture, variant).await?;
            let scores = scorer.score(&run)?;
            let total_score = if scores.is_empty() {
                None
            } else {
                Some(scores.iter().map(|score| score.value).sum::<f64>() / scores.len() as f64)
            };
            runs.push(ScoredRunResult {
                run,
                scores,
                total_score,
            });
        }
        Ok(ScoredEvaluationReport { runs })
    }
}

trait LoopInputExt {
    fn metadata_opt(self, metadata: Option<serde_json::Value>) -> Self;
    fn model_opt(self, model: Option<String>) -> Self;
    fn temperature_opt(self, temperature: Option<f64>) -> Self;
    fn max_tokens_opt(self, max_tokens: Option<u32>) -> Self;
}

impl LoopInputExt for LoopInput {
    fn metadata_opt(self, metadata: Option<serde_json::Value>) -> Self {
        match metadata {
            Some(metadata) => self.metadata(metadata),
            None => self,
        }
    }

    fn model_opt(self, model: Option<String>) -> Self {
        match model {
            Some(model) => self.model(model),
            None => self,
        }
    }

    fn temperature_opt(self, temperature: Option<f64>) -> Self {
        match temperature {
            Some(temperature) => self.temperature(temperature),
            None => self,
        }
    }

    fn max_tokens_opt(self, max_tokens: Option<u32>) -> Self {
        match max_tokens {
            Some(max_tokens) => self.max_tokens(max_tokens),
            None => self,
        }
    }
}

#[cfg(test)]
mod tests {
    use remi_core::tool::{FunctionDefinition, ToolDefinition};
    use remi_core::types::{LoopInput, Message};
    use serde_json::json;

    use super::build_replay_input;
    use crate::capture::SessionCapture;
    use crate::variant::ExperimentVariant;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: name.to_string(),
                description: name.to_string(),
                parameters: json!({"type": "object", "properties": {}}),
                extra_prompt: None,
            },
        }
    }

    #[test]
    fn replay_input_replaces_or_inserts_system_prompt() {
        let capture = SessionCapture::new(Message::user("solve"))
            .history(vec![Message::system("old system"), Message::assistant("ack")]);
        let variant = ExperimentVariant::new("v1", "variant 1").system_prompt("new system");

        let input = build_replay_input(&capture, &variant);
        let LoopInput::Start { history, .. } = input else {
            panic!("expected start input");
        };

        assert_eq!(history[0].content.text_content(), "new system");
    }

    #[test]
    fn replay_input_appends_extra_tools_by_default() {
        let capture = SessionCapture::new(Message::user("solve")).extra_tools(vec![tool("base")]);
        let variant = ExperimentVariant::new("v1", "variant 1").extra_tools(vec![tool("test")]);

        let input = build_replay_input(&capture, &variant);
        let LoopInput::Start { extra_tools, .. } = input else {
            panic!("expected start input");
        };

        assert_eq!(extra_tools.len(), 2);
        assert_eq!(extra_tools[0].function.name, "base");
        assert_eq!(extra_tools[1].function.name, "test");
    }
}