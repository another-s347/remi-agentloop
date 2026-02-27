# Remi AgentLoop — 设计文档索引

> 组合式、强类型、泛型、状态步进、异步流式 AI Agent 框架（Rust，可编译到 WASM）

## 核心设计原则

- **核心抽象**：`async chat(Request) -> Result<Stream<Response>, Error>`，全链路强类型，零用户可见类型擦除
- **多模态输入**：Message content 支持纯文本 / 图片 URL / 图片 Base64 / 音频 / 文件，与 OpenAI multimodal API 对齐
- **标识符体系**：ThreadId / RunId / MessageId，贯穿会话、执行、消息全生命周期
- **上下文管理**：ContextStore trait 可插拔存储后端（内存 / Redis / SQLite / 远程 API）
- **组合式**：适配器（map_response / map_request / map_err / transform / layer）将底层 Agent 组合为新 Agent
- **单线程循环，按需并发**：Agent loop 单线程驱动，仅内部需要时（如并行 tool call）并发，不要求 Send
- **流式 Tool + 并行执行**：Tool 返回 `ToolResult<Stream<ToolOutput>>`（参考 `Result<T, E>` 设计）：`Output(stream)` 表示正常流式执行，`Interrupt(req)` 表示中断请求——同一次调用不可混合。多 tool 并行执行，支持进度报告和增量结果
- **Interrupt / Resume**：Tool 可请求中断（人工审批、自动化策略检查、外部系统确认等），携带 InterruptId 暂停 AgentLoop，调用方（人工或应用层自动逻辑）处理后 resume 继续。支持多个并行 interrupt + 批量 resume。**RunId 在 interrupt/resume 全程保持不变**——resume 是同一 Run 的延续，不产生新 Run。Tracer 事件链连续，不重发 RunStart
- **运行时配置注入**：AgentConfig 携带 API key、model、base_url 等参数，不编译进二进制。WASM Guest 通过宿主 set_config() 注入。ConfigProvider trait 支持动态密钥轮换
- **Metadata 透传**：请求可携带业务自定义 metadata（JSON），透传到 ToolContext、Tracer、RunStart 事件，框架不解释内容
- **可观测性 / Tracing**：Tracer trait 可插拔追踪后端，覆盖 Run/Model/Tool/Interrupt 全生命周期。内置 LangSmithTracer + StdoutTracer。CompositeTracer 支持多后端
- **WASM 兼容**：核心 trait 无 Send/Sync bound，通过 feature flag 切换运行时

## 技术选型

- **Trait 风格**：RPITIT（Rust 1.75+），实现者直接 `async fn`，适配器零成本
- **依赖**：futures 0.3 / async-stream / pin-project-lite / serde + serde_json / thiserror / reqwest(可选) / axum(可选) / wasmi(可选) / tokio(native) / wasm-bindgen-futures(wasm-guest)
- **Feature flags**：`native`(默认) / `http-client` / `http-server` / `wasm-host` / `wasm-guest` / `tracing-langsmith` / `tool-bash` / `tool-fs` / `tool-fs-virtual` / `tools` / `tui`
- **过程宏**：`#[tool]` 宏自动从函数签名生成 Tool impl，doc comment → description，类型 → JSON Schema

## 模块结构

