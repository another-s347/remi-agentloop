# AgentLoop 状态机 + Builder

> LoopState enum 状态机、AgentLoop Stream 实现（流式 tool + 并行执行 + interrupt/resume）、Typestate AgentBuilder、BuiltAgent

## 内部状态

```rust
enum LoopState {
    /// 初始化：构建 prompt
    Init,
    /// 调用 model（等待 Future 完成）
    CallingModel,
    /// 消费 model 返回的 stream
    Streaming {
        stream: Pin<Box<dyn Stream<Item = ChatResponseChunk>>>,
        tool_calls: Vec<ToolCallAccumulator>,
        text_buffer: String,
    },
    /// 并行执行工具（每个 tool 返回 stream）
    ExecutingTools {
        tool_calls: Vec<ParsedToolCall>,
    },
    /// 中断——等待外部 resume
    Interrupted {
        /// 已完成的 tool results（非 interrupt 的）
        completed_results: Vec<ToolCallResult>,
        /// 待恢复的中断列表
        pending_interrupts: Vec<InterruptInfo>,
    },
    /// 完成
    Done,
}

/// 累积流式 tool call 的增量数据
struct ToolCallAccumulator {
    index: usize,
    id: String,
    name: String,
    arguments: String,  // 逐步拼接
}

struct ParsedToolCall {
    id: String,
    name: String,
    arguments: serde_json::Value,
}

/// 单个 tool call 的执行结果
struct ToolCallResult {
    id: String,
    name: String,
    result: String,
}
```

## AgentLoop

```rust
pub struct AgentLoop<'a, M: ChatModel, S: ContextStore = NoStore> {
    model: &'a M,
    tools: &'a ToolRegistry,
    store: Option<&'a S>,         // 可选上下文存储
    tracer: Option<&'a dyn DynTracer>,  // 可选追踪器
    thread_id: ThreadId,           // 当前会话 ID
    run_id: RunId,                 // 当前运行 ID
    metadata: Option<serde_json::Value>, // 请求携带的业务 metadata
    messages: Vec<Message>,
    state: LoopState,
    max_turns: usize,
    current_turn: usize,
}
```

## 状态转换图（含 interrupt/resume）

```
Init
  │ 构建 messages (system + user input)
  ▼
CallingModel
  │ model.chat(request).await
  ▼
Streaming
  │ 逐个 yield AgentEvent::TextDelta / ToolCallStart / ToolCallArgumentsDelta
  │ 收集完成后：
  │   ├─ 有 tool calls → ExecutingTools
  │   └─ 无 tool calls → Done (yield AgentEvent::Done)
  ▼
ExecutingTools  ← 并行启动所有 tool，每个返回 ToolResult<Stream>
  │ 对每个 tool 的 ToolResult：
  │   ├─ ToolResult::Output(stream) → 消费 stream：
  │   │     ├─ ToolOutput::Delta → yield AgentEvent::ToolDelta
  │   │     └─ ToolOutput::Result → yield AgentEvent::ToolResult，收集结果
  │   └─ ToolResult::Interrupt(req) → 直接记录 InterruptInfo（无 stream）
  │
  │ 全部 tool 处理完毕后：
  │   ├─ 有 interrupt(s) → Interrupted
  │   │     yield AgentEvent::Interrupt { interrupts }
  │   │     stream 暂停（return）
  │   │
  │   └─ 无 interrupt → 将所有 results 追加到 messages
  │         current_turn += 1
  │         ├─ current_turn < max_turns → CallingModel
  │         └─ current_turn >= max_turns → Done (MaxTurnsExceeded)
  ▼
Interrupted
  │ AgentLoop stream 已结束（yield Interrupt 后 return）
  │ 调用方收到 Interrupt 事件，处理后调用 resume()
  │ （处理可以是人工操作，也可以是应用层自动策略）
  │
  │ **RunId 保持不变**——resume 复用原始 RunId，逻辑上属于同一个 Run
  │
  │ resume(Vec<ResumePayload>)
  │   ├─ 校验：|payloads| == |pending_interrupts|，所有 interrupt_id 匹配
  │   ├─ 将 resume results 合并为 ToolCallResult
  │   ├─ 连同 completed_results 一起追加到 messages
  │   ├─ current_turn += 1
  │   ├─ Tracer: on_resume()（使用同一 run_id）
  │   └─ 返回新的 AgentLoop stream（从 CallingModel 继续，不再 yield RunStart）
  ▼
Done
```

## 实现方案：async-stream

