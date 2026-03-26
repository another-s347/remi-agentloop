# MCP Server - Model Context Protocol Implementation

独立的 MCP (Model Context Protocol) 服务器和客户端实现。

## 特性

- ✅ **完整的 MCP 协议支持** - 实现完整的 MCP 规范
- ✅ **JSON-RPC 2.0 基础** - 标准的请求/响应格式
- ✅ **HTTP/HTTPS 传输** - 基于 axum 和 reqwest
- ✅ **类型安全** - 强类型 Rust API
- ✅ **完全独立** - 零依赖于 remi-agentloop 框架

### 客户端增强功能

- ✅ **连接状态管理** - 跟踪连接状态（Disconnected/Connecting/Connected/Error）
- ✅ **智能缓存** - 自动缓存工具/资源/提示列表
- ✅ **Schema 自省** - 查询和导出完整的服务器 schema
- ✅ **健康检查** - Ping 和详细的健康状态
- ✅ **批量操作** - 批量调用工具或读取资源
- ✅ **动态发现** - 搜索和过滤工具/资源/提示
- ✅ **统计跟踪** - 请求计数、错误率、使用统计
- ✅ **交互式 CLI** - 完整的命令行工具

## 安装

将以下内容添加到您的 `Cargo.toml`:

```toml
[dependencies]
mcp-server = { path = "../mcp-server", features = ["server"] }
# 或者只需要客户端
mcp-server = { path = "../mcp-server", features = ["client"] }
```

## 快速开始

### 创建服务器

```rust
use mcp_server::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = McpServer::new("My Server", "1.0.0")
        .with_tool(
            Tool {
                name: "calculator".into(),
                description: Some("Perform calculations".into()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "expression": { "type": "string" }
                    }
                }),
            },
            |args| async move {
                // 工具实现
                Ok(CallToolResult {
                    content: vec![ToolContent::text("42")],
                    is_error: None,
                })
            },
        )
        .with_resource(
            Resource {
                uri: "file:///data.txt".into(),
                name: "Data File".into(),
                description: Some("Sample data".into()),
                mime_type: Some("text/plain".into()),
            },
            |_uri| async move {
                Ok(ReadResourceResult {
                    contents: vec![ResourceContent::Text {
                        uri: "file:///data.txt".into(),
                        mime_type: Some("text/plain".into()),
                        text: "Sample data content".into(),
                    }],
                })
            },
        );

    server.serve(([0, 0, 0, 0], 3000)).await?;
    Ok(())
}
```

### 使用客户端

```rust
use mcp_server::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = McpClient::new("http://localhost:3000");
    
    // 初始化连接
    client.initialize("My Client", "1.0.0").await?;
    
    // 检查连接状态
    if client.is_connected().await {
        println!("Connected!");
    }
    
    // 列出工具（自动缓存）
    let tools = client.list_tools().await?;
    for tool in tools.tools {
        println!("Tool: {}", tool.name);
    }
    
    // 获取工具 schema
    let schema = client.get_tool_schema("calculator").await?;
    println!("Schema: {}", serde_json::to_string_pretty(&schema)?);
    
    // 调用工具
    let result = client.call_tool(
        "calculator",
        Some(serde_json::json!({
            "operation": "add",
            "a": 10,
            "b": 20
        })),
    ).await?;
    
    // 搜索工具
    let found = client.search_tools("calc").await?;
    println!("Found {} tools", found.len());
    
    // 获取服务器能力摘要
    let summary = client.get_capabilities_summary().await?;
    println!("Server: {} with {} tools", 
        summary.server_name, summary.tools_count);
    
    // 健康检查
    let health = client.health_check().await;
    println!("Server reachable: {}", health.server_reachable);
    
    // 查看统计
    let stats = client.stats().await;
    println!("Requests sent: {}", stats.requests_sent);
    
    Ok(())
}
```

### 使用交互式 CLI

```bash
# 启动 CLI
cargo run --example mcp_cli --features client -- http://localhost:3000

# 在 CLI 中使用命令
mcp> help                     # 显示帮助
mcp> tools                    # 列出工具
mcp> schema calculator        # 查看工具 schema
mcp> call calculator {"operation":"add","a":10,"b":20}
mcp> search calc              # 搜索工具
mcp> stats                    # 查看统计
mcp> export                   # 导出所有 schema
mcp> exit                     # 退出
```

## 运行示例

### 启动服务器

```bash
cargo run --example simple_server --features server
```

服务器将监听 `http://localhost:3000`

### 运行基础客户端

在另一个终端：

```bash
cargo run --example simple_client --features client
```

### 运行高级客户端（演示所有功能）

```bash
cargo run --example advanced_client --features client
```

### 使用交互式 CLI

```bash
cargo run --example mcp_cli --features client -- http://localhost:3000
```

CLI 命令示例：
- `help` - 显示所有命令
- `tools` - 列出所有工具
- `schema calculator` - 查看工具 schema
- `call calculator {"operation":"add","a":10,"b":20}` - 调用工具
- `search calc` - 搜索工具
- `export` - 导出所有 schemas
- `stats` - 查看客户端统计
- `ping` - 测试服务器连接

