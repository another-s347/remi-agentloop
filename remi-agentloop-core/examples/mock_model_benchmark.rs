use std::env;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_stream::stream;
use futures::{Stream, StreamExt};
use remi_agentloop_core::agent::Agent;
use remi_agentloop_core::builder::AgentBuilder;
use remi_agentloop_core::error::AgentError;
use remi_agentloop_core::tool::{
    FunctionDefinition, Tool, ToolDefinition, ToolOutput, ToolResult,
};
use remi_agentloop_core::types::{
    AgentEvent, ChatCtx, ChatResponseChunk, Content, LoopInput, ModelRequest, ParsedToolCall, Role,
    ToolCallOutcome,
};
use serde::Serialize;
use serde_json::json;
use tokio::time::{sleep, sleep_until, Instant as TokioInstant};

const INTERNAL_TOOL_NAME: &str = "internal_benchmark_tool";
const EXTERNAL_TOOL_NAME: &str = "external_benchmark_tool";
const DEFAULT_SEED: u64 = 0x5eed_baad_f00d;

#[derive(Clone, Debug)]
struct BenchmarkConfig {
    rates: Vec<u32>,
    total_tokens: usize,
    chunk_tokens: usize,
    tool_argument_tokens: usize,
    internal_tool_calls: usize,
    external_tool_calls: usize,
    internal_tool_probability: f64,
    external_tool_probability: f64,
    internal_tool_latency_ms: u64,
    external_tool_latency_ms: u64,
    warmup_rounds: usize,
    measured_rounds: usize,
    seed: u64,
    json: bool,
}

#[derive(Clone, Copy, Debug)]
struct BenchmarkSpec {
    target_tps: u32,
    total_tokens: usize,
    chunk_tokens: usize,
    tool_argument_tokens: usize,
    internal_tool_calls: usize,
    external_tool_calls: usize,
    internal_tool_probability: f64,
    external_tool_probability: f64,
    internal_tool_latency_ms: u64,
    external_tool_latency_ms: u64,
    seed: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolKind {
    Internal,
    External,
}

#[derive(Clone, Debug)]
struct ToolCallPlan {
    kind: ToolKind,
    id: String,
    arguments_json: String,
}

#[derive(Clone, Debug)]
struct TurnPlan {
    text_chunks: Vec<usize>,
    tool_call: Option<ToolCallPlan>,
}

#[derive(Clone, Debug)]
struct ModelPlan {
    turns: Vec<TurnPlan>,
    model_output_tokens: usize,
    internal_tool_calls: usize,
    external_tool_calls: usize,
}

#[derive(Clone)]
struct ThrottledMockModel {
    spec: BenchmarkSpec,
    plan: Arc<ModelPlan>,
    next_turn: Arc<Mutex<usize>>,
}

struct InternalBenchmarkTool {
    latency: Duration,
}

#[derive(Clone, Debug, Serialize)]
struct RoundMetrics {
    round: usize,
    target_tps: u32,
    total_visible_tokens: usize,
    assistant_text_tokens: usize,
    tool_output_tokens: usize,
    model_output_tokens: usize,
    chunk_tokens: usize,
    tool_argument_tokens: usize,
    internal_tool_calls: usize,
    external_tool_calls: usize,
    tool_call_starts: usize,
    tool_result_events: usize,
    tool_delta_events: usize,
    need_tool_execution_events: usize,
    total_events: usize,
    observed_tps: f64,
    observed_model_tps: f64,
    first_event_ms: Option<f64>,
    ttft_ms: Option<f64>,
    total_ms: f64,
    theoretical_ms: f64,
    total_overhead_ms: f64,
    per_event_overhead_us: f64,
}

#[derive(Clone, Debug, Serialize)]
struct AggregateMetrics {
    target_tps: u32,
    total_visible_tokens: usize,
    model_output_tokens: usize,
    chunk_tokens: usize,
    tool_argument_tokens: usize,
    internal_tool_calls: usize,
    external_tool_calls: usize,
    rounds: usize,
    average_observed_tps: f64,
    average_observed_model_tps: f64,
    average_first_event_ms: f64,
    average_ttft_ms: f64,
    average_total_ms: f64,
    average_total_overhead_ms: f64,
    average_per_event_overhead_us: f64,
}

#[derive(Clone, Debug, Serialize)]
struct BenchmarkReport {
    mode: &'static str,
    warmup_rounds: usize,
    measured_rounds: usize,
    results: Vec<RateReport>,
}

#[derive(Clone, Debug, Serialize)]
struct RateReport {
    target_tps: u32,
    measured: Vec<RoundMetrics>,
    aggregate: AggregateMetrics,
}

#[derive(Clone, Copy, Debug)]
enum PlanStep {
    TextChunk(usize),
    ToolCall(ToolKind),
}

#[derive(Clone, Copy, Debug)]
struct Lcg64 {
    state: u64,
}

impl Lcg64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for index in (1..items.len()).rev() {
            let swap_index = (self.next_u64() as usize) % (index + 1);
            items.swap(index, swap_index);
        }
    }

    fn next_f64(&mut self) -> f64 {
        const SCALE: f64 = (1u64 << 53) as f64;
        ((self.next_u64() >> 11) as f64) / SCALE
    }
}

