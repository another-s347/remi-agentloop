# Benchmarking

This repository now includes two repeatable benchmark harnesses for measuring framework output performance with a controlled mock token stream.

## Native Streaming Baseline

The native harness measures the real streaming path inside `remi-agentloop-core`: mock model `ChatResponseChunk` output, `step()` translation, and final `AgentEvent` emission.

Run it with:

```sh
cargo run -p remi-agentloop-core --example mock_model_benchmark -- \
  --rates 50,100,1000 \
  --tokens 1000 \
  --chunk-tokens 5 \
  --warmup 1 \
  --rounds 5
```

To benchmark mixed tool usage, add internal and external tool calls. The mock model randomizes where those tool calls appear in its output, while the benchmark driver resumes external execution and keeps the run going:

```sh
cargo run -p remi-agentloop-core --example mock_model_benchmark -- \
  --rates 50,100,1000 \
  --tokens 1000 \
  --chunk-tokens 5 \
  --tool-arg-tokens 5 \
  --internal-tool-calls 2 \
  --external-tool-calls 2 \
  --seed 7 \
  --warmup 1 \
  --rounds 5
```

To make the model insert additional random tool calls probabilistically, add per-slot probabilities on top of any fixed minimum call counts:

```sh
cargo run -p remi-agentloop-core --example mock_model_benchmark -- \
  --rates 50,100,1000 \
  --tokens 1000 \
  --chunk-tokens 5 \
  --tool-arg-tokens 5 \
  --internal-tool-calls 1 \
  --external-tool-calls 1 \
  --internal-tool-probability 0.02 \
  --external-tool-probability 0.02 \
  --seed 7 \
  --warmup 1 \
  --rounds 5
```

Add `--json` for machine-readable output.

Reported metrics:

- `observed_tps`: end-to-end visible output throughput in mock tokens per second, including assistant text and tool output content
- `observed_model_tps`: model-side output throughput, counting assistant text plus synthetic tool-call argument tokens
- `first_event_ms`: time to the first substantive output event (`TextDelta`, `ToolCallStart`, `ToolDelta`, `ToolResult`, or `NeedToolExecution`)
- `ttft_ms`: time to first `TextDelta`
- `total_ms`: total end-to-end elapsed time
- `total_overhead_ms`: measured time minus theoretical model emission time and configured synthetic tool latencies
- `per_event_overhead_us`: approximate framework overhead per emitted event across text, tool, checkpoint, and resume handling

Additional options:

- `--internal-tool-calls`: number of in-process tool calls handled by `AgentLoop`
- `--external-tool-calls`: number of tool calls yielded as `NeedToolExecution` and resumed externally
- `--internal-tool-probability`: probability of inserting an additional internal tool call after each text slot
- `--external-tool-probability`: probability of inserting an additional external tool call after each text slot
- `--internal-tool-latency-ms`: synthetic latency applied inside each internal tool implementation
- `--external-tool-latency-ms`: synthetic latency applied while the benchmark driver executes external tools
- `--tool-arg-tokens`: synthetic model-output token cost of each emitted tool call
- `--seed`: deterministic seed controlling randomized placement of tool calls across the model output plan

The harness treats one ASCII character as one mock token, so token counting stays deterministic without a model tokenizer.

## WASM Component Batch Comparison

The wasm harness measures the current `WasmAgent` host path. It is intentionally reported as a batch-return path benchmark, not a real streaming benchmark.

Build the benchmark guest:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-tools
cd examples/wasm-benchmark-guest
cargo build --target wasm32-unknown-unknown --release
wasm-tools component new \
  target/wasm32-unknown-unknown/release/wasm_benchmark_guest.wasm \
  -o benchmark_guest.wasm
cd ../..
```

Run the wasm benchmark:

```sh
cargo run -p remi-agentloop-wasm --example wasm_component_benchmark -- \
  --wasm examples/wasm-benchmark-guest/benchmark_guest.wasm \
  --tokens 1000 \
  --chunk-tokens 5 \
  --warmup 1 \
  --rounds 5
```

Reported metrics:

- `batch_ready_ms`: time until `agent.chat()` returns a stream after the guest has already produced its full event vector
- `first_event_ms`: when the first host-visible event becomes available
- `total_ms`: total host-observed elapsed time
- `per_event_us`: average host-side cost per emitted protocol event

Current limitation:

- `remi-agentloop-wasm` currently collects the entire guest event sequence into a `Vec` before exposing it as a stream, so this benchmark cannot represent real per-token TTFT.