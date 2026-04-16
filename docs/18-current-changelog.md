# 当前改动与设计 Changelog

> 面向当前工作树的汇总说明。目标不是逐文件罗列 diff，而是把这批改动背后的能力边界、公共 API 变化、设计收敛方向和未完成项说明白。

## 1. 总览

当前仓库的改动可以归纳为五条主线：

1. **调用上下文被显式提升为 `ChatCtx`**
2. **请求面与内部状态面进一步解耦**
3. **外部工具执行能力被收敛为统一、可叠加的 `ToolLayer`**
4. **tool 定义与参数 schema 生成改为 typed-first**
5. **subagent / tracing / cancellation / eval 开始形成完整闭环**

换句话说，这一轮不是零散修补，而是在把框架从“能跑的 agent loop”继续收敛成“调用链、工具链、追踪链、恢复链都能统一表达”的基础设施。

---

## 2. `ChatCtx` 成为一级抽象

### 2.1 `Agent::chat` 现在显式接收 `ChatCtx`

核心 trait 从：

```rust
async fn chat(&self, req: Request) -> Result<Stream<Response>, Error>
```

演进为：

```rust
async fn chat(&self, ctx: ChatCtx, req: Request) -> Result<Stream<Response>, Error>
```

设计含义：

- `req` 只表达“这一步要做什么”
- `ctx` 表达“这次调用沿整条链路共享的上下文”

这解决了之前几个问题：

- tracing 的 parent-child 关系没有统一承载面
- cancellation 只能做外围打断，不能自然传播到 model / tool / subagent
- tool 共享状态、metadata、thread/run 标识分散在不同结构里
- 子 agent 想继承父调用链信息时，只能靠额外约定

### 2.2 `ChatCtx` 承载的东西

当前 `ChatCtx` 负责：

- `thread_id`
- `run_id`
- `metadata`
- `user_state`
- active tool call chain
- tracing span 节点
- runtime cancellation token

这意味着：

- tool 不再需要额外的 `ToolContext`
- subagent 可以直接通过 `ctx.fork()` / `ctx.fork_for_tool(...)` 继承 lineage
- transport / model / outer layer 都能共享同一取消与追踪语义

### 2.3 `ToolContext` 被移除，tool 统一使用 `ChatCtx`

`Tool::execute(...)` 现在的上下文参数是：

```rust
async fn execute(
    &self,
    arguments: serde_json::Value,
    resume: Option<ResumePayload>,
    ctx: ChatCtx,
) -> Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>
```

这让 tool 的能力边界更统一：

- 读写共享 user_state
- 读取 metadata
- 感知取消
- 派生子 span
- 把 tool call chain 继续传给内部子调用

### 2.4 新增 `SpanId` / `SpanNode` / `SpanKind`

Tracing 不再只靠 `RunId + turn + tool_call_id` 做派生，而是显式引入 span tree：

- `SpanId`
- `SpanNode`
- `SpanKind::{Run, Model, Tool, Subagent, Custom}`

这让 tracing 从“事件日志”提升为“结构化调用树”。后续无论对接 LangSmith、OpenTelemetry 风格后端，还是做自定义 replay，可表达性都明显更好。

---

## 3. 请求面与状态面继续解耦

### 3.1 `ChatRequest` 与 `ModelRequest` 分家

原本模型层和顶层请求层容易混在一起。现在：

- 顶层交互请求是 `ChatRequest` / `LoopInput`
- 发给底层模型的是 `ModelRequest`

设计上更清楚了：

- `ModelRequest` 只服务于模型调用
- `LoopInput` 服务于 agent trajectory

### 3.2 `LoopInput::Start` 改为直接接收 `Message`

这一点很关键。启动输入现在不是一段裸 `content`，而是完整的：

```rust
LoopInput::Start {
    message: Message,
    history: Vec<Message>,
    extra_tools: Vec<ToolDefinition>,
    model: Option<String>,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    metadata: Option<Value>,
}
```

好处：

- `name` / `metadata` / multimodal content 都成为 message 自身属性
- 不需要再额外拆出 `message_metadata` / `user_name`
- 应用层可以构造真正完整的用户输入，而不是被框架二次翻译

### 3.3 `LoopInput::Resume` 增加 `pending_interrupts`

resume 输入现在显式带上：