```rust
impl<'a, M: ChatModel, S: ContextStore> AgentLoop<'a, M, S> {
    pub fn into_stream(self) -> impl Stream<Item = AgentEvent> + 'a {
        stream! {
            // 首个事件：RunStart（回显 metadata）
            yield AgentEvent::RunStart {
                thread_id: self.thread_id.clone(),
                run_id: self.run_id.clone(),
                metadata: self.metadata.clone(),
            };

            let mut messages = self.messages;
            for turn in 0..self.max_turns {
                // ── CallingModel ──
                let request = ChatRequest { messages: messages.clone(), ... };
                let mut chat_stream = match self.model.chat(request).await {
                    Ok(s) => s,
                    Err(e) => { yield AgentEvent::Error(e); return; }
                };

                // ── Streaming ──
                let mut tool_calls = Vec::new();
                while let Some(chunk) = chat_stream.next().await {
                    match chunk {
                        ChatResponseChunk::Delta { content, .. } => {
                            yield AgentEvent::TextDelta(content);
                        }
                        ChatResponseChunk::ToolCallStart { id, name, .. } => {
                            yield AgentEvent::ToolCallStart { id: id.clone(), name: name.clone() };
                            tool_calls.push(ToolCallAccumulator { id, name, .. });
                        }
                        ChatResponseChunk::ToolCallDelta { index, arguments_delta } => {
                            yield AgentEvent::ToolCallArgumentsDelta { .. };
                            tool_calls[index].arguments.push_str(&arguments_delta);
                        }
                        _ => {}
                    }
                }

                // 无 tool calls → 完成
                if tool_calls.is_empty() {
                    yield AgentEvent::Done;
                    return;
                }

                // ── ExecutingTools（并行） ──
                let parsed: Vec<ParsedToolCall> = tool_calls.into_iter()
                    .map(|tc| ParsedToolCall {
                        id: tc.id,
                        name: tc.name,
                        arguments: serde_json::from_str(&tc.arguments).unwrap_or_default(),
                    })
                    .collect();

                // 并行启动所有 tool，每个返回 ToolResult<Stream>
                let tool_results = self.tools.execute_parallel(&parsed).await;

                let mut completed_results: Vec<ToolCallResult> = Vec::new();
                let mut pending_interrupts: Vec<InterruptInfo> = Vec::new();

                // 处理每个 tool 的 ToolResult
                for (tool_call_id, tool_result) in tool_results {
                    let tc = parsed.iter().find(|p| p.id == tool_call_id).unwrap();
                    match tool_result {
                        Err(e) => {
                            yield AgentEvent::Error(e);
                            completed_results.push(ToolCallResult {
                                id: tool_call_id,
                                name: tc.name.clone(),
                                result: "Error".into(),
                            });
                        }
                        Ok(ToolResult::Interrupt(req)) => {
                            // ToolResult::Interrupt —— 无 stream，直接记录中断信息
                            pending_interrupts.push(InterruptInfo {
                                interrupt_id: req.interrupt_id,
                                tool_call_id: tool_call_id.clone(),
                                tool_name: tc.name.clone(),
                                kind: req.kind,
                                data: req.data,
                            });
                        }
                        Ok(ToolResult::Output(mut tool_stream)) => {
                            // ToolResult::Output —— 消费 stream（只有 Delta / Result，无 Interrupt）
                            let mut last_result = None;
                            while let Some(output) = tool_stream.next().await {
                                match output {
                                    ToolOutput::Delta(delta) => {
                                        yield AgentEvent::ToolDelta {
                                            id: tool_call_id.clone(),
                                            name: tc.name.clone(),
                                            delta,
                                        };
                                    }
                                    ToolOutput::Result(result) => {
                                        yield AgentEvent::ToolResult {
                                            id: tool_call_id.clone(),
                                            name: tc.name.clone(),
                                            result: result.clone(),
                                        };
                                        last_result = Some(result);
                                    }
                                }
                            }
                            if let Some(result) = last_result {
                                completed_results.push(ToolCallResult {
                                    id: tool_call_id,
                                    name: tc.name.clone(),
                                    result,
                                });
                            }
                        }
                    }
                }

                // ── 检查 interrupt ──
                if !pending_interrupts.is_empty() {
                    // 保存中断状态（如有 store）
                    // yield Interrupt 事件，stream 暂停
                    yield AgentEvent::Interrupt {
                        interrupts: pending_interrupts,
                    };
                    // stream 在此结束——调用方需要 resume() 获取新 stream
                    return;
                }

                // ── 无 interrupt：追加 tool results 到 messages ──
                for tr in &completed_results {
                    let tool_msg = Message::tool_result(&tr.id, &tr.result);
                    if let Some(store) = &self.store {
                        store.append_message(&self.thread_id, tool_msg.clone()).await.ok();
                    }
                    messages.push(tool_msg);
                }

                yield AgentEvent::TurnStart { turn: turn + 1 };
            }

            yield AgentEvent::Error(AgentError::MaxTurnsExceeded { max: self.max_turns });
        }
    }
}
```

