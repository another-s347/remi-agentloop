//! WASM Calculator Guest Agent
//!
//! A calculator that parses math expressions and delegates each arithmetic
//! operation to the host via `NeedToolExecution`. Demonstrates the
//! **WASM + external tool calling** pattern:
//!
//! 1. Guest receives expression (e.g. "2 + 3 * 4")
//! 2. Guest parses it, identifies first operation (mul 3 4)
//! 3. Guest emits `NeedToolExecution` → host executes `multiply(3, 4) = 12`
//! 4. Host resumes with result → guest continues (add 2 12)
//! 5. Guest emits `NeedToolExecution` → host executes `add(2, 12) = 14`
//! 6. Host resumes → guest emits `Delta("14")` + `Done`
//!
//! # Build
//!
//! ```sh
//! # Install wasm target & tooling
//! rustup target add wasm32-unknown-unknown
//! cargo install wasm-tools
//!
//! # Build the guest
//! cd examples/wasm-calculator-guest
//! cargo build --target wasm32-unknown-unknown --release
//!
//! # Convert to WASM component
//! wasm-tools component new \
//!     target/wasm32-unknown-unknown/release/wasm_calculator_guest.wasm \
//!     -o calculator.wasm
//!
//! # Run with the host
//! cd ../..
//! cargo run -p remi-agentloop-wasm --example wasm_calculator -- \
//!     examples/wasm-calculator-guest/calculator.wasm "2 + 3 * 4"
//! ```

use remi_agentloop_guest::prelude::*;
use serde::{Deserialize, Serialize};

mod abi_smoke {
    pub type GuestProtocolEvent =
        remi_agentloop_guest::bindings::exports::remi::agentloop::agent::ProtocolEvent;
    pub type GuestConfig = remi_agentloop_guest::bindings::remi::agentloop::config::AgentConfig;
    pub type GuestApiVersion =
        remi_agentloop_guest::bindings::exports::remi::agentloop::agent_info::ApiVersion;

    #[allow(dead_code)]
    pub fn _touch_bindings(
        _event: Option<GuestProtocolEvent>,
        _config: Option<GuestConfig>,
        _version: GuestApiVersion,
    ) {
    }
}

// ── Agent ───────────────────────────────────────────────────────────────────

#[derive(Default)]
struct CalculatorGuest;

impl GuestAgent for CalculatorGuest {
    async fn chat(&self, input: LoopInput) -> Result<Vec<ProtocolEvent>, String> {
        match input {
            LoopInput::Start { content, .. } => {
                let expr = content.text_content();
                let tokens = tokenize(&expr)?;
                let postfix = to_postfix(tokens)?;
                let ser: Vec<SerToken> = postfix.into_iter().map(|t| t.into_ser()).collect();
                evaluate(ser, vec![], 0)
            }
            LoopInput::Resume { state, results } => {
                let eval: EvalState = serde_json::from_value(state.user_state)
                    .map_err(|e| format!("bad state: {e}"))?;

                let value = match results.first() {
                    Some(ToolCallOutcome::Result { content, .. }) => content
                        .text_content()
                        .parse::<f64>()
                        .map_err(|e| format!("bad result: {e}"))?,
                    Some(ToolCallOutcome::Error { error, .. }) => {
                        return Err(format!("tool error: {error}"));
                    }
                    None => return Err("no tool result".into()),
                };

                let mut stack = eval.stack;
                stack.push(value);
                evaluate(eval.postfix, stack, eval.pos)
            }
        }
    }
}

remi_agentloop_guest::export_agent!(CalculatorGuest);

// ── Evaluation state (serialized in AgentState.user_state) ──────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvalState {
    postfix: Vec<SerToken>,
    stack: Vec<f64>,
    pos: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t")]
enum SerToken {
    #[serde(rename = "n")]
    Num { v: f64 },
    #[serde(rename = "o")]
    Op { v: String },
}

// ── Evaluate postfix with external operations ───────────────────────────────