- `state`
- `pending_interrupts`
- `results`

这让 resume 不只是“把 tool result 塞回去”，而是能更明确地表达恢复现场。对多 interrupt、嵌套 interrupt、跨 layer resume 都是必要铺垫。

### 3.4 `ChatInput` 简化

高层 `ChatInput::Message` 现在也改为直接持有 `Message`。这和 `LoopInput` 保持一致，减少了两层 API 之间的心智切换。

### 3.5 `Action::UserContent` 被消掉，统一成 `Action::UserMessage(Message)`

在 `state::step()` 这一层，启动动作不再区分：

- 纯文本 user message
- 富内容 user content

它们都统一成完整 `Message`。这降低了 step 层的概念数量，也让 message 的 identity 与 metadata 能穿透得更完整。

---

## 4. 取消、恢复与 runtime 传播被真正接通

### 4.1 step / loop / model / transport 都开始感知 cancellation

这轮不只是加了一个取消标记，而是把 cancellation 传播到了关键路径：

- `step()` 在拉取 model stream 时检查 `ctx.is_cancelled()`
- `AgentLoop::run()` 会在 tool 批次、step 途中、resume 前后处理取消
- `OpenAIClient` 的 SSE 读取会检查 `ctx.is_cancelled()`
- HTTP SSE client 也会感知取消
- subagent 通过 `ctx.fork()` 继承取消传播

结果是：取消不再只是“外面不读流了”，而是框架内部也能收敛出 `Cancelled` 语义。

### 4.2 `AgentEvent::Cancelled` 成为一等事件

配套变化：

- `RunStatus` 新增 `Cancelled`
- `BuiltAgent` / TUI / protocol / tracing 都能处理 cancelled 路径
- cancel 会生成对应 checkpoint / run end 语义

这让“暂停等待恢复”和“明确取消本轮执行”终于不是同一种东西。

### 4.3 `ChatCtx` 带 runtime sidecar

`ChatCtx` 现在可序列化，但 cancellation token 等 runtime-only 内容不会序列化。这个设计是对的：

- 可恢复的只有语义状态
- 不可恢复的 runtime handle 不应该进 checkpoint / wire format

这也是后续把 `ChatCtx` 带到跨进程 / 跨 wasm host 调用链里的前提。

---

## 5. Tracing 从扁平事件升级为结构化 span 链

### 5.1 trace 结构体统一带 `span`

`RunStartTrace` / `RunEndTrace` / `ModelStartTrace` / `ToolStartTrace` / `ResumeTrace` 等，都开始携带 `SpanNode`。

这带来几个变化：

- tracer 不必自行猜 parent-child 关系
- subagent trace 可以挂到父 tool span 下
- tool / model / run 的 identity 不再依赖各 tracer 自己派生 UUID

### 5.2 LangSmith tracer 改为基于 span 写入

之前 LangSmith 侧主要靠 `run_id`、`turn`、`tool_call_id` 派生子 run ID。现在改成：

- `id = event.span.span_id`
- `parent_run_id = span.parent`
- `run_type` 从 `SpanKind` 推导

这样 LangSmith 里的层级结构会更稳定，也能自然表达 subagent span。

### 5.3 subagent 事件开始可被父 tracer 转发还原

`AgentLoop` 里新增了 forwarded subagent trace state，用来把 subagent tool 内部转发上来的 custom event 重新拼成：

- subagent run start/end
- subagent model start/end
- subagent tool start/end
- subagent interrupt

这意味着“父 agent 调了一个 task tool，里面又跑了一个 agent”这条链，现在在 tracing 上开始是可见的，而不是黑盒字符串。

---

## 6. `ToolRegistry` 与外部工具层完成第一次统一

### 6.1 新增 core `tool/external.rs`

`remi-agentloop-core` 新增了通用外层工具执行实现：

- `ExternalToolLayer`
- `ExternalToolAgent`
- `AgentEventEnvelope`
- `ExternalToolHook`

它解决的是“工具不属于 inner loop，而属于外层 agent wrapper”这一模式。

### 6.2 统一公开 API：`ToolRegistry::into_layer()`

更重要的是，公共 API 没有继续暴露两套概念，而是统一收敛成：

