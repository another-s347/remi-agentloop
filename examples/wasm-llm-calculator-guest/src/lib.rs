//! LLM Calculator WASM Guest Agent
//!
//! A fully self-contained agent that runs entirely inside WASM:
//! - Receives a natural-language math question
//! - Calls an OpenAI-compatible LLM API via **wasi:http** (waki)
//! - Executes calculator tools (add/subtract/multiply/divide) in pure Rust
//! - Loops until the LLM produces a text answer
//! - Emits [`ProtocolEvent`] from the stream
//!
//! # Architecture
//!
//! ```
//! Browser / wasmtime
//!   └─ jco / wasmtime-wasi-http (polyfills wasi:http)
//!       └─ WASM component (this crate)
//!           ├─ imports wasi:http → real LLM API calls
//!           └─ exports remi:agentloop/agent
//! ```
//!
//! The same `.wasm` file runs in:
//! - **Browser**: `jco transpile` → polyfills wasi:http with `fetch`
//! - **wasmtime**: standard WASI http support
//!
//! # Input
//!
//! Pass config via `LoopInput::Start`:
//! ```json
//! {
//!   "type": "start",
//!   "content": "What is (3 + 7) * 2^4? Show step by step.",
//!   "model": "moonshot-v1-8k",
//!   "metadata": {
//!     "api_key": "sk-...",
//!     "base_url": "https://api.moonshot.cn/v1"
//!   }
//! }
//! ```

use remi_agentloop_guest::prelude::*;
use serde_json::{json, Value};
use waki::Client;

// ── Agent ────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct LlmCalculatorGuest;

impl GuestAgent for LlmCalculatorGuest {
    async fn chat(&self, input: LoopInput) -> Result<Vec<ProtocolEvent>, String> {
        let (question, api_key, base_url, model) = parse_start(input)?;

        // Build initial message history
        let system_msg = json!({
            "role": "system",
            "content": "You are a precise calculator assistant. \
                        Use the provided tools to perform arithmetic operations. \
                        Always use tools for calculations — never compute mentally. \
                        Show your work step by step."
        });
        let user_msg = json!({
            "role": "user",
            "content": question
        });

        let tools = calculator_tools();
        let mut messages: Vec<Value> = vec![system_msg, user_msg];
        let mut events: Vec<ProtocolEvent> = Vec::new();

        // Multi-turn loop: call LLM → execute tools → repeat until done
        loop {
            let request_body = json!({
                "model": model,
                "messages": messages,
                "tools": tools,
                "temperature": 0.0,
            });

            let response_json =
                call_llm(&api_key, &base_url, &request_body)?;

            let choice = &response_json["choices"][0];
            let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop");
            let msg = &choice["message"];

            // Record assistant turn
            messages.push(msg.clone());

            if let Some(usage) = response_json["usage"].as_object() {
                let prompt = usage["prompt_tokens"].as_u64().unwrap_or(0) as u32;
                let completion = usage["completion_tokens"].as_u64().unwrap_or(0) as u32;
                events.push(ProtocolEvent::Usage {
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                });
            }

            match finish_reason {
                "tool_calls" => {
                    let tool_calls = msg["tool_calls"]
                        .as_array()
                        .ok_or("missing tool_calls array")?;

                    let mut tool_results: Vec<Value> = Vec::new();

                    for tc in tool_calls {
                        let id = tc["id"].as_str().unwrap_or("").to_string();
                        let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                        let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                        let args: Value = serde_json::from_str(args_str)
                            .unwrap_or(json!({}));

                        events.push(ProtocolEvent::ToolCallStart {
                            id: id.clone(),
                            name: name.clone(),
                        });

                        let result = execute_tool(&name, &args);

                        events.push(ProtocolEvent::ToolResult {
                            id: id.clone(),
                            name: name.clone(),
                            result: result.clone(),
                        });

                        tool_results.push(json!({
                            "role": "tool",
                            "tool_call_id": id,
                            "content": result,
                        }));
                    }

                    messages.extend(tool_results);
                    // Continue loop — call LLM again with tool results
                }

                _ => {
                    // "stop" or anything else — extract text and finish
                    let text = msg["content"].as_str().unwrap_or("").to_string();
                    if !text.is_empty() {
                        events.push(ProtocolEvent::Delta {
                            content: text,
                            role: Some("assistant".into()),
                        });
                    }
                    events.push(ProtocolEvent::Done);
                    break;
                }
            }
        }

        Ok(events)
    }
}

