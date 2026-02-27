# 可观测性 —— Tracing

> Tracer trait、TracingLayer 适配器、LangSmith 集成、全链路追踪

## 1. 设计动机

AI Agent 的执行链路复杂：model 调用 → tool 执行 → 多轮循环 → interrupt/resume。生产环境必须具备可观测性：

- **调试**——定位 prompt / tool 调用 / 响应中的问题
- **成本监控**——跟踪 token 用量、API 调用次数、延迟分布
- **质量评估**——记录输入输出用于离线评估和回归测试
- **合规审计**——保留完整的调用链路日志

框架通过 `Tracer` trait 提供可插拔的追踪能力，不绑定特定后端。初期内置 `LangSmithTracer`。

### Interrupt / Resume 追踪连续性

**核心语义：interrupt → resume 是同一个 Run 的延续，RunId 不变。**

Tracer 实现者必须遵守以下约定：

1. **RunId 不变**——`on_interrupt()` 和随后的 `on_resume()` 使用同一个 `run_id`。resume 后的所有事件（`on_model_start`/`on_tool_start` 等）继续使用原 RunId
2. **不重发 `on_run_start()`**——resume 路径调用 `on_resume()` 而非 `on_run_start()`。`on_run_start()` 仅在 `into_stream()` 首次调用时触发一次
3. **`on_run_end()` 延迟到真正结束**——中断时 `on_run_end()` 的 `status` 为 `Interrupted`，表示暂停但 Run 未终结。如果 resume 成功并最终完成，会再次触发 `on_run_end()` 且 `status` 为 `Completed`。Tracer 实现者（如 LangSmithTracer）应将 `Interrupted` 状态视为"暂停"而非"结束"，在 resume 后用最终状态更新（PATCH）原 Chain Run
4. **事件时间线连续**——`on_interrupt()` 和 `on_resume()` 的 `timestamp` 反映实际挂起/恢复时间，实现者可据此计算等待耗时

```
时间线示例（单个 Run，中间有 interrupt）：

on_run_start(run_id=R1)
  on_turn_start(run_id=R1, turn=0)
    on_model_start(run_id=R1)
    on_model_end(run_id=R1)
    on_tool_start(run_id=R1, "payment")
    on_tool_end(run_id=R1, interrupted=true)
  on_interrupt(run_id=R1)            ← stream 暂停
  on_run_end(run_id=R1, Interrupted) ← 暂停状态（非终态）
                                     ← 等待人工/自动处理...
  on_resume(run_id=R1)               ← stream 恢复，RunId 不变
  on_turn_start(run_id=R1, turn=1)   ← 继续下一轮
    on_model_start(run_id=R1)
    on_model_end(run_id=R1)
  on_run_end(run_id=R1, Completed)   ← 真正结束
```

## 2. Tracer trait（tracing.rs）

```rust
use std::future::Future;
use std::time::Duration;

/// 追踪事件——AgentLoop 在关键节点调用 Tracer 方法
/// 所有方法有默认空实现（opt-in），实现者只需覆盖关心的事件
pub trait Tracer {
    /// Run 开始
    fn on_run_start(
        &self,
        _event: &RunStartTrace,
    ) -> impl Future<Output = ()> {
        async {}
    }

    /// Run 结束（正常完成或出错）
    fn on_run_end(
        &self,
        _event: &RunEndTrace,
    ) -> impl Future<Output = ()> {
        async {}
    }

    /// Model 调用开始（发送请求到 LLM）
    fn on_model_start(
        &self,
        _event: &ModelStartTrace,
    ) -> impl Future<Output = ()> {
        async {}
    }

    /// Model 调用结束（收到完整响应）
    fn on_model_end(
        &self,
        _event: &ModelEndTrace,
    ) -> impl Future<Output = ()> {
        async {}
    }

    /// Tool 调用开始
    fn on_tool_start(
        &self,
        _event: &ToolStartTrace,
    ) -> impl Future<Output = ()> {
        async {}
    }

    /// Tool 调用结束
    fn on_tool_end(
        &self,
        _event: &ToolEndTrace,
    ) -> impl Future<Output = ()> {
        async {}
    }

    /// 中断发生
    fn on_interrupt(
        &self,
        _event: &InterruptTrace,
    ) -> impl Future<Output = ()> {
        async {}
    }

    /// 中断恢复
    fn on_resume(
        &self,
        _event: &ResumeTrace,
    ) -> impl Future<Output = ()> {
        async {}
    }

    /// 新一轮开始
    fn on_turn_start(
        &self,
        _event: &TurnStartTrace,
    ) -> impl Future<Output = ()> {
        async {}
    }

    /// 自定义事件（扩展点）
    fn on_custom(
        &self,
        _name: &str,
        _data: &serde_json::Value,
    ) -> impl Future<Output = ()> {
        async {}
    }
}
```

