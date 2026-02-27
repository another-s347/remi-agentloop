# 类型系统

> 标识符、多模态 Content、Message、Role、ChatRequest、ChatResponseChunk、AgentEvent、AgentError

## 标识符 (types.rs)

```rust
/// 会话线程 ID——一个 Thread 包含多轮 Run，多条 Message
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadId(pub String);

/// 单次运行 ID——一次 agent.chat() 调用
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

/// 消息 ID——标识 Thread 中的每条消息
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

impl ThreadId  { pub fn new() -> Self { Self(uuid_v4()) } }
impl RunId     { pub fn new() -> Self { Self(uuid_v4()) } }
impl MessageId { pub fn new() -> Self { Self(uuid_v4()) } }
```

详见 [11-identifiers-context.md](11-identifiers-context.md) 中的 ID 层级关系。

## 多模态内容 (types.rs)

```rust
/// 消息内容——支持纯文本或多模态
/// 兼容 OpenAI content 字段：可以是 string 也可以是 array of parts
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    /// 纯文本（向后兼容 "content": "hello"）
    Text(String),
    /// 多模态内容部分列表（"content": [{"type": "text", ...}, {"type": "image_url", ...}]）
    Parts(Vec<ContentPart>),
}

impl Content {
    pub fn text(s: impl Into<String>) -> Self { Content::Text(s.into()) }
    pub fn parts(parts: Vec<ContentPart>) -> Self { Content::Parts(parts) }

    /// 提取所有文本内容（忽略非文本部分）
    pub fn text_content(&self) -> String {
        match self {
            Content::Text(s) => s.clone(),
            Content::Parts(parts) => parts.iter()
                .filter_map(|p| match p { ContentPart::Text { text } => Some(text.as_str()), _ => None })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    /// 是否包含非文本内容（图片/音频/文件）
    pub fn is_multimodal(&self) -> bool {
        matches!(self, Content::Parts(parts) if parts.iter().any(|p| !matches!(p, ContentPart::Text { .. })))
    }
}

/// 单个内容部分——对应 OpenAI 多模态 content part
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    /// 文本
    #[serde(rename = "text")]
    Text { text: String },

    /// 图片 URL
    #[serde(rename = "image_url")]
    ImageUrl {
        image_url: ImageUrlDetail,
    },

    /// 图片 Base64（内联）
    #[serde(rename = "image_base64")]
    ImageBase64 {
        media_type: String,   // "image/png", "image/jpeg", "image/webp", "image/gif"
        data: String,          // base64 encoded
    },

    /// 音频
    #[serde(rename = "input_audio")]
    Audio {
        input_audio: AudioDetail,
    },

    /// 文件（PDF 等文档）
    #[serde(rename = "file")]
    File {
        file_id: Option<String>,      // 服务端文件 ID
        filename: Option<String>,
        media_type: Option<String>,    // "application/pdf"
        data: Option<String>,          // base64 encoded
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrlDetail {
    pub url: String,
    /// 细节级别："low" | "high" | "auto"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDetail {
    pub data: String,             // base64 encoded
    pub format: String,           // "wav", "mp3", "ogg"
}
```

### Content 便捷构造

```rust
impl ContentPart {
    pub fn text(s: impl Into<String>) -> Self {
        ContentPart::Text { text: s.into() }
    }
    pub fn image_url(url: impl Into<String>) -> Self {
        ContentPart::ImageUrl {
            image_url: ImageUrlDetail { url: url.into(), detail: None },
        }
    }
    pub fn image_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        ContentPart::ImageBase64 { media_type: media_type.into(), data: data.into() }
    }
}
```

## 消息类型 (types.rs)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// 消息唯一标识
    pub id: MessageId,
    pub role: Role,
    /// 多模态内容（纯文本 或 多模态 parts）
    pub content: Content,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}
