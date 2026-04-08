# ACP (Agent Communication Protocol)

ACP 是一个完整的、独立的多智能体通信协议，专为智能体之间的协作和任务委派而设计。

## 核心特性

### 1. 智能体发现与能力协商
- 智能体通过能力声明（工具、领域、语言）注册到注册中心
- 支持基于能力的查询和自动路由
- 性能和成本信息透明化

### 2. 任务委派与编排
- 智能体可以将子任务委派给专门的智能体
- 支持嵌套委派和复杂的多智能体协作
- 委派链全程可观测

### 3. 流式双向通信
- 基于 SSE (Server-Sent Events) 的流式响应
- 支持思考过程、工具调用、进度报告的实时流式传输
- 低延迟、高吞吐

### 4. 状态同步与上下文共享
- 智能体之间可以共享状态和上下文
- 支持版本控制和冲突解决
- 会话持久化

## 协议架构

```
┌─────────────────────────────────────────────────────────────┐
│                         ACP 协议层                           │
├─────────────────────────────────────────────────────────────┤
│  AcpRequest  →  Agent Router  →  Agent Registry             │
│       ↓              ↓                  ↓                    │
│  AcpEvent ←  Agent Execution ← Capability Matching          │
└─────────────────────────────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────────────┐
│                      传输层 (HTTP/SSE)                       │
└─────────────────────────────────────────────────────────────┘
```

## 核心类型

### AcpRequest - 请求

```rust
pub struct AcpRequest {
    pub session_id: Option<SessionId>,      // 会话标识
    pub content: AcpContent,                // 用户消息（文本或多模态）
    pub target_agent: Option<AgentId>,      // 目标智能体（可选）
    pub routing: Option<RoutingHints>,      // 路由提示
    pub history: Vec<AcpMessage>,           // 对话历史
    pub constraints: Option<ExecutionConstraints>, // 执行约束
    pub metadata: HashMap<String, Value>,   // 元数据
}
```

### AcpEvent - 流式响应事件

```rust
pub enum AcpEvent {
    AgentStart { ... },           // 智能体开始执行
    ContentDelta { ... },         // 内容流式输出
    ThinkingStart/Delta/End,      // 思考过程
    ToolCallStart { ... },        // 工具调用开始
    ToolProgress { ... },         // 工具执行进度
    ToolResult { ... },           // 工具执行结果
    DelegateStart { ... },        // 委派开始
    DelegateEvent { ... },        // 委派事件（嵌套）
    DelegateEnd { ... },          // 委派结束
    Usage { ... },                // 使用统计
    AgentEnd { ... },             // 智能体执行结束
    Error { ... },                // 错误
    Trace { ... },                // 追踪事件
}
```

### AgentCapabilities - 智能体能力

```rust
pub struct AgentCapabilities {
    pub agent_id: AgentId,               // 唯一标识
    pub name: String,                    // 名称
    pub description: String,             // 描述
    pub version: String,                 // 版本
    pub tools: Vec<AcpToolDefinition>,   // 可用工具
    pub domains: Vec<String>,            // 专业领域
    pub languages: Vec<String>,          // 支持的语言
    pub performance: Option<AgentPerformance>, // 性能指标
    pub cost: Option<AgentCost>,         // 成本信息
    pub metadata: HashMap<String, Value>,
}
```

## 使用示例

### 1. 创建专门的智能体

```rust
use remi_agentloop_transport::acp::*;

struct SearchAgent {
    agent_id: AgentId,
}

impl AcpAgent for SearchAgent {
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            agent_id: AgentId::new("search_agent"),
            name: "Search Agent".into(),
            description: "Web search specialist".into(),
            version: "1.0.0".into(),
            tools: vec![
                AcpToolDefinition {
                    name: "web_search".into(),
                    description: "Search the web".into(),
                    parameters: vec![...],
                    metadata: HashMap::new(),
                },
            ],
            domains: vec!["search".into(), "web".into()],
            languages: vec!["en".into(), "zh".into()],
            performance: Some(...),
            cost: Some(...),
            metadata: HashMap::new(),
        }
    }

    fn execute(&self, request: AcpRequest) 
        -> Pin<Box<dyn Future<Output = Result<...>>>>
    {
        Box::pin(async move {
            // 实现智能体逻辑
            Ok(Box::pin(stream! {
                yield AcpEvent::AgentStart { ... };
                yield AcpEvent::ContentDelta { delta: "Searching...".into() };
                yield AcpEvent::AgentEnd { ... };
            }))
        })
    }
}
```

