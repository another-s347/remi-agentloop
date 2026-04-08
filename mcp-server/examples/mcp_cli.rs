//! Interactive MCP CLI Client
//!
//! A command-line tool for interacting with MCP servers.
//!
//! Usage:
//! ```bash
//! cargo run --example mcp_cli --features client -- http://localhost:3000
//! ```

use mcp_server::*;
use std::io::{self, Write};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    
    let endpoint = if args.len() > 1 {
        args[1].clone()
    } else {
        println!("Usage: {} <endpoint>", args[0]);
        println!("Example: {} http://localhost:3000", args[0]);
        std::process::exit(1);
    };

    println!("╔══════════════════════════════════════════╗");
    println!("║     MCP Interactive CLI Client v1.0     ║");
    println!("╚══════════════════════════════════════════╝\n");

    let client = McpClient::new(&endpoint);
    println!("🔗 Connecting to {}", endpoint);

    // Initialize
    match client.initialize("MCP CLI", "1.0.0").await {
        Ok(info) => {
            println!("✅ Connected to {} v{}", 
                info.server_info.name, 
                info.server_info.version
            );
            println!("   Protocol: {}\n", info.protocol_version);
        }
        Err(e) => {
            eprintln!("❌ Connection failed: {}", e);
            std::process::exit(1);
        }
    }

    // Main loop
    loop {
        print!("mcp> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        let parts: Vec<&str> = input.split_whitespace().collect();
        let command = parts[0];

        match command {
            "help" | "?" => {
                print_help();
            }

            "status" => {
                let health = client.health_check().await;
                println!("📊 Status:");
                println!("  State: {:?}", health.state);
                println!("  Server reachable: {}", health.server_reachable);
                if let Some(info) = health.server_info {
                    println!("  Server: {} v{}", info.name, info.version);
                }
            }

            "stats" => {
                let stats = client.stats().await;
                println!("📊 Statistics:");
                println!("  Requests: {} sent, {} received", 
                    stats.requests_sent, stats.responses_received);
                println!("  Errors: {}", stats.errors);
                println!("  Tools called: {}", stats.tools_called);
                println!("  Resources read: {}", stats.resources_read);
                println!("  Prompts fetched: {}", stats.prompts_fetched);
            }

            "capabilities" => {
                match client.get_capabilities_summary().await {
                    Ok(summary) => {
                        println!("🎯 Server Capabilities:");
                        println!("  Server: {} v{}", summary.server_name, summary.server_version);
                        println!("  Protocol: {}", summary.protocol_version);
                        println!("  Tools: {}", summary.tools_count);
                        println!("  Resources: {}", summary.resources_count);
                        println!("  Prompts: {}", summary.prompts_count);
                        println!("  Logging: {}", if summary.has_logging { "✓" } else { "✗" });
                    }
                    Err(e) => eprintln!("❌ Error: {}", e),
                }
            }

            "tools" | "list-tools" => {
                match client.list_tools().await {
                    Ok(result) => {
                        println!("🔧 Available tools:");
                        for tool in result.tools {
                            println!("  • {} - {}", 
                                tool.name,
                                tool.description.as_deref().unwrap_or("No description")
                            );
                        }
                    }
                    Err(e) => eprintln!("❌ Error: {}", e),
                }
            }

            "resources" | "list-resources" => {
                match client.list_resources().await {
                    Ok(result) => {
                        println!("📦 Available resources:");
                        for resource in result.resources {
                            println!("  • {} ({}) - {}", 
                                resource.name,
                                resource.uri,
                                resource.description.as_deref().unwrap_or("No description")
                            );
                        }
                    }
                    Err(e) => eprintln!("❌ Error: {}", e),
                }
            }

            "prompts" | "list-prompts" => {
                match client.list_prompts().await {
                    Ok(result) => {
                        println!("💬 Available prompts:");
                        for prompt in result.prompts {
                            println!("  • {} - {}", 
                                prompt.name,
                                prompt.description.as_deref().unwrap_or("No description")
                            );
                            if let Some(args) = prompt.arguments {
                                for arg in args {
                                    let req = if arg.required.unwrap_or(false) { "*" } else { "" };
                                    println!("    - {}{}: {}", 
                                        arg.name,
                                        req,
                                        arg.description.as_deref().unwrap_or("")
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => eprintln!("❌ Error: {}", e),
                }
            }

            "schema" => {
                if parts.len() < 2 {
                    eprintln!("Usage: schema <tool_name>");
                    continue;
                }
                let tool_name = parts[1];
                match client.get_tool_schema(tool_name).await {
                    Ok(schema) => {
                        println!("📋 Schema for '{}':", tool_name);
                        println!("{}", serde_json::to_string_pretty(&schema)?);
                    }
                    Err(e) => eprintln!("❌ Error: {}", e),
                }
            }

            "call" => {
                if parts.len() < 2 {
                    eprintln!("Usage: call <tool_name> [json_args]");
                    continue;
                }
                let tool_name = parts[1];
                let args = if parts.len() > 2 {
                    let json_str = parts[2..].join(" ");
                    match serde_json::from_str(&json_str) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            eprintln!("❌ Invalid JSON: {}", e);
                            continue;
                        }
                    }
                } else {
                    None
                };

                match client.call_tool(tool_name, args).await {
                    Ok(result) => {
                        println!("✅ Result:");
                        for content in result.content {
                            match content {
                                ToolContent::Text { text } => println!("  {}", text),
                                ToolContent::Image { mime_type, .. } => {
                                    println!("  [Image: {}]", mime_type);
                                }
                                ToolContent::Resource { uri, text, .. } => {
                                    println!("  [Resource: {}]", uri);
                                    println!("  {}", text);
                                }
                            }
                        }
                    }
                    Err(e) => eprintln!("❌ Error: {}", e),
                }
            }

            "read" => {
                if parts.len() < 2 {
                    eprintln!("Usage: read <resource_uri>");
                    continue;
                }
                let uri = parts[1];
                match client.read_resource(uri).await {
                    Ok(result) => {
                        println!("📄 Resource content:");
                        for content in result.contents {
                            match content {
                                ResourceContent::Text { text, .. } => {
                                    println!("─────────────────────────────────");
                                    println!("{}", text);
                                    println!("─────────────────────────────────");
                                }
                                ResourceContent::Blob { mime_type, .. } => {
                                    println!("  [Binary data: {}]", mime_type.unwrap_or_default());
                                }
                            }
                        }
                    }
                    Err(e) => eprintln!("❌ Error: {}", e),
                }
            }

            "prompt" => {
                if parts.len() < 2 {
                    eprintln!("Usage: prompt <name> [key=value ...]");
                    continue;
                }
                let name = parts[1];
                let mut args = HashMap::new();
                for i in 2..parts.len() {
                    if let Some((k, v)) = parts[i].split_once('=') {
                        args.insert(k.to_string(), v.to_string());
                    }
                }

                match client.get_prompt(name, Some(args)).await {
                    Ok(result) => {
                        println!("💬 Prompt:");
                        for msg in result.messages {
                            match msg.content {
                                MessageContent::Text { text } => {
                                    println!("  [{:?}]: {}", msg.role, text);
                                }
                                MessageContent::Image { mime_type, .. } => {
                                    println!("  [{:?}]: [Image: {}]", msg.role, mime_type);
                                }
                            }
                        }
                    }
                    Err(e) => eprintln!("❌ Error: {}", e),
                }
            }

            "search" => {
                if parts.len() < 2 {
                    eprintln!("Usage: search <pattern>");
                    continue;
                }
                let pattern = parts[1..].join(" ");
                
                match client.search_tools(&pattern).await {
                    Ok(tools) => {
                        if !tools.is_empty() {
                            println!("🔧 Tools matching '{}':", pattern);
                            for tool in tools {
                                println!("  • {}", tool.name);
                            }
                        }
                    }
                    Err(e) => eprintln!("❌ Error searching tools: {}", e),
                }

                match client.search_resources(&pattern).await {
                    Ok(resources) => {
                        if !resources.is_empty() {
                            println!("📦 Resources matching '{}':", pattern);
                            for res in resources {
                                println!("  • {}", res.name);
                            }
                        }
                    }
                    Err(e) => eprintln!("❌ Error searching resources: {}", e),
                }
            }

            "export" => {
                match client.export_schemas_json().await {
                    Ok(json) => {
                        println!("{}", json);
                    }
                    Err(e) => eprintln!("❌ Error: {}", e),
                }
            }

            "ping" => {
                match client.ping().await {
                    Ok(true) => println!("🏓 Pong! Server is alive"),
                    Ok(false) => println!("❌ Server not responding"),
                    Err(e) => eprintln!("❌ Error: {}", e),
                }
            }

            "clear" | "clear-cache" => {
                client.clear_cache().await;
                println!("🗑️  Cache cleared");
            }

            "exit" | "quit" | "q" => {
                println!("👋 Goodbye!");
                client.disconnect().await;
                break;
            }

            _ => {
                eprintln!("❌ Unknown command: {}", command);
                println!("   Type 'help' for available commands");
            }
        }

        println!();
    }

    Ok(())
}

fn print_help() {
    println!("📖 Available commands:\n");
    println!("  Connection:");
    println!("    status              - Show connection status");
    println!("    ping                - Ping server");
    println!("    capabilities        - Show server capabilities");
    println!();
    println!("  Discovery:");
    println!("    tools               - List all tools");
    println!("    resources           - List all resources");
    println!("    prompts             - List all prompts");
    println!("    search <pattern>    - Search tools/resources/prompts");
    println!();
    println!("  Operations:");
    println!("    call <tool> [args]  - Call a tool (args as JSON)");
    println!("    read <uri>          - Read a resource");
    println!("    prompt <name> [k=v] - Get a prompt with arguments");
    println!();
    println!("  Introspection:");
    println!("    schema <tool>       - Show tool schema");
    println!("    export              - Export all schemas as JSON");
    println!();
    println!("  Management:");
    println!("    stats               - Show client statistics");
    println!("    clear               - Clear cache");
    println!("    help, ?             - Show this help");
    println!("    exit, quit, q       - Exit the CLI");
    println!();
    println!("  Examples:");
    println!("    call calculator {{\"operation\":\"add\",\"a\":10,\"b\":20}}");
    println!("    read resource://readme");
    println!("    prompt greeting name=Alice");
    println!("    search calc");
}
