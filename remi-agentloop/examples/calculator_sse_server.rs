//! Calculator SSE Server
//!
//! Exposes a calculator agent over HTTP SSE at `POST /chat`.
//! Pair with `calculator_sse_client` to see composability in action.
//!
//! Run with:
//!   OPENAI_API_KEY=sk-... cargo run --example calculator_sse_server --features http-server,http-client

use std::sync::Arc;

use futures::StreamExt;
use remi_agentloop::prelude::*;
use remi_agentloop::tool_macro as tool;
use remi_agentloop::transport::HttpSseServer;

// ── Tool definitions ─────────────────────────────────────────────────────────

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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    let agent = Arc::new(
        AgentBuilder::new()
            .model(oai)
            .system("You are a calculator assistant. Use the provided tools to compute results precisely.")
            .tool(Add::new())
            .tool(Multiply::new())
            .tool(Power::new())
            .max_turns(5)
            .build(),
    );

    // The Agent trait doesn't require Send on streams (WASM-compatible design),
    // but axum needs Send streams. Bridge via channel: spawn an OS thread to
    // drive the non-Send agent stream, send ProtocolEvents through a channel.
    let server = HttpSseServer::new(move |req: LoopInput| {
        let agent = agent.clone();
        async move {
            let (tx, rx) = tokio::sync::mpsc::channel::<ProtocolEvent>(32);

            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async move {
                    match agent.chat(req).await {
                        Ok(stream) => {
                            let mut stream = std::pin::pin!(stream);
                            while let Some(event) = stream.next().await {
                                let proto: ProtocolEvent = event.into();
                                if tx.send(proto).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx
                                .send(ProtocolEvent::Error {
                                    message: e.to_string(),
                                    code: Some("agent_error".into()),
                                })
                                .await;
                        }
                    }
                });
            });

            Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
        }
    })
    .bind(([0, 0, 0, 0], 8080));

    println!("Calculator SSE server listening on http://0.0.0.0:8080/chat");
    server.serve().await?;

    Ok(())
}