### 使用 curl 测试

```bash
# 初始化
curl -X POST http://localhost:3000 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": "1",
    "method": "initialize",
    "params": {
      "protocolVersion": "2024-11-05",
      "capabilities": {},
      "clientInfo": {"name": "Test", "version": "1.0"}
    }
  }'

# 列出工具
curl -X POST http://localhost:3000 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":"2","method":"tools/list"}'

# 调用计算器
curl -X POST http://localhost:3000 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": "3",
    "method": "tools/call",
    "params": {
      "name": "calculator",
      "arguments": {"operation": "add", "a": 10, "b": 20}
    }
  }'
```

## MCP 协议概述

### 支持的功能

1. **Tools (工具)** - 可执行的函数
   - `tools/list` - 列出所有工具
   - `tools/call` - 调用工具

2. **Resources (资源)** - 数据源
   - `resources/list` - 列出所有资源
   - `resources/read` - 读取资源内容

3. **Prompts (提示模板)** - 可重用的提示
   - `prompts/list` - 列出所有提示
   - `prompts/get` - 获取提示内容

4. **Logging (日志)** - 结构化日志记录

### 协议流程

```
Client                    Server
  |                         |
  |-- initialize ---------->|
  |<-- capabilities --------|
  |                         |
  |-- tools/list ---------->|
  |<-- tools ---------------|
  |                         |
  |-- tools/call ---------->|
  |<-- result --------------|
```

## API 参考

### 服务器 API

#### McpServer

```rust
impl McpServer {
    // 创建新服务器
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self;
    
    // 注册工具
    pub fn with_tool<F>(self, tool: Tool, handler: F) -> Self
    where F: Fn(serde_json::Value) -> Future<Result<CallToolResult>>;
    
    // 注册资源
    pub fn with_resource<F>(self, resource: Resource, handler: F) -> Self
    where F: Fn(String) -> Future<Result<ReadResourceResult>>;
    
    // 注册提示
    pub fn with_prompt<F>(self, prompt: Prompt, handler: F) -> Self
    where F: Fn(HashMap<String, String>) -> Future<Result<GetPromptResult>>;
    
    // 启动 HTTP 服务器
    pub async fn serve(self, addr: impl Into<SocketAddr>) -> Result<()>;
}
```

### 客户端 API

#### McpClient

```rust
impl McpClient {
    // 连接管理
    pub fn new(endpoint: impl Into<String>) -> Self;
    pub async fn initialize(&self, name: impl Into<String>, version: impl Into<String>) 
        -> Result<InitializeResult>;
    pub async fn disconnect(&self);
    pub async fn state(&self) -> ConnectionState;
    pub async fn is_connected(&self) -> bool;
    
    // 服务器信息
    pub async fn server_info(&self) -> Option<Implementation>;
    pub async fn server_capabilities(&self) -> Option<ServerCapabilities>;
    pub async fn get_capabilities_summary(&self) -> Result<CapabilitiesSummary>;
    
    // 工具操作
    pub async fn list_tools(&self) -> Result<ListToolsResult>;
    pub async fn list_tools_fresh(&self) -> Result<ListToolsResult>; // 强制刷新
    pub async fn get_tool(&self, name: &str) -> Result<Option<Tool>>;
    pub async fn has_tool(&self, name: &str) -> Result<bool>;
    pub async fn get_tool_schema(&self, name: &str) -> Result<Value>;
    pub async fn call_tool(&self, name: impl Into<String>, args: Option<Value>) 
        -> Result<CallToolResult>;
    
    // 资源操作
    pub async fn list_resources(&self) -> Result<ListResourcesResult>;
    pub async fn list_resources_fresh(&self) -> Result<ListResourcesResult>;
    pub async fn get_resource(&self, uri: &str) -> Result<Option<Resource>>;
    pub async fn has_resource(&self, uri: &str) -> Result<bool>;
    pub async fn read_resource(&self, uri: impl Into<String>) 
        -> Result<ReadResourceResult>;
    
    // 提示操作
    pub async fn list_prompts(&self) -> Result<ListPromptsResult>;
    pub async fn list_prompts_fresh(&self) -> Result<ListPromptsResult>;
    pub async fn get_prompt_info(&self, name: &str) -> Result<Option<Prompt>>;
    pub async fn has_prompt(&self, name: &str) -> Result<bool>;
    pub async fn get_prompt(&self, name: impl Into<String>, args: Option<HashMap<String, String>>) 
        -> Result<GetPromptResult>;
    
    // Schema 自省
    pub async fn get_all_schemas(&self) -> Result<ServerSchemas>;
    pub async fn export_schemas_json(&self) -> Result<String>;
    
    // 动态发现
    pub async fn search_tools(&self, pattern: &str) -> Result<Vec<Tool>>;
    pub async fn search_resources(&self, pattern: &str) -> Result<Vec<Resource>>;
    pub async fn search_prompts(&self, pattern: &str) -> Result<Vec<Prompt>>;
    pub async fn get_tools_by_prefix(&self, prefix: &str) -> Result<Vec<Tool>>;
    
    // 批量操作
    pub async fn call_tools_batch(&self, calls: Vec<(String, Option<Value>)>) 
        -> Vec<Result<CallToolResult>>;
    pub async fn read_resources_batch(&self, uris: Vec<String>) 
        -> Vec<Result<ReadResourceResult>>;
    
    // 缓存管理
    pub async fn clear_cache(&self);
    pub async fn clear_tools_cache(&self);
    pub async fn clear_resources_cache(&self);
    pub async fn clear_prompts_cache(&self);
    
    // 健康和统计
    pub async fn ping(&self) -> Result<bool>;
    pub async fn health_check(&self) -> HealthCheckResult;
    pub async fn stats(&self) -> ClientStats;
    pub async fn reset_stats(&self);
}
```

