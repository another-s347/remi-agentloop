//! Simple MCP Client Example
//!
//! Demonstrates using the MCP client to connect to a server.
//!
//! First start the server:
//! ```bash
//! cargo run --example simple_server --features server
//! ```
//!
//! Then run this client:
//! ```bash
//! cargo run --example simple_client --features client
//! ```

use mcp_server::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔌 MCP Client Example\n");

    let client = McpClient::new("http://localhost:3000");

    println!("📡 Initializing connection...");
    let init_result = client.initialize("Example Client", "1.0.0").await?;
    println!("✅ Connected to: {} v{}", 
        init_result.server_info.name, 
        init_result.server_info.version
    );
    println!("   Protocol: {}", init_result.protocol_version);
    println!();

    // List tools
    println!("🔧 Available tools:");
    let tools = client.list_tools().await?;
    for tool in &tools.tools {
        println!("  • {} - {}", 
            tool.name, 
            tool.description.as_deref().unwrap_or("No description")
        );
    }
    println!();

    // Call calculator tool
    println!("🧮 Calling calculator tool (10 + 20)...");
    let result = client.call_tool(
        "calculator",
        Some(serde_json::json!({
            "operation": "add",
            "a": 10,
            "b": 20
        })),
    ).await?;
    
    for content in &result.content {
        match content {
            ToolContent::Text { text } => println!("   Result: {}", text),
            _ => println!("   (non-text content)"),
        }
    }
    println!();

    // Call echo tool
    println!("📢 Calling echo tool...");
    let result = client.call_tool(
        "echo",
        Some(serde_json::json!({
            "message": "Hello from MCP client!"
        })),
    ).await?;
    
    for content in &result.content {
        match content {
            ToolContent::Text { text } => println!("   {}", text),
            _ => println!("   (non-text content)"),
        }
    }
    println!();

    // List resources
    println!("📦 Available resources:");
    let resources = client.list_resources().await?;
    for resource in &resources.resources {
        println!("  • {} - {}", 
            resource.name, 
            resource.description.as_deref().unwrap_or("No description")
        );
    }
    println!();

    // Read resource
    println!("📄 Reading README resource...");
    let resource = client.read_resource("resource://readme").await?;
    for content in &resource.contents {
        match content {
            ResourceContent::Text { text, .. } => {
                println!("─────────────────────────────────────");
                println!("{}", text);
                println!("─────────────────────────────────────");
            }
            _ => println!("   (non-text content)"),
        }
    }
    println!();

    // List prompts
    println!("💬 Available prompts:");
    let prompts = client.list_prompts().await?;
    for prompt in &prompts.prompts {
        println!("  • {} - {}", 
            prompt.name, 
            prompt.description.as_deref().unwrap_or("No description")
        );
        if let Some(args) = &prompt.arguments {
            for arg in args {
                println!("    - {}: {}", 
                    arg.name,
                    arg.description.as_deref().unwrap_or("No description")
                );
            }
        }
    }
    println!();

    // Get prompt
    println!("💬 Getting greeting prompt...");
    let mut args = std::collections::HashMap::new();
    args.insert("name".to_string(), "Alice".to_string());
    let prompt = client.get_prompt("greeting", Some(args)).await?;
    
    for message in &prompt.messages {
        match &message.content {
            MessageContent::Text { text } => {
                println!("   [{:?}]: {}", message.role, text);
            }
            _ => println!("   (non-text content)"),
        }
    }
    println!();

    println!("✅ All tests completed successfully!");
    Ok(())
}
