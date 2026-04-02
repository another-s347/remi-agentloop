# 高级组合模式——Task / Sub-Agent / 会话分叉 / 上下文压缩 / 监控

> 基于现有架构的可行性分析 + 实现方案

补充：如果你要实现“子 agent 作为 tool，过程投影到子会话”的标准做法，见 [17-sub-agent-sub-session.md](17-sub-agent-sub-session.md)。

## 结论速览

| 模式 | 是否需要改架构 | 实现难度 | 关键依赖 |
|------|-------------|---------|---------|
| Task（独立 memory） | **否** | 低 | Tool 内部构造独立 Agent + Store |
| Sub-Agent（共享 memory） | **否**（建议加便利设施） | 中 | ContextStore 共享引用 `Rc<S>` |
| 会话分叉 | **否** | 低 | 现有 ContextStore API 已足够 |
| Context Compact（上下文压缩） | **否** | 低–中 | `get_recent_messages` + Layer/用户侧压缩 |
| Token/调用/时间监控 | **否** | 低 | 现有 Tracer trait 已完全覆盖 |
| 实时打断（流式取消 + 保存已输出） | **否**（框架已支持） | 低 | `ChatInput::Cancel { partial_response }` |

五种模式均可在**不修改核心架构**的情况下实现。下面逐一分析。

---

## 1. Task（独立 memory）

### 场景

Agent 在执行过程中需要启动一个"子任务"，子任务拥有**独立的对话上下文**（自己的 Thread），不污染主会话的消息历史。例如：

- 研究工具：主 Agent 委托一个"调研 Agent"独立进行多轮搜索 + 总结，最后只把结论返回给主 Agent
- 代码生成：主 Agent 启动一个独立的编码 Agent，用独立 Thread 管理多轮改进

### 可行性分析

当前架构已具备所有必要组件：

1. **ContextStore.create_thread()**——创建独立 Thread ✓
2. **AgentBuilder + BuiltAgent**——任何 Tool 内部都可以构造独立 Agent ✓
3. **InMemoryStore**——轻量级内存 store，适合 task 级别的短期上下文 ✓
4. **Tool 返回 Stream\<ToolOutput\>**——task 的增量进度可通过 ToolOutput::Delta 上报 ✓

### 实现方案

Tool 内部构造一个独立的 BuiltAgent，使用自己的 InMemoryStore：

```rust
/// 研究任务——启动独立 Agent 进行多轮调研
struct ResearchTask {
    model: Arc<OpenAIClient>,  // 共享 model client
}

impl Tool for ResearchTask {
    fn name(&self) -> &str { "deep_research" }
    fn description(&self) -> &str { "Perform multi-turn research on a topic" }
    fn parameters_schema(&self) -> serde_json::Value { /* ... */ }

    fn execute_with_context(
        &self,
        arguments: serde_json::Value,
        ctx: &ToolContext,
    ) -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>> {
        let model = self.model.clone();
        async move {
            let topic = arguments["topic"].as_str().unwrap_or("unknown");

            // ── 独立 store + 独立 thread ──
            let task_store = InMemoryStore::new();
            let task_agent = AgentBuilder::new()
                .model(model)
                .system("You are a research assistant. Be thorough.")
                .tool(SearchTool)
                .tool(SummarizeTool)
                .context_store(task_store)
                .max_turns(5)
                .build();

            let task_thread = task_agent.create_thread().await?;

            Ok(ToolResult::Output(stream! {
                yield ToolOutput::Delta(format!("Starting research on: {topic}"));

                // 在独立 Thread 中运行——不影响主会话
                let mut stream = task_agent
                    .chat_in_thread(&task_thread, format!("Research: {topic}"))
                    .await?;

                let mut final_text = String::new();
                while let Some(event) = stream.next().await {
                    match event {
                        AgentEvent::TextDelta(s) => {
                            yield ToolOutput::Delta(s.clone());
                            final_text.push_str(&s);
                        }
                        AgentEvent::Done => break,
                        _ => {}
                    }
                }

                yield ToolOutput::Result(final_text);
            }))
        }
    }
}
```

### 无需改动的原因

- Tool 天然可以构造新的 Agent + Store——Rust 的所有权系统保证隔离
- ToolOutput::Delta 支持流式进度上报——task 的中间过程对外可见
- Task 结束后 InMemoryStore 自然 drop——无泄漏

### 可选增强（非必须）

如果需要 task 的 Thread 也持久化到与主 Agent 相同的存储后端（用于审计/回溯），可以在 ToolContext 中增加 `store` 引用：

```rust
pub struct ToolContext {
    pub config: AgentConfig,
    pub thread_id: Option<ThreadId>,
    pub run_id: RunId,
    pub metadata: Option<serde_json::Value>,
    // ── 可选增强 ──
    // pub store: Option<&dyn DynContextStore>,  // 共享 store 引用
}
```