impl ThrottledMockModel {
    fn new(spec: BenchmarkSpec) -> Self {
        Self {
            spec,
            plan: Arc::new(build_model_plan(spec)),
            next_turn: Arc::new(Mutex::new(0)),
        }
    }
}

impl Tool for InternalBenchmarkTool {
    fn name(&self) -> &str {
        INTERNAL_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Synthetic internal benchmark tool that simulates in-loop tool execution."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "payload": { "type": "string" },
                "sequence": { "type": "integer" }
            },
            "required": ["payload", "sequence"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<remi_agentloop_core::types::ResumePayload>,
        _ctx: &ChatCtx,
    ) -> Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError> {
        let payload = arguments
            .get("payload")
            .and_then(|value| value.as_str())
            .unwrap_or("internal");
        let sequence = arguments
            .get("sequence")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let latency = self.latency;
        let progress = format!("internal-progress:{sequence}:{payload}");
        let result = format!("internal-result:{sequence}:{payload}");

        Ok(ToolResult::Output(stream! {
            if !latency.is_zero() {
                sleep(latency).await;
            }
            yield ToolOutput::Delta(progress);
            yield ToolOutput::text(result);
        }))
    }
}

impl Agent for ThrottledMockModel {
    type Request = ModelRequest;
    type Response = ChatResponseChunk;
    type Error = AgentError;

    async fn chat(
        &self,
        _ctx: ChatCtx,
        _req: Self::Request,
    ) -> Result<impl Stream<Item = Self::Response>, Self::Error> {
        let turn_index = {
            let mut next_turn = self.next_turn.lock().unwrap();
            let current = *next_turn;
            *next_turn += 1;
            current
        };

        let spec = self.spec;
        let turn = self.plan.turns.get(turn_index).cloned();

        Ok(stream! {
            let Some(turn) = turn else {
                yield ChatResponseChunk::Done;
                return;
            };

            let start = TokioInstant::now();
            let mut emitted_model_tokens = 0usize;
            let mut emitted_any_text = false;

            for chunk_tokens in turn.text_chunks {
                emitted_model_tokens += chunk_tokens;
                let due = start + Duration::from_secs_f64(emitted_model_tokens as f64 / spec.target_tps as f64);
                sleep_until(due).await;

                yield ChatResponseChunk::Delta {
                    content: "x".repeat(chunk_tokens),
                    role: if emitted_any_text {
                        None
                    } else {
                        emitted_any_text = true;
                        Some(Role::Assistant)
                    },
                };
            }

            if let Some(tool_call) = turn.tool_call {
                emitted_model_tokens += spec.tool_argument_tokens;
                let due = start + Duration::from_secs_f64(emitted_model_tokens as f64 / spec.target_tps as f64);
                sleep_until(due).await;

                let tool_name = match tool_call.kind {
                    ToolKind::Internal => INTERNAL_TOOL_NAME,
                    ToolKind::External => EXTERNAL_TOOL_NAME,
                };

                yield ChatResponseChunk::ToolCallStart {
                    index: 0,
                    id: tool_call.id,
                    name: tool_name.to_string(),
                };
                yield ChatResponseChunk::ToolCallDelta {
                    index: 0,
                    arguments_delta: tool_call.arguments_json,
                };
            }

            yield ChatResponseChunk::Usage {
                prompt_tokens: 0,
                completion_tokens: emitted_model_tokens as u32,
                total_tokens: emitted_model_tokens as u32,
            };
            yield ChatResponseChunk::Done;
        })
    }
}

