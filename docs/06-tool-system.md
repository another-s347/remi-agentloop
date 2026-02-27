# Tool 系统

> Tool trait（流式返回 + interrupt/resume）、ToolDefinition、DynTool、ToolRegistry、InterruptId

## 核心概念

1. **Tool 返回 `ToolResult<S>`**——参考 Rust `Result<T, E>` 设计，Tool 执行结果在类型层面分离为两种情况：`ToolResult::Output(Stream)` 表示正常流式执行，`ToolResult::Interrupt(InterruptRequest)` 表示中断请求。**同一次 Tool 调用要么返回 stream，要么返回 interrupt，不可混合**。
2. **并行 Tool Calling**——多个 tool call 可以并发执行（`futures::join_all`），每个 tool 独立返回 `ToolResult`。
3. **Interrupt / Resume**——Tool 返回 `ToolResult::Interrupt(InterruptRequest)` 时，携带 `InterruptId` 和上下文数据，暂停当前 AgentLoop。**调用方可以是人工操作者，也可以是上层应用的自动化逻辑**（如规则引擎、审批策略、外部系统回调等），处理后通过 `resume(Vec<ResumePayload>)` 恢复执行。多个并行 tool 可以各自 interrupt，resume 时要求**一次性提供全部 interrupt 的结果**。

> **设计意图**：Interrupt 机制不限于 human-in-the-loop。`kind` 字段是语义化标签，上层应用可以根据 `kind` 自动路由到对应的处理器（`InterruptHandler`），实现全自动或半自动的中断处理流水线。
> 
> **类型设计动机**：旧设计中 `ToolOutput` 同时包含 `Delta`、`Result`、`Interrupt` 三种 variant，允许 stream 内混合 interrupt，语义模糊且 AgentLoop 处理复杂。新设计参考 `Result<T, E>` 将 "执行流"（stream）与 "中断请求"（interrupt）在类型层面彻底分离，消除歧义，简化状态机。

## InterruptId (types.rs)

```rust
/// 中断标识符——一个 tool 的一次 interrupt
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InterruptId(pub String);

impl InterruptId {
    pub fn new() -> Self { Self(uuid_v4()) }
}
```

## ToolOutput (tool/mod.rs)

```rust
/// Tool 执行的流式输出项——仅包含正常执行的增量和结果
#[derive(Debug, Clone)]
pub enum ToolOutput {
    /// 进度/增量文本（可选，用于流式展示 tool 执行过程）
    Delta(String),

    /// 工具执行最终结果（stream 中最后一项）
    Result(String),
}
```

## InterruptRequest (tool/mod.rs)

```rust
/// 中断请求——Tool 用此表示需要外部输入（人工审批、策略检查等）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptRequest {
    /// 中断标识符
    pub interrupt_id: InterruptId,
    /// 中断类型（语义化标签）
    /// 常见值："human_approval", "payment_confirm", "policy_check", "rate_limit_wait"
    /// 上层应用可根据 kind 自动路由到对应的 InterruptHandler
    pub kind: String,
    /// 传递给调用方的上下文（JSON，描述中断原因/所需信息）
    pub data: serde_json::Value,
}
```

## ToolResult (tool/mod.rs)

参考 Rust `Result<T, E>` 的设计，`ToolResult<S>` 在类型层面将 Tool 的正常执行和中断请求分离：

```rust
/// Tool 执行结果——要么是流式输出，要么是中断请求，不可混合
///
/// 类似 Rust `Result<T, E>`：
/// - `Output(S)` ≈ `Ok(T)`  —— 正常执行，消费 stream 获取 Delta / Result
/// - `Interrupt(InterruptRequest)` ≈ `Err(E)` —— 请求外部介入，无 stream
pub enum ToolResult<S> {
    /// 正常执行——返回流式输出（Stream<Item = ToolOutput>）
    Output(S),
    /// 中断请求——暂停 AgentLoop，等待外部输入
    Interrupt(InterruptRequest),
}

impl<S> ToolResult<S> {
    /// 是否为正常输出
    pub fn is_output(&self) -> bool {
        matches!(self, Self::Output(_))
    }

    /// 是否为中断请求
    pub fn is_interrupt(&self) -> bool {
        matches!(self, Self::Interrupt(_))
    }

    /// 转换 stream 类型（map over Output variant）
    pub fn map_stream<S2>(self, f: impl FnOnce(S) -> S2) -> ToolResult<S2> {
        match self {
            Self::Output(s) => ToolResult::Output(f(s)),
            Self::Interrupt(req) => ToolResult::Interrupt(req),
        }
    }
}
```

## Tool Trait

```rust
pub trait Tool {
    /// 工具名称（函数名）
    fn name(&self) -> &str;

    /// 工具描述
    fn description(&self) -> &str;

    /// JSON Schema 参数定义
    fn parameters_schema(&self) -> serde_json::Value;

    /// 执行工具——返回 ToolResult：
    /// - `ToolResult::Output(stream)` → 正常执行，stream yield Delta / Result
    /// - `ToolResult::Interrupt(req)` → 请求中断，无 stream
    fn execute(
        &self,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>;
}
```

### 简单 Tool 实现示例

```rust
struct SearchTool;

impl Tool for SearchTool {
    fn name(&self) -> &str { "web_search" }
    fn description(&self) -> &str { "Search the web" }
    fn parameters_schema(&self) -> serde_json::Value { /* ... */ }

    fn execute(&self, args: serde_json::Value)
        -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        async move {
            // 正常执行——返回 ToolResult::Output 包装的 stream
            Ok(ToolResult::Output(stream! {
                yield ToolOutput::Delta("Searching...".into());
                let results = do_search(args["query"].as_str().unwrap()).await;
                yield ToolOutput::Result(results);
            }))
        }
    }
}
```