## 3. Trace 事件结构体

```rust
/// Run 开始事件
#[derive(Debug, Clone, Serialize)]
pub struct RunStartTrace {
    pub thread_id: Option<ThreadId>,
    pub run_id: RunId,
    pub model: String,
    pub system_prompt: Option<String>,
    pub input_messages: Vec<Message>,
    pub metadata: Option<serde_json::Value>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Run 结束事件
#[derive(Debug, Clone, Serialize)]
pub struct RunEndTrace {
    pub run_id: RunId,
    pub status: RunStatus,
    pub output_messages: Vec<Message>,
    pub total_turns: usize,
    pub total_prompt_tokens: u32,
    pub total_completion_tokens: u32,
    pub duration: Duration,
    pub error: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub enum RunStatus {
    Completed,
    Interrupted,
    Error,
    MaxTurnsExceeded,
}

/// Model 调用开始
#[derive(Debug, Clone, Serialize)]
pub struct ModelStartTrace {
    pub run_id: RunId,
    pub turn: usize,
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<String>,   // tool names
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Model 调用结束
#[derive(Debug, Clone, Serialize)]
pub struct ModelEndTrace {
    pub run_id: RunId,
    pub turn: usize,
    pub response_text: Option<String>,
    pub tool_calls: Vec<ToolCallTrace>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub duration: Duration,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallTrace {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Tool 调用开始
#[derive(Debug, Clone, Serialize)]
pub struct ToolStartTrace {
    pub run_id: RunId,
    pub turn: usize,
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Tool 调用结束
#[derive(Debug, Clone, Serialize)]
pub struct ToolEndTrace {
    pub run_id: RunId,
    pub turn: usize,
    pub tool_call_id: String,
    pub tool_name: String,
    pub result: Option<String>,
    pub interrupted: bool,
    pub duration: Duration,
    pub error: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// 中断事件
#[derive(Debug, Clone, Serialize)]
pub struct InterruptTrace {
    pub run_id: RunId,
    pub turn: usize,
    pub interrupts: Vec<InterruptInfo>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// 恢复事件
#[derive(Debug, Clone, Serialize)]
pub struct ResumeTrace {
    pub run_id: RunId,
    pub payloads_count: usize,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// 新一轮开始
#[derive(Debug, Clone, Serialize)]
pub struct TurnStartTrace {
    pub run_id: RunId,
    pub turn: usize,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}
```

## 4. AgentLoop 集成

AgentLoop 在关键节点调用 Tracer 方法。Tracer 是可选的（`Option<&dyn DynTracer>`），不配置时零开销：