```
src/
├── lib.rs              # 公共 re-exports（含 pub use remi_agentloop_macros::tool）
├── agent.rs            # Agent trait + AgentExt + Layer trait + BoxedAgent
├── protocol.rs         # 标准协议（ProtocolRequest, ProtocolEvent）
├── adapters/
│   ├── map.rs          # MapResponse, MapRequest, MapErr
│   ├── transform.rs    # TransformStream
│   ├── logging.rs      # LoggingLayer
│   └── retry.rs        # RetryLayer
├── model/
│   ├── mod.rs          # ChatModel trait alias
│   └── openai.rs       # OpenAI Compatible SSE client
├── transport/
│   ├── sse.rs          # SSE 编解码
│   ├── http_client.rs  # HttpSseClient [http-client]
│   ├── http_server.rs  # HttpSseServer [http-server]
│   └── wasm_host.rs    # WasmAgent [wasm-host]
├── guest/
│   └── exports.rs      # WASM guest 导出 [wasm-guest]
├── tool/
│   ├── mod.rs          # Tool trait, ToolDefinition, ToolContext
│   ├── registry.rs     # ToolRegistry
│   ├── bash.rs         # BashTool          [tool-bash]
│   ├── fs.rs           # FsTool            [tool-fs]
│   └── vfs.rs          # VirtualFsTool     [tool-fs-virtual]
├── config.rs           # AgentConfig, ConfigProvider trait
├── interrupt.rs        # InterruptHandler, InterruptRouter
├── tracing/
│   ├── mod.rs          # Tracer trait, DynTracer, CompositeTracer, trace events
│   ├── langsmith.rs    # LangSmithTracer [tracing-langsmith]
│   └── stdout.rs       # StdoutTracer
├── tui/                # TUI 终端界面              [tui]
│   ├── mod.rs          # TuiApp, TuiEvent
│   ├── app.rs          # 状态 + 事件处理
│   ├── render.rs       # Ratatui 渲染
│   ├── markdown.rs     # 流式 Markdown 解析/渲染
│   ├── input.rs        # 输入状态 + 按键绑定
│   ├── commands.rs     # 斜杠命令
│   └── theme.rs        # 配色
├── builder.rs          # Typestate AgentBuilder
├── context.rs          # ContextStore trait + InMemoryStore
├── state.rs            # AgentLoop 状态机
├── types.rs            # ThreadId, RunId, MessageId, Content, ContentPart, Message, AgentEvent
└── error.rs            # AgentError
```

```
bin/                      # 可执行二进制
└── remi-tui.rs           # Claude Code 风格 TUI 入口 [tui]
```

```
macros/                   # proc-macro crate (remi-agentloop-macros)
├── Cargo.toml            # proc-macro = true, syn/quote/proc-macro2
└── src/
    └── lib.rs            # #[tool] 宏实现
```

## 详细设计文档

| # | 文档 | 内容 |
|---|------|------|
| 01 | [Core Trait](docs/01-core-trait.md) | Agent trait, AgentExt, Layer, BoxedAgent |
| 02 | [Protocol](docs/02-protocol.md) | ProtocolRequest, ProtocolEvent, ProtocolError, SSE wire format |
| 03 | [Type System](docs/03-type-system.md) | ThreadId/RunId/MessageId, Content/ContentPart(多模态), Message, AgentEvent, AgentError |
| 04 | [Adapters](docs/04-adapters.md) | MapResponse, MapRequest, MapErr, TransformStream, RetryLayer, LoggingLayer |
| 05 | [Model Layer](docs/05-model-layer.md) | ChatModel trait, OpenAIClient, SSE parsing |
| 06 | [Tool System](docs/06-tool-system.md) | Tool trait(流式返回 + interrupt), ToolOutput, ToolResult, InterruptRequest, InterruptId, ResumePayload, DynTool, ToolRegistry |
| 07 | [Agent Loop](docs/07-agent-loop.md) | LoopState 状态机(含 Interrupted), 并行 tool执行, interrupt/resume, AgentBuilder, BuiltAgent |
| 08 | [Transport](docs/08-transport.md) | HTTP SSE 客户端/服务端, WASM 宿主/Guest, 三种传输层对比 |
| 09 | [Examples](docs/09-examples.md) | 7 个端到端示例 + 类型流动全景图 |
| 10 | [Roadmap](docs/10-roadmap.md) | 6 阶段实现优先级 + 10 项验证检查 |
| 11 | [Identifiers & Context](docs/11-identifiers-context.md) | ThreadId/RunId/MessageId, ContextStore trait, InMemoryStore |
| 12 | [Config & Interrupt Handling](docs/12-config.md) | AgentConfig, ConfigProvider, WASM 配置注入, ToolContext(+metadata), InterruptHandler/Router |
| 13 | [Tracing](docs/13-tracing.md) | Tracer trait, DynTracer, LangSmithTracer, StdoutTracer, CompositeTracer, TracingLayer |
| 14 | [Advanced Patterns](docs/14-advanced-patterns.md) | Task（独立 memory）、Sub-Agent（共享 memory）、会话分叉——可行性分析 + 实现方案 |
| 15 | [Tool Macro & Builtins](docs/15-tool-macro-builtins.md) | #[tool] 过程宏、BashTool、FsTool、VirtualFsTool |
| 16 | [TUI](docs/16-tui.md) | Claude Code 风格终端 UI：流式对话、Tool 可视化、Interrupt 审批、斜杠命令 |