#[tokio::main]
async fn main() {
    let config = match parse_args() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            print_usage_and_exit(2);
        }
    };

    let mut reports = Vec::with_capacity(config.rates.len());

    for &rate in &config.rates {
        let spec = BenchmarkSpec {
            target_tps: rate,
            total_tokens: config.total_tokens,
            chunk_tokens: config.chunk_tokens,
            tool_argument_tokens: config.tool_argument_tokens,
            internal_tool_calls: config.internal_tool_calls,
            external_tool_calls: config.external_tool_calls,
            internal_tool_probability: config.internal_tool_probability,
            external_tool_probability: config.external_tool_probability,
            internal_tool_latency_ms: config.internal_tool_latency_ms,
            external_tool_latency_ms: config.external_tool_latency_ms,
            seed: config.seed ^ rate as u64,
        };

        for _ in 0..config.warmup_rounds {
            let _ = run_round(spec).await;
        }

        let mut measured = Vec::with_capacity(config.measured_rounds);
        for round_index in 0..config.measured_rounds {
            measured.push(run_round(spec).await.with_round(round_index + 1));
        }

        let aggregate = aggregate_metrics(spec, &measured);
        reports.push(RateReport {
            target_tps: rate,
            measured,
            aggregate,
        });
    }

    let report = BenchmarkReport {
        mode: "native_streaming_agent_loop",
        warmup_rounds: config.warmup_rounds,
        measured_rounds: config.measured_rounds,
        results: reports,
    };

    if config.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("failed to serialize benchmark report")
        );
        return;
    }

    print_report(&report);
}

fn build_model_plan(spec: BenchmarkSpec) -> ModelPlan {
    let mut text_chunks = Vec::new();
    let mut remaining_tokens = spec.total_tokens;
    while remaining_tokens > 0 {
        let next_chunk = spec.chunk_tokens.min(remaining_tokens);
        text_chunks.push(next_chunk);
        remaining_tokens -= next_chunk;
    }

    let final_text_chunk = text_chunks
        .pop()
        .expect("benchmark requires at least one text chunk");

    let mut steps: Vec<PlanStep> = text_chunks.into_iter().map(PlanStep::TextChunk).collect();
    steps.extend(std::iter::repeat_n(
        PlanStep::ToolCall(ToolKind::Internal),
        spec.internal_tool_calls,
    ));
    steps.extend(std::iter::repeat_n(
        PlanStep::ToolCall(ToolKind::External),
        spec.external_tool_calls,
    ));

    let mut rng = Lcg64::new(spec.seed);
    rng.shuffle(&mut steps);
    steps.push(PlanStep::TextChunk(final_text_chunk));

    if spec.internal_tool_probability > 0.0 || spec.external_tool_probability > 0.0 {
        let mut probabilistic_steps = Vec::with_capacity(steps.len() * 2);
        let last_index = steps.len().saturating_sub(1);
        for (index, step) in steps.into_iter().enumerate() {
            probabilistic_steps.push(step);
            if index == last_index {
                continue;
            }
            if spec.internal_tool_probability > 0.0
                && rng.next_f64() < spec.internal_tool_probability
            {
                probabilistic_steps.push(PlanStep::ToolCall(ToolKind::Internal));
            }
            if spec.external_tool_probability > 0.0
                && rng.next_f64() < spec.external_tool_probability
            {
                probabilistic_steps.push(PlanStep::ToolCall(ToolKind::External));
            }
        }
        steps = probabilistic_steps;
    }

    let mut turns = Vec::new();
    let mut text_before_tool = Vec::new();
    let mut internal_index = 0usize;
    let mut external_index = 0usize;

    for step in steps {
        match step {
            PlanStep::TextChunk(chunk_tokens) => {
                text_before_tool.push(chunk_tokens);
            }
            PlanStep::ToolCall(kind) => {
                let sequence = match kind {
                    ToolKind::Internal => {
                        internal_index += 1;
                        internal_index
                    }
                    ToolKind::External => {
                        external_index += 1;
                        external_index
                    }
                };
                turns.push(TurnPlan {
                    text_chunks: std::mem::take(&mut text_before_tool),
                    tool_call: Some(ToolCallPlan {
                        kind,
                        id: format!("{}-{sequence}", tool_kind_tag(kind)),
                        arguments_json: json!({
                            "payload": format!("{}-{sequence}", tool_kind_tag(kind)),
                            "sequence": sequence,
                        })
                        .to_string(),
                    }),
                });
            }
        }
    }

    if !text_before_tool.is_empty() {
        turns.push(TurnPlan {
            text_chunks: text_before_tool,
            tool_call: None,
        });
    }

    let model_output_tokens =
        spec.total_tokens + (internal_index + external_index) * spec.tool_argument_tokens;

    ModelPlan {
        turns,
        model_output_tokens,
        internal_tool_calls: internal_index,
        external_tool_calls: external_index,
    }
}

