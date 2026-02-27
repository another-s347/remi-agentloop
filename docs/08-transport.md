# 传输层

> HTTP SSE 客户端/服务端、WASM 宿主/Guest、SSE 编解码、三种传输层对比

## 1. SSE 编解码 (transport/sse.rs)

独立于 HTTP 框架的 SSE 编解码逻辑：

```rust
/// 将 ProtocolEvent 编码为 SSE 文本帧
pub fn encode_sse_event(event: &ProtocolEvent) -> String {
    let event_type = match event {
        ProtocolEvent::Delta { .. } => "delta",
        ProtocolEvent::ToolCallStart { .. } => "tool_call_start",
        ProtocolEvent::Done => "done",
        // ...
    };
    let data = serde_json::to_string(event).unwrap();
    format!("event: {event_type}\ndata: {data}\n\n")
}

/// 从 SSE 文本行解码为 ProtocolEvent
pub fn decode_sse_line(event_type: &str, data: &str) -> Result<ProtocolEvent, ProtocolError> {
    serde_json::from_str(data).map_err(|e| ProtocolError {
        code: "sse_parse_error".into(),
        message: e.to_string(),
    })
}
```

---

## 2. HttpSseClient (transport/http_client.rs)

一个 Agent 实现：将请求通过 HTTP POST 发到远程服务，读取 SSE 响应流。

```rust
/// 连接远程 Agent SSE 服务的客户端
/// impl Agent<Request = ProtocolRequest, Response = ProtocolEvent, Error = ProtocolError>
pub struct HttpSseClient {
    client: reqwest::Client,
    endpoint: String,   // e.g. "https://my-agent.example.com/chat"
    headers: HeaderMap, // 自定义 headers（auth 等）
}

impl HttpSseClient {
    pub fn new(endpoint: impl Into<String>) -> Self;
    pub fn with_header(self, key: &str, value: &str) -> Self;
    pub fn with_bearer_token(self, token: &str) -> Self;
}

impl Agent for HttpSseClient {
    type Request = ProtocolRequest;
    type Response = ProtocolEvent;
    type Error = ProtocolError;

    fn chat(&self, req: ProtocolRequest)
        -> impl Future<Output = Result<impl Stream<Item = ProtocolEvent>, ProtocolError>>
    {
        async move {
            let response = self.client
                .post(&self.endpoint)
                .headers(self.headers.clone())
                .json(&req)
                .send()
                .await
                .map_err(|e| ProtocolError::from(e))?;

            Ok(stream! {
                // 读取 SSE response body
                // 逐行解析 event: / data: 字段
                // decode_sse_line() → yield ProtocolEvent
            })
        }
    }
}
```

**特点**：
- 与 `OpenAIClient` 不同，`HttpSseClient` 使用**标准协议**而非 OpenAI 私有格式
- 可连接任何暴露标准协议的远程 Agent 服务
- `reqwest` feature-gated，WASM 下使用 `fetch` API

---

## 3. HttpSseServer (transport/http_server.rs)

将任意符合标准协议的 Agent 暴露为 HTTP SSE 端点：

```rust
/// 将一个 ProtocolAgent 包装为 HTTP SSE 服务
pub struct HttpSseServer<A: ProtocolAgent> {
    agent: Arc<A>,
    bind_addr: SocketAddr,
}

impl<A: ProtocolAgent + Send + Sync + 'static> HttpSseServer<A>
where
    // 服务端需要 Send（多连接并发）
    // 注意：这是服务端独有的约束，不影响 Agent trait 本身
{
    pub fn new(agent: A) -> Self;
    pub fn bind(self, addr: impl Into<SocketAddr>) -> Self;

    /// 启动 HTTP 服务
    pub async fn serve(self) -> Result<(), std::io::Error> {
        let app = axum::Router::new()
            .route("/chat", post(Self::handle_chat));
        // ...
    }

    /// 处理单个请求
    async fn handle_chat(
        State(agent): State<Arc<A>>,
        Json(req): Json<ProtocolRequest>,
    ) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
        let stream = agent.chat(req).await;
        // 将 Result<Stream<ProtocolEvent>, ProtocolError> 转为 SSE 事件流
        // 每个 ProtocolEvent → encode_sse_event()
    }
}
```

**使用示例**：

```rust
let agent = AgentBuilder::new()
    .model(OpenAIClient::new("sk-..."))
    .system("You are helpful.")
    .tool(SearchTool)
    .build()
    .into_protocol();  // BuiltAgent → impl ProtocolAgent

HttpSseServer::new(agent)
    .bind(([0, 0, 0, 0], 8080))
    .serve()
    .await?;
```

