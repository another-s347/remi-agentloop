//! Host-side runner for the WASM calculator guest agent.
//!
//! Loads a WASM component compiled from `examples/wasm-calculator-guest`,
//! provides calculator tools (add, subtract, multiply, divide) on the host,
//! and runs the full NeedToolExecution → execute → resume loop.
//!
//! # Usage
//!
//! ```sh
//! cargo run -p remi-agentloop-wasm --example wasm_calculator -- \
//!     path/to/calculator.wasm "2 + 3 * 4"
//! ```

use futures::StreamExt;

use remi_agentloop::agent::Agent;
use remi_agentloop::protocol::ProtocolEvent;
use remi_agentloop::types::{LoopInput, ParsedToolCall, ToolCallOutcome};
use remi_agentloop_wasm::WasmAgent;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let wasm_path = args.get(1).unwrap_or_else(|| {
        eprintln!("Usage: wasm_calculator <path/to/calculator.wasm> [expression]");
        eprintln!("       expression defaults to \"2 + 3 * 4\"");
        std::process::exit(1);
    });
    let expr = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "2 + 3 * 4".into());

    println!("Loading WASM component: {wasm_path}");
    let agent = WasmAgent::from_file(wasm_path).expect("Failed to load WASM component");

    println!("Evaluating: {expr}");
    println!("---");

    let mut input: LoopInput = LoopInput::start(&expr);
    let mut step = 0;

    loop {
        step += 1;
        let stream = agent
            .chat(input.clone())
            .await
            .expect("agent.chat() failed");
        let events: Vec<ProtocolEvent> = stream.collect().await;

        let mut resume_input = None;

        for event in events {
            match event {
                ProtocolEvent::Delta { content, .. } => {
                    println!("Result: {content}");
                }
                ProtocolEvent::NeedToolExecution {
                    state,
                    tool_calls,
                    completed_results,
                } => {
                    println!("Step {step}: executing {} tool(s)", tool_calls.len());
                    let mut results: Vec<ToolCallOutcome> = completed_results;
                    results.extend(execute_tools(&tool_calls));
                    resume_input = Some(LoopInput::Resume { state, results });
                }
                ProtocolEvent::Done => {
                    println!("---");
                    println!("Done in {step} step(s)");
                }
                ProtocolEvent::Error { message, code } => {
                    let code_str = code.as_deref().unwrap_or("unknown");
                    eprintln!("Error [{code_str}]: {message}");
                }
                _ => {}
            }
        }

        match resume_input {
            Some(next) => input = next,
            None => break,
        }
    }
}

/// Execute calculator tools on the host side.
fn execute_tools(tool_calls: &[ParsedToolCall]) -> Vec<ToolCallOutcome> {
    tool_calls
        .iter()
        .map(|tc| {
            let a = tc.arguments["a"].as_f64().unwrap_or(0.0);
            let b = tc.arguments["b"].as_f64().unwrap_or(0.0);

            let result = match tc.name.as_str() {
                "add" => a + b,
                "subtract" => a - b,
                "multiply" => a * b,
                "divide" => {
                    if b == 0.0 {
                        return ToolCallOutcome::Error {
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            error: "division by zero".into(),
                        };
                    }
                    a / b
                }
                other => {
                    return ToolCallOutcome::Error {
                        tool_call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        error: format!("unknown tool: {other}"),
                    };
                }
            };

            let result_str = if result.fract() == 0.0 && result.abs() < 1e15 {
                format!("{}", result as i64)
            } else {
                format!("{result}")
            };

            println!("  {}({}, {}) = {}", tc.name, a, b, result_str);

            ToolCallOutcome::Result {
                tool_call_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                content: remi_agentloop::types::Content::text(result_str),
            }
        })
        .collect()
}