fn tool_kind_tag(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Internal => "internal",
        ToolKind::External => "external",
    }
}

fn external_tool_definition() -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".into(),
        function: FunctionDefinition {
            name: EXTERNAL_TOOL_NAME.into(),
            description: "Synthetic external benchmark tool executed by the outer loop.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "payload": { "type": "string" },
                    "sequence": { "type": "integer" }
                },
                "required": ["payload", "sequence"]
            }),
            extra_prompt: None,
        },
    }
}

async fn execute_external_tool_calls(
    spec: BenchmarkSpec,
    tool_calls: Vec<ParsedToolCall>,
    mut outcomes: Vec<ToolCallOutcome>,
) -> (Vec<ToolCallOutcome>, usize) {
    let mut visible_tool_tokens = 0usize;

    for tool_call in tool_calls {
        if spec.external_tool_latency_ms > 0 {
            sleep(Duration::from_millis(spec.external_tool_latency_ms)).await;
        }

        let payload = tool_call
            .arguments
            .get("payload")
            .and_then(|value| value.as_str())
            .unwrap_or("external");
        let sequence = tool_call
            .arguments
            .get("sequence")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let result = format!("external-result:{sequence}:{payload}");
        visible_tool_tokens += result.len();

        outcomes.push(ToolCallOutcome::Result {
            tool_call_id: tool_call.id,
            tool_name: tool_call.name,
            content: Content::text(result),
        });
    }

    (outcomes, visible_tool_tokens)
}

fn aggregate_metrics(spec: BenchmarkSpec, rounds: &[RoundMetrics]) -> AggregateMetrics {
    let rounds_count = rounds.len().max(1) as f64;
    let avg = |selector: fn(&RoundMetrics) -> f64| -> f64 {
        rounds.iter().map(selector).sum::<f64>() / rounds_count
    };

    let average_first_event_ms = if rounds.is_empty() {
        0.0
    } else {
        rounds
            .iter()
            .filter_map(|round| round.first_event_ms)
            .sum::<f64>()
            / rounds_count
    };

    let average_ttft_ms = if rounds.is_empty() {
        0.0
    } else {
        rounds.iter().filter_map(|round| round.ttft_ms).sum::<f64>() / rounds_count
    };

    AggregateMetrics {
        target_tps: spec.target_tps,
        total_visible_tokens: rounds
            .last()
            .map(|round| round.total_visible_tokens)
            .unwrap_or(0),
        model_output_tokens: rounds
            .last()
            .map(|round| round.model_output_tokens)
            .unwrap_or(0),
        chunk_tokens: spec.chunk_tokens,
        tool_argument_tokens: spec.tool_argument_tokens,
        internal_tool_calls: rounds
            .last()
            .map(|round| round.internal_tool_calls)
            .unwrap_or(0),
        external_tool_calls: rounds
            .last()
            .map(|round| round.external_tool_calls)
            .unwrap_or(0),
        rounds: rounds.len(),
        average_observed_tps: avg(|round| round.observed_tps),
        average_observed_model_tps: avg(|round| round.observed_model_tps),
        average_first_event_ms,
        average_ttft_ms,
        average_total_ms: avg(|round| round.total_ms),
        average_total_overhead_ms: avg(|round| round.total_overhead_ms),
        average_per_event_overhead_us: avg(|round| round.per_event_overhead_us),
    }
}

async fn run_round(spec: BenchmarkSpec) -> RoundMetrics {
    let model = ThrottledMockModel::new(spec);
    let model_output_tokens = model.plan.model_output_tokens;
    let planned_internal_tool_calls = model.plan.internal_tool_calls;
    let planned_external_tool_calls = model.plan.external_tool_calls;

    let mut builder = AgentBuilder::new().model(model);
    if planned_internal_tool_calls > 0 {
        builder = builder.tool(InternalBenchmarkTool {
            latency: Duration::from_millis(spec.internal_tool_latency_ms),
        });
    }
    let agent = builder.build_loop();

    let mut next_input = LoopInput::start("benchmark mock model throughput with tool calls");
    if planned_external_tool_calls > 0 {
        next_input = next_input.extra_tools(vec![external_tool_definition()]);
    }

    let start = Instant::now();
    let mut first_event_at = None;
    let mut first_text_at = None;
    let mut assistant_text_tokens = 0usize;
    let mut tool_output_tokens = 0usize;
    let mut tool_call_starts = 0usize;
    let mut tool_result_events = 0usize;
    let mut tool_delta_events = 0usize;
    let mut need_tool_execution_events = 0usize;
    let mut total_events = 0usize;

    loop {
        let stream = agent
            .chat(ChatCtx::default(), next_input)
            .await
            .expect("agent.chat failed");
        let mut stream = std::pin::pin!(stream);
        let mut resume_input = None;
        let mut done = false;

        while let Some(event) = stream.next().await {
            total_events += 1;
            match event {
                AgentEvent::TextDelta(content) => {
                    if first_event_at.is_none() {
                        first_event_at = Some(start.elapsed());
                    }
                    assistant_text_tokens += content.len();
                    if first_text_at.is_none() {
                        first_text_at = Some(start.elapsed());
                    }
                }
                AgentEvent::ToolCallStart { .. } => {
                    if first_event_at.is_none() {
                        first_event_at = Some(start.elapsed());
                    }
                    tool_call_starts += 1;
                }
                AgentEvent::ToolDelta { delta, .. } => {
                    if first_event_at.is_none() {
                        first_event_at = Some(start.elapsed());
                    }
                    tool_delta_events += 1;
                    tool_output_tokens += delta.len();
                }
                AgentEvent::ToolResult { result, .. } => {
                    if first_event_at.is_none() {
                        first_event_at = Some(start.elapsed());
                    }
                    tool_result_events += 1;
                    tool_output_tokens += result.len();
                }
                AgentEvent::NeedToolExecution {
                    state,
                    tool_calls,
                    completed_results,
                } => {
                    if first_event_at.is_none() {
                        first_event_at = Some(start.elapsed());
                    }
                    need_tool_execution_events += 1;
                    let (merged_results, external_output_tokens) =
                        execute_external_tool_calls(spec, tool_calls, completed_results).await;
                    tool_output_tokens += external_output_tokens;
                    resume_input = Some(LoopInput::resume(state, merged_results));
                    break;
                }
                AgentEvent::Done => {
                    done = true;
                }
                AgentEvent::Error(error) => {
                    panic!("benchmark agent error: {error}");
                }
                _ => {}
            }
        }

        if let Some(input) = resume_input {
            next_input = input;
            continue;
        }

        if done {
            break;
        }

        panic!("benchmark loop exited without Done or NeedToolExecution");
    }

    let total_elapsed = start.elapsed();
    let total_ms = total_elapsed.as_secs_f64() * 1000.0;
    let total_visible_tokens = assistant_text_tokens + tool_output_tokens;
    let theoretical_ms = model_output_tokens as f64 / spec.target_tps as f64 * 1000.0
        + planned_internal_tool_calls as f64 * spec.internal_tool_latency_ms as f64
        + planned_external_tool_calls as f64 * spec.external_tool_latency_ms as f64;
    let total_overhead_ms = total_ms - theoretical_ms;

    RoundMetrics {
        round: 0,
        target_tps: spec.target_tps,
        total_visible_tokens,
        assistant_text_tokens,
        tool_output_tokens,
        model_output_tokens,
        chunk_tokens: spec.chunk_tokens,
        tool_argument_tokens: spec.tool_argument_tokens,
        internal_tool_calls: planned_internal_tool_calls,
        external_tool_calls: planned_external_tool_calls,
        tool_call_starts,
        tool_result_events,
        tool_delta_events,
        need_tool_execution_events,
        total_events,
        observed_tps: rate_per_second(total_visible_tokens, total_elapsed),
        observed_model_tps: rate_per_second(model_output_tokens, total_elapsed),
        first_event_ms: first_event_at.map(duration_ms),
        ttft_ms: first_text_at.map(duration_ms),
        total_ms,
        theoretical_ms,
        total_overhead_ms,
        per_event_overhead_us: if total_events == 0 {
            0.0
        } else {
            total_overhead_ms * 1000.0 / total_events as f64
        },
    }
}