任何 `HttpSseClient::new("http://localhost:8080/chat")` 都可以连接这个服务，得到的是一个 `impl Agent`，可以继续 `.map_response()` 组合。

### 传输层类型流动

```
进程 A (Server)                          进程 B (Client)
──────────────                           ──────────────
BuiltAgent<OpenAI>                       HttpSseClient
  impl Agent<                              impl Agent<
    Req = String,                            Req = ProtocolRequest,
    Resp = AgentEvent,                       Resp = ProtocolEvent,
    Err = AgentError                         Err = ProtocolError
  >                                        >
  │                                        │
  ├─ .into_protocol()                      ├─ .map_request(|s| ...)
  │   map to ProtocolRequest/Event         │   .map_response(|e| ...)
  │                                        │
  ▼                                        ▼
  HttpSseServer                            用户代码
  POST /chat ←─── HTTP SSE ──────────────→ reqwest
  JSON body                                SSE stream
```

---

## 4. WASM Agent 传输层

### 4.1 设计概述

两个方向：

1. **宿主端 (`wasm-host` feature)**：`WasmAgent` 通过 `wasmi` 加载 `.wasm` 模块，调用其导出函数，按标准协议通信。`WasmAgent` 自身是一个 `impl Agent`。

2. **Guest 端 (`wasm-guest` feature)**：整个 crate 编译为 `.wasm` 模块时，`guest/exports.rs` 导出标准协议的 FFI 函数供宿主调用。

一个 remi-agentloop 实例可以通过 `WasmAgent` 加载另一个编译好的 remi-agentloop `.wasm` 模块作为子 agent，完全走标准协议。

### 4.2 WASM Guest 导出接口

Guest 模块导出以下函数（C ABI，通过 linear memory 传递数据）：

```rust
// guest/exports.rs — 编译到 .wasm 后导出的函数

/// 分配内存（宿主写入请求数据）
#[no_mangle]
pub extern "C" fn alloc(len: u32) -> u32;  // 返回 ptr

/// 释放内存
#[no_mangle]
pub extern "C" fn dealloc(ptr: u32, len: u32);

/// 注入运行时配置（宿主在 chat() 前调用）
/// 宿主先 alloc() 写入 AgentConfig JSON，然后调用此函数
/// 返回 0 = 成功，非 0 = 错误
#[no_mangle]
pub extern "C" fn set_config(config_ptr: u32, config_len: u32) -> u32;

/// 开始一个 chat 会话
/// 宿主先 alloc() 写入 ProtocolRequest JSON，然后调用此函数
/// 返回 session handle（u32）
#[no_mangle]
pub extern "C" fn chat(request_ptr: u32, request_len: u32) -> u32;  // handle

/// 轮询下一个事件
/// 返回：0 = pending（宿主需要再次调用），ptr = 指向 JSON 编码的 ProtocolEvent
/// 宿主通过 event_len() 获取长度
#[no_mangle]
pub extern "C" fn poll_next(handle: u32) -> u32;  // 0 或 event_ptr

/// 获取最后返回的事件 JSON 长度
#[no_mangle]
pub extern "C" fn event_len() -> u32;

/// 释放 chat 会话
#[no_mangle]
pub extern "C" fn close(handle: u32);
```

Guest 内部胶水代码将这些调用桥接到实际的 Agent 实现：

```rust
// guest/exports.rs 内部实现
use std::collections::HashMap;
use std::sync::LazyLock;

static SESSIONS: LazyLock<RefCell<HashMap<u32, Session>>> = ...;

struct Session {
    // 内部持有 Agent 的 stream
    // poll_next() 时推进 stream 状态机
    // 由于 WASM 单线程，可以用 RefCell + 手动 poll
}

/// 用户需要实现这个函数来注册自己的 Agent
#[no_mangle]
pub extern "C" fn init() {
    guest::register_agent(my_agent);
}
```

### 4.3 WasmAgent 宿主端 (transport/wasm_host.rs)

