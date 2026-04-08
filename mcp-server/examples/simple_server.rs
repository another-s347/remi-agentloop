//! Simple MCP Server Example
//!
//! Demonstrates a basic MCP server with tools and resources.
//!
//! Run with:
//! ```bash
//! cargo run --example simple_server --features server
//! ```
//!
//! Test with:
//! ```bash
//! curl -X POST http://localhost:3000 \
//!   -H "Content-Type: application/json" \
//!   -d '{
//!     "jsonrpc": "2.0",
//!     "id": "1",
//!     "method": "initialize",
//!     "params": {
//!       "protocolVersion": "2024-11-05",
//!       "capabilities": {},
//!       "clientInfo": {"name": "Test", "version": "1.0"}
//!     }
//!   }'
//! ```

use mcp_server::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Starting MCP Server Example\n");

    let server = McpServer::new("Simple MCP Server", "1.0.0")
        // Calculator tool
        .with_tool(
            Tool {
                name: "calculator".into(),
                description: Some("Perform basic arithmetic calculations".into()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["add", "subtract", "multiply", "divide"],
                            "description": "The operation to perform"
                        },
                        "a": {
                            "type": "number",
                            "description": "First operand"
                        },
                        "b": {
                            "type": "number",
                            "description": "Second operand"
                        }
                    },
                    "required": ["operation", "a", "b"]
                }),
            },
            |args| async move {
                let op = args.get("operation")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpError::InvalidParams("Missing operation".into()))?;
                
                let a = args.get("a")
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| McpError::InvalidParams("Missing a".into()))?;
                
                let b = args.get("b")
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| McpError::InvalidParams("Missing b".into()))?;

                let result = match op {
                    "add" => a + b,
                    "subtract" => a - b,
                    "multiply" => a * b,
                    "divide" => {
                        if b == 0.0 {
                            return Ok(CallToolResult {
                                content: vec![ToolContent::error("Division by zero")],
                                is_error: Some(true),
                            });
                        }
                        a / b
                    }
                    _ => {
                        return Ok(CallToolResult {
                            content: vec![ToolContent::error(format!("Unknown operation: {}", op))],
                            is_error: Some(true),
                        });
                    }
                };

                Ok(CallToolResult {
                    content: vec![ToolContent::text(format!("{} {} {} = {}", a, op, b, result))],
                    is_error: None,
                })
            },
        )
        // Echo tool
        .with_tool(
            Tool {
                name: "echo".into(),
                description: Some("Echo back the input".into()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": {
                            "type": "string",
                            "description": "Message to echo"
                        }
                    },
                    "required": ["message"]
                }),
            },
            |args| async move {
                let message = args.get("message")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpError::InvalidParams("Missing message".into()))?;

                Ok(CallToolResult {
                    content: vec![ToolContent::text(format!("Echo: {}", message))],
                    is_error: None,
                })
            },
        )
        // Static resource
        .with_resource(
            Resource {
                uri: "resource://readme".into(),
                name: "README".into(),
                description: Some("Server README file".into()),
                mime_type: Some("text/plain".into()),
            },
            |_uri| async move {
                Ok(ReadResourceResult {
                    contents: vec![ResourceContent::Text {
                        uri: "resource://readme".into(),
                        mime_type: Some("text/plain".into()),
                        text: "This is the MCP server README.\nIt provides tools and resources.".into(),
                    }],
                })
            },
        )
        // Simple prompt
        .with_prompt(
            Prompt {
                name: "greeting".into(),
                description: Some("Generate a greeting".into()),
                arguments: Some(vec![PromptArgument {
                    name: "name".into(),
                    description: Some("Name to greet".into()),
                    required: Some(true),
                }]),
            },
            |args| async move {
                let name = args.get("name").cloned().unwrap_or_else(|| "World".into());
                Ok(GetPromptResult {
                    messages: vec![PromptMessage {
                        role: MessageRole::User,
                        content: MessageContent::Text {
                            text: format!("Say hello to {}", name),
                        },
                    }],
                    description: Some(format!("Greeting prompt for {}", name)),
                })
            },
        );

    println!("📋 Server Capabilities:");
    println!("  ✓ Tools: calculator, echo");
    println!("  ✓ Resources: resource://readme");
    println!("  ✓ Prompts: greeting");
    println!();
    
    println!("📡 Endpoints:");
    println!("  POST http://localhost:3000/");
    println!();
    
    println!("📝 Example requests:");
    println!(r#"  # Initialize"#);
    println!(r#"  curl -X POST http://localhost:3000 -H "Content-Type: application/json" -d '{{"jsonrpc":"2.0","id":"1","method":"initialize","params":{{"protocolVersion":"2024-11-05","capabilities":{{}},"clientInfo":{{"name":"Test","version":"1.0"}}}}}}'"#);
    println!();
    println!(r#"  # List tools"#);
    println!(r#"  curl -X POST http://localhost:3000 -H "Content-Type: application/json" -d '{{"jsonrpc":"2.0","id":"2","method":"tools/list"}}'"#);
    println!();
    println!(r#"  # Call calculator"#);
    println!(r#"  curl -X POST http://localhost:3000 -H "Content-Type: application/json" -d '{{"jsonrpc":"2.0","id":"3","method":"tools/call","params":{{"name":"calculator","arguments":{{"operation":"add","a":10,"b":20}}}}}}'"#);
    println!();

    server.serve(([0, 0, 0, 0], 3000)).await?;
    Ok(())
}