```rust
// state.rs — AgentLoop::into_stream() 中的追踪调用点

stream! {
    let run_start_time = Instant::now();

    // ── RunStart ──
    if let Some(tracer) = &self.tracer {
        tracer.on_run_start(&RunStartTrace {
            thread_id: self.thread_id.clone(),
            run_id: self.run_id.clone(),
            model: self.model_name.clone(),
            system_prompt: self.system_prompt.clone(),
            input_messages: self.messages.clone(),
            metadata: self.metadata.clone(),
            timestamp: chrono::Utc::now(),
        }).await;
    }
    yield AgentEvent::RunStart { ... };

    for turn in 0..self.max_turns {
        // ── TurnStart ──
        if let Some(tracer) = &self.tracer {
            tracer.on_turn_start(&TurnStartTrace {
                run_id: self.run_id.clone(),
                turn,
                timestamp: chrono::Utc::now(),
            }).await;
        }

        // ── ModelStart ──
        let model_start = Instant::now();
        if let Some(tracer) = &self.tracer {
            tracer.on_model_start(&ModelStartTrace {
                run_id: self.run_id.clone(),
                turn,
                model: self.model_name.clone(),
                messages: messages.clone(),
                tools: self.tools.names(),
                timestamp: chrono::Utc::now(),
            }).await;
        }

        let mut chat_stream = self.model.chat(request).await?;
        // ... consume stream ...

        // ── ModelEnd ──
        if let Some(tracer) = &self.tracer {
            tracer.on_model_end(&ModelEndTrace {
                run_id: self.run_id.clone(),
                turn,
                response_text: accumulated_text.clone(),
                tool_calls: tool_call_traces.clone(),
                prompt_tokens,
                completion_tokens,
                duration: model_start.elapsed(),
                timestamp: chrono::Utc::now(),
            }).await;
        }

        // ── ToolStart / ToolEnd（per tool） ──
        for (tool_call_id, tool_stream_result) in tool_streams {
            let tool_start = Instant::now();
            if let Some(tracer) = &self.tracer {
                tracer.on_tool_start(&ToolStartTrace {
                    run_id: self.run_id.clone(),
                    turn,
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                    timestamp: chrono::Utc::now(),
                }).await;
            }

            // ... consume tool stream ...

            if let Some(tracer) = &self.tracer {
                tracer.on_tool_end(&ToolEndTrace {
                    run_id: self.run_id.clone(),
                    turn,
                    tool_call_id,
                    tool_name: tc.name.clone(),
                    result: last_result.clone(),
                    interrupted: has_interrupt,
                    duration: tool_start.elapsed(),
                    error: tool_error.clone(),
                    timestamp: chrono::Utc::now(),
                }).await;
            }
        }

        // ── Interrupt ──
        if !pending_interrupts.is_empty() {
            if let Some(tracer) = &self.tracer {
                tracer.on_interrupt(&InterruptTrace {
                    run_id: self.run_id.clone(),
                    turn,
                    interrupts: pending_interrupts.clone(),
                    timestamp: chrono::Utc::now(),
                }).await;
            }
        }
    }

    // ── RunEnd ──
    if let Some(tracer) = &self.tracer {
        tracer.on_run_end(&RunEndTrace {
            run_id: self.run_id.clone(),
            status,
            output_messages: messages.clone(),
            total_turns: turn_count,
            total_prompt_tokens,
            total_completion_tokens,
            duration: run_start_time.elapsed(),
            error: final_error.map(|e| e.to_string()),
            timestamp: chrono::Utc::now(),
        }).await;
    }
}
```

## 5. DynTracer — Object-safe 包装

`Tracer` 使用 RPITIT，不 object-safe。提供 `DynTracer` 包装（与 `DynTool` 同模式）：

