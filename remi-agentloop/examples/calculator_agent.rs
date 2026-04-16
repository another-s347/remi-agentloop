//! Calculator agent example
//!
//! Demonstrates:
//! - `#[tool]` proc-macro for quick tool definition
//! - `AgentBuilder` + `BuiltAgent::chat()`
//! - Streaming `AgentEvent` consumption
//!
//! Run with:
//!   OPENAI_API_KEY=sk-... cargo run --example calculator_agent --features http-client

use futures::StreamExt;
use remi_agentloop::prelude::*;
use remi_agentloop::tool_macro as tool;

// ── Tool definitions via #[tool] macro ───────────────────────────────────────

/// Add two integers together.
#[tool]
async fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// Multiply two integers together.
#[tool]
async fn multiply(a: i64, b: i64) -> i64 {
    a * b
}

/// Compute integer power: base^exp.
#[tool]
async fn power(base: i64, exp: i64) -> i64 {
    base.pow(exp.max(0) as u32)
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), AgentError> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("REMI_API_KEY"))
        .expect("OPENAI_API_KEY or REMI_API_KEY must be set");

    let model = std::env::var("REMI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
    let base_url = std::env::var("REMI_BASE_URL")
        .or_else(|_| std::env::var("OPENAI_BASE_URL"))
        .ok();

    let mut oai = OpenAIClient::new(api_key).with_model(model);
    if let Some(url) = base_url {
        oai = oai.with_base_url(url);
    }

    let agent = AgentBuilder::new()
        .model(oai)
        .system(
            "You are a calculator assistant. Use the provided tools to compute results precisely.",
        )
        .tool(Add::new())
        .tool(Multiply::new())
        .tool(Power::new())
        .max_turns(5)
        .build();

    println!("Calculator Agent — asking: (3 + 7) * 2^4 = ?");
    println!("{}", "─".repeat(50));

    let stream = agent
        .chat(
            ChatCtx::default(),
            "What is (3 + 7) * 2^4? Show step-by-step.".into(),
        )
        .await?;
    let mut stream = std::pin::pin!(stream);

    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::TextDelta(text) => print!("{text}"),
            AgentEvent::ToolCallStart { name, .. } => {
                println!();
                print!("  [tool: {name}(");
            }
            AgentEvent::ToolCallArgumentsDelta { delta, .. } => {
                print!("{delta}");
            }
            AgentEvent::ToolResult { name, result, .. } => {
                println!(") → {result}]  [{name}]");
            }
            AgentEvent::TurnStart { turn } => {
                if turn > 1 {
                    println!("\n--- turn {turn} ---");
                }
            }
            AgentEvent::Usage {
                prompt_tokens,
                completion_tokens,
            } => {
                eprintln!("\n[tokens: prompt={prompt_tokens} completion={completion_tokens}]");
            }
            AgentEvent::Done => {
                println!();
                println!("{}", "─".repeat(50));
                println!("Done.");
            }
            AgentEvent::Error(e) => {
                eprintln!("\nError: {e}");
            }
            _ => {}
        }
    }

    Ok(())
}