但这**不是必须的**——Task 可以用完全独立的 store。

---

## 2. Sub-Agent（共享 memory）

### 场景

主 Agent 将某类请求代理给另一个 Agent（不同的 system prompt / tool / model），但双方共享同一个 Thread 的消息历史。例如：

- 路由 Agent：根据用户意图将请求分发到"客服 Agent""技术 Agent""订单 Agent"，它们共享同一个 Thread
- 专家系统：主 Agent 遇到特定领域问题时，委托给领域专家 Sub-Agent，Sub-Agent 的回复追加到同一 Thread

### 可行性分析

关键问题是：**两个 BuiltAgent 能否共享同一个 ContextStore 实例？**

1. **ContextStore 是泛型 trait**——不要求 Clone。但 `InMemoryStore` 内部使用 `RefCell<HashMap<...>>`，可用 `Rc<InMemoryStore>` 实现共享 ✓
2. **ContextStore trait 的 `&self` 方法**——所有方法都是 `&self`，通过 `Rc<T>` 调用天然可行 ✓
3. **Agent.chat_in_thread() 接受 &ThreadId**——多个 Agent 可以在同一个 Thread 上交替调用 ✓
4. **Agent trait 是组合式的**——可以用 Layer 包装 routing 逻辑 ✓

### 实现方案

#### 方案 A：Rc 共享 ContextStore

为 `Rc<S>` 实现 ContextStore（blanket impl）：

```rust
/// Rc 共享引用自动获得 ContextStore 能力
impl<S: ContextStore> ContextStore for Rc<S> {
    fn create_thread(&self) -> impl Future<Output = Result<ThreadId, AgentError>> {
        (**self).create_thread()
    }
    fn get_messages(&self, thread_id: &ThreadId)
        -> impl Future<Output = Result<Vec<Message>, AgentError>>
    {
        (**self).get_messages(thread_id)
    }
    fn append_message(&self, thread_id: &ThreadId, message: Message)
        -> impl Future<Output = Result<MessageId, AgentError>>
    {
        (**self).append_message(thread_id, message)
    }
    // ... 其余方法同理
}
```

然后两个 Agent 共享同一个 store：

```rust
let shared_store = Rc::new(InMemoryStore::new());

let customer_agent = AgentBuilder::new()
    .model(model.clone())
    .system("You are a customer service agent.")
    .tool(OrderLookupTool)
    .context_store(Rc::clone(&shared_store))
    .build();

let tech_agent = AgentBuilder::new()
    .model(model.clone())
    .system("You are a technical support agent.")
    .tool(DiagnosticTool)
    .context_store(Rc::clone(&shared_store))
    .build();

// 同一个 Thread——共享消息历史
let thread = shared_store.create_thread().await?;

// 路由逻辑
let stream = if is_technical_question(&input) {
    tech_agent.chat_in_thread(&thread, input).await?
} else {
    customer_agent.chat_in_thread(&thread, input).await?
};
```

#### 方案 B：Agent-as-Tool（Sub-Agent 作为 Tool）

将 Sub-Agent 封装为主 Agent 的 Tool，在同一 Thread 中运行：

```rust
/// 将另一个 BuiltAgent 包装为 Tool
struct SubAgentTool<M: ChatModel, S: ContextStore> {
    agent: BuiltAgent<M, S>,
    name: String,
}

impl<M: ChatModel, S: ContextStore> Tool for SubAgentTool<M, S> {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { "Delegate to a specialized agent" }
    fn parameters_schema(&self) -> serde_json::Value { /* ... */ }

    fn execute_with_context(
        &self,
        arguments: serde_json::Value,
        ctx: &ToolContext,
    ) -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>> {
        async move {
            let query = arguments["query"].as_str().unwrap_or("");
            let thread_id = ctx.thread_id.as_ref()
                .ok_or(AgentError::Model("No thread_id".into()))?;

            // ── 在同一 Thread 对话——共享 memory ──
            let mut stream = self.agent
                .chat_in_thread(thread_id, query.to_string())
                .await?;

            Ok(ToolResult::Output(stream! {
                while let Some(event) = stream.next().await {
                    match event {
                        AgentEvent::TextDelta(s) => yield ToolOutput::Delta(s),
                        AgentEvent::Done => break,
                        _ => {}
                    }
                }
                yield ToolOutput::Result("Sub-agent completed".into());
            }))
        }
    }
}
```

### 无需改动的原因

- `ContextStore` trait 的 `&self` 方法 + `Rc` 共享是 Rust 标准模式
- Agent 组合（Layer / map / Tool 封装）是核心设计，Sub-Agent 是自然的组合模式
- ThreadId 可自由传递，多个 Agent 可在同一 Thread 上操作

