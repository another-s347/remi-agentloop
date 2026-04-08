/// Example: ACP HTTP Server
///
/// This example demonstrates:
/// 1. Setting up an ACP HTTP server
/// 2. Exposing multiple agents via REST API
/// 3. Agent discovery endpoint
/// 4. Streaming responses via SSE

use std::collections::HashMap;
use remi_agentloop_transport::acp::*;
use async_stream::stream;
use futures::{Stream, StreamExt};
use std::pin::Pin;

// ── Simple Math Agent ─────────────────────────────────────────────────────────

struct MathAgent {
    agent_id: AgentId,
}

impl MathAgent {
    fn new() -> Self {
        Self {
            agent_id: AgentId::new("math_agent"),
        }
    }
}

impl AcpAgent for MathAgent {
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            agent_id: self.agent_id.clone(),
            name: "Math Agent".into(),
            description: "Performs mathematical calculations and analysis".into(),
            version: "1.0.0".into(),
            tools: vec![
                AcpToolDefinition {
                    name: "calculate".into(),
                    description: "Evaluate a mathematical expression".into(),
                    parameters: vec![
                        ToolParameter {
                            name: "expression".into(),
                            description: "Math expression to evaluate".into(),
                            param_type: "string".into(),
                            required: true,
                            enum_values: None,
                        },
                    ],
                    metadata: HashMap::new(),
                },
            ],
            domains: vec!["math".into(), "calculation".into(), "arithmetic".into()],
            languages: vec!["en".into(), "zh".into()],
            performance: Some(AgentPerformance {
                avg_latency_ms: 100,
                max_concurrency: 50,
                rate_limit_rpm: 1000,
            }),
            cost: Some(AgentCost {
                input_cost: 0.0001,
                output_cost: 0.0002,
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
        let query = request.content.text_content();
        
        Box::pin(async move {
            let task_id = TaskId::new();
            let session_id = request.session_id.unwrap_or_else(SessionId::new);

            Ok(Box::pin(stream! {
                yield AcpEvent::AgentStart {
                    session_id: session_id.clone(),
                    task_id: task_id.clone(),
                    agent_id: agent_id.clone(),
                    agent_name: "Math Agent".into(),
                    metadata: None,
                };

                // Extract number from query (simple parsing)
                let numbers: Vec<f64> = query
                    .split_whitespace()
                    .filter_map(|s| s.parse::<f64>().ok())
                    .collect();

                let tool_call_id = format!("tc_{}", uuid::Uuid::new_v4());
                yield AcpEvent::ToolCallStart {
                    task_id: task_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    tool_name: "calculate".into(),
                    arguments: serde_json::json!({ "expression": query }),
                };

                let result = if numbers.len() >= 2 {
                    // Simple addition
                    numbers.iter().sum::<f64>()
                } else if numbers.len() == 1 {
                    numbers[0]
                } else {
                    42.0 // Default
                };

                yield AcpEvent::ToolResult {
                    task_id: task_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    result: AcpContent::text(result.to_string()),
                    error: None,
                };

                yield AcpEvent::ContentDelta {
                    task_id: task_id.clone(),
                    delta: format!("The result is: {}", result),
                };

                yield AcpEvent::Usage {
                    task_id: task_id.clone(),
                    input_tokens: 10,
                    output_tokens: 20,
                    cost: 0.000003,
                    currency: "USD".into(),
                };

                yield AcpEvent::AgentEnd {
                    task_id: task_id.clone(),
                    status: TaskStatus::Success,
                    result: Some(AcpContent::text(result.to_string())),
                    error: None,
                };
            }) as Pin<Box<dyn Stream<Item = AcpEvent> + Send>>)
        })
    }
}

// ── Main Server ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Starting ACP HTTP Server...\n");

    // Create registry
    let registry = AgentRegistry::new();

    // Register agents
    let math_agent = Box::new(MathAgent::new());
    let router = AcpRouter::new(registry.clone())
        .register_agent(math_agent);

    println!("✅ Registered agents:");
    for cap in registry.list() {
        println!("  - {} ({})", cap.name, cap.agent_id);
        println!("    Domains: {}", cap.domains.join(", "));
        println!("    Tools: {}", cap.tools.iter().map(|t| &t.name).cloned().collect::<Vec<_>>().join(", "));
        if let Some(perf) = &cap.performance {
            println!("    Performance: {}ms latency, {} max concurrent", perf.avg_latency_ms, perf.max_concurrency);
        }
    }
    println!();

    #[cfg(feature = "http-server")]
    {
        // Start HTTP server using closure-based API
        let router_for_server = router.clone();
        let server = AcpServer::new(move |req| {
            let router = router_for_server.clone();
            async move { router.execute(req).await }
        })
        .bind(([0, 0, 0, 0], 8080));

        println!("📡 Server endpoints:");
        println!("  POST http://localhost:8080/acp          - Execute agent request");
        println!("\n📝 Example curl commands:");
        println!(r#"  curl -X POST http://localhost:8080/acp -H "Content-Type: application/json" -d '{{"content":"calculate 2 + 3"}}'"#);
        println!();

        server.serve().await?;
    }

    #[cfg(not(feature = "http-server"))]
    {
        println!("❌ HTTP server feature not enabled. Enable with:");
        println!("   cargo run --features http-server");
    }

    Ok(())
}