### 带 Interrupt 的 Tool 实现示例

```rust
struct PaymentTool;

impl Tool for PaymentTool {
    fn name(&self) -> &str { "process_payment" }
    fn description(&self) -> &str { "Process a payment, requires approval" }
    fn parameters_schema(&self) -> serde_json::Value { /* ... */ }

    fn execute(&self, args: serde_json::Value)
        -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        async move {
            let amount = args["amount"].as_f64().unwrap();

            if amount > 100.0 {
                // 需要审批——直接返回 ToolResult::Interrupt，无 stream
                return Ok(ToolResult::Interrupt(InterruptRequest {
                    interrupt_id: InterruptId::new(),
                    kind: "human_approval".into(),
                    data: serde_json::json!({
                        "amount": amount,
                        "description": "Payment requires approval"
                    }),
                }));
            }

            // 小额直接处理——返回 ToolResult::Output stream
            Ok(ToolResult::Output(stream! {
                yield ToolOutput::Delta(format!("Processing payment of ${amount}..."));
                let result = process_payment(amount).await;
                yield ToolOutput::Result(result);
            }))
        }
    }
}
```

> **注意**：`ToolResult::Interrupt` 不包含 stream——中断请求是一个独立的返回路径。resume 时 AgentLoop 将 resume 结果直接作为该 tool 的最终 result 注入 messages。见 [07-agent-loop.md](07-agent-loop.md) 中的详细流程。
>
> 类比 `Result<T, E>`：`ToolResult::Output(stream)` 类似 `Ok(value)`，`ToolResult::Interrupt(req)` 类似 `Err(reason)`。Tool 在 execute 内部可以根据参数、业务逻辑决定走哪条路径。

## ResumePayload (types.rs)

```rust
/// 恢复中断时传入的数据——一个 interrupt 对应一个 payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumePayload {
    /// 对应的中断 ID
    pub interrupt_id: InterruptId,
    /// 调用方提供的结果（JSON）
    pub result: serde_json::Value,
}
```

## ToolDefinition（发送给 LLM 的描述）

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,  // "function"
    pub function: FunctionDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}
```

从 `Tool` trait 自动生成：

```rust
pub fn tool_to_definition(tool: &dyn DynTool) -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".into(),
        function: FunctionDefinition {
            name: tool.name().into(),
            description: tool.description().into(),
            parameters: tool.parameters_schema(),
        },
    }
}
```

## DynTool + ToolRegistry

由于 `Tool` trait 使用 RPITIT（`execute()` 返回 `impl Future<..ToolResult<impl Stream>>`），不 object-safe。提供 `DynTool` 包装：

```rust
/// Boxed stream 类型别名
type BoxedToolStream<'a> = Pin<Box<dyn Stream<Item = ToolOutput> + 'a>>;

/// Boxed ToolResult 类型别名
type BoxedToolResult<'a> = ToolResult<BoxedToolStream<'a>>;

/// Object-safe 版本（内部使用）
pub(crate) trait DynTool {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn execute_boxed(
        &self,
        arguments: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<BoxedToolResult<'_>, AgentError>> + '_>>;
}

// blanket impl: Tool → DynTool
impl<T: Tool> DynTool for T {
    // ...将 impl Future<..ToolResult<impl Stream>> 包装为
    // Pin<Box<..ToolResult<Pin<Box<..>>>>>
    // 使用 ToolResult::map_stream(|s| Box::pin(s)) 进行转换
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn DynTool>>,
}

impl ToolRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, tool: impl Tool + 'static);
    pub fn get(&self, name: &str) -> Option<&dyn DynTool>;
    pub fn definitions(&self) -> Vec<ToolDefinition>;

    /// 并行执行多个 tool calls，返回每个的 ToolResult
    pub async fn execute_parallel(
        &self,
        calls: &[ParsedToolCall],
    ) -> Vec<(String, Result<BoxedToolResult<'_>, AgentError>)> {
        let futs: Vec<_> = calls.iter().map(|tc| {
            let tool = self.get(&tc.name);
            async move {
                match tool {
                    Some(t) => (tc.id.clone(), t.execute_boxed(tc.arguments.clone()).await),
                    None => (tc.id.clone(), Err(AgentError::ToolNotFound(tc.name.clone()))),
                }
            }
        }).collect();
        futures::future::join_all(futs).await
    }
}
```

`DynTool` 是框架内部实现细节（`pub(crate)`），用户只需实现 `Tool` trait。`ToolRegistry` 通过 blanket impl 自动将 `Tool` 转换为 `DynTool` 存储。`ToolResult::map_stream()` 用于在 blanket impl 中将 `impl Stream` 转换为 `Pin<Box<dyn Stream>>`。

## `#[tool]` 宏 + 内置 Tool

手动实现 `Tool` trait 存在大量 boilerplate。框架提供 `#[tool]` 过程宏，从函数签名自动生成 `Tool` impl（doc comment → description，参数类型 → JSON Schema）。同时内置 `BashTool`（shell 命令执行）、`FsTool`（物理文件系统）、`VirtualFsTool`（内存虚拟文件系统）等常用 Tool。

详见 [15-tool-macro-builtins.md](15-tool-macro-builtins.md)。