remi_agentloop_guest::export_agent!(LlmCalculatorGuest);

// ── Tool definitions ─────────────────────────────────────────────────────────

fn calculator_tools() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "add",
                "description": "Add two numbers: a + b",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "number", "description": "First operand" },
                        "b": { "type": "number", "description": "Second operand" }
                    },
                    "required": ["a", "b"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "subtract",
                "description": "Subtract two numbers: a - b",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "number" },
                        "b": { "type": "number" }
                    },
                    "required": ["a", "b"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "multiply",
                "description": "Multiply two numbers: a * b",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "number" },
                        "b": { "type": "number" }
                    },
                    "required": ["a", "b"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "divide",
                "description": "Divide two numbers: a / b",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "number" },
                        "b": { "type": "number" }
                    },
                    "required": ["a", "b"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "power",
                "description": "Raise base to exponent: base ^ exp",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "base": { "type": "number" },
                        "exp":  { "type": "number" }
                    },
                    "required": ["base", "exp"]
                }
            }
        }
    ])
}

// ── In-guest tool execution (pure Rust, no I/O) ───────────────────────────────

fn execute_tool(name: &str, args: &Value) -> String {
    let a = || args["a"].as_f64().unwrap_or(0.0);
    let b = || args["b"].as_f64().unwrap_or(0.0);

    let result: f64 = match name {
        "add"      => a() + b(),
        "subtract" => a() - b(),
        "multiply" => a() * b(),
        "divide" => {
            let denom = b();
            if denom == 0.0 { return "Error: division by zero".into(); }
            a() / denom
        }
        "power" => {
            let base = args["base"].as_f64().unwrap_or(0.0);
            let exp  = args["exp"].as_f64().unwrap_or(0.0);
            base.powf(exp)
        }
        _ => return format!("Error: unknown tool '{name}'"),
    };

    fmt_num(result)
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

// ── HTTP call via waki (wasi:http) ────────────────────────────────────────────

fn call_llm(api_key: &str, base_url: &str, body: &Value) -> Result<Value, String> {
    let url = format!("{base_url}/chat/completions");
    let body_bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;

    let resp = Client::new()
        .post(&url)
        .header("Authorization", &format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .map_err(|e| format!("HTTP error: {e:?}"))?;

    let status = resp.status_code();
    let resp_bytes = resp.body().map_err(|e| format!("body error: {e:?}"))?;

    let json: Value = serde_json::from_slice(&resp_bytes)
        .map_err(|e| format!("JSON parse error: {e}; status={status}"))?;

    if status < 200 || status >= 300 {
        let msg = json["error"]["message"]
            .as_str()
            .unwrap_or("unknown error")
            .to_string();
        return Err(format!("LLM API error {status}: {msg}"));
    }

    Ok(json)
}

// ── Input parsing ─────────────────────────────────────────────────────────────

fn parse_start(input: LoopInput) -> Result<(String, String, String, String), String> {
    match input {
        LoopInput::Start {
            content,
            model,
            metadata,
            ..
        } => {
            let question = content.text_content();

            let meta = metadata.unwrap_or(Value::Null);
            let api_key = meta["api_key"]
                .as_str()
                .ok_or("metadata.api_key is required")?
                .to_string();
            let base_url = meta["base_url"]
                .as_str()
                .unwrap_or("https://api.openai.com/v1")
                .to_string();
            let model_name = model
                .as_deref()
                .unwrap_or("gpt-4o")
                .to_string();

            Ok((question, api_key, base_url, model_name))
        }
        LoopInput::Resume { .. } => {
            Err("LlmCalculatorGuest does not support resume — it manages its own loop".into())
        }
    }
}
