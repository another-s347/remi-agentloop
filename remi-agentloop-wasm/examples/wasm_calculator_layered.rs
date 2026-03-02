//! Layered WASM calculator — demonstrates composable external-tool interception.
//!
//! Architecture (inner → outer):
//!
//! ```
//!  ┌──────────────────────────────────────────────────────┐
//!  │  App (no knowledge of any tools)                     │
//!  │  ┌────────────────────────────────────────────────┐  │
//!  │  │  DivideLayer  — intercepts "divide" calls      │  │
//!  │  │  ┌──────────────────────────────────────────┐  │  │
//!  │  │  │  MultiplyLayer — intercepts "multiply"   │  │  │
//!  │  │  │  ┌────────────────────────────────────┐  │  │  │
//!  │  │  │  │  WasmAgent  (WASM component)       │  │  │  │
//!  │  │  │  │  • "add"      ← handled internally │  │  │  │
//!  │  │  │  │  • "subtract" ← handled internally │  │  │  │
//!  │  │  │  │  • "multiply" → NeedToolExecution  │  │  │  │
//!  │  │  │  │  • "divide"   → NeedToolExecution  │  │  │  │
//!  │  │  │  └────────────────────────────────────┘  │  │  │
//!  │  │  └──────────────────────────────────────────┘  │  │
//!  │  └────────────────────────────────────────────────┘  │
//!  └──────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```sh
//! cargo run -p remi-agentloop-wasm --example wasm_calculator_layered -- \
//!     examples/wasm-calculator-guest/calculator.wasm "12 * 3 + 8 / 2 - 1"
//! ```

use futures::StreamExt;

use remi_agentloop::agent::Agent;
use remi_agentloop::protocol::{ProtocolError, ProtocolEvent};
use remi_agentloop::types::{LoopInput, ToolCallOutcome};
use remi_agentloop_wasm::WasmAgent;

// ── ToolInterceptLayer ────────────────────────────────────────────────────────
//
// A transparent adapter that wraps any ProtocolAgent and intercepts
// `NeedToolExecution` events for exactly ONE named tool.  Any
// `NeedToolExecution` carrying a different tool name is passed through
// unchanged to the outer layer.

struct ToolInterceptLayer<A> {
    inner: A,
    /// The single tool name this layer owns.
    tool_name: &'static str,
    /// Display label used in log lines so it's clear which layer acted.
    label: &'static str,
    /// Pure, synchronous compute function.
    compute: fn(f64, f64) -> f64,
}

impl<A> ToolInterceptLayer<A> {
    fn new(
        inner: A,
        tool_name: &'static str,
        label: &'static str,
        compute: fn(f64, f64) -> f64,
    ) -> Self {
        Self { inner, tool_name, label, compute }
    }
}

impl<A> Agent for ToolInterceptLayer<A>
where
    A: Agent<Request = LoopInput, Response = ProtocolEvent, Error = ProtocolError>,
{
    type Request = LoopInput;
    type Response = ProtocolEvent;
    type Error = ProtocolError;

    fn chat(
        &self,
        req: LoopInput,
    ) -> impl std::future::Future<
        Output = Result<impl futures::Stream<Item = ProtocolEvent>, ProtocolError>,
    > {
        // `self` is &Self (Copy), so `async move` copies the thin pointer and
        // the returned future borrows `*self` for the duration of the poll.
        let tool_name = self.tool_name;
        let label = self.label;
        let compute = self.compute;

        async move {
            let mut current_input = req;
            // Events to forward upward once we have no more work to do in
            // this layer.  Includes pass-through NeedToolExecution for tools
            // owned by outer layers, and all terminal events (Delta, Done, …).
            let mut forwarded: Vec<ProtocolEvent> = Vec::new();

            loop {
                let stream = self.inner.chat(current_input.clone()).await?;
                let events: Vec<ProtocolEvent> = stream.collect().await;

                let mut resume = None;

                for event in events {
                    match event {
                        ProtocolEvent::NeedToolExecution {
                            ref state,
                            ref tool_calls,
                            ref completed_results,
                        } if tool_calls.iter().all(|tc| tc.name == tool_name) => {
                            // This layer owns all calls in this batch — execute
                            // them and prepare a Resume input.
                            let mut results = completed_results.clone();
                            for tc in tool_calls {
                                let a = tc.arguments["a"].as_f64().unwrap_or(0.0);
                                let b = tc.arguments["b"].as_f64().unwrap_or(0.0);
                                let r = compute(a, b);
                                println!(
                                    "  [{label}] {}({}, {}) = {}",
                                    tc.name,
                                    a,
                                    b,
                                    fmt_num(r)
                                );
                                results.push(ToolCallOutcome::Result {
                                    tool_call_id: tc.id.clone(),
                                    tool_name: tc.name.clone(),
                                    result: fmt_num(r),
                                });
                            }
                            resume = Some(LoopInput::Resume {
                                state: state.clone(),
                                results,
                            });
                        }
                        // Unknown tool or non-NeedToolExecution event —
                        // collect it and let the outer layer deal with it.
                        other => forwarded.push(other),
                    }
                }

                match resume {
                    Some(next) => current_input = next,
                    // Nothing left to do at this layer; hand everything upward.
                    None => break,
                }
            }

            Ok(futures::stream::iter(forwarded))
        }
    }
}