## Resume 机制

```rust
impl<'a, M: ChatModel, S: ContextStore> AgentLoop<'a, M, S> {
    /// 从中断恢复——返回新的 stream 继续执行
    ///
    /// **RunId 不变**：resume 继续使用 into_stream() 时的同一个 RunId，
    /// 不会产生新的 Run。Tracer 事件（on_resume 及后续 on_model_start 等）
    /// 的 run_id 与中断前一致，保持追踪事件链连续。
    /// resume 返回的 stream **不会** yield RunStart（已在首次 into_stream 发出）。
    ///
    /// 校验：payloads 必须覆盖 Interrupt 事件中的所有 interrupt_id
    pub fn resume(
        &mut self,
        completed_results: Vec<ToolCallResult>,
        pending_interrupts: Vec<InterruptInfo>,
        payloads: Vec<ResumePayload>,
    ) -> Result<impl Stream<Item = AgentEvent> + '_, AgentError> {
        // 1. 校验完整性
        if payloads.len() != pending_interrupts.len() {
            return Err(AgentError::ResumeIncomplete {
                expected: pending_interrupts.len(),
                got: payloads.len(),
            });
        }
        for intr in &pending_interrupts {
            if !payloads.iter().any(|p| p.interrupt_id == intr.interrupt_id) {
                return Err(AgentError::InterruptNotFound(intr.interrupt_id.clone()));
            }
        }

        // 2. 将已完成结果 + resume 结果合并，追加到 messages
        for tr in &completed_results {
            let msg = Message::tool_result(&tr.id, &tr.result);
            self.messages.push(msg);
        }
        for payload in &payloads {
            let intr = pending_interrupts.iter()
                .find(|i| i.interrupt_id == payload.interrupt_id).unwrap();
            let result_str = serde_json::to_string(&payload.result).unwrap_or_default();
            let msg = Message::tool_result(&intr.tool_call_id, &result_str);
            self.messages.push(msg);
        }

        // 3. 持久化
        if let Some(store) = &self.store {
            // batch append all new messages
        }

        // 4. 递增 turn，返回新 stream（从 CallingModel 继续）
        self.current_turn += 1;
        Ok(self.continue_stream())
    }

    /// 从当前 messages 和 turn 继续执行
    ///
    /// **不 yield RunStart**——RunStart 仅在 into_stream() 首次调用时发出。
    /// resume 路径复用同一 RunId，Tracer 先 on_resume() 再继续后续事件。
    fn continue_stream(&self) -> impl Stream<Item = AgentEvent> + '_ {
        // 和 into_stream() 逻辑相同，但跳过 Init（直接从 CallingModel 开始）
        // 不 yield RunStart，不调用 tracer.on_run_start()
        stream! {
            // Tracer: on_resume()
            if let Some(tracer) = &self.tracer {
                tracer.on_resume(&ResumeTrace {
                    run_id: self.run_id.clone(),  // 同一 RunId
                    payloads_count: /* ... */,
                    timestamp: chrono::Utc::now(),
                }).await;
            }
            // ...same loop logic from current_turn..max_turns
            // 所有后续 tracer 事件的 run_id 均为 self.run_id（不变）
        }
    }
}
```

---

## Builder (builder.rs)

Typestate 模式保证编译期必须设置 model：

```rust
/// 未设置 model 的标记类型
pub struct NoModel;

pub struct AgentBuilder<M, S = NoStore> {
    model: M,
    store: S,             // 上下文存储（默认 NoStore）
    config: Option<AgentConfig>,  // 运行时配置
    tracer: Option<Box<dyn DynTracer>>,  // 可选追踪器
    system_prompt: Option<String>,
    tools: ToolRegistry,
    max_turns: usize,
}

impl AgentBuilder<NoModel> {
    pub fn new() -> Self {
        AgentBuilder {
            model: NoModel,
            store: NoStore,
            system_prompt: None,
            tools: ToolRegistry::new(),
            max_turns: 10,
        }
    }

    /// 设置 model——类型从 NoModel 变为 WithModel<M>
    pub fn model<M: ChatModel>(self, model: M) -> AgentBuilder<M> {
        AgentBuilder {
            model,
            store: self.store,
            config: self.config,
            system_prompt: self.system_prompt,
            tools: self.tools,
            max_turns: self.max_turns,
        }
    }
}

impl<M: ChatModel, S> AgentBuilder<M, S> {
    /// 设置上下文存储——启用有状态模式
    pub fn context_store<S2: ContextStore>(self, store: S2) -> AgentBuilder<M, S2> {
        AgentBuilder {
            model: self.model,
            store,
            system_prompt: self.system_prompt,
            tools: self.tools,
            max_turns: self.max_turns,
        }
    }
}

impl<M: ChatModel> AgentBuilder<M> {
    pub fn system(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn tool(mut self, tool: impl Tool + 'static) -> Self {
        self.tools.register(tool);
        self
    }

    pub fn max_turns(mut self, n: usize) -> Self {
        self.max_turns = n;
        self
    }

    /// 注入运行时配置（API key、model 覆盖、自定义 headers 等）
    pub fn config(mut self, config: AgentConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// 注入追踪器（LangSmith / 自定义 Tracer）
    pub fn tracer(mut self, tracer: impl Tracer + 'static) -> Self {
        self.tracer = Some(Box::new(tracer));
        self
    }

    /// 构建 Agent——仅在已设置 model 时可用
    pub fn build(self) -> BuiltAgent<M> {
        BuiltAgent {
            model: self.model,
            system_prompt: self.system_prompt.unwrap_or_default(),
            tools: self.tools,
            max_turns: self.max_turns,
        }
    }
}
```