```rust
/// Object-safe 版本（框架内部使用）
pub(crate) trait DynTracer {
    fn on_run_start(&self, event: &RunStartTrace)
        -> Pin<Box<dyn Future<Output = ()> + '_>>;
    fn on_run_end(&self, event: &RunEndTrace)
        -> Pin<Box<dyn Future<Output = ()> + '_>>;
    fn on_model_start(&self, event: &ModelStartTrace)
        -> Pin<Box<dyn Future<Output = ()> + '_>>;
    fn on_model_end(&self, event: &ModelEndTrace)
        -> Pin<Box<dyn Future<Output = ()> + '_>>;
    fn on_tool_start(&self, event: &ToolStartTrace)
        -> Pin<Box<dyn Future<Output = ()> + '_>>;
    fn on_tool_end(&self, event: &ToolEndTrace)
        -> Pin<Box<dyn Future<Output = ()> + '_>>;
    fn on_interrupt(&self, event: &InterruptTrace)
        -> Pin<Box<dyn Future<Output = ()> + '_>>;
    fn on_resume(&self, event: &ResumeTrace)
        -> Pin<Box<dyn Future<Output = ()> + '_>>;
    fn on_turn_start(&self, event: &TurnStartTrace)
        -> Pin<Box<dyn Future<Output = ()> + '_>>;
    fn on_custom(&self, name: &str, data: &serde_json::Value)
        -> Pin<Box<dyn Future<Output = ()> + '_>>;
}

// blanket impl: Tracer → DynTracer
impl<T: Tracer> DynTracer for T {
    fn on_run_start(&self, event: &RunStartTrace)
        -> Pin<Box<dyn Future<Output = ()> + '_>>
    {
        Box::pin(Tracer::on_run_start(self, event))
    }
    // ... 其余方法同理
}
```

## 6. CompositeTracer — 多 Tracer 组合

```rust
/// 组合多个 Tracer（同时发送到多个后端）
pub struct CompositeTracer {
    tracers: Vec<Box<dyn DynTracer>>,
}

impl CompositeTracer {
    pub fn new() -> Self { Self { tracers: Vec::new() } }

    pub fn add(mut self, tracer: impl Tracer + 'static) -> Self {
        self.tracers.push(Box::new(tracer));
        self
    }
}

impl Tracer for CompositeTracer {
    fn on_run_start(&self, event: &RunStartTrace)
        -> impl Future<Output = ()>
    {
        async move {
            for t in &self.tracers {
                t.on_run_start(event).await;
            }
        }
    }
    // ... 其余方法类似，依次调用所有内部 tracer
}
```

## 7. LangSmithTracer