```rust
/// 通过 wasmi 加载 .wasm 模块并作为 Agent 使用
pub struct WasmAgent {
    engine: wasmi::Engine,
    module: wasmi::Module,
    // 按需为每个 chat() 调用创建 Instance
}

impl WasmAgent {
    /// 从 .wasm 字节加载
    pub fn from_bytes(wasm_bytes: &[u8]) -> Result<Self, ProtocolError>;

    /// 从文件加载
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ProtocolError>;
}

impl Agent for WasmAgent {
    type Request = ProtocolRequest;
    type Response = ProtocolEvent;
    type Error = ProtocolError;

    fn chat(&self, req: ProtocolRequest)
        -> impl Future<Output = Result<impl Stream<Item = ProtocolEvent>, ProtocolError>>
    {
        async move {
            // 1. 创建 wasmi Instance
            let mut store = wasmi::Store::new(&self.engine, ());
            let instance = self.module.instantiate(&mut store, ...)?;

            // 2. 序列化请求 JSON
            let req_json = serde_json::to_vec(&req)?;

            // 3. 调用 guest::alloc() 分配内存，写入请求
            let alloc_fn = instance.get_typed_func::<u32, u32>(&store, "alloc")?;
            let ptr = alloc_fn.call(&mut store, req_json.len() as u32)?;
            // 写入 memory...

            // 4. 调用 guest::chat() 创建会话
            let chat_fn = instance.get_typed_func::<(u32, u32), u32>(&store, "chat")?;
            let handle = chat_fn.call(&mut store, (ptr, req_json.len() as u32))?;

            // 5. 返回一个 stream，逐次调用 poll_next()
            Ok(stream! {
                let poll_fn = instance.get_typed_func::<u32, u32>(&store, "poll_next")?;
                let event_len_fn = instance.get_typed_func::<(), u32>(&store, "event_len")?;

                loop {
                    let event_ptr = poll_fn.call(&mut store, handle)?;
                    if event_ptr == 0 {
                        break; // stream 结束
                    }

                    let len = event_len_fn.call(&mut store, ())?;
                    let event_json = read_guest_memory(&store, &instance, event_ptr, len);
                    let event: ProtocolEvent = serde_json::from_slice(&event_json)?;

                    let is_done = matches!(event, ProtocolEvent::Done);
                    yield event;

                    if is_done { break; }
                }

                // 清理
                let close_fn = instance.get_typed_func::<u32, ()>(&store, "close")?;
                close_fn.call(&mut store, handle).ok();
            })
        }
    }
}
```

**关键设计点**：
- **wasmi 是纯 Rust**，本身可编译到 WASM —— 所以 `WasmAgent` 可以在 WASM 环境中运行，加载另一个 WASM 模块（嵌套）
- **同步轮询**：wasmi 的 `call()` 是同步的，Guest 的 `poll_next()` 每次返回一个事件，与 Agent loop 的单线程模型完美匹配
- **每次 `chat()` 创建独立 Instance**：天然隔离，无状态泄漏
- **标准协议**：宿主和 guest 之间仅通过 `ProtocolRequest` / `ProtocolEvent` JSON 通信

### 4.4 WASM 嵌套组合示例

```rust
// 加载一个 .wasm 模块作为子 agent
let sub_agent = WasmAgent::from_file("plugins/code_reviewer.wasm")?;

// 它就是一个普通的 Agent，可以组合
let agent = sub_agent
    .map_response(|event| match event {
        ProtocolEvent::Delta { content, .. } => MyEvent::Review(content),
        ProtocolEvent::Done => MyEvent::Done,
        _ => MyEvent::Other,
    });

// 或者作为 BuiltAgent 的一部分，通过 HttpSseServer 对外暴露
HttpSseServer::new(sub_agent)
    .bind(([0, 0, 0, 0], 9090))
    .serve()
    .await?;
```

### 4.5 编译为 WASM Guest

```rust
// src/main.rs（编译为 WASM guest 时）
use remi_agentloop::prelude::*;
use remi_agentloop::guest;

struct MyAgent { /* ... */ }

impl Agent for MyAgent {
    type Request = ProtocolRequest;
    type Response = ProtocolEvent;
    type Error = ProtocolError;
    // ...
}

guest::register(MyAgent::new());
```

编译命令：
```bash
cargo build --target wasm32-unknown-unknown --no-default-features --features wasm-guest
```

---

## 5. 三种传输层对比

| | 直接调用 | HTTP SSE | WASM (wasmi) |
|---|---|---|---|
| 协议 | Rust 泛型 | 标准协议 JSON+SSE | 标准协议 JSON+FFI |
| 延迟 | 0（同进程） | 网络延迟 | ~μs（内存调用） |
| 隔离 | 无 | 进程隔离 | 沙箱隔离 |
| 可组合 | ✅ 强类型 | ✅ 通过 ProtocolAgent | ✅ 通过 ProtocolAgent |
| 类型安全 | 编译期 | 运行时（JSON） | 运行时（JSON） |
| 部署 | 同一二进制 | 分布式 | 插件式 |
| WASM 中可用 | ✅ | ✅（reqwest/wasm） | ✅（wasmi 可编译到 WASM） |