- `ToolRegistry`
- `ToolLayer`
- `ToolLayerAgent`
- `ToolLayerHook`
- `ToolRegistry::into_layer()`
- `ToolRegistry::into_layer_with_hook(...)`

设计结论已经很明确：

- inner local tools 用 `ToolRegistry`
- outer composable tools 也从 `ToolRegistry` 派生

没有“external tool registry”这一独立一级概念了。

### 6.3 `DefaultToolRegistry::tool(...)` 支持 builder-style 叠加

这为 layer composition 提供了自然写法：

```rust
let audit = DefaultToolRegistry::new()
    .tool(AuditTool)
    .into_layer();
```

### 6.4 deepagent 的 todo / skill 已迁移到统一工具层

`TodoLayer` 和 `SkillLayer` 已经不再自己维护一套 `NeedToolExecution -> execute -> resume` 的流拦截逻辑，而是改成：

- 构建 registry
- `into_layer_with_hook(...)`
- 用 hook 发出 domain event

这说明新的统一抽象已经能承接真实业务 wrapper，不只是 demo。

### 6.5 仍未完全统一的部分

`remi-agentloop-deepagent/src/agent.rs` 顶层 `DeepAgent` 仍保留一段自定义 local tool 驱动逻辑。

所以当前状态不是“全部彻底统一”，而是：

- **todo / skill wrapper 已统一**
- **deepagent 顶层 orchestration 还没完全用 `ToolLayer` 重写**

这应视为下一轮清理项，而不是这轮已经完全完成的事情。

---

## 7. Tool 系统开始走 typed-first 路线

### 7.1 core 暴露 `schemars` / `serde`

`remi-agentloop-core` 和 facade crate 开始 re-export：

- `schemars`
- `serde`

这是为 proc macro 和 typed schema 生态做准备。

### 7.2 新增 `schema_for_type<T>()` 与 `parse_arguments<T>()`

core tool 模块现在提供两个关键 helper：

- `schema_for_type<T: JsonSchema>()`
- `parse_arguments<T: DeserializeOwned>()`

这意味着 tool 参数 schema 不再鼓励手写 JSON object，而是：

- Rust 类型定义参数
- 自动导出 schema
- 自动反序列化参数

### 7.3 `#[tool]` 宏切到 typed args 结构生成

宏层的变化很重要：

- 为每个 tool 函数生成一个 `Args` struct
- 自动派生 `Deserialize + JsonSchema`
- 对 `&str` 等参数做 binding 适配
- `execute()` 内直接走 `parse_arguments(...)`

结果：

- schema 和实际执行参数共用同一套 Rust 类型
- 减少手写 schema 与运行时参数解析不一致的风险
- Optional 字段能更自然地映射成 schema required/optional

### 7.4 内建工具开始迁移到 typed schema

已经明显迁移的一批包括：

- todo 工具
- bash 工具
- 若干 deepagent/tool crate 工具

这轮改动说明 typed-first 不是试验，而是新的默认方向。

---

## 8. Tool 输出面变宽：支持 `Custom`

### 8.1 `ToolOutput::Custom`

tool 输出从：

- `Delta`
- `Result`

扩展为：

- `Delta`
- `Custom { event_type, extra }`
- `Result`

### 8.2 `AgentEvent::Custom`

相应地 agent 流也开始支持结构化 custom event。它不是字符串日志，而是可以被 protocol、tracer、outer layer 保真转发的事件面。

当前最重要的使用场景就是：

- subagent 向父 agent 转发结构化事件

这一步很关键，因为没有这个事件面，subagent 只能把一切都压成 text delta，父层就不可能恢复出真实生命周期。

---

## 9. Subagent 能力从“返回字符串”升级为“转发生命周期”

### 9.1 `SubAgentTaskTool` 现在内部 runner 返回的是事件流

原先更偏向：

- 跑一个子 agent
- 收集最终文本
- 返回字符串

现在变成：

- runner 返回 `Stream<Item = AgentEvent>`
- tool 在外层把事件翻译成：
  - 用户可见 delta
  - `ToolOutput::Custom("subagent_event", ...)`
  - 最终 `ToolResult`

### 9.2 新增一组回归测试覆盖关键语义

已有测试覆盖：

- tracing lineage 是否正确挂父子 span
- 多个并行 subagent tool 是否能分别输出
- cancel 是否传播进 subagent
- subagent interrupt 是否被正确转发和追踪