### 建议增强（非必须）

1. **内置 `impl ContextStore for Rc<S>`**——一行 blanket impl，方便用户
2. **ToolContext 增加 store 引用**——让 agent-as-tool 模式更自然
3. **内置 RouterAgent**——根据条件路由到不同 sub-agent 的通用组件

---

## 3. 会话分叉（Conversation Forking）

### 场景

从对话的某个时间点"分叉"，创建一个新的分支会话。两条分支共享分叉点之前的历史，之后独立演化。例如：

- A/B 测试：在同一上下文下测试不同的 system prompt 或 model
- 探索性对话：用户想"回到之前的某个点重新开始"
- 并行策略：同一问题用不同工具集并行处理，取最优结果

### 可行性分析

现有 ContextStore API **完全足够**：

1. `get_messages(thread_id)` → 获取源 Thread 的所有消息 ✓
2. `create_thread()` → 创建新 Thread 作为分叉目标 ✓
3. `append_messages(new_thread, messages[..fork_point])` → 复制消息到分叉点 ✓

无需新增任何 API 即可实现分叉。

### 实现方案

分叉是纯用户侧代码——几行即可：

```rust
/// 在指定消息之后分叉 Thread
/// 新 Thread 包含 up_to_message（含）之前的所有消息
async fn fork_thread<S: ContextStore>(
    store: &S,
    source_thread_id: &ThreadId,
    up_to_message: &MessageId,
) -> Result<ThreadId, AgentError> {
    // 1. 获取源 Thread 的所有消息
    let messages = store.get_messages(source_thread_id).await?;

    // 2. 截取到分叉点（含 up_to_message）
    let fork_point = messages.iter()
        .position(|m| m.id == *up_to_message)
        .ok_or(AgentError::MessageNotFound(up_to_message.clone()))?;
    let forked_messages: Vec<Message> = messages[..=fork_point]
        .iter()
        .map(|m| {
            // 为分叉的消息生成新 ID（避免两个 Thread 的消息 ID 冲突）
            Message { id: MessageId::new(), ..m.clone() }
        })
        .collect();

    // 3. 创建新 Thread 并写入
    let new_thread = store.create_thread().await?;
    store.append_messages(&new_thread, forked_messages).await?;

    Ok(new_thread)
}
```

### 使用示例

```rust
let store = InMemoryStore::new();
let agent = AgentBuilder::new()
    .model(model)
    .system("You are helpful.")
    .context_store(&store)
    .build();

// 主对话
let thread = agent.create_thread().await?;
consume_stream(agent.chat_in_thread(&thread, "Tell me about Rust".into()).await?).await;
// 假设第3条消息是我们想分叉的点
let messages = store.get_messages(&thread).await?;
let fork_at = &messages[2].id;

// ── 分叉 ──
let branch_thread = fork_thread(&store, &thread, fork_at).await?;

// 两条分支独立演化
let stream_a = agent.chat_in_thread(&thread, "Focus on async".into()).await?;
let stream_b = agent.chat_in_thread(&branch_thread, "Focus on WASM".into()).await?;
// stream_a 和 stream_b 共享前 3 条消息，之后独立
```

### 无需改动的原因

- `get_messages` + `create_thread` + `append_messages` 组合已覆盖分叉语义
- MessageId 保证消息可精确定位分叉点
- 分叉后的 Thread 完全独立，无需特殊处理

### 可选便利方法（非必须）

如果分叉是高频操作，可以在 ContextStore trait 或 extension trait 上添加便利方法：

```rust
/// ContextStore 扩展——会话分叉（可选便利方法）
pub trait ContextStoreExt: ContextStore {
    /// 分叉 Thread——复制消息到 up_to_message（含）
    fn fork_thread(
        &self,
        source: &ThreadId,
        up_to_message: &MessageId,
    ) -> impl Future<Output = Result<ThreadId, AgentError>> {
        async {
            let messages = self.get_messages(source).await?;
            let idx = messages.iter()
                .position(|m| m.id == *up_to_message)
                .ok_or(AgentError::MessageNotFound(up_to_message.clone()))?;
            let new_thread = self.create_thread().await?;
            let forked: Vec<Message> = messages[..=idx]
                .iter()
                .map(|m| Message { id: MessageId::new(), ..m.clone() })
                .collect();
            self.append_messages(&new_thread, forked).await?;
            Ok(new_thread)
        }
    }
}

// blanket impl
impl<S: ContextStore> ContextStoreExt for S {}
```

---

## 4. 模式对比与组合

三种模式可以**相互组合**：

