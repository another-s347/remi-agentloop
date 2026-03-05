//! Test: external tool calling using the raw step() API.
//!
//! This test demonstrates that tools are NEVER called inside step().
//! The caller (this code) executes tools manually and feeds results back.
//!
//! Run with:
//!   REMI_API_KEY=... REMI_BASE_URL=... REMI_MODEL=... \
//!     cargo run --example step_external_tools --features http-client

use futures::StreamExt;
use remi_agentloop::prelude::*;
use remi_agentloop::tool::ToolDefinition;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("REMI_API_KEY"))
        .expect("OPENAI_API_KEY or REMI_API_KEY must be set");

    let model_name = std::env::var("REMI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
    let base_url = std::env::var("REMI_BASE_URL")
        .or_else(|_| std::env::var("OPENAI_BASE_URL"))
        .ok();

    let mut oai = OpenAIClient::new(api_key).with_model(model_name.clone());
    if let Some(url) = base_url {
        oai = oai.with_base_url(url);
    }

    // ── Define tool definitions (schemas only, no implementation inside step) ──
    let tool_defs = vec![
        ToolDefinition {
            tool_type: "function".to_string(),
            function: remi_agentloop::tool::FunctionDefinition {
                name: "add".to_string(),
                description: "Add two numbers together".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "a": { "type": "number" },
                        "b": { "type": "number" }
                    },
                    "required": ["a", "b"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: remi_agentloop::tool::FunctionDefinition {
                name: "multiply".to_string(),
                description: "Multiply two numbers together".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "a": { "type": "number" },
                        "b": { "type": "number" }
                    },
                    "required": ["a", "b"]
                }),
            },
        },
    ];

    // ── Build initial state ──────────────────────────────────────────────
    let state = AgentState::new(StepConfig::new(model_name))
        .with_system_prompt("You are a calculator. Use the tools to compute results. Always show your work.")
        .with_tool_definitions(tool_defs);

    eprintln!("═══ External Tool Calling Test ═══");
    eprintln!("Question: What is (3 + 7) * 5?");
    eprintln!();

    // ── Step loop (tools executed externally) ─────────────────────────────
    let mut current_state = state;
    let mut action = Action::UserMessage("What is (3 + 7) * 5? Use the tools.".to_string());
    let mut turn = 0;

    loop {
        turn += 1;
        if turn > 10 {
            eprintln!("⚠ Max turns exceeded");
            break;
        }

        eprintln!("── step {turn} ──");
        let step_stream = step(current_state, action.clone(), &oai);
        let mut step_stream = std::pin::pin!(step_stream);

        let mut next_state = None;
        let mut pending_tool_calls = None;

        while let Some(event) = step_stream.next().await {
            match event {
                StepEvent::TextDelta(text) => {
                    print!("{text}");
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }
                StepEvent::ToolCallStart { id, name } => {
                    eprintln!("  [step yields] ToolCallStart: id={id} name={name}");
                }
                StepEvent::ToolCallArgumentsDelta { id, delta } => {
                    eprint!("{delta}");
                    let _ = id;
                }
                StepEvent::Usage { prompt_tokens, completion_tokens } => {
                    eprintln!("  [tokens] prompt={prompt_tokens} completion={completion_tokens}");
                }
                StepEvent::Done { state } => {
                    eprintln!("  [step yields] Done — conversation complete");
                    next_state = Some(state);
                }
                StepEvent::NeedToolExecution { state, tool_calls } => {
                    eprintln!();
                    eprintln!("  [step yields] NeedToolExecution — {} tool call(s):", tool_calls.len());
                    for tc in &tool_calls {
                        eprintln!("    → {}({})", tc.name, tc.arguments);
                    }
                    next_state = Some(state);
                    pending_tool_calls = Some(tool_calls);
                }
                StepEvent::Error { error, .. } => {
                    eprintln!("  [step yields] Error: {error}");
                    return Err(error.into());
                }
            }
        }

        current_state = next_state.expect("step() should always yield a terminal event");

        match pending_tool_calls {
            None => {
                // Done — no tool calls
                println!();
                break;
            }
            Some(tool_calls) => {
                // ── EXTERNAL tool execution (this is the key test) ──
                eprintln!("  [caller] Executing tools externally...");
                let mut outcomes = Vec::new();
                for tc in &tool_calls {
                    let result = match tc.name.as_str() {
                        "add" => {
                            let a = tc.arguments["a"].as_f64().unwrap_or(0.0);
                            let b = tc.arguments["b"].as_f64().unwrap_or(0.0);
                            let r = a + b;
                            eprintln!("  [caller] add({a}, {b}) = {r}");
                            r.to_string()
                        }
                        "multiply" => {
                            let a = tc.arguments["a"].as_f64().unwrap_or(0.0);
                            let b = tc.arguments["b"].as_f64().unwrap_or(0.0);
                            let r = a * b;
                            eprintln!("  [caller] multiply({a}, {b}) = {r}");
                            r.to_string()
                        }
                        other => {
                            format!("error: unknown tool '{other}'")
                        }
                    };
                    outcomes.push(ToolCallOutcome::Result {
                        tool_call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        result,
                    });
                }

                // Feed results back
                action = Action::ToolResults(outcomes);
            }
        }
    }

    // ── Verify final state ───────────────────────────────────────────────
    eprintln!();
    eprintln!("═══ Final State ═══");
    eprintln!("Phase: {:?}", current_state.phase);
    eprintln!("Messages: {} total", current_state.messages.len());
    for (i, msg) in current_state.messages.iter().enumerate() {
        let role = format!("{:?}", msg.role);
        let preview = msg.content.text_content();
        let preview = if preview.chars().count() > 80 {
            format!("{}…", preview.chars().take(80).collect::<String>())
        } else {
            preview
        };
        eprintln!("  [{i}] {role}: {preview}");
    }

    // ── Test: state is serializable ──────────────────────────────────────
    let json = serde_json::to_string_pretty(&current_state)?;
    eprintln!();
    eprintln!("═══ State Serialization Test ═══");
    eprintln!("Serialized state size: {} bytes", json.len());
    let deserialized: AgentState = serde_json::from_str(&json)?;
    eprintln!("Deserialized messages: {} (matches: {})",
        deserialized.messages.len(),
        deserialized.messages.len() == current_state.messages.len()
    );
    eprintln!("✓ State round-trip OK");

    Ok(())
}
