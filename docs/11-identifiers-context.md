# 标识符 + 上下文管理

> ThreadId / RunId / MessageId、ContextStore trait、InMemoryStore 默认实现

## 1. 标识符体系 (types.rs)

三个强类型 ID 包装，贯穿整个框架：

```rust
use std::fmt;

/// 会话线程 ID——一个 Thread 包含多轮 Run，多条 Message
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadId(pub String);

/// 单次运行 ID——一次 agent.chat() 调用
/// 一个 Run 可能包含多轮 model 调用 + tool 调用
///
/// **Interrupt / Resume 语义**：
/// interrupt → resume 不产生新 RunId。
/// resume_run() 复用原始 RunId，逻辑上整个 interrupt-resume 周期
/// 属于同一个 Run。只有下一次 chat_in_thread() 才产生新 Run。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

/// 消息 ID——标识 Thread 中的每条消息
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

// 便捷生成（默认 UUID v4）
impl ThreadId {
    pub fn new() -> Self { Self(uuid_v4()) }
}
impl RunId {
    pub fn new() -> Self { Self(uuid_v4()) }
}
impl MessageId {
    pub fn new() -> Self { Self(uuid_v4()) }
}

// Display impl
impl fmt::Display for ThreadId { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str(&self.0) } }
impl fmt::Display for RunId    { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str(&self.0) } }
impl fmt::Display for MessageId { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str(&self.0) } }
```

### ID 层级关系

```
Thread (ThreadId)
├── Run 1 (RunId)
│   ├── Message: user input         (MessageId)
│   ├── Message: assistant delta    (MessageId)
│   ├── Message: tool call          (MessageId)
│   ├── Message: tool result        (MessageId)
│   └── Message: assistant final    (MessageId)
├── Run 2 (RunId)
│   ├── Message: user input         (MessageId)
│   └── Message: assistant response (MessageId)
└── ...
```

- **Thread**：一个完整的对话会话，跨越多次用户交互
- **Run**：单次 `agent.chat()` 调用。一个 Run 内部可能经历多轮 model → tool → model 循环。**Interrupt / Resume 不创建新 Run**——resume 后继续使用同一个 RunId，整个 interrupt-resume 周期视为同一次执行
- **Message**：Thread 中的每一条消息（system / user / assistant / tool），每条有唯一 ID

### Message 增加 ID 字段

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// 消息唯一标识
    pub id: MessageId,
    pub role: Role,
    pub content: Content,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}
```

### AgentEvent 补充 ID 信息

```rust
pub enum AgentEvent {
    /// Run 开始（首个事件）
    RunStart { thread_id: ThreadId, run_id: RunId },

    TextDelta(String),
    ToolCallStart { id: String, name: String },
    ToolCallArgumentsDelta { id: String, delta: String },
    ToolResult { id: String, name: String, result: String },
    TurnStart { turn: usize },
    Usage { prompt_tokens: u32, completion_tokens: u32 },
    Done,
    Error(AgentError),
}
```

### ProtocolRequest / ProtocolEvent 补充 ID

```rust
pub struct ProtocolRequest {
    /// 所属 Thread（可选，不传则服务端自动创建）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,

    pub messages: Vec<Message>,
    // ... 其余字段不变
}

pub enum ProtocolEvent {
    /// Run 开始
    #[serde(rename = "run_start")]
    RunStart { thread_id: String, run_id: String },

    // ... 其余变体不变
    Done,
}
```

---

## 2. ContextStore trait (context.rs)

上下文管理的核心抽象——可插拔存储后端：

```rust
/// 上下文存储 trait
/// 管理 Thread、Run、Message 的持久化
/// 使用 RPITIT，与 Agent trait 风格一致
pub trait ContextStore {
    // ── Thread 操作 ──

    /// 创建新的 Thread
    fn create_thread(&self) -> impl Future<Output = Result<ThreadId, AgentError>>;

    /// 获取 Thread 的所有消息（按时间顺序）
    fn get_messages(
        &self,
        thread_id: &ThreadId,
    ) -> impl Future<Output = Result<Vec<Message>, AgentError>>;

