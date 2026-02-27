# 使用示例 + 类型流动图

> 端到端示例代码、类型推导全景图

## 1. 类型流动全景

### 1.1 直接调用（强类型，零擦除）

```
用户代码                          框架                        底层
────────                          ────                        ────
                                                      OpenAIClient
                                                      impl Agent<
                                                        Req = ChatRequest,
                                                        Resp = ChatResponseChunk,
                                                        Err = AgentError
                                                      >
                                                           │
                                    BuiltAgent<OpenAI>     │
                                    impl Agent<            │
                                      Req = String,     uses model.chat()
                                      Resp = AgentEvent,   │
                                      Err = AgentError     │
                                    >                      │
       │                                                   │
       ├─ .map_response(|e| MyEvent::from(e))
       │   → MapResponse<BuiltAgent<OpenAI>, F>
       │     impl Agent<
       │       Req = String,
       │       Resp = MyEvent,     ← 用户自定义强类型
       │       Err = AgentError
       │     >
       │
user:  let stream = agent.chat("hello").await?;
       while let Some(event) = stream.next().await {
           match event {                ← 编译器知道是 MyEvent
               MyEvent::Text(s) => ...,
               MyEvent::ToolUse(t) => ...,
           }
       }
```

### 1.2 跨进程（HTTP SSE，标准协议）

```
进程 A (Server)                                进程 B (Client)
──────────────                                 ──────────────
BuiltAgent<OpenAI>                             HttpSseClient
  Req=String, Resp=AgentEvent                    Req=ProtocolRequest
       │                                         Resp=ProtocolEvent
       ├─ .into_protocol()                       Err=ProtocolError
       │   map → ProtocolAgent                        │
       ▼                                         ├─ .map_response(|e| MyEvent::from(e))
HttpSseServer                                    │   → 强类型 MyEvent
  POST /chat ◄═══════ HTTP SSE ═══════════════► reqwest
  ProtocolRequest → JSON                        SSE data: {...} → ProtocolEvent
  ProtocolEvent → SSE events                         │
                                                     ▼
                                                用户代码 match MyEvent { ... }
```

### 1.3 跨沙箱（WASM，标准协议）

```
宿主进程                                        WASM 模块 (.wasm)
────────                                        ──────────────────
WasmAgent                                       guest::exports
  Req=ProtocolRequest                             #[no_mangle] fn chat(ptr, len) → handle
  Resp=ProtocolEvent                              #[no_mangle] fn poll_next(handle) → ptr
  Err=ProtocolError                               #[no_mangle] fn close(handle)
       │                                               │
       ├─ wasmi::Instance::call("chat", ...)           ├─ 内部运行任意 Agent impl
       ├─ 循环 call("poll_next", handle)               ├─ 通过标准协议 JSON 通信
       ├─ 读 guest memory → ProtocolEvent JSON         ├─ 可以是 BuiltAgent + tool
       ▼                                               ▼
  impl Agent                                    编译自 remi-agentloop
  可继续 .map_response() 组合                    cargo build --target wasm32-unknown-unknown
```

### 1.4 三层统一

```
                                   ┌─────────────────────────┐
                                   │      Agent Trait         │
                                   │  chat(Req) → Stream<Resp>│
                                   └────────┬────────────────┘
                          ┌─────────────────┼─────────────────┐
                          │                 │                  │
                   ┌──────▼──────┐   ┌──────▼──────┐   ┌──────▼──────┐
                   │  直接调用    │   │  HTTP SSE   │   │  WASM      │
                   │  (in-proc)  │   │  (network)  │   │  (sandbox) │
                   ├─────────────┤   ├─────────────┤   ├────────────┤
                   │ 强类型泛型   │   │ Protocol*   │   │ Protocol*  │
                   │ 零开销       │   │ JSON + SSE  │   │ JSON + FFI │
                   │ 编译期检查   │   │ 运行时检查   │   │ 运行时检查  │
                   └─────────────┘   └─────────────┘   └────────────┘
                                          │                  │
                                          └──────┬───────────┘
                                                 │
                                     ProtocolRequest / ProtocolEvent
                                     (标准协议，JSON 可序列化)
```