### 2. 注册智能体并创建路由

```rust
// 创建注册中心
let registry = AgentRegistry::new();

// 创建并注册智能体
let search_agent = Box::new(SearchAgent::new());
let code_agent = Box::new(CodeAgent::new());

let router = AcpRouter::new(registry.clone())
    .register_agent(search_agent)
    .register_agent(code_agent);
```

### 3. 执行请求（自动路由）

```rust
// 让路由器自动选择合适的智能体
let request = AcpRequest {
    session_id: None,
    content: AcpContent::text("search for rust tutorials"),
    target_agent: None,  // 路由器会自动选择
    routing: Some(RoutingHints {
        domains: vec!["search".into()],
        ..Default::default()
    }),
    ..Default::default()
};

let mut stream = router.execute(request).await?;
while let Some(event) = stream.next().await {
    match event {
        AcpEvent::ContentDelta { delta, .. } => print!("{}", delta),
        AcpEvent::AgentEnd { .. } => break,
        _ => {}
    }
}
```

### 4. HTTP 服务器

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = AgentRegistry::new();
    let router = AcpRouter::new(registry)
        .register_agent(Box::new(MathAgent::new()))
        .register_agent(Box::new(SearchAgent::new()));

    let server = AcpServer::new(router)
        .bind(([0, 0, 0, 0], 8080));

    server.serve().await?;
    Ok(())
}
```

### 5. HTTP 客户端

```rust
let client = AcpClient::new("http://localhost:8080/acp");

// 发现智能体
let agents = client.discover(AgentQueryRequest {
    domains: vec!["math".into()],
    ..Default::default()
}).await?;

// 执行任务
let request = AcpRequest {
    content: AcpContent::text("calculate 2 + 3"),
    target_agent: Some(AgentId::new("math_agent")),
    ..Default::default()
};

let mut stream = client.execute(request).await?;
while let Some(event) = stream.next().await {
    // 处理事件
}
```

## API 端点

### POST /acp
执行智能体请求，返回 SSE 流式响应。

**请求体:**
```json
{
  "content": "search for AI papers",
  "target_agent": "search_agent",
  "routing": {
    "domains": ["search"],
    "language": "en"
  },
  "constraints": {
    "timeout_secs": 30,
    "max_cost": 0.1
  }
}
```

**响应 (SSE):**
```
event: agent_start
data: {"type":"agent_start","session_id":"...","task_id":"...","agent_id":"search_agent","agent_name":"Search Agent"}

event: content_delta
data: {"type":"content_delta","task_id":"...","delta":"Searching..."}

event: agent_end
data: {"type":"agent_end","task_id":"...","status":"success","result":{...}}
```

### POST /acp/discover
发现可用的智能体。

**请求体:**
```json
{
  "domains": ["code"],
  "required_tools": ["analyze_code"],
  "language": "en"
}
```

**响应:**
```json
[
  {
    "agent_id": "code_agent",
    "name": "Code Agent",
    "description": "Code analysis specialist",
    "version": "1.0.0",
    "tools": [...],
    "domains": ["code", "programming"],
    "performance": {...},
    "cost": {...}
  }
]
```

## 协议流程

### 简单任务执行

```
Client              Router              Agent
  |                   |                   |
  |-- AcpRequest ---->|                   |
  |                   |-- select agent -->|
  |                   |-- execute ------->|
  |<-- AgentStart ----|<-- AgentStart ----|
  |<-- ContentDelta --|<-- ContentDelta --|
  |<-- AgentEnd ------|<-- AgentEnd ------|
```

### 带委派的复杂任务

```
Client          Orchestrator      Search Agent      Code Agent
  |                 |                  |                |
  |-- Request ----->|                  |                |
  |<-- AgentStart --|                  |                |
  |<-- DelegateStart (to search) ---->|                |
  |<---- DelegateEvent (nested) ------|                |
  |<-- DelegateEnd --|<-- result ------|                |
  |<-- DelegateStart (to code) --------|--------------->|
  |<---- DelegateEvent (nested) -------|----------------|
  |<-- DelegateEnd --|<-- result -------|----------------|
  |<-- AgentEnd -----|                  |                |
