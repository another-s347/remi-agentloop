/// Simple ACP example without complex delegation
/// 
/// This demonstrates the basic ACP protocol with a single agent.

use async_stream::stream;
use futures::{Stream, StreamExt};
use std::collections::HashMap;
use std::pin::Pin;
use remi_agentloop_transport::acp::*;

// ── Simple Echo Agent ─────────────────────────────────────────────────────────

struct EchoAgent {
    agent_id: AgentId,
}

impl EchoAgent {
    fn new() -> Self {
        Self {
            agent_id: AgentId::new("echo_agent"),
        }
    }
}

impl AcpAgent for EchoAgent {
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            agent_id: self.agent_id.clone(),
            name: "Echo Agent".into(),
            description: "Simple echo agent for testing".into(),
            version: "1.0.0".into(),
            tools: vec![],
            domains: vec!["echo".into(), "test".into()],
            languages: vec!["en".into(), "zh".into()],
            performance: Some(AgentPerformance {
                avg_latency_ms: 50,
                max_concurrency: 100,
                rate_limit_rpm: 1000,
            }),
            cost: Some(AgentCost {
                input_cost: 0.0,
                output_cost: 0.0,
                currency: "USD".into(),
            }),
            metadata: HashMap::new(),
        }
    }

    fn execute(
        &self,
        request: AcpRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Pin<Box<dyn Stream<Item = AcpEvent> + Send>>, AcpError>> + Send>> {
        let agent_id = self.agent_id.clone();
        let input_text = request.content.text_content();
        
        Box::pin(async move {
            let task_id = TaskId::new();
            let session_id = request.session_id.unwrap_or_else(SessionId::new);

            Ok(Box::pin(stream! {
                // Start
                yield AcpEvent::AgentStart {
                    session_id: session_id.clone(),
                    task_id: task_id.clone(),
                    agent_id: agent_id.clone(),
                    agent_name: "Echo Agent".into(),
                    metadata: None,
                };

                // Echo the input
                yield AcpEvent::ContentDelta {
                    task_id: task_id.clone(),
                    delta: format!("You said: {}", input_text),
                };

                // Usage
                yield AcpEvent::Usage {
                    task_id: task_id.clone(),
                    input_tokens: input_text.len() as u64,
                    output_tokens: input_text.len() as u64,
                    cost: 0.0,
                    currency: "USD".into(),
                };

                // End
                yield AcpEvent::AgentEnd {
                    task_id,
                    status: TaskStatus::Success,
                    result: Some(AcpContent::text(format!("Echo: {}", input_text))),
                    error: None,
                };
            }) as Pin<Box<dyn Stream<Item = AcpEvent> + Send>>)
        })
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🤖 Simple ACP Example\n");

    // Create registry and register agent
    let registry = AgentRegistry::new();
    let echo_agent = Box::new(EchoAgent::new());
    let router = AcpRouter::new(registry.clone())
        .register_agent(echo_agent);

    println!("✅ Registered agents:");
    for cap in registry.list() {
        println!("  - {} ({}): {}", cap.name, cap.agent_id, cap.description);
        println!("    Domains: {}", cap.domains.join(", "));
    }
    println!();

    // Execute a simple request
    println!("📝 Executing request...\n");
    let request = AcpRequest {
        session_id: None,
        content: AcpContent::text("Hello, ACP!"),
        target_agent: Some(AgentId::new("echo_agent")),
        routing: None,
        history: vec![],
        constraints: None,
        metadata: HashMap::new(),
    };

    let mut stream = router.execute(request).await?;
    
    println!("📡 Response stream:");
    while let Some(event) = stream.next().await {
        match &event {
            AcpEvent::AgentStart { agent_name, task_id, .. } => {
                println!("  🤖 Agent '{}' started (task: {})", agent_name, task_id);
            }
            AcpEvent::ContentDelta { delta, .. } => {
                println!("  💬 {}", delta);
            }
            AcpEvent::Usage { input_tokens, output_tokens, cost, currency, .. } => {
                println!("  💰 Usage: in={}, out={}, cost={} {}", 
                    input_tokens, output_tokens, cost, currency);
            }
            AcpEvent::AgentEnd { status, result, .. } => {
                println!("  ✨ Agent finished: {:?}", status);
                if let Some(res) = result {
                    println!("  📊 Final result: {}", res.text_content());
                }
            }
            _ => {}
        }
    }

    println!("\n✅ Example completed!");
    Ok(())
}