[LangSmith](https://docs.smith.langchain.com/) 是 LangChain 的追踪平台。通过 REST API 上报追踪数据。

```rust
/// LangSmith 追踪后端
pub struct LangSmithTracer {
    client: reqwest::Client,
    api_key: String,
    api_url: String,         // 默认 "https://api.smith.langchain.com"
    project_name: String,    // LangSmith project 名
    /// 异步上报队列（不阻塞 AgentLoop）
    tx: tokio::sync::mpsc::UnboundedSender<LangSmithPayload>,
}

impl LangSmithTracer {
    pub fn new(api_key: impl Into<String>) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let tracer = Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            api_url: "https://api.smith.langchain.com".into(),
            project_name: "default".into(),
            tx,
        };
        // 启动后台上报任务
        tokio::spawn(Self::background_sender(
            tracer.client.clone(),
            tracer.api_key.clone(),
            tracer.api_url.clone(),
            rx,
        ));
        tracer
    }

    pub fn with_project(mut self, name: impl Into<String>) -> Self {
        self.project_name = name.into(); self
    }

    pub fn with_api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = url.into(); self
    }

    /// 也可从 AgentConfig 构造
    pub fn from_config(config: &AgentConfig) -> Option<Self> {
        let api_key = config.extra.get("langsmith_api_key")?.as_str()?;
        let mut tracer = Self::new(api_key);
        if let Some(project) = config.extra.get("langsmith_project").and_then(|v| v.as_str()) {
            tracer = tracer.with_project(project);
        }
        if let Some(url) = config.extra.get("langsmith_api_url").and_then(|v| v.as_str()) {
            tracer = tracer.with_api_url(url);
        }
        Some(tracer)
    }

    async fn background_sender(
        client: reqwest::Client,
        api_key: String,
        api_url: String,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<LangSmithPayload>,
    ) {
        while let Some(payload) = rx.recv().await {
            // 批量上报：收集 buffer 中的 payloads，合并发送
            let _ = client
                .post(format!("{api_url}/runs"))
                .header("x-api-key", &api_key)
                .json(&payload)
                .send()
                .await;
        }
    }
}
```

### LangSmith 数据模型映射

LangSmith 使用 Run 树结构，每个 Run 有 parent_run_id：

```
LangSmith Run 树
├── Chain Run (= AgentLoop Run)
│   ├── LLM Run (= model call, turn 0)
│   │   └─ inputs: messages, outputs: response
│   ├── Tool Run (= tool execution, interrupted)
│   │   └─ inputs: args, outputs: {interrupted: true}
│   ├──── interrupt ──── resume ────   ← RunId 不变
│   ├── LLM Run (= model call, turn 1) ← resume 后继续追加子 Run
│   └── ...
```

#### Interrupt/Resume 处理

LangSmithTracer 在 interrupt/resume 场景下：

1. **`on_interrupt()`**——发送 PATCH 更新 Chain Run（同一 `id`），设置 `status: "interrupted"`，记录 interrupt 详情到 `extra.interrupts`
2. **`on_run_end(Interrupted)`**——PATCH Chain Run `end_time`，但 `status` 标记为 `"interrupted"`（非最终态）
3. **`on_resume()`**——PATCH Chain Run，清除 `end_time`，恢复 `status: null`（表示运行中），记录 `extra.resume_time`
4. **后续 LLM/Tool Run**——使用同一个 `parent_run_id`（= 原始 Chain Run ID），追加到同一棵 Run 树
5. **`on_run_end(Completed)`**——最终 PATCH Chain Run，设置 `status: "success"` 和最终 `end_time`

这保证 LangSmith UI 中 interrupt/resume 的整个生命周期显示为**一棵 Run 树**，而非两个独立 Chain Run。

```rust
/// LangSmith API payload
#[derive(Debug, Serialize)]
struct LangSmithPayload {
    /// Run ID（UUID）
    id: String,
    /// 父 Run ID（Chain Run 的子节点指向父）
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_run_id: Option<String>,
    /// "chain" | "llm" | "tool"
    run_type: String,
    /// Run 名称
    name: String,
    /// 输入
    inputs: serde_json::Value,
    /// 输出（Run 结束时才有）
    #[serde(skip_serializing_if = "Option::is_none")]
    outputs: Option<serde_json::Value>,
    /// 开始时间
    start_time: String,    // ISO 8601
    /// 结束时间
    #[serde(skip_serializing_if = "Option::is_none")]
    end_time: Option<String>,
    /// 额外信息
    #[serde(skip_serializing_if = "Option::is_none")]
    extra: Option<serde_json::Value>,
    /// Session/Project 名称
    session_name: String,
    /// 状态
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,   // "success" | "error"
    /// 错误信息
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// 用户自定义 metadata（透传）
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
    /// token 用量
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_metadata: Option<serde_json::Value>,
}
```

### Tracer 实现

```rust
impl Tracer for LangSmithTracer {
    fn on_run_start(&self, event: &RunStartTrace)
        -> impl Future<Output = ()>
    {
        async move {
            let _ = self.tx.send(LangSmithPayload {
                id: event.run_id.0.clone(),
                parent_run_id: None,
                run_type: "chain".into(),
                name: "AgentLoop".into(),
                inputs: serde_json::json!({
                    "messages": event.input_messages,
                    "model": event.model,
                }),
                outputs: None,
                start_time: event.timestamp.to_rfc3339(),
                end_time: None,
                extra: Some(serde_json::json!({
                    "system_prompt": event.system_prompt,
                    "thread_id": event.thread_id,
                })),
                session_name: self.project_name.clone(),
                status: None,
                error: None,
                metadata: event.metadata.clone(),
                usage_metadata: None,
            });
        }
    }

    fn on_run_end(&self, event: &RunEndTrace)
        -> impl Future<Output = ()>
    {
        async move {
            let _ = self.tx.send(LangSmithPayload {
                id: event.run_id.0.clone(),
                parent_run_id: None,
                run_type: "chain".into(),
                name: "AgentLoop".into(),
                inputs: serde_json::json!({}),
                outputs: Some(serde_json::json!({
                    "messages": event.output_messages,
                    "status": event.status,
                })),
                start_time: String::new(),  // update only
                end_time: Some(event.timestamp.to_rfc3339()),
                extra: None,
                session_name: self.project_name.clone(),
                status: Some(match &event.status {
                    RunStatus::Completed => "success",
                    RunStatus::Error => "error",
                    _ => "success",
                }.into()),
                error: event.error.clone(),
                metadata: None,
                usage_metadata: Some(serde_json::json!({
                    "total_tokens": event.total_prompt_tokens + event.total_completion_tokens,
                    "prompt_tokens": event.total_prompt_tokens,
                    "completion_tokens": event.total_completion_tokens,
                })),
            });
        }
    }

    fn on_model_start(&self, event: &ModelStartTrace)
        -> impl Future<Output = ()>
    {
        async move {
            let llm_run_id = format!("{}-llm-{}", event.run_id.0, event.turn);
            let _ = self.tx.send(LangSmithPayload {
                id: llm_run_id,
                parent_run_id: Some(event.run_id.0.clone()),
                run_type: "llm".into(),
                name: event.model.clone(),
                inputs: serde_json::json!({
                    "messages": event.messages,
                    "tools": event.tools,
                }),
                outputs: None,
                start_time: event.timestamp.to_rfc3339(),
                end_time: None,
                extra: None,
                session_name: self.project_name.clone(),
                status: None,
                error: None,
                metadata: None,
                usage_metadata: None,
            });
        }
    }

    fn on_model_end(&self, event: &ModelEndTrace)
        -> impl Future<Output = ()>
    {
        async move {
            let llm_run_id = format!("{}-llm-{}", event.run_id.0, event.turn);
            let _ = self.tx.send(LangSmithPayload {
                id: llm_run_id,
                parent_run_id: Some(event.run_id.0.clone()),
                run_type: "llm".into(),
                name: String::new(),
                inputs: serde_json::json!({}),
                outputs: Some(serde_json::json!({
                    "response": event.response_text,
                    "tool_calls": event.tool_calls,
                })),
                start_time: String::new(),
                end_time: Some(event.timestamp.to_rfc3339()),
                extra: None,
                session_name: self.project_name.clone(),
                status: Some("success".into()),
                error: None,
                metadata: None,
                usage_metadata: Some(serde_json::json!({
                    "prompt_tokens": event.prompt_tokens,
                    "completion_tokens": event.completion_tokens,
                    "total_tokens": event.prompt_tokens + event.completion_tokens,
                })),
            });
        }
    }

    fn on_tool_start(&self, event: &ToolStartTrace)
        -> impl Future<Output = ()>
    {
        async move {
            let _ = self.tx.send(LangSmithPayload {
                id: event.tool_call_id.clone(),
                parent_run_id: Some(event.run_id.0.clone()),
                run_type: "tool".into(),
                name: event.tool_name.clone(),
                inputs: serde_json::json!({
                    "arguments": event.arguments,
                }),
                outputs: None,
                start_time: event.timestamp.to_rfc3339(),
                end_time: None,
                extra: None,
                session_name: self.project_name.clone(),
                status: None,
                error: None,
                metadata: None,
                usage_metadata: None,
            });
        }
    }

    fn on_tool_end(&self, event: &ToolEndTrace)
        -> impl Future<Output = ()>
    {
        async move {
            let _ = self.tx.send(LangSmithPayload {
                id: event.tool_call_id.clone(),
                parent_run_id: Some(event.run_id.0.clone()),
                run_type: "tool".into(),
                name: event.tool_name.clone(),
                inputs: serde_json::json!({}),
                outputs: Some(serde_json::json!({
                    "result": event.result,
                    "interrupted": event.interrupted,
                })),
                start_time: String::new(),
                end_time: Some(event.timestamp.to_rfc3339()),
                extra: None,
                session_name: self.project_name.clone(),
                status: Some(if event.error.is_some() { "error" } else { "success" }.into()),
                error: event.error.clone(),
                metadata: None,
                usage_metadata: None,
            });
        }
    }
}
```

## 8. AgentBuilder 集成

```rust
impl<M: ChatModel> AgentBuilder<M> {
    /// 注入追踪器
    pub fn tracer(mut self, tracer: impl Tracer + 'static) -> Self {
        self.tracer = Some(Box::new(tracer));
        self
    }
}

pub struct BuiltAgent<M: ChatModel, S = NoStore> {
    model: M,
    store: S,
    config: AgentConfig,
    tracer: Option<Box<dyn DynTracer>>,   // 可选
    system_prompt: String,
    tools: ToolRegistry,
    max_turns: usize,
}
```

## 9. TracingLayer 适配器

作为 Layer 实现，可以无侵入地为任意 Agent 添加追踪：

```rust
/// 为任意 Agent 添加 tracing——作为 Layer 使用
pub struct TracingLayer<T: Tracer> {
    tracer: T,
}

impl<T: Tracer> TracingLayer<T> {
    pub fn new(tracer: T) -> Self { Self { tracer } }
}

impl<T: Tracer + Clone + 'static> Layer for TracingLayer<T> {
    type Inner = /* any Agent */;
    // 包装 chat() 调用，在前后插入 tracer 调用

    // TracingLayer 主要用于 ProtocolAgent 或 BuiltAgent，
    // 在 HTTP Server / WASM Host 边界添加追踪
}

// 便捷方法
impl<A: Agent> AgentExt for A {
    /// 添加 tracing
    fn with_tracing<T: Tracer + Clone + 'static>(self, tracer: T) -> TracedAgent<Self, T> {
        TracedAgent { inner: self, tracer }
    }
}

pub struct TracedAgent<A, T> {
    inner: A,
    tracer: T,
}

impl<A: Agent, T: Tracer> Agent for TracedAgent<A, T> {
    type Request = A::Request;
    type Response = A::Response;
    type Error = A::Error;

    fn chat(&self, req: A::Request)
        -> impl Future<Output = Result<impl Stream<Item = A::Response>, A::Error>>
    {
        async move {
            // tracer.on_custom("agent_call_start", ...).await;
            let stream = self.inner.chat(req).await?;
            // 返回包装 stream，在每个 yield 前调用 tracer
            Ok(stream)
        }
    }
}
```

## 10. 使用示例

### 10.1 LangSmith 追踪

```rust
use remi_agentloop::prelude::*;
use remi_agentloop::tracing::LangSmithTracer;

#[tokio::main]
async fn main() -> Result<(), AgentError> {
    let config = AgentConfig::from_env();

    let tracer = LangSmithTracer::new(
        std::env::var("LANGSMITH_API_KEY").unwrap()
    ).with_project("my-agent-v1");

    let agent = AgentBuilder::new()
        .model(OpenAIClient::from_config(&config))
        .config(config)
        .system("You are helpful.")
        .tool(SearchTool)
        .tracer(tracer)
        .build();

    let mut stream = agent.chat("What's new in Rust?".into()).await?;
    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::TextDelta(s) => print!("{s}"),
            AgentEvent::Done => println!(),
            _ => {}
        }
    }
    // 追踪数据已异步上报到 LangSmith
    Ok(())
}
```

### 10.2 多后端追踪

```rust
use remi_agentloop::tracing::{CompositeTracer, LangSmithTracer, StdoutTracer};

let tracer = CompositeTracer::new()
    .add(LangSmithTracer::new("ls-key").with_project("prod"))
    .add(StdoutTracer::new());  // 同时输出到控制台

let agent = AgentBuilder::new()
    .model(model)
    .tracer(tracer)
    .build();
```

### 10.3 自定义 Tracer

```rust
/// 仅记录 token 用量到数据库
struct UsageTracer {
    db: DatabasePool,
}

impl Tracer for UsageTracer {
    fn on_model_end(&self, event: &ModelEndTrace)
        -> impl Future<Output = ()>
    {
        async move {
            self.db.insert_usage(
                &event.run_id.0,
                event.prompt_tokens,
                event.completion_tokens,
            ).await.ok();
        }
    }
    // 其余方法使用默认空实现
}
```

### 10.4 从 AgentConfig 自动配置

```rust
let config = AgentConfig::new()
    .with_api_key("sk-...")
    .with_extra(serde_json::json!({
        "langsmith_api_key": "ls-...",
        "langsmith_project": "prod-agents",
    }));

// 自动从 config.extra 创建 LangSmithTracer
let tracer = LangSmithTracer::from_config(&config);
if let Some(tracer) = tracer {
    builder = builder.tracer(tracer);
}
```

## 11. StdoutTracer（调试用）

```rust
/// 将追踪事件输出到 stdout（开发调试用）
pub struct StdoutTracer {
    verbose: bool,
}

impl StdoutTracer {
    pub fn new() -> Self { Self { verbose: false } }
    pub fn verbose(mut self) -> Self { self.verbose = true; self }
}

impl Tracer for StdoutTracer {
    fn on_run_start(&self, event: &RunStartTrace)
        -> impl Future<Output = ()>
    {
        async move {
            eprintln!("[TRACE] Run started: {} (model: {})", event.run_id, event.model);
        }
    }

    fn on_run_end(&self, event: &RunEndTrace)
        -> impl Future<Output = ()>
    {
        async move {
            eprintln!(
                "[TRACE] Run ended: {} ({:?}, {}ms, tokens: {}+{})",
                event.run_id, event.status,
                event.duration.as_millis(),
                event.total_prompt_tokens, event.total_completion_tokens,
            );
        }
    }

    fn on_model_start(&self, event: &ModelStartTrace)
        -> impl Future<Output = ()>
    {
        async move {
            eprintln!("[TRACE]   Model call: {} (turn {})", event.model, event.turn);
        }
    }

    fn on_model_end(&self, event: &ModelEndTrace)
        -> impl Future<Output = ()>
    {
        async move {
            eprintln!(
                "[TRACE]   Model done: turn {} ({}ms, tokens: {}+{})",
                event.turn, event.duration.as_millis(),
                event.prompt_tokens, event.completion_tokens,
            );
        }
    }

    fn on_tool_start(&self, event: &ToolStartTrace)
        -> impl Future<Output = ()>
    {
        async move {
            eprintln!("[TRACE]   Tool start: {} ({})", event.tool_name, event.tool_call_id);
        }
    }

    fn on_tool_end(&self, event: &ToolEndTrace)
        -> impl Future<Output = ()>
    {
        async move {
            eprintln!(
                "[TRACE]   Tool done: {} ({}ms, interrupted: {})",
                event.tool_name, event.duration.as_millis(), event.interrupted,
            );
        }
    }
}
```

## 12. 模块结构更新

```
src/
├── tracing/
│   ├── mod.rs          # Tracer trait, DynTracer, CompositeTracer, trace event structs
│   ├── langsmith.rs    # LangSmithTracer  [feature: tracing-langsmith]
│   └── stdout.rs       # StdoutTracer（始终可用）
└── ...
```

### Feature flags

```toml
[features]
tracing-langsmith = ["dep:reqwest", "dep:chrono"]
```

`Tracer` trait 和 `StdoutTracer` 始终可用（零额外依赖）。`LangSmithTracer` 需要 `reqwest` + `chrono`，通过 feature flag 启用。