这批测试非常重要，因为它们验证的不是某个函数，而是整个 runtime contract。

### 9.3 当前限制

目前外层 `ToolLayer` 对“layer-owned tool 自己再发 interrupt”仍是受限的，当前实现会把它转成 tool-call error，而不是完整外层 resume 语义。

这是一个已知设计边界，文档里应该明确，而不是假装已经支持完整外层 interrupt resume。

---

## 10. `remi-agentloop-eval` crate 新增

新增 `remi-agentloop-eval`，提供的是实验与 replay 能力，而不是训练/打分大而全系统。

当前已有的核心抽象：

- `SessionCapture`
- `ExperimentVariant`
- `ExperimentRunner`
- `EvaluationReport`
- `ScoreCard`
- scorer trait

设计定位很清楚：

- 从一次真实 `LoopInput::Start` 抽取 capture
- 对同一 session 生成多个 variant
- 改写 system prompt / model / tool set / metadata
- 重放并对比输出

这和当前动态 agent / prompt 实验需求是高度契合的，也是后续做 benchmark + eval 闭环的基础。

---

## 11. WASM / WIT / guest 协议同步更新

这批改动不是只停留在 native core，WASM 边界也同步在追：

- WIT `message` 结构增加 `name`
- `loop-input-start` 从 `content` 改成 `message`
- `loop-input-resume` 增加 `pending-interrupts`
- guest / wasm host 的输入输出转换同步更新

这保证了 native 和 wasm 的语义面不会继续漂移。

当前仍需注意：

- 并不是所有 example 都完成了同等程度的端到端验证
- 语义面已经同步，但 WASM 路径的更多行为回归仍值得继续补测

---

## 12. 文档层面的设计收敛结论

如果只看这批改动背后的设计结论，可以归纳成下面几句：

### 12.1 Request / State / Ctx 三分法已经落地

- Request 驱动 trajectory
- State 保存内部可恢复运行态
- ChatCtx 串联 tracing / cancellation / metadata / nested call context

### 12.2 Tool 能力正在统一到“registry + layer”模型

- inner loop 本地 tool：`ToolRegistry`
- outer wrapper tool：`ToolRegistry::into_layer()`
- domain side event：`ToolLayerHook`

### 12.3 subagent 不再被视为黑盒字符串工具

subagent 正在被当作“有自己 run/model/tool lifecycle 的子调用树”处理。

### 12.4 typed schema 是新默认方向

以后 tool 参数 schema 更合理的写法，不是手写 JSON，而是写 Rust 类型并自动导出 schema。

---

## 13. 已验证与未验证

当前已经跑过并通过的验证主要包括：

- `cargo test -p remi-agentloop-core --test external_tool_layer -- --nocapture`
- `cargo test -p remi-agentloop-deepagent task:: --lib`
- `cargo check -p remi-agentloop-core -p remi-agentloop-tool -p remi-agentloop-deepagent -p remi-agentloop`
- `cargo test -p remi-agentloop --test tool_macro_schemars`

但需要区分：

- **core / deepagent 主路径已经有针对性验证**
- **所有 example / wasm 组合路径并没有在这轮全部跑透**

所以当前最准确的说法是：主抽象与关键回归场景已验证，外围兼容面仍有继续扫尾空间。

---

## 14. 当前仍然建议继续做的收尾项

1. 把 `DeepAgent` 顶层自定义 local tool loop 也迁移到统一 `ToolLayer` 组合模型
2. 给顶层请求补一个更直接的 `system_prompt` override 入口，避免应用层总是把它编码成 `history[0] = Message::system(...)`
3. 给动态 subagent 暴露更公共的 runner / dispatcher 构造面，减少 `SubAgentTaskTool::new(...)` 的静态预绑定味道
4. 继续补 wasm / transport / example 的行为回归

---

## 15. 一句话总结

这一轮改动的本质，不是“多了几个 helper”，而是把框架的执行边界重新划清了：

- **调用上下文有了统一承载面**
- **工具执行有了统一可叠加模型**
- **subagent 开始拥有可追踪、可取消、可转发的真实生命周期**
- **tool schema 开始走向类型驱动**

这几件事叠在一起，才使“动态 agent 系统”真正开始变成可落地的应用层能力。