fn duration_ms(value: Duration) -> f64 {
    value.as_secs_f64() * 1000.0
}

fn rate_per_second(tokens: usize, elapsed: Duration) -> f64 {
    if elapsed.is_zero() {
        0.0
    } else {
        tokens as f64 / elapsed.as_secs_f64()
    }
}

fn parse_args() -> Result<BenchmarkConfig, String> {
    let mut config = BenchmarkConfig {
        rates: vec![50, 100, 1000],
        total_tokens: 1000,
        chunk_tokens: 5,
        tool_argument_tokens: 5,
        internal_tool_calls: 0,
        external_tool_calls: 0,
        internal_tool_probability: 0.0,
        external_tool_probability: 0.0,
        internal_tool_latency_ms: 0,
        external_tool_latency_ms: 0,
        warmup_rounds: 1,
        measured_rounds: 5,
        seed: DEFAULT_SEED,
        json: false,
    };

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rates" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --rates".to_string())?;
                let rates = value
                    .split(',')
                    .filter(|item| !item.is_empty())
                    .map(|item| {
                        item.parse::<u32>()
                            .map_err(|_| format!("invalid rate: {item}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if rates.is_empty() {
                    return Err("--rates must contain at least one integer".into());
                }
                config.rates = rates;
            }
            "--tokens" => {
                config.total_tokens = parse_usize_arg("--tokens", args.next())?;
            }
            "--chunk-tokens" => {
                config.chunk_tokens = parse_usize_arg("--chunk-tokens", args.next())?;
            }
            "--tool-arg-tokens" => {
                config.tool_argument_tokens = parse_usize_arg("--tool-arg-tokens", args.next())?;
            }
            "--internal-tool-calls" => {
                config.internal_tool_calls = parse_usize_arg("--internal-tool-calls", args.next())?;
            }
            "--external-tool-calls" => {
                config.external_tool_calls = parse_usize_arg("--external-tool-calls", args.next())?;
            }
            "--internal-tool-probability" => {
                config.internal_tool_probability =
                    parse_probability_arg("--internal-tool-probability", args.next())?;
            }
            "--external-tool-probability" => {
                config.external_tool_probability =
                    parse_probability_arg("--external-tool-probability", args.next())?;
            }
            "--internal-tool-latency-ms" => {
                config.internal_tool_latency_ms =
                    parse_u64_arg("--internal-tool-latency-ms", args.next())?;
            }
            "--external-tool-latency-ms" => {
                config.external_tool_latency_ms =
                    parse_u64_arg("--external-tool-latency-ms", args.next())?;
            }
            "--warmup" => {
                config.warmup_rounds = parse_usize_arg("--warmup", args.next())?;
            }
            "--rounds" => {
                config.measured_rounds = parse_usize_arg("--rounds", args.next())?;
            }
            "--seed" => {
                config.seed = parse_u64_arg("--seed", args.next())?;
            }
            "--json" => {
                config.json = true;
            }
            "--help" | "-h" => {
                print_usage_and_exit(0);
            }
            other => {
                return Err(format!("unknown argument: {other}"));
            }
        }
    }

    if config.chunk_tokens == 0 {
        return Err("--chunk-tokens must be greater than zero".into());
    }
    if config.total_tokens == 0 {
        return Err("--tokens must be greater than zero".into());
    }
    if config.tool_argument_tokens == 0
        && (config.internal_tool_calls + config.external_tool_calls) > 0
    {
        return Err(
            "--tool-arg-tokens must be greater than zero when tool calls are enabled".into(),
        );
    }
    if config.tool_argument_tokens == 0
        && (config.internal_tool_probability > 0.0 || config.external_tool_probability > 0.0)
    {
        return Err(
            "--tool-arg-tokens must be greater than zero when probabilistic tool calls are enabled"
                .into(),
        );
    }
    if config.measured_rounds == 0 {
        return Err("--rounds must be greater than zero".into());
    }

    Ok(config)
}

