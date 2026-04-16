# 实现路线图 + 验证

> 6 阶段实现优先级、10 项验证检查

## 实现优先级

### Phase 1：核心骨架
1. `Cargo.toml` + feature flags
2. `error.rs` — AgentError（含 ThreadNotFound, RunNotFound）
3. `types.rs` — ThreadId, RunId, MessageId, Content, ContentPart, Message(多模态), Role, ModelRequest / ChatRequest / LoopInput, ChatResponseChunk, ChatCtx, AgentEvent
4. `protocol.rs` — ProtocolRequest(含 thread_id, metadata), ProtocolEvent(含 RunStart+metadata), ProtocolError, ProtocolAgent
5. `agent.rs` — Agent trait + AgentExt + Layer trait
6. `context.rs` — ContextStore trait + InMemoryStore
7. `config.rs` — AgentConfig + ConfigProvider trait + from_env()
8. `adapters/map.rs` — MapResponse, MapRequest, MapErr

### Phase 2：Model 层
8. `model/openai.rs` — OpenAIClient + SSE 解析（多模态 content 支持）

### Phase 3：Tool 系统 + Agent Loop + Tracing
9. `tool/mod.rs` + `tool/registry.rs` — Tool trait(流式返回 Stream<ToolOutput>, `execute(arguments, resume, &ChatCtx)`) + typed schema helpers + ToolRegistry(并行执行)
9a. `macros/` — `#[tool]` 过程宏（函数签名 → Tool impl，doc comment → description，类型 → JSON Schema）
9b. `tool/bash.rs` — BashTool [tool-bash]（shell 执行，白/黑名单，超时，输出截断）
9c. `tool/fs.rs` — LocalFs*Tool [tool-fs]（物理文件系统读写，按操作拆分为 read/write/mkdir/remove/ls）
9d. `tool/vfs.rs` / `tool/bkfs.rs` — VirtualFs / Fs*Tool [tool-fs-virtual]（内存虚拟文件系统，WASM 兼容）
10. `interrupt.rs` — InterruptHandler trait + InterruptRouter
11. `tracing/mod.rs` — Tracer trait + DynTracer + CompositeTracer + trace event structs
12. `tracing/stdout.rs` — StdoutTracer
13. `tracing/langsmith.rs` — LangSmithTracer [tracing-langsmith]
14. `state.rs` — AgentLoop 状态机 (async-stream, 集成 ContextStore + IDs + interrupt/resume + config + metadata + tracer)
15. `builder.rs` — Typestate AgentBuilder<M, S> + BuiltAgent + chat_in_thread() + resume_run() + .config() + .tracer() + .interrupt_router()

### Phase 4：SSE 传输层
16. `transport/sse.rs` — SSE 编解码
17. `transport/http_client.rs` — HttpSseClient
18. `transport/http_server.rs` — HttpSseServer (axum)

### Phase 5：WASM 传输层
19. `guest/exports.rs` — WASM guest 导出函数（含 set_config）
20. `transport/wasm_host.rs` — WasmAgent (wasmi)（含 from_bytes_with_config）

### Phase 6：完善
21. `adapters/retry.rs` — RetryLayer
22. `adapters/logging.rs` — LoggingLayer
23. `adapters/transform.rs` — TransformStream
24. BoxedAgent 动态分发辅助
25. 自定义 ContextStore 后端示例（Redis / SQLite）
26. 多模态示例（图片 + 文本输入）
27. interrupt/resume 端到端示例（自动审批 + 人工审批混合流程）
28. WASM 配置注入示例（宿主注入 API key）
29. LangSmith tracing 端到端示例
30. metadata 透传示例（业务标签到 tool）
31. 测试 + 示例
32. `#[tool]` 宏 + 内置 tool 端到端示例
33. BashTool 安全策略测试（白/黑名单 + 超时）
34. `src/tui/` — TUI 模块（ratatui + crossterm + syntect）
35. `bin/remi-tui.rs` — CLI 入口（clap）
36. TUI Markdown 流式渲染 + 代码高亮
37. TUI Interrupt 交互式审批
38. TUI 端到端测试（headless terminal）

---

## 验证方式

1. `cargo check` — native 编译通过
2. `cargo check --no-default-features` — 纯核心（无运行时）编译通过
3. `cargo check --features http-server` — HTTP server 编译通过
4. `cargo check --features wasm-host` — WASM host 编译通过
5. `cargo build --target wasm32-unknown-unknown --no-default-features --features wasm-guest` — WASM guest 编译通过
6. `cargo test` — 单元测试（mock agent，不依赖真实 API）
7. `cargo run --example simple_chat` — 连接真实 OpenAI 验证 SSE
8. `cargo run --example agent_with_tools` — 验证 tool loop
9. `cargo run --example http_server` — 启动 SSE 服务端
10. `cargo run --example wasm_host` — 加载 .wasm 模块并调用