```

## 智能体委派示例

智能体内部可以委派给其他智能体：

```rust
impl AcpAgent for OrchestratorAgent {
    fn execute(&self, request: AcpRequest) -> ... {
        Box::pin(async move {
            Ok(Box::pin(stream! {
                yield AcpEvent::AgentStart { ... };
                
                // 委派给搜索智能体
                let delegation_id = DelegationId::new();
                yield AcpEvent::DelegateStart {
                    delegation_id: delegation_id.clone(),
                    target_agent_id: AgentId::new("search_agent"),
                    task_description: "find information".into(),
                    ...
                };
                
                // 执行委派
                let delegate_req = AcpRequest {
                    content: AcpContent::text("search query"),
                    target_agent: Some(AgentId::new("search_agent")),
                    ...
                };
                
                let mut delegate_stream = self.router.execute(delegate_req).await?;
                while let Some(event) = delegate_stream.next().await {
                    // 转发委派事件
                    yield AcpEvent::DelegateEvent {
                        delegation_id: delegation_id.clone(),
                        event: Box::new(event),
                    };
                }
                
                yield AcpEvent::DelegateEnd { ... };
                yield AcpEvent::AgentEnd { ... };
            }))
        })
    }
}
```

## 运行示例

### 1. 多智能体本地示例

```bash
cd examples/acp-multi-agent
cargo run
```

这会演示：
- 直接智能体调用
- 自动路由
- 多智能体委派
- 智能体发现

### 2. HTTP 服务器

```bash
cd examples/acp-http-server
cargo run --features http-server
```

服务器将在 `http://localhost:8080` 启动。

### 3. HTTP 客户端

在另一个终端：

```bash
cd examples/acp-http-client
cargo run --features http-client
```

或使用 curl：

```bash
# 发现智能体
curl -X POST http://localhost:8080/acp/discover \
  -H "Content-Type: application/json" \
  -d '{}'

# 执行计算任务
curl -X POST http://localhost:8080/acp \
  -H "Content-Type: application/json" \
  -d '{"content":"calculate 10 + 20","target_agent":"math_agent"}'
```

## 协议设计亮点

### 1. 完全类型安全
所有请求和响应都是强类型的 Rust 结构体，编译时保证正确性。

### 2. 零拷贝流式传输
使用 `async-stream` 和 `futures::Stream`，支持高效的流式处理。

### 3. 传输层无关
核心协议与传输层解耦，可以在 HTTP、WebSocket、WASM、IPC 等多种传输方式上运行。

### 4. 可组合性
ACP 智能体可以嵌套和组合，支持任意复杂的多智能体拓扑。

### 5. 可观测性
内置追踪事件（`AcpEvent::Trace`），支持完整的调用链追踪。

## 扩展性

### 自定义智能体类型

实现 `AcpAgent` trait：

```rust
impl AcpAgent for YourAgent {
    fn capabilities(&self) -> AgentCapabilities { ... }
    fn execute(&self, request: AcpRequest) -> ... { ... }
}
```

### 自定义路由策略

扩展 `AgentRegistry::select_agent()` 方法，实现基于 LLM 的智能路由。

### 自定义传输层

ACP 协议是传输无关的，可以轻松适配到：
- WebSocket (双向流)
- gRPC (高性能 RPC)
- WASM 组件模型
- 进程间通信 (IPC)

## 与标准协议的区别

| 特性 | 标准协议 (ProtocolEvent) | ACP |
|------|-------------------------|-----|
| 目标 | LLM 聊天服务 | 多智能体协作 |
| 委派 | ❌ | ✅ 原生支持 |
| 能力发现 | ❌ | ✅ 内置注册中心 |
| 路由 | 人工指定 | 自动选择 |
| 嵌套流 | ❌ | ✅ DelegateEvent |
| 成本追踪 | 基础 | 详细（每智能体） |
| 状态同步 | ❌ | ✅ StateSync 事件 |

## 应用场景

1. **微服务智能体架构**: 每个智能体是独立的微服务，通过 ACP 协作
2. **专家系统**: 编排多个专家智能体完成复杂任务
3. **分布式推理**: 在多个节点上分配推理任务
4. **智能体市场**: 智能体可以发现和购买其他智能体的服务

## 未来增强

- [ ] 智能体认证与授权
- [ ] 速率限制和配额管理
- [ ] 智能体健康检查和故障转移
- [ ] 负载均衡和自动扩展
- [ ] 基于 LLM 的智能路由决策
- [ ] 委派链优化和缓存
- [ ] 多租户支持
- [ ] 智能体版本管理和滚动更新