---

## 2. 用户端使用示例

### 2.1 最简单的 chat

```rust
use remi_agentloop::prelude::*;

#[tokio::main]
async fn main() -> Result<(), AgentError> {
    let client = OpenAIClient::new("sk-...")
        .with_model("gpt-4o");

    let mut stream = client.chat(ChatRequest::simple("Hello!")).await?;

    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatResponseChunk::Delta { content, .. } => print!("{content}"),
            ChatResponseChunk::Done => println!(),
            _ => {}
        }
    }

    Ok(())
}
```

### 2.2 带工具的 Agent Loop（LangChain 1.0 风格）

```rust
use remi_agentloop::prelude::*;

struct SearchTool;

impl Tool for SearchTool {
    fn name(&self) -> &str { "web_search" }
    fn description(&self) -> &str { "Search the web" }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<String, AgentError> {
        let query = args["query"].as_str().unwrap_or_default();
        Ok(format!("Search results for: {query}"))
    }
}

#[tokio::main]
async fn main() -> Result<(), AgentError> {
    let agent = AgentBuilder::new()
        .model(OpenAIClient::new("sk-...").with_model("gpt-4o"))
        .system("You are a helpful assistant with web search capability.")
        .tool(SearchTool)
        .max_turns(5)
        .build();

    let mut stream = agent.chat("What's the weather in Tokyo?".into()).await?;

    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::TextDelta(text) => print!("{text}"),
            AgentEvent::ToolCallStart { name, .. } => println!("\n[Calling tool: {name}]"),
            AgentEvent::ToolResult { name, result, .. } => {
                println!("[Tool {name} returned: {result}]")
            }
            AgentEvent::TurnStart { turn } => println!("\n--- Turn {turn} ---"),
            AgentEvent::Done => println!("\n[Done]"),
            AgentEvent::Error(e) => eprintln!("Error: {e}"),
            _ => {}
        }
    }

    Ok(())
}
```

### 2.3 组合式使用

```rust
// 自定义事件类型
enum MyEvent {
    Text(String),
    Thinking(String),
    Done,
}

let agent = AgentBuilder::new()
    .model(OpenAIClient::new("sk-..."))
    .system("Think step by step.")
    .build()
    .map_response(|event| match event {
        AgentEvent::TextDelta(s) => MyEvent::Text(s),
        AgentEvent::Done => MyEvent::Done,
        _ => MyEvent::Thinking(format!("{:?}", event)),
    });
// agent: MapResponse<BuiltAgent<OpenAIClient>, F>
// agent 的 Response 类型是 MyEvent

let stream = agent.chat("Solve x^2 = 4".into()).await?;
// stream: impl Stream<Item = MyEvent>  ← 编译器完全知道
```

### 2.4 HTTP SSE 服务端

```rust
use remi_agentloop::prelude::*;
use remi_agentloop::transport::HttpSseServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = AgentBuilder::new()
        .model(OpenAIClient::new("sk-...").with_model("gpt-4o"))
        .system("You are helpful.")
        .tool(SearchTool)
        .build()
        .into_protocol();  // → impl ProtocolAgent

    println!("Listening on http://0.0.0.0:8080/chat");
    HttpSseServer::new(agent)
        .bind(([0, 0, 0, 0], 8080))
        .serve()
        .await?;

    Ok(())
}
```

### 2.5 HTTP SSE 客户端

```rust
use remi_agentloop::prelude::*;
use remi_agentloop::transport::HttpSseClient;

#[tokio::main]
async fn main() -> Result<(), ProtocolError> {
    let remote_agent = HttpSseClient::new("http://localhost:8080/chat")
        .with_bearer_token("my-token");

    let req = ProtocolRequest {
        messages: vec![Message::user("Hello from remote client!")],
        tools: None,
        model: None,
        temperature: None,
        max_tokens: None,
        extra: Default::default(),
    };

    let mut stream = remote_agent.chat(req).await?;
    while let Some(event) = stream.next().await {
        match event {
            ProtocolEvent::Delta { content, .. } => print!("{content}"),
            ProtocolEvent::Done => println!("\n[Done]"),
            _ => {}
        }
    }

    Ok(())
}
```

