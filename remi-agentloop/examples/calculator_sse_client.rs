//! Calculator SSE Client
//!
//! Connects to the calculator SSE server and calls it as a composable Agent.
//! Demonstrates that `HttpSseClient` implements `Agent`, so a remote agent
//! can be used interchangeably with a local one.
//!
//! Run with (after starting the server):
//!   cargo run --example calculator_sse_client --features http-client

use futures::StreamExt;
use remi_agentloop::prelude::*;
use remi_agentloop::transport::HttpSseClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = std::env::var("REMI_SSE_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:8080/chat".to_string());

    // HttpSseClient implements Agent<LoopInput, ProtocolEvent, ProtocolError>
    // — it's fully composable, you can layer/map/boxed it just like a local agent.
    let client = HttpSseClient::new(&endpoint);

    println!("Connecting to SSE server at {endpoint}");
    println!("Asking: (3 + 7) * 2^4 = ?");
    println!("{}", "─".repeat(50));

    let stream = client
        .chat("What is (3 + 7) * 2^4? Show step-by-step.".into())
        .await
        .map_err(|e| format!("Failed to connect: {e}"))?;
    let mut stream = std::pin::pin!(stream);

    while let Some(event) = stream.next().await {
        match event {
            ProtocolEvent::Delta { content, .. } => print!("{content}"),
            ProtocolEvent::ToolCallStart { name, .. } => {
                println!();
                print!("  [tool: {name}(");
            }
            ProtocolEvent::ToolCallDelta { arguments_delta, .. } => {
                print!("{arguments_delta}");
            }
            ProtocolEvent::ToolResult { name, result, .. } => {
                println!(") → {result}]  [{name}]");
            }
            ProtocolEvent::TurnStart { turn } => {
                if turn > 1 {
                    println!("\n--- turn {turn} ---");
                }
            }
            ProtocolEvent::Usage {
                prompt_tokens,
                completion_tokens,
            } => {
                eprintln!("\n[tokens: prompt={prompt_tokens} completion={completion_tokens}]");
            }
            ProtocolEvent::Done => {
                println!();
                println!("{}", "─".repeat(50));
                println!("Done.");
            }
            ProtocolEvent::Error { message, code } => {
                eprintln!("\nError: {message} (code: {code:?})");
            }
            _ => {}
        }
    }

    // ── Composability demo: layer the remote agent ────────────────────────────
    println!();
    println!("=== Composability demo: wrapping HttpSseClient with .map_response() ===");
    println!();

    let client2 = HttpSseClient::new(&endpoint);
    // Wrap the remote agent with a map_response adapter — same as any local agent
    let logged_client = client2.map_response(|event: ProtocolEvent| {
        if let ProtocolEvent::Delta { ref content, .. } = event {
            eprint!("[log] delta: {content}");
        }
        event
    });

    let stream = logged_client
        .chat("What is 5 + 3?".into())
        .await
        .map_err(|e| format!("Failed to connect: {e}"))?;
    let mut stream = std::pin::pin!(stream);

    while let Some(event) = stream.next().await {
        match event {
            ProtocolEvent::Delta { content, .. } => print!("{content}"),
            ProtocolEvent::Done => {
                println!();
                println!("Done (composed).");
            }
            _ => {}
        }
    }

    Ok(())
}