    /// 获取 Thread 的最近 N 条消息（滑动窗口）
    fn get_recent_messages(
        &self,
        thread_id: &ThreadId,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<Message>, AgentError>>;

    /// 追加消息到 Thread
    fn append_message(
        &self,
        thread_id: &ThreadId,
        message: Message,
    ) -> impl Future<Output = Result<MessageId, AgentError>>;

    /// 批量追加消息
    fn append_messages(
        &self,
        thread_id: &ThreadId,
        messages: Vec<Message>,
    ) -> impl Future<Output = Result<Vec<MessageId>, AgentError>>;

    /// 删除 Thread 及其所有消息
    fn delete_thread(
        &self,
        thread_id: &ThreadId,
    ) -> impl Future<Output = Result<(), AgentError>>;

    // ── Run 跟踪 ──

    /// 创建新的 Run（关联到 Thread）
    fn create_run(
        &self,
        thread_id: &ThreadId,
    ) -> impl Future<Output = Result<RunId, AgentError>>;

    /// 标记 Run 完成
    fn complete_run(
        &self,
        run_id: &RunId,
    ) -> impl Future<Output = Result<(), AgentError>>;
}
```

### 设计要点

- **RPITIT**——与 Agent trait 保持风格一致，无 Send bound，WASM 兼容
- **Thread-centric**——所有消息归属 Thread，Run 是 Thread 内的执行单元
- **不依赖具体存储**——只定义接口，实现者决定存储方式
- **滑动窗口**——`get_recent_messages` 支持上下文窗口策略，避免 token 超限

---

## 3. InMemoryStore (context/memory.rs)

默认的内存实现，开箱即用：

```rust
use std::cell::RefCell;
use std::collections::HashMap;

/// 内存上下文存储——开发/测试/小规模使用
pub struct InMemoryStore {
    threads: RefCell<HashMap<ThreadId, Vec<Message>>>,
    runs: RefCell<HashMap<RunId, RunRecord>>,
}

struct RunRecord {
    thread_id: ThreadId,
    created_at: u64,       // Unix timestamp
    completed: bool,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            threads: RefCell::new(HashMap::new()),
            runs: RefCell::new(HashMap::new()),
        }
    }
}

impl ContextStore for InMemoryStore {
    fn create_thread(&self) -> impl Future<Output = Result<ThreadId, AgentError>> {
        async {
            let id = ThreadId::new();
            self.threads.borrow_mut().insert(id.clone(), Vec::new());
            Ok(id)
        }
    }

    fn get_messages(&self, thread_id: &ThreadId)
        -> impl Future<Output = Result<Vec<Message>, AgentError>>
    {
        let result = self.threads.borrow()
            .get(thread_id)
            .cloned()
            .ok_or(AgentError::ThreadNotFound(thread_id.clone()));
        async { result }
    }

    fn get_recent_messages(&self, thread_id: &ThreadId, limit: usize)
        -> impl Future<Output = Result<Vec<Message>, AgentError>>
    {
        let result = self.threads.borrow()
            .get(thread_id)
            .map(|msgs| {
                let start = msgs.len().saturating_sub(limit);
                msgs[start..].to_vec()
            })
            .ok_or(AgentError::ThreadNotFound(thread_id.clone()));
        async { result }
    }

    fn append_message(&self, thread_id: &ThreadId, message: Message)
        -> impl Future<Output = Result<MessageId, AgentError>>
    {
        let id = message.id.clone();
        let result = match self.threads.borrow_mut().get_mut(thread_id) {
            Some(msgs) => { msgs.push(message); Ok(id) }
            None => Err(AgentError::ThreadNotFound(thread_id.clone())),
        };
        async { result }
    }

    fn append_messages(&self, thread_id: &ThreadId, messages: Vec<Message>)
        -> impl Future<Output = Result<Vec<MessageId>, AgentError>>
    {
        let ids: Vec<_> = messages.iter().map(|m| m.id.clone()).collect();
        let result = match self.threads.borrow_mut().get_mut(thread_id) {
            Some(msgs) => { msgs.extend(messages); Ok(ids) }
            None => Err(AgentError::ThreadNotFound(thread_id.clone())),
        };
        async { result }
    }