### 2.6 WASM 模块加载

```rust
use remi_agentloop::prelude::*;
use remi_agentloop::transport::WasmAgent;

fn main() -> Result<(), ProtocolError> {
    let plugin = WasmAgent::from_file("plugins/code_reviewer.wasm")?;

    let reviewer = plugin
        .map_response(|event| match event {
            ProtocolEvent::Delta { content, .. } => ReviewEvent::Comment(content),
            ProtocolEvent::Done => ReviewEvent::Done,
            _ => ReviewEvent::Other,
        });

    let req = ProtocolRequest {
        messages: vec![Message::user("Review this code: fn main() {}")],
        ..Default::default()
    };

    futures::executor::block_on(async {
        let mut stream = reviewer.chat(req).await.unwrap();
        while let Some(event) = stream.next().await {
            match event {
                ReviewEvent::Comment(s) => println!("Review: {s}"),
                ReviewEvent::Done => println!("[Review complete]"),
                _ => {}
            }
        }
    });

    Ok(())
}
```

### 2.7 编写 WASM Guest 模块

```rust
// src/main.rs（编译目标：wasm32-unknown-unknown）
use remi_agentloop::prelude::*;
use remi_agentloop::guest;

struct MyReviewAgent;

impl Agent for MyReviewAgent {
    type Request = ProtocolRequest;
    type Response = ProtocolEvent;
    type Error = ProtocolError;

    fn chat(&self, req: ProtocolRequest)
        -> impl Future<Output = Result<impl Stream<Item = ProtocolEvent>, ProtocolError>>
    {
        async move {
            Ok(stream! {
                yield ProtocolEvent::Delta {
                    content: "Looks good!".into(),
                    role: Some("assistant".into()),
                };
                yield ProtocolEvent::Done;
            })
        }
    }
}

guest::register(MyReviewAgent);
```

编译：
```bash
cargo build --target wasm32-unknown-unknown --no-default-features --features wasm-guest
# 生成 target/wasm32-unknown-unknown/release/my_agent.wasm
```

### 2.8 运行时配置注入（WASM 插件）

```rust
use remi_agentloop::prelude::*;
use remi_agentloop::transport::WasmAgent;

fn main() -> Result<(), ProtocolError> {
    // 配置不编译进 .wasm，由宿主运行时注入
    let config = AgentConfig::new()
        .with_api_key(std::env::var("OPENAI_API_KEY").unwrap())
        .with_model("gpt-4o")
        .with_base_url("https://api.openai.com/v1")
        .with_extra(serde_json::json!({
            "weather_api_key": std::env::var("WEATHER_KEY").unwrap(),
        }));

    let plugin = WasmAgent::from_bytes_with_config(
        include_bytes!("plugins/weather_agent.wasm"),
        config,
    )?;

    // plugin 就是普通 Agent，可继续组合
    let agent = plugin.map_response(|e| match e {
        ProtocolEvent::Delta { content, .. } => MyEvent::Text(content),
        ProtocolEvent::Done => MyEvent::Done,
        _ => MyEvent::Other,
    });

    futures::executor::block_on(async {
        let req = ProtocolRequest::simple("What's the weather in Berlin?");
        let mut stream = agent.chat(req).await.unwrap();
        while let Some(event) = stream.next().await {
            // ...
        }
    });
    Ok(())
}
```

### 2.9 自动中断处理（InterruptRouter）

