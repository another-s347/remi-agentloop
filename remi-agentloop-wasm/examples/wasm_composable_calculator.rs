//! Host-side runner for the composable calculator WASM component (with HTTP).
//!
//! Loads a WASM component compiled from `examples/composable-calculator-wasip2`,
//! provides HTTP transport to the guest (so it can reach the LLM API), and
//! runs the full multi-turn agent loop.
//!
//! The WASM guest agent handles **all** tool execution internally — no
//! NeedToolExecution is surfaced to the host. The host just sends the user
//! query and receives streaming events.
//!
//! # Usage
//!
//! ```sh
//! # 1. Build the guest component
//! cd examples/composable-calculator-wasip2
//! cargo build --target wasm32-wasip2 --release
//!
//! # 2. Run the host
//! cargo run -p remi-agentloop-wasm --example wasm_composable_calculator -- \
//!     examples/composable-calculator-wasip2/target/wasm32-wasip2/release/composable_calculator_wasip2.wasm \
//!     "What is (12 + 8) * 3 / 4?"
//! ```

use futures::StreamExt;

use remi_agentloop::agent::Agent;
use remi_agentloop::protocol::ProtocolEvent;
use remi_agentloop::types::{ChatCtx, Content, LoopInput};
use remi_agentloop_wasm::WasmAgentWithHttp;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let wasm_path = args.get(1).unwrap_or_else(|| {
        eprintln!(
            "Usage: wasm_composable_calculator <path/to/component.wasm> [expression]\n\
             \n\
             Environment variables:\n\
             \n\
             Required:\n\
             \n\
               OPENAI_API_KEY    LLM API key\n\
             \n\
             Optional:\n\
             \n\
               OPENAI_BASE_URL   (default: https://api.openai.com/v1)\n\
               OPENAI_MODEL      (default: gpt-4o)\n"
        );
        std::process::exit(1);
    });
    let expr = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "(12 + 8) * 3 / 4".into());

    // Read API config from environment
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        eprintln!("Error: OPENAI_API_KEY environment variable is required");
        std::process::exit(1);
    });
    let base_url = std::env::var("OPENAI_BASE_URL").unwrap_or_default();
    let model = std::env::var("OPENAI_MODEL").unwrap_or_default();

    // Pack API config into metadata — the guest extracts it in extract_config()
    let metadata = serde_json::json!({
        "api_key": api_key,
        "base_url": base_url,
        "model": model,
    });

    println!("Loading WASM component: {wasm_path}");
    let agent = WasmAgentWithHttp::from_file(wasm_path).expect("Failed to load WASM component");

    println!("Expression: {expr}");
    println!("---");

    // The guest agent handles all tool execution internally.
    // We just send the query and collect events — no resume loop needed.
    let input = LoopInput::Start {
        content: Content::Text(expr),
        history: vec![],
        extra_tools: vec![],
        model: None,
        temperature: None,
        max_tokens: None,
        metadata: Some(metadata),
        message_metadata: None,
        user_name: None,
    };

    let stream = agent
        .chat(ChatCtx::default(), input)
        .await
        .expect("agent.chat() failed");
    let events: Vec<ProtocolEvent> = stream.collect().await;

    for event in &events {
        match event {
            ProtocolEvent::Delta { content, .. } => {
                print!("{content}");
            }
            ProtocolEvent::ToolCallDelta {
                id,
                arguments_delta,
            } => {
                println!("[tool:{id}] {arguments_delta}");
            }
            ProtocolEvent::ToolResult { name, result, .. } => {
                println!("[tool:{name}] → {result}");
            }
            ProtocolEvent::Done => {
                println!("\n---\nDone ({} events total)", events.len());
            }
            ProtocolEvent::Error { message, code } => {
                let code = code.as_deref().unwrap_or("unknown");
                eprintln!("Error [{code}]: {message}");
            }
            _ => {}
        }
    }
}
