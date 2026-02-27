# 标准协议

> 跨进程/跨主机/跨 WASM 边界的统一线上格式（wire format）

## 设计动机

Agent trait 的 Request/Response/Error 是泛型的，但跨进程/跨主机/跨 WASM 边界通信需要一个**统一的线上格式（wire format）**。标准协议定义了 JSON 可序列化的请求和流式响应事件，作为 HTTP SSE 和 WASM 传输层的公共语言。

任何 `Agent<Request = ProtocolRequest, Response = ProtocolEvent, Error = ProtocolError>` 都自动符合标准协议，可以通过 HTTP SSE 暴露为服务或编译为 WASM 模块。

## ProtocolRequest

```rust
/// 标准协议请求——JSON 可序列化
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolRequest {
    /// 所属 Thread（可选，不传则服务端自动创建）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,

    /// 对话消息列表（多模态 Content）
    pub messages: Vec<Message>,

    /// 可用工具定义（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,

    /// 模型名称（可选，由服务端决定默认值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// 温度
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// 最大 token 数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// 业务自定义 metadata（JSON，透传到 tool calling / tracing）
    /// 框架不解释内容，原样传递给 ToolContext 和 Tracer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,

    /// 扩展字段（向前兼容）
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
```

## ProtocolEvent

```rust
/// 标准协议流式响应事件——JSON 可序列化，tagged enum
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProtocolEvent {
    /// Run 开始（首个事件，携带标识符）
    #[serde(rename = "run_start")]
    RunStart {
        thread_id: String,
        run_id: String,
        /// 回显请求中的 metadata（如有）
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },

    /// 文本增量
    #[serde(rename = "delta")]
    Delta {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },

    /// 工具调用开始
    #[serde(rename = "tool_call_start")]
    ToolCallStart {
        id: String,
        name: String,
    },

    /// 工具调用参数增量
    #[serde(rename = "tool_call_delta")]
    ToolCallDelta {
        id: String,
        arguments_delta: String,
    },

    /// 工具执行增量（tool stream delta）
    #[serde(rename = "tool_delta")]
    ToolDelta {
        id: String,
        name: String,
        delta: String,
    },

    /// 工具执行结果
    #[serde(rename = "tool_result")]
    ToolResult {
        id: String,
        name: String,
        result: String,
    },

    /// 中断——AgentLoop 暂停，等待调用方 resume
    #[serde(rename = "interrupt")]
    Interrupt {
        /// 所有待处理的中断
        interrupts: Vec<InterruptInfo>,
    },

    /// 新一轮开始
    #[serde(rename = "turn_start")]
    TurnStart {
        turn: usize,
    },

    /// 用量统计
    #[serde(rename = "usage")]
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
    },

    /// 错误（非致命）
    #[serde(rename = "error")]
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },

    /// 流结束
    #[serde(rename = "done")]
    Done,
}
```

## ProtocolError

```rust
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct ProtocolError {
    pub code: String,       // "invalid_request", "model_error", "timeout" 等
    pub message: String,
}
```

## ProtocolAgent trait alias

```rust
/// 符合标准协议的 Agent
pub trait ProtocolAgent:
    Agent<Request = ProtocolRequest, Response = ProtocolEvent, Error = ProtocolError>
{
}

// blanket impl
impl<T> ProtocolAgent for T
where
    T: Agent<Request = ProtocolRequest, Response = ProtocolEvent, Error = ProtocolError>,
{
}
```

## 协议与内部类型的桥接

`ChatRequest` ↔ `ProtocolRequest` 和 `AgentEvent` ↔ `ProtocolEvent` 提供 `From` impl：

```rust
impl From<ProtocolRequest> for ChatRequest { ... }
impl From<ChatRequest> for ProtocolRequest { ... }
impl From<AgentEvent> for ProtocolEvent { ... }
impl From<ProtocolEvent> for AgentEvent { ... }
```

这样任何 `BuiltAgent<M>` 都可以通过 `map_request` + `map_response` 适配为 `ProtocolAgent`：

```rust
let protocol_agent = built_agent
    .map_request(|req: ProtocolRequest| req.into())   // ProtocolRequest → String
    .map_response(|event: AgentEvent| event.into())   // AgentEvent → ProtocolEvent
    .map_err(|e: AgentError| e.into());                // AgentError → ProtocolError
```

## SSE 线上格式

标准协议走 SSE 时的线上格式：

```
POST /chat HTTP/1.1
Content-Type: application/json

{"messages": [...], "tools": [...], "metadata": {"user_id": "u_123", "session": "abc"}}

---

HTTP/1.1 200 OK
Content-Type: text/event-stream

event: run_start
data: {"type":"run_start","thread_id":"th_1","run_id":"run_1","metadata":{"user_id":"u_123","session":"abc"}}

event: delta
data: {"type":"delta","content":"Hello"}

event: tool_call_start
data: {"type":"tool_call_start","id":"tc_1","name":"search"}

event: tool_delta
data: {"type":"tool_delta","id":"tc_1","name":"search","delta":"Searching..."}

event: tool_result
data: {"type":"tool_result","id":"tc_1","name":"search","result":"..."}

event: interrupt
data: {"type":"interrupt","interrupts":[{"interrupt_id":"int_1","tool_call_id":"tc_2","tool_name":"payment","kind":"human_approval","data":{...}}]}

--- stream pauses here, client sends resume ---
--- run_id 保持不变（"run_1"），这是同一个 Run 的延续 ---

POST /resume HTTP/1.1
Content-Type: application/json

{"thread_id":"th_1","run_id":"run_1","payloads":[{"interrupt_id":"int_1","result":{"approved":true}}]}

--- stream resumes（注意：不再发送 run_start，因为 RunId 不变） ---

event: tool_result
data: {"type":"tool_result","id":"tc_2","name":"payment","result":"approved"}

event: done
data: {"type":"done"}
```

`event:` 字段为 `ProtocolEvent` 的 `type` 值，`data:` 字段为完整 JSON。

## ResumeRequest

```rust
/// 恢复中断的请求
///
/// **RunId 不变**：run_id 必须与触发 Interrupt 的原始 Run 一致。
/// Resume 不创建新 Run——它是同一个 Run 的延续。
/// 服务端收到 ResumeRequest 后，resume stream 不会再次 yield RunStart。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeRequest {
    /// 所属 Thread
    pub thread_id: ThreadId,
    /// 所属 Run（与原始 RunStart 中的 run_id 相同）
    pub run_id: RunId,
    /// 所有中断的响应（必须覆盖 Interrupt 事件中的全部 interrupt_id）
    pub payloads: Vec<ResumePayload>,
}
```