```rust
use remi_agentloop::prelude::*;
use remi_agentloop::interrupt::{InterruptHandler, InterruptRouter};

/// 低金额支付自动批准
struct AutoApproveSmallPayment;

impl InterruptHandler for AutoApproveSmallPayment {
    fn can_handle(&self, kind: &str) -> bool {
        kind == "payment_approval"
    }
    fn handle(&self, info: &InterruptInfo)
        -> impl Future<Output = Result<serde_json::Value, AgentError>>
    {
        async move {
            let amount = info.data["amount"].as_f64().unwrap_or(f64::MAX);
            if amount < 100.0 {
                Ok(serde_json::json!({ "approved": true, "auto": true }))
            } else {
                Err(AgentError::Model("Requires manual approval".into()))
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), AgentError> {
    let config = AgentConfig::from_env();
    let router = InterruptRouter::new()
        .register(AutoApproveSmallPayment);

    let agent = AgentBuilder::new()
        .model(OpenAIClient::from_config(&config))
        .config(config)
        .system("You can process payments.")
        .tool(PaymentTool)
        .interrupt_router(router)
        .build();

    let thread_id = agent.create_thread().await?;
    let mut stream = agent.chat_in_thread(&thread_id, "Pay $50 for dinner".into()).await?;

    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::TextDelta(s) => print!("{s}"),
            AgentEvent::Interrupt { interrupts } => {
                // InterruptRouter 已自动处理 amount < 100 的情况
                // 如果仍有未处理的 interrupt，需要人工干预
                println!("\n[Interrupt requires manual action: {:?}]", interrupts);
            }
            AgentEvent::Done => println!("\n[Done]"),
            _ => {}
        }
    }
    Ok(())
}
```

### 2.10 Metadata 透传到 Tool

```rust
use remi_agentloop::prelude::*;

struct AuditTool;

impl Tool for AuditTool {
    fn name(&self) -> &str { "audit_action" }
    fn description(&self) -> &str { "Perform an audited action" }
    fn parameters_schema(&self) -> serde_json::Value { /* ... */ }

    fn execute(&self, args: serde_json::Value)
        -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        async move { Ok(ToolResult::Output(stream! { yield ToolOutput::Result("ok".into()); })) }
    }

    fn execute_with_context(&self, args: serde_json::Value, ctx: &ToolContext)
        -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        async move {
            // 从 metadata 中读取业务上下文
            let user_id = ctx.metadata.as_ref()
                .and_then(|m| m["user_id"].as_str())
                .unwrap_or("unknown");
            let tenant = ctx.metadata.as_ref()
                .and_then(|m| m["tenant"].as_str())
                .unwrap_or("default");

            Ok(ToolResult::Output(stream! {
                yield ToolOutput::Delta(format!("Auditing for user {user_id} (tenant: {tenant})..."));
                yield ToolOutput::Result(format!("Audited by {user_id}"));
            }))
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), AgentError> {
    let agent = AgentBuilder::new()
        .model(OpenAIClient::new("sk-...").with_model("gpt-4o"))
        .system("You perform audited actions.")
        .tool(AuditTool)
        .build();

    // metadata 通过 ProtocolRequest 传入，透传到 ToolContext
    let req = ProtocolRequest {
        messages: vec![Message::user("Run the audit")],
        metadata: Some(serde_json::json!({
            "user_id": "u_12345",
            "tenant": "acme-corp",
            "request_ip": "10.0.0.1",
        })),
        ..Default::default()
    };
    // ...
    Ok(())
}
```

### 2.11 LangSmith Tracing

```rust
use remi_agentloop::prelude::*;
use remi_agentloop::tracing::{LangSmithTracer, StdoutTracer, CompositeTracer};

#[tokio::main]
async fn main() -> Result<(), AgentError> {
    let config = AgentConfig::from_env();

    // 同时追踪到 LangSmith 和 stdout
    let tracer = CompositeTracer::new()
        .add(LangSmithTracer::new("ls-api-key").with_project("prod-v2"))
        .add(StdoutTracer::new());

    let agent = AgentBuilder::new()
        .model(OpenAIClient::from_config(&config))
        .config(config)
        .system("You are a research assistant.")
        .tool(SearchTool)
        .tracer(tracer)
        .build();

    let mut stream = agent.chat("Summarize recent AI news".into()).await?;
    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::TextDelta(s) => print!("{s}"),
            AgentEvent::Done => println!(),
            _ => {}
        }
    }
    // LangSmith 控制台可查看完整的 Run 树：
    // Chain Run → LLM Run(turn 0) → Tool Run(search) → LLM Run(turn 1) → Done
    Ok(())
}
```