fn parse_usize_arg(flag: &str, value: Option<String>) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("missing value for {flag}"))?;
    value
        .parse::<usize>()
        .map_err(|_| format!("invalid integer for {flag}: {value}"))
}

fn parse_u64_arg(flag: &str, value: Option<String>) -> Result<u64, String> {
    let value = value.ok_or_else(|| format!("missing value for {flag}"))?;
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid integer for {flag}: {value}"))
}

fn parse_probability_arg(flag: &str, value: Option<String>) -> Result<f64, String> {
    let value = value.ok_or_else(|| format!("missing value for {flag}"))?;
    let probability = value
        .parse::<f64>()
        .map_err(|_| format!("invalid probability for {flag}: {value}"))?;
    if !(0.0..=1.0).contains(&probability) {
        return Err(format!("{flag} must be between 0.0 and 1.0"));
    }
    Ok(probability)
}

fn print_report(report: &BenchmarkReport) {
    println!("mode: {}", report.mode);
    println!(
        "warmup_rounds: {} | measured_rounds: {}",
        report.warmup_rounds, report.measured_rounds
    );
    println!(
        "target_tps | visible_tps(avg) | model_tps(avg) | first_event_ms(avg) | ttft_ms(avg) | overhead_ms(avg) | tool_calls(i/e)"
    );

    for rate in &report.results {
        let aggregate = &rate.aggregate;
        println!(
            "{:>10} | {:>16.2} | {:>14.2} | {:>19.2} | {:>12.2} | {:>16.2} | {:>6}/{:<6}",
            aggregate.target_tps,
            aggregate.average_observed_tps,
            aggregate.average_observed_model_tps,
            aggregate.average_first_event_ms,
            aggregate.average_ttft_ms,
            aggregate.average_total_overhead_ms,
            aggregate.internal_tool_calls,
            aggregate.external_tool_calls,
        );
    }

    println!();
    println!("Per-round detail:");
    for rate in &report.results {
        println!("  target_tps={}:", rate.target_tps);
        for round in &rate.measured {
            println!(
                "    round={} visible_tps={:.2} model_tps={:.2} first_event_ms={:.2} ttft_ms={:.2} total_ms={:.2} overhead_ms={:.2} tool_starts={} tool_results={} external_resumes={} total_events={}",
                round.round,
                round.observed_tps,
                round.observed_model_tps,
                round.first_event_ms.unwrap_or(0.0),
                round.ttft_ms.unwrap_or(0.0),
                round.total_ms,
                round.total_overhead_ms,
                round.tool_call_starts,
                round.tool_result_events,
                round.need_tool_execution_events,
                round.total_events,
            );
        }
    }
}

fn print_usage_and_exit(code: i32) -> ! {
    eprintln!(
        "Usage: cargo run -p remi-agentloop-core --example mock_model_benchmark -- [options]\n\n\
Options:\n\
  --rates 50,100,1000       Comma-separated target token rates\n\
  --tokens 1000             Total assistant text tokens emitted across the run\n\
  --chunk-tokens 5          Tokens packed into each text delta\n\
  --tool-arg-tokens 5       Synthetic model-output tokens spent on each tool call\n\
  --internal-tool-calls 0   Number of in-loop tool calls requested by the model\n\
  --external-tool-calls 0   Number of outer-loop tool calls requested by the model\n\
    --internal-tool-probability 0.0\n\
                                                     Per text-slot probability of inserting another internal tool call\n\
    --external-tool-probability 0.0\n\
                                                     Per text-slot probability of inserting another external tool call\n\
  --internal-tool-latency-ms 0\n\
                           Sleep injected into each internal tool execution\n\
  --external-tool-latency-ms 0\n\
                           Sleep injected into each external tool execution\n\
  --seed {DEFAULT_SEED}     Deterministic seed for randomized tool-call placement\n\
  --warmup 1                Warmup rounds per rate\n\
  --rounds 5                Measured rounds per rate\n\
  --json                    Emit JSON instead of a text table\n"
    );
    std::process::exit(code);
}

trait WithRound {
    fn with_round(self, round: usize) -> Self;
}

impl WithRound for RoundMetrics {
    fn with_round(mut self, round: usize) -> Self {
        self.round = round;
        self
    }
}
