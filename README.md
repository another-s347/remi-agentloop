# remi-agentloop

A composable, strongly-typed, async-streaming AI agent framework for Rust.

## Features

### Strongly typed, fully generic

The whole framework is built on one trait: `Agent<Request, Response, Error>`. Every layer — model client, tool loop, memory, transport, adapters — implements `Agent` with concrete types. No `Box<dyn Any>`, no stringly-typed middleware, no hidden type erasure. Composition is enforced at compile time via a typestate `AgentBuilder`.

### Async streaming

`chat()` returns `impl Stream<Item = AgentEvent>`. Text deltas, tool call events, usage stats, interrupts, and checkpoints all flow through a single typed stream that the caller drives at their own pace. No `Send` bound — works in both native async runtimes and `wasm32`.

### Clear boundary: request vs state vs ctx

The framework separates three concepts on purpose.

- `Request` is what drives the agent trajectory. It contains the user-facing input for the next transition, including messages, tool definitions, and resume payloads.
- `State` is internal runtime state that tools and layers maintain across steps. It is resumable, but it is not the source of truth for what the user is asking next.
- `ChatCtx` carries cross-cutting invocation context through the full chain: tracing lineage, cancellation, shared metadata, and tool/layer-owned shared state.

That split keeps user intent, internal runtime bookkeeping, and full-run context from leaking into each other.

### Tools, external tool calling & interrupt/resume

Tools are defined with a `#[tool]` proc-macro: the doc comment becomes the description and argument types map to JSON Schema automatically. Tools execute locally inside the agent loop by default.

For tools that live outside the loop (e.g. in an outer orchestration layer), the raw `step()` API surfaces `NeedToolExecution` events so any caller can execute tools externally and feed results back.

Tools that require human or policy approval can pause the loop by returning `ToolResult::Interrupt`. The agent emits `AgentEvent::Interrupt`, saves a resumable checkpoint, and waits. The caller resumes the exact same run with `ChatInput::Resume` after collecting approvals — the `RunId` and tracer chain are preserved across the pause.

### WASM

Core traits carry no `Send`/`Sync` bounds, so the same agent code compiles to `wasm32-wasip2` (server-side, via wasmtime) and `wasm32-unknown-unknown` (browser). Agent logic can be packaged as a WASM component with WIT guest bindings and hosted by `remi-agentloop-wasm`.

### Hot-reload via WASM

`remi-agentloop-wasm` supports runtime agent hot-reloading: the host watches the compiled `.wasm` file for changes and swaps in the new module without restarting the process. The `remi dev` CLI subcommand wraps this into a watch-build-reload loop — any save to the agent's source triggers a rebuild, and the running server loads the new WASM component and continues serving.

### Tracing & observability

A pluggable `Tracer` trait covers the full run lifecycle (run start/end, model call, tool call, interrupt, resume). Built-in backends:

- `StdoutTracer` — structured logging to stdout
- `LangSmithTracer` — sends traces to [LangSmith](https://smith.langchain.com/) (feature `tracing-langsmith`)
- `CompositeTracer` — fan-out to multiple backends simultaneously

## Crate structure

| Crate | Description |
|-------|-------------|
| `remi-agentloop` | Facade — one dependency for everything |
| `remi-agentloop-core` | `Agent` trait, builder, loop, tools, types |
| `remi-agentloop-model` | OpenAI-compatible streaming client |
| `remi-agentloop-transport` | HTTP transport + SSE |
| `remi-agentloop-tool` | `BashTool`, `FsTool`, sandboxed variants |
| `remi-agentloop-macros` | `#[tool]` proc-macro |
| `remi-agentloop-wasm` | WASM host runtime (wasmtime) + hot-reload |
| `remi-agentloop-guest` | WASM guest bindings (WIT) |
| `remi-agentloop-deepagent` | Long-horizon agent with built-in planning |
| `remi-agentloop-cli` | `remi` CLI + `remi-tui` terminal UI |

## License

MIT OR Apache-2.0