## BuiltAgent

```rust
pub struct BuiltAgent<M: ChatModel, S = NoStore> {
    model: M,
    store: S,
    config: AgentConfig,
    tracer: Option<Box<dyn DynTracer>>,
    system_prompt: String,
    tools: ToolRegistry,
    max_turns: usize,
}

impl<M: ChatModel> Agent for BuiltAgent<M, NoStore> {
    type Request = String;          // 用户输入文本
    type Response = AgentEvent;     // 强类型事件流
    type Error = AgentError;

    /// 无状态模式：每次调用独立，无上下文持久化
    fn chat(&self, user_input: String)
        -> impl Future<Output = Result<impl Stream<Item = AgentEvent>, AgentError>>
    {
        async move {
            let mut messages = Vec::new();
            if !self.system_prompt.is_empty() {
                messages.push(Message::system(&self.system_prompt));
            }
            messages.push(Message::user(&user_input));

            Ok(AgentLoop::new(&self.model, &self.tools, messages, self.max_turns)
                .into_stream())
        }
    }
}

impl<M: ChatModel, S: ContextStore> BuiltAgent<M, S> {
    /// 创建新的会话线程
    pub async fn create_thread(&self) -> Result<ThreadId, AgentError> {
        self.store.create_thread().await
    }

    /// 有状态模式：在 Thread 内 chat，自动加载历史 + 持久化
    pub async fn chat_in_thread(
        &self,
        thread_id: &ThreadId,
        user_input: String,
    ) -> Result<impl Stream<Item = AgentEvent> + '_, AgentError> {
        // 创建 Run
        let run_id = self.store.create_run(thread_id).await?;
        // 加载历史
        let mut messages = self.store.get_messages(thread_id).await?;
        // 确保 system prompt 在首位
        if !self.system_prompt.is_empty()
            && !messages.first().is_some_and(|m| matches!(m.role, Role::System))
        {
            messages.insert(0, Message::system(&self.system_prompt));
        }
        // 追加 user 消息并持久化
        let user_msg = Message::user(&user_input);
        self.store.append_message(thread_id, user_msg.clone()).await?;
        messages.push(user_msg);

        Ok(AgentLoop::new_with_store(
            &self.model, &self.tools, &self.store,
            thread_id.clone(), run_id,
            messages, self.max_turns,
        ).into_stream())
    }

    /// 恢复中断的 Run
    ///
    /// 调用方在收到 `AgentEvent::Interrupt` 后，处理完所有中断，
    /// 携带全部 ResumePayload 调用此方法获取新的 stream 继续执行。
    ///
    /// **RunId 保持不变**——resume 复用原始 RunId，不创建新 Run。
    /// 返回的 stream 不会 yield RunStart（已在首次 chat_in_thread 发出）。
    /// Tracer 事件链连续：on_resume() → on_model_start() → ... → on_run_end()。
    ///
    /// 中断处理方可以是：
    /// - 人工操作者（UI 审批、用户确认）
    /// - 应用层自动逻辑（规则引擎、审批策略、外部系统回调）
    /// - 混合模式（部分 kind 自动处理，部分转人工）
    pub async fn resume_run(
        &self,
        thread_id: &ThreadId,
        run_id: &RunId,
        payloads: Vec<ResumePayload>,
    ) -> Result<impl Stream<Item = AgentEvent> + '_, AgentError> {
        // 1. 从 store 加载中断状态（pending_interrupts + completed_results）
        // 2. 校验 payloads 覆盖所有 interrupt_id
        // 3. 将 completed_results + resume results 追加到 messages
        // 4. 返回新 stream（从 CallingModel 继续）
        todo!()
    }
}
```