## 架构

```
mcp-server/
├── src/
│   ├── lib.rs          - 库入口
│   ├── protocol.rs     - MCP 协议类型定义
│   ├── server.rs       - 服务器实现
│   └── client.rs       - 客户端实现
└── examples/
    ├── simple_server.rs - 服务器示例
    └── simple_client.rs - 客户端示例
```

## 与 remi-agentloop 的关系

这是一个**完全独立**的 crate，不依赖 remi-agentloop 的任何组件。它可以：

1. **独立使用** - 作为标准的 MCP 服务器/客户端
2. **集成使用** - 可以作为 remi-agentloop 的 MCP 适配器
3. **学习参考** - 作为 MCP 协议的参考实现

## 协议规范

基于 MCP 规范版本: `2024-11-05`

官方文档: https://modelcontextprotocol.io/

## License

与 remi-agentloop 项目保持一致。

## 示例输出

### 服务器

```
🚀 Starting MCP Server Example

📋 Server Capabilities:
  ✓ Tools: calculator, echo
  ✓ Resources: resource://readme
  ✓ Prompts: greeting

📡 Endpoints:
  POST http://localhost:3000/

🚀 MCP Server listening on http://0.0.0.0:3000
```

### 基础客户端

```
🔌 MCP Client Example

📡 Initializing connection...
✅ Connected to: Simple MCP Server v1.0.0
   Protocol: 2024-11-05

🔧 Available tools:
  • echo - Echo back the input
  • calculator - Perform basic arithmetic calculations

🧮 Calling calculator tool (10 + 20)...
   Result: 10 add 20 = 30

✅ All tests completed successfully!
```

### 高级客户端

```
🔌 Advanced MCP Client Example

📊 Initial state:
  Connection: Disconnected
  Connected: false

📡 Initializing connection...
✅ Connected to: Simple MCP Server v1.0.0
   Protocol: 2024-11-05
   State: Connected

🎯 Server Capabilities Summary:
  Server: Simple MCP Server v1.0.0
  Protocol: 2024-11-05
  Tools: 2 - ["echo", "calculator"]
  Resources: 1 - ["resource://readme"]
  Prompts: 1 - ["greeting"]
  Logging: ✓

🔍 Tool Schema Introspection:
  calculator schema:
{
  "properties": {
    "operation": { "type": "string", "enum": ["add", "subtract", "multiply", "divide"] },
    "a": { "type": "number" },
    "b": { "type": "number" }
  },
  "required": ["operation", "a", "b"]
}

🔧 Batch tool calls:
  1. 5 add 3 = 8
  2. 4 multiply 7 = 28
  3. Echo: Batch test

📊 Client Statistics:
  Requests sent: 8
  Responses received: 8
  Errors: 0
  Tools called: 3
  Resources read: 1

✅ Advanced features demo completed!
```

### 交互式 CLI

```
╔══════════════════════════════════════════╗
║     MCP Interactive CLI Client v1.0     ║
╚══════════════════════════════════════════╝

🔗 Connecting to http://localhost:3000
✅ Connected to Simple MCP Server v1.0.0
   Protocol: 2024-11-05

mcp> help
📖 Available commands:
  Connection:
    status              - Show connection status
    ping                - Ping server
    capabilities        - Show server capabilities
  
  Discovery:
    tools               - List all tools
    resources           - List all resources
    prompts             - List all prompts
    search <pattern>    - Search tools/resources/prompts
  
  Operations:
    call <tool> [args]  - Call a tool (args as JSON)
    read <uri>          - Read a resource
    prompt <name> [k=v] - Get a prompt with arguments

mcp> call calculator {"operation":"add","a":15,"b":25}
✅ Result:
  15 add 25 = 40

mcp> stats
📊 Statistics:
  Requests: 9 sent, 9 received
  Errors: 0
  Tools called: 2
```

## 测试

运行单元测试：

```bash
cargo test
```

运行特定特性的测试：

```bash
cargo test --features server
cargo test --features client
```

## 未来增强

- [ ] WebSocket 传输支持
- [ ] Stdio 传输支持（用于本地进程间通信）
- [ ] 流式工具响应
- [ ] 批量请求支持
- [ ] 更多内置工具示例
- [ ] 资源订阅通知
- [ ] 工具进度报告