// ── App layer ─────────────────────────────────────────────────────────────────
//
// The outermost consumer.  It only knows that `.chat()` returns a stream of
// `ProtocolEvent`; it has no concept of tools, NeedToolExecution, or WASM.

async fn run_app(
    agent: &impl Agent<Request = LoopInput, Response = ProtocolEvent, Error = ProtocolError>,
    expr: &str,
) {
    println!("Expression : {expr}");
    println!("{}", "─".repeat(52));

    let input = LoopInput::start(expr);
    let stream = agent
        .chat(input)
        .await
        .expect("app: chat() failed");
    let mut stream = std::pin::pin!(stream);

    while let Some(event) = stream.next().await {
        match event {
            ProtocolEvent::Delta { content, .. } => println!("Result     : {content}"),
            ProtocolEvent::Done => println!("{}", "─".repeat(52)),
            ProtocolEvent::Error { message, code } => {
                let c = code.as_deref().unwrap_or("?");
                eprintln!("Error [{c}]: {message}");
            }
            // NeedToolExecution should never reach here — the middle layers
            // should have absorbed everything.
            ProtocolEvent::NeedToolExecution { tool_calls, .. } => {
                let names: Vec<_> = tool_calls.iter().map(|tc| tc.name.as_str()).collect();
                eprintln!("BUG: unhandled NeedToolExecution reached app layer: {names:?}");
            }
            _ => {}
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let wasm_path = args.get(1).cloned().unwrap_or_else(|| {
        eprintln!("Usage: wasm_calculator_layered <path/to/calculator.wasm> [expression]");
        std::process::exit(1);
    });
    let expr = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "12 * 3 + 8 / 2 - 1".into());

    println!("Loading WASM component : {wasm_path}");
    println!();

    // Layer 0 — core WASM agent
    //   Handles +  and -  internally.
    //   Surfaces "multiply" and "divide" as NeedToolExecution.
    let wasm = WasmAgent::from_file(&wasm_path).expect("failed to load WASM component");

    // Layer 1 — multiply interception
    //   Observes NeedToolExecution for "multiply", executes it, resumes.
    //   Passes through everything else (including "divide") unchanged.
    let with_mul = ToolInterceptLayer::new(
        wasm,
        "multiply",
        "MultiplyLayer",
        |a, b| a * b,
    );

    // Layer 2 — divide interception
    //   Observes NeedToolExecution for "divide", executes it, resumes.
    //   At this point no more NeedToolExecution should appear upstream.
    let with_div = ToolInterceptLayer::new(
        with_mul,
        "divide",
        "DivideLayer",
        |a, b| {
            if b == 0.0 { f64::NAN } else { a / b }
        },
    );

    // App — calls the fully-composed agent, sees only Delta / Done
    run_app(&with_div, &expr).await;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}
