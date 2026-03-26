//! Advanced MCP Client Example
//!
//! Demonstrates advanced client features:
//! - Connection state management
//! - Schema introspection
//! - Health checks
//! - Batch operations
//! - Dynamic discovery
//!
//! Start server first:
//! ```bash
//! cargo run --example simple_server --features server
//! ```
//!
//! Then run this:
//! ```bash
//! cargo run --example advanced_client --features client
//! ```

use mcp_server::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔌 Advanced MCP Client Example\n");

    let client = McpClient::new("http://localhost:3000");

    // Check initial state
    println!("📊 Initial state:");
    println!("  Connection: {:?}", client.state().await);
    println!("  Connected: {}", client.is_connected().await);
    println!();

    // Initialize connection
    println!("📡 Initializing connection...");
    let init_result = client.initialize("Advanced Client", "2.0.0").await?;
    println!("✅ Connected to: {} v{}", 
        init_result.server_info.name, 
        init_result.server_info.version
    );
    println!("   Protocol: {}", init_result.protocol_version);
    println!("   State: {:?}", client.state().await);
    println!();

    // Get server capabilities summary
    println!("🎯 Server Capabilities Summary:");
    let summary = client.get_capabilities_summary().await?;
    println!("  Server: {} v{}", summary.server_name, summary.server_version);
    println!("  Protocol: {}", summary.protocol_version);
    println!("  Tools: {} - {:?}", summary.tools_count, summary.tools);
    println!("  Resources: {} - {:?}", summary.resources_count, summary.resources);
    println!("  Prompts: {} - {:?}", summary.prompts_count, summary.prompts);
    println!("  Logging: {}", if summary.has_logging { "✓" } else { "✗" });
    println!();

    // Export all schemas
    println!("📋 Exporting schemas...");
    let schemas_json = client.export_schemas_json().await?;
    println!("  Schemas exported ({} bytes)", schemas_json.len());
    println!();

    // Tool introspection
    println!("🔍 Tool Schema Introspection:");
    let calc_schema = client.get_tool_schema("calculator").await?;
    println!("  calculator schema:");
    println!("{}", serde_json::to_string_pretty(&calc_schema)?);
    println!();

    // Check if specific tools exist
    println!("🔎 Tool Existence Checks:");
    println!("  calculator exists: {}", client.has_tool("calculator").await?);
    println!("  echo exists: {}", client.has_tool("echo").await?);
    println!("  nonexistent exists: {}", client.has_tool("nonexistent").await?);
    println!();

    // Search tools
    println!("🔍 Searching tools with 'calc':");
    let found_tools = client.search_tools("calc").await?;
    for tool in found_tools {
        println!("  • {}: {}", tool.name, tool.description.unwrap_or_default());
    }
    println!();

    // Batch tool calls
    println!("🔧 Batch tool calls:");
    let batch_calls = vec![
        ("calculator".to_string(), Some(serde_json::json!({
            "operation": "add", "a": 5, "b": 3
        }))),
        ("calculator".to_string(), Some(serde_json::json!({
            "operation": "multiply", "a": 4, "b": 7
        }))),
        ("echo".to_string(), Some(serde_json::json!({
            "message": "Batch test"
        }))),
    ];

    let results = client.call_tools_batch(batch_calls).await;
    for (i, result) in results.iter().enumerate() {
        match result {
            Ok(res) => {
                if let Some(ToolContent::Text { text }) = res.content.first() {
                    println!("  {}. {}", i + 1, text);
                }
            }
            Err(e) => println!("  {}. Error: {}", i + 1, e),
        }
    }
    println!();

    // Health check
    println!("🏥 Health Check:");
    let health = client.health_check().await;
    println!("  State: {:?}", health.state);
    println!("  Server reachable: {}", health.server_reachable);
    if let Some(info) = health.server_info {
        println!("  Server: {} v{}", info.name, info.version);
    }
    println!();

    // Statistics
    println!("📊 Client Statistics:");
    let stats = client.stats().await;
    println!("  Requests sent: {}", stats.requests_sent);
    println!("  Responses received: {}", stats.responses_received);
    println!("  Errors: {}", stats.errors);
    println!("  Tools called: {}", stats.tools_called);
    println!("  Resources read: {}", stats.resources_read);
    println!("  Prompts fetched: {}", stats.prompts_fetched);
    println!();

    // Cache demonstration
    println!("🗄️  Cache Demonstration:");
    println!("  First call (cache miss)...");
    let start = std::time::Instant::now();
    let _tools1 = client.list_tools().await?;
    println!("    Time: {:?}", start.elapsed());
    
    println!("  Second call (cache hit)...");
    let start = std::time::Instant::now();
    let _tools2 = client.list_tools().await?;
    println!("    Time: {:?}", start.elapsed());
    
    println!("  Clearing cache...");
    client.clear_tools_cache().await;
    
    println!("  Third call (cache miss after clear)...");
    let start = std::time::Instant::now();
    let _tools3 = client.list_tools().await?;
    println!("    Time: {:?}", start.elapsed());
    println!();

    // Test ping
    println!("🏓 Ping test:");
    let ping_result = client.ping().await?;
    println!("  Server alive: {}", ping_result);
    println!();

    // Disconnect
    println!("🔌 Disconnecting...");
    client.disconnect().await;
    println!("  State: {:?}", client.state().await);
    println!("  Connected: {}", client.is_connected().await);
    println!();

    println!("✅ Advanced features demo completed!");
    Ok(())
}