    fn delete_thread(&self, thread_id: &ThreadId)
        -> impl Future<Output = Result<(), AgentError>>
    {
        self.threads.borrow_mut().remove(thread_id);
        async { Ok(()) }
    }

    fn create_run(&self, thread_id: &ThreadId)
        -> impl Future<Output = Result<RunId, AgentError>>
    {
        let run_id = RunId::new();
        self.runs.borrow_mut().insert(run_id.clone(), RunRecord {
            thread_id: thread_id.clone(),
            created_at: now_unix(),
            completed: false,
        });
        async { Ok(run_id) }
    }

    fn complete_run(&self, run_id: &RunId)
        -> impl Future<Output = Result<(), AgentError>>
    {
        let result = match self.runs.borrow_mut().get_mut(run_id) {
            Some(r) => { r.completed = true; Ok(()) }
            None => Err(AgentError::RunNotFound(run_id.clone())),
        };
        async { result }
    }
}
```

### InMemoryStore 特点

- **RefCell 而非 Mutex**——遵循单线程原则，WASM 兼容
- **零额外依赖**——无需数据库
- **可替换**——由 ContextStore trait 保证接口统一

---

## 4. 与 AgentBuilder 的集成

Builder 新增 `.context_store()` 方法：

```rust
pub struct AgentBuilder<M, S = NoStore> {
    model: M,
    store: S,           // 新增
    system_prompt: Option<String>,
    tools: ToolRegistry,
    max_turns: usize,
}

pub struct NoStore;

impl<M: ChatModel> AgentBuilder<M, NoStore> {
    /// 设置上下文存储
    /// 不设置时，BuiltAgent 不做持久化（无状态模式）
    pub fn context_store<S: ContextStore>(self, store: S) -> AgentBuilder<M, S> {
        AgentBuilder {
            model: self.model,
            store,
            system_prompt: self.system_prompt,
            tools: self.tools,
            max_turns: self.max_turns,
        }
    }
}
```

### BuiltAgent 有无 store 的行为差异

```rust
// 无 store（无状态模式）——与之前行为一致
let agent = AgentBuilder::new()
    .model(client)
    .system("You are helpful.")
    .build();

agent.chat("hello".into()).await?;  // 每次调用独立，无上下文

// 有 store（有状态模式）
let store = InMemoryStore::new();
let agent = AgentBuilder::new()
    .model(client)
    .system("You are helpful.")
    .context_store(store)
    .build();

// 创建 thread
let thread_id = agent.create_thread().await?;
// 在 thread 上 chat——自动加载历史消息 + 持久化新消息
let stream = agent.chat_in_thread(&thread_id, "hello".into()).await?;
// 后续调用自动携带上下文
let stream = agent.chat_in_thread(&thread_id, "what did I just say?".into()).await?;
```

## 5. AgentLoop 集成（有 store 时）

```
chat_in_thread(thread_id, user_input)
  │
  ├─ store.create_run(thread_id)  → RunId
  ├─ store.get_messages(thread_id) → 历史 messages
  ├─ 追加 system prompt（如不在历史中）
  ├─ store.append_message(thread_id, user_message)
  │
  ▼
  AgentLoop（正常 state machine）
  │
  ├─ yield RunStart { thread_id, run_id }
  ├─ 每个 assistant message → store.append_message(...)
  ├─ 每个 tool result      → store.append_message(...)
  │
  ▼
  Done → store.complete_run(run_id)
```

## 6. 自定义存储后端示例

```rust
/// Redis 存储后端（示例）
struct RedisStore {
    client: redis::Client,
}

impl ContextStore for RedisStore {
    fn create_thread(&self) -> impl Future<Output = Result<ThreadId, AgentError>> {
        async {
            let id = ThreadId::new();
            // SADD threads {id}
            Ok(id)
        }
    }

    fn get_messages(&self, thread_id: &ThreadId)
        -> impl Future<Output = Result<Vec<Message>, AgentError>>
    {
        async {
            // LRANGE thread:{id}:messages 0 -1
            // 反序列化每条 JSON
            todo!()
        }
    }

    // ... 其余方法
}

/// SQLite 存储后端（示例）
struct SqliteStore { /* ... */ }

/// 远程 API 存储（如 OpenAI Threads API 兼容）
struct RemoteStore { /* ... */ }
```