```

### Message 便捷构造方法

```rust
impl Message {
    /// 纯文本 user 消息
    pub fn user(text: impl Into<String>) -> Self {
        Self { id: MessageId::new(), role: Role::User, content: Content::text(text), tool_calls: None, tool_call_id: None }
    }
    /// 纯文本 system 消息
    pub fn system(text: impl Into<String>) -> Self {
        Self { id: MessageId::new(), role: Role::System, content: Content::text(text), tool_calls: None, tool_call_id: None }
    }
    /// 多模态 user 消息
    pub fn user_multimodal(parts: Vec<ContentPart>) -> Self {
        Self { id: MessageId::new(), role: Role::User, content: Content::parts(parts), tool_calls: None, tool_call_id: None }
    }
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallMessage {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,  // "function"
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,  // JSON string
}
```

## ChatRequest / ChatResponseChunk

### ChatRequest（发给 OpenAI 等 LLM 的请求）

```rust
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    pub stream: bool,

    /// 业务自定义 metadata（JSON，透传到 tool calling / tracing）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}
```

### ChatResponseChunk（流式 LLM 响应 chunk）

```rust
/// 流式响应——强类型 enum
#[derive(Debug, Clone)]
pub enum ChatResponseChunk {
    /// 文本增量
    Delta { content: String, role: Option<Role> },
    /// Tool call 开始
    ToolCallStart { index: usize, id: String, name: String },
    /// Tool call 参数增量
    ToolCallDelta { index: usize, arguments_delta: String },
    /// 用量统计
    Usage { prompt_tokens: u32, completion_tokens: u32, total_tokens: u32 },
    /// 流结束
    Done,
}
```

## AgentEvent（用户侧最终拿到的强类型事件）

```rust
/// Agent loop 对外 yield 的事件
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Run 开始（首个事件，携带 thread/run 标识 + metadata 回显）
    RunStart {
        thread_id: ThreadId,
        run_id: RunId,
        /// 回显请求中的 metadata（如有）
        metadata: Option<serde_json::Value>,
    },
    /// LLM 输出的文本增量
    TextDelta(String),
    /// 开始调用工具
    ToolCallStart { id: String, name: String },
    /// 工具参数增量（流式）
    ToolCallArgumentsDelta { id: String, delta: String },
    /// 工具执行增量（tool stream 中的 Delta）
    ToolDelta { id: String, name: String, delta: String },
    /// 工具调用结果（tool stream 中的 Result）
    ToolResult { id: String, name: String, result: String },
    /// 工具请求中断——AgentLoop 暂停，等待调用方 resume
    Interrupt {
        /// 本次中断涉及的所有 interrupt（可能多个并行 tool 各自 interrupt）
        interrupts: Vec<InterruptInfo>,
    },
    /// 新一轮 model 调用开始（tool result 后重新调用 LLM）
    TurnStart { turn: usize },
    /// 用量信息
    Usage { prompt_tokens: u32, completion_tokens: u32 },
    /// 完成
    Done,
    /// 错误（非致命，loop 可能继续）
    Error(AgentError),
}

/// 单个中断的详情
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptInfo {
    pub interrupt_id: InterruptId,
    /// 来源 tool call ID
    pub tool_call_id: String,
    /// 来源 tool 名称
    pub tool_name: String,
    /// 中断类型（语义化标签，如 "human_approval", "policy_check", "rate_limit_wait"）
    /// 上层应用可根据 kind 自动路由：人工审批、规则引擎、外部回调等
    pub kind: String,
    /// 传递给调用方的上下文数据
    pub data: serde_json::Value,
}
```

## AgentError (error.rs)

```rust
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SSE parse error: {message}")]
    SseParse { message: String },

    #[error("Tool execution error [{tool_name}]: {message}")]
    ToolExecution { tool_name: String, message: String },

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Model error: {0}")]
    Model(String),

    #[error("Max turns ({max}) exceeded")]
    MaxTurnsExceeded { max: usize },

    #[error("Thread not found: {0}")]
    ThreadNotFound(ThreadId),

    #[error("Run not found: {0}")]
    RunNotFound(RunId),

    #[error("Interrupt not found: {0}")]
    InterruptNotFound(InterruptId),

    #[error("Resume incomplete: expected {expected} interrupt results, got {got}")]
    ResumeIncomplete { expected: usize, got: usize },
}
```