```
主 Agent (Thread A)
├── Tool: ResearchTask               ← Task 模式（独立 Thread B）
│   └── 独立 Agent + InMemoryStore
│       └── Thread B（独立 memory）
├── Tool: SubAgentTool(tech_agent)   ← Sub-Agent 模式（共享 Thread A）
│   └── tech_agent.chat_in_thread(Thread A, ...)
└── fork_thread(Thread A, msg_5)     ← 分叉模式
    └── Thread A'（独立分支）
```

### 关键设计决策

| 维度 | Task | Sub-Agent | 分叉 |
|------|------|-----------|------|
| Thread | 新建独立 Thread | 共享同一 Thread | 复制后独立 Thread |
| ContextStore | 独立 or 共享 | 必须共享 | 共享同一 store |
| RunId | 独立 Run | 独立 Run（在共享 Thread 内） | 独立 Run |
| 消息可见性 | 主 Agent 不可见 | 主 Agent 可见（同 Thread） | 分叉前共享，分叉后独立 |
| 典型用途 | 后台调研/批量处理 | 专家路由/能力委托 | A/B 测试/回溯 |

---

## 5. 实时打断（流式取消 + 保存已输出内容）

> **适用场景**：需要构建实时 Agent 应用（聊天 UI、SSE 接口等），用户可在模型输出中途点击"停止"，并希望已输出的部分文字被记录进会话历史，以便后续恢复或展示。

### 机制说明

框架的 `ChatInput::Cancel` 是**下一次独立调用**，此时当前流已 drop，框架层无法自动获取已输出内容。因此采用"调用方积累 + 框架落盘"的分工：

- **调用方**：在消费流的过程中累积 `AgentEvent::TextDelta`，收到中断信号后停止消费
- **框架**：`cancel_loop()` 接收 `partial_response`，将其作为 assistant message 插入 state，随 `Cancelled` checkpoint 一起持久化到 `ContextStore` 和 `CheckpointStore`

### 使用模式

```rust
// 1. 消费流，同时积累已输出文字
let mut partial = String::new();
let run_id; // 从 AgentEvent::RunStart 或 Checkpoint 事件中取得
let mut stream = agent.chat_in_thread(&tid, "你的问题").await?;

loop {
    tokio::select! {
        maybe_ev = stream.next() => {
            match maybe_ev {
                Some(AgentEvent::TextDelta(t)) => {
                    partial.push_str(&t);
                    send_to_ui(&t); // 实时推送给前端
                }
                Some(AgentEvent::Done) => break,
                Some(_) => {}
                None => break,
            }
        }
        _ = user_cancel_signal.notified() => {
            // 2. 用户打断——drop 当前流，携带已输出内容发起 cancel
            drop(stream);
            agent.chat_in_thread(
                &tid,
                ChatInput::Cancel {
                    run_id,
                    partial_response: if partial.is_empty() { None } else { Some(partial) },
                },
            ).await?;
            break;
        }
    }
}
```

### 保存效果

- partial assistant message 被写入 `state.messages` 并持久化到 `ContextStore` 和 `CheckpointStore`
- checkpoint status 为 `Cancelled`，下次可通过 `resume_from_checkpoint()` 或发送新消息继续对话
- 如果打断发生在模型开始响应之前（`partial` 为空），传 `partial_response: None`，行为与原来一致

### WASM 场景

WASM guest 的 `run_guest()` 是同步循环（`call_next()` 逐事件拉取），不涉及 `ChatInput::Cancel`。取消逻辑由宿主在每次 `call_next()` 调用之间检查 flag 实现，已收集的 `events` Vec 直接截断返回，部分输出自然保留在 events 列表中，无需额外处理。

---

## 6. 总结

当前架构的核心抽象——**ContextStore（Thread/Message 管理）+ Agent 组合（Layer/Tool/Builder）+ 强类型 ID 体系**——提供了足够的灵活性，各种高级模式**均无需修改核心架构**：

- **Task**：Tool 内构造独立 Agent + Store，天然隔离
- **Sub-Agent**：`Rc<ContextStore>` 共享 + 同一 ThreadId，天然复用
- **会话分叉**：`get_messages` + `create_thread` + `append_messages` 组合即可
- **实时打断**：调用方积累 `TextDelta`，通过 `ChatInput::Cancel { partial_response }` 落盘

建议的**可选增强**（均可后续按需添加，不影响现有 API）：

1. `impl ContextStore for Rc<S>` —— blanket impl，几行代码
2. `ContextStoreExt::fork_thread()` —— 便利方法
3. `ToolContext` 增加可选 `store` 引用 —— 让 agent-as-tool 模式更自然
4. 内置 `RouterAgent` / `SubAgentTool` 组件 —— 减少 boilerplate