fn evaluate(
    postfix: Vec<SerToken>,
    mut stack: Vec<f64>,
    start: usize,
) -> Result<Vec<ProtocolEvent>, String> {
    let mut pos = start;

    while pos < postfix.len() {
        match &postfix[pos] {
            SerToken::Num { v } => {
                stack.push(*v);
                pos += 1;
            }
            SerToken::Op { v: op } => {
                if stack.len() < 2 {
                    return Err("invalid expression: not enough operands".into());
                }
                let b = stack.pop().unwrap();
                let a = stack.pop().unwrap();
                pos += 1;

                // + and - are handled internally (no host round-trip needed)
                match op.as_str() {
                    "+" => {
                        stack.push(a + b);
                        continue;
                    }
                    "-" => {
                        stack.push(a - b);
                        continue;
                    }
                    _ => {}
                }

                // * and / are delegated to the host via NeedToolExecution
                let tool_name = match op.as_str() {
                    "*" => "multiply",
                    "/" => "divide",
                    _ => return Err(format!("unknown op: {op}")),
                };

                let tool_call = ParsedToolCall {
                    id: format!("tc_{pos}"),
                    name: tool_name.to_string(),
                    arguments: serde_json::json!({ "a": a, "b": b }),
                };

                let eval_state = EvalState {
                    postfix: postfix.clone(),
                    stack: stack.clone(),
                    pos,
                };

                let state = AgentState {
                    messages: vec![],
                    system_prompt: None,
                    tool_definitions: vec![],
                    config: StepConfig {
                        model: String::new(),
                        temperature: None,
                        max_tokens: None,
                        metadata: None,
                        rate_limit_retry: None,
                    },
                    thread_id: ThreadId("wasm-calc".into()),
                    run_id: RunId("run-0".into()),
                    turn: 0,
                    phase: AgentPhase::AwaitingToolExecution {
                        tool_calls: vec![tool_call.clone()],
                    },
                    user_state: serde_json::to_value(&eval_state).unwrap(),
                };

                return Ok(vec![ProtocolEvent::NeedToolExecution {
                    state,
                    tool_calls: vec![tool_call],
                    completed_results: vec![],
                }]);
            }
        }
    }

    // All tokens consumed — result on stack
    if stack.len() != 1 {
        return Err(format!(
            "invalid expression: {} values left on stack",
            stack.len()
        ));
    }

    let result = stack[0];
    let text = if result.fract() == 0.0 && result.abs() < 1e15 {
        format!("{}", result as i64)
    } else {
        format!("{result}")
    };

    Ok(vec![
        ProtocolEvent::Delta {
            content: text,
            role: None,
        },
        ProtocolEvent::Done,
    ])
}

// ── Tokenizer ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Token {
    Number(f64),
    Op(String),
    LParen,
    RParen,
}

impl Token {
    fn into_ser(self) -> SerToken {
        match self {
            Token::Number(v) => SerToken::Num { v },
            Token::Op(v) => SerToken::Op { v },
            Token::LParen | Token::RParen => unreachable!("parens removed by to_postfix"),
        }
    }
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&ch) = chars.peek() {
        match ch {
            ' ' | '\t' | '\n' => {
                chars.next();
            }
            '0'..='9' | '.' => {
                let mut num = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_digit() || c == '.' {
                        num.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let n: f64 = num.parse().map_err(|_| format!("bad number: {num}"))?;
                tokens.push(Token::Number(n));
            }
            '+' | '-' | '*' | '/' => {
                // Handle unary minus: if '-' at start, after '(' or after an operator
                if ch == '-' {
                    let is_unary = tokens.is_empty()
                        || matches!(
                            tokens.last(),
                            Some(Token::Op(_)) | Some(Token::LParen)
                        );
                    if is_unary {
                        chars.next();
                        // Read the number after unary minus
                        let mut num = String::from("-");
                        while let Some(&c) = chars.peek() {
                            if c.is_ascii_digit() || c == '.' {
                                num.push(c);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        if num == "-" {
                            return Err("unexpected '-'".into());
                        }
                        let n: f64 = num.parse().map_err(|_| format!("bad number: {num}"))?;
                        tokens.push(Token::Number(n));
                        continue;
                    }
                }
                tokens.push(Token::Op(ch.to_string()));
                chars.next();
            }
            '(' => {
                tokens.push(Token::LParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::RParen);
                chars.next();
            }
            _ => return Err(format!("unexpected char: '{ch}'")),
        }
    }

    Ok(tokens)
}

// ── Shunting-Yard → postfix ─────────────────────────────────────────────────

fn precedence(op: &str) -> u8 {
    match op {
        "+" | "-" => 1,
        "*" | "/" => 2,
        _ => 0,
    }
}

fn to_postfix(tokens: Vec<Token>) -> Result<Vec<Token>, String> {
    let mut output = Vec::new();
    let mut ops: Vec<Token> = Vec::new();

    for tok in tokens {
        match tok {
            Token::Number(_) => output.push(tok),
            Token::Op(ref op) => {
                let p = precedence(op);
                while let Some(top) = ops.last() {
                    match top {
                        Token::Op(top_op) if precedence(top_op) >= p => {
                            output.push(ops.pop().unwrap());
                        }
                        _ => break,
                    }
                }
                ops.push(tok);
            }
            Token::LParen => ops.push(tok),
            Token::RParen => {
                loop {
                    match ops.pop() {
                        Some(Token::LParen) => break,
                        Some(t) => output.push(t),
                        None => return Err("mismatched parentheses".into()),
                    }
                }
            }
        }
    }

    while let Some(op) = ops.pop() {
        if matches!(op, Token::LParen) {
            return Err("mismatched parentheses".into());
        }
        output.push(op);
    }

    Ok(output)
}
