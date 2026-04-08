/// Example: ACP HTTP Client
///
/// Demonstrates connecting to an ACP server and executing requests.

use std::collections::HashMap;
use remi_agentloop_transport::acp::*;
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔌 ACP HTTP Client Example\n");

    let client = AcpClient::new("http://localhost:8080/acp");

    // Example 1: Discover agents
    println!("📝 Example 1: Discovering agents...");
    let query = AgentQueryRequest {
        domains: vec![],
        required_tools: vec![],
        query: None,
        language: None,
    };

    match client.discover(query).await {
        Ok(agents) => {
            println!("✅ Found {} agents:", agents.len());
            for cap in agents {
                println!("  - {} ({}): {}", cap.name, cap.agent_id, cap.description);
                println!("    Domains: {}", cap.domains.join(", "));
                if let Some(perf) = cap.performance {
                    println!("    Latency: {}ms", perf.avg_latency_ms);
                }
            }
        }
        Err(e) => {
            println!("❌ Discovery failed: {}", e);
        }
    }

    println!("\n{}\n", "=".repeat(80));

    // Example 2: Execute a math task
    println!("📝 Example 2: Executing math calculation...");
    let request = AcpRequest {
        session_id: None,
        content: AcpContent::text("calculate 15 + 27"),
        target_agent: Some(AgentId::new("math_agent")),
        routing: None,
        history: vec![],
        constraints: None,
        metadata: HashMap::new(),
    };

    match client.execute(request).await {
        Ok(mut stream) => {
            println!("📡 Streaming response:");
            while let Some(event) = stream.next().await {
                match event {
                    AcpEvent::AgentStart { agent_name, .. } => {
                        println!("  🤖 Agent: {}", agent_name);
                    }
                    AcpEvent::ToolCallStart { tool_name, .. } => {
                        println!("  🔧 Using tool: {}", tool_name);
                    }
                    AcpEvent::ToolResult { result, .. } => {
                        println!("  ✅ Tool result: {}", result.text_content());
                    }
                    AcpEvent::ContentDelta { delta, .. } => {
                        print!("{}", delta);
                    }
                    AcpEvent::AgentEnd { status, result, .. } => {
                        println!("\n  ✨ Task completed: {:?}", status);
                        if let Some(res) = result {
                            println!("  📊 Final result: {}", res.text_content());
                        }
                        break;
                    }
                    _ => {}
                }
            }
        }
        Err(e) => {
            println!("❌ Execution failed: {}", e);
        }
    }

    Ok(())
}
