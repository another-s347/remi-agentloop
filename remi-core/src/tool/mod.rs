use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use futures::Stream;
use serde::{Deserialize, Serialize};
use crate::config::AgentConfig;
use crate::error::AgentError;
use crate::types::{InterruptId, ResumePayload, RunId, ThreadId};

// ── ToolOutput ────────────────────────────────────────────────────────────────

/// Tool 执行的流式输出项
#[derive(Debug, Clone)]
pub enum ToolOutput {
    /// 进度/增量文本
    Delta(String),
    /// 工具执行最终结果
    Result(String),
}

// ── ToolContext ────────────────────────────────────────────────────────────────

/// Tool 执行时的上下文，由 AgentLoop 注入
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// 当前 agent 的运行时配置
    pub config: AgentConfig,
    /// 当前 Thread ID（如有）
    pub thread_id: Option<ThreadId>,
    /// 当前 Run ID
    pub run_id: RunId,
    /// 请求携带的业务自定义 metadata（透传）
    pub metadata: Option<serde_json::Value>,
    /// 用户自定义可变状态 — tool 可在执行时读写。
    ///
    /// 与 `AgentState.user_state` 同源：`run_loop` 在每批 tool 执行前
    /// 从 state 注入，执行后回写。用于渐进式披露等跨 tool 通信场景。
    ///
    /// # Example
    /// ```ignore
    /// // 在 tool execute 中：
    /// let mut us = ctx.user_state.write().unwrap();
    /// us["search_done"] = json!(true);
    /// ```
    pub user_state: Arc<RwLock<serde_json::Value>>,
}

// ── InterruptRequest ──────────────────────────────────────────────────────────

/// 中断请求——Tool 用此表示需要外部输入
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptRequest {
    pub interrupt_id: InterruptId,
    /// 中断类型，如 "human_approval", "payment_confirm", "policy_check"
    pub kind: String,
    /// 传递给调用方的上下文
    pub data: serde_json::Value,
}

impl InterruptRequest {
    pub fn new(kind: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            interrupt_id: InterruptId::new(),
            kind: kind.into(),
            data,
        }
    }
}

// ── ToolResult ────────────────────────────────────────────────────────────────

/// Tool 执行结果——要么是流式输出，要么是中断请求，不可混合
/// 设计类比 Rust `Result<T, E>`
pub enum ToolResult<S> {
    /// 正常执行——返回流式输出
    Output(S),
    /// 中断请求——暂停 AgentLoop，等待外部输入
    Interrupt(InterruptRequest),
}

impl<S> ToolResult<S> {
    pub fn is_output(&self) -> bool { matches!(self, Self::Output(_)) }
    pub fn is_interrupt(&self) -> bool { matches!(self, Self::Interrupt(_)) }

    pub fn map_stream<S2>(self, f: impl FnOnce(S) -> S2) -> ToolResult<S2> {
        match self {
            Self::Output(s) => ToolResult::Output(f(s)),
            Self::Interrupt(req) => ToolResult::Interrupt(req),
        }
    }
}

// ── Tool Trait ────────────────────────────────────────────────────────────────

/// Tool trait — RPITIT, not object-safe; use DynTool for dynamic dispatch
pub trait Tool {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;

    /// 当前 tool 是否启用。
    ///
    /// `user_state` 是 `AgentState.user_state` 的快照。
    /// 返回 `false` 时, registry 不会将此 tool 的 definition 发送给模型,
    /// 从而实现渐进式披露（progressive disclosure）。
    ///
    /// 默认返回 `true` — 始终启用。
    fn enabled(&self, _user_state: &serde_json::Value) -> bool {
        true
    }

    /// Execute the tool.
    ///
    /// `resume` is `Some` when this call is resuming from a previous
    /// [`InterruptRequest`]. The tool should use the payload to complete
    /// the operation that was interrupted.
    ///
    /// `ctx` provides runtime context (config, thread_id, run_id, metadata).
    /// Tools that don't need context can simply ignore the parameter.
    fn execute(
        &self,
        arguments: serde_json::Value,
        resume: Option<ResumePayload>,
        ctx: &ToolContext,
    ) -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>;
}

// ── ToolDefinition ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

// ── DynTool ───────────────────────────────────────────────────────────────────

pub type BoxedToolStream<'a> = Pin<Box<dyn Stream<Item = ToolOutput> + 'a>>;
pub type BoxedToolResult<'a> = ToolResult<BoxedToolStream<'a>>;

/// Object-safe version of Tool (framework-internal)
pub(crate) trait DynTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn enabled(&self, user_state: &serde_json::Value) -> bool;
    fn execute_boxed<'a>(
        &'a self,
        arguments: serde_json::Value,
        resume: Option<ResumePayload>,
        ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<BoxedToolResult<'a>, AgentError>> + 'a>>;
}

impl<T: Tool + Send + Sync> DynTool for T {
    fn name(&self) -> &str { Tool::name(self) }
    fn description(&self) -> &str { Tool::description(self) }
    fn parameters_schema(&self) -> serde_json::Value { Tool::parameters_schema(self) }
    fn enabled(&self, user_state: &serde_json::Value) -> bool { Tool::enabled(self, user_state) }

    fn execute_boxed<'a>(
        &'a self,
        arguments: serde_json::Value,
        resume: Option<ResumePayload>,
        ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<BoxedToolResult<'a>, AgentError>> + 'a>> {
        Box::pin(async move {
            let result = Tool::execute(self, arguments, resume, ctx).await?;
            Ok(result.map_stream(|s| -> BoxedToolStream<'_> { Box::pin(s) }))
        })
    }
}

/// Helper: convert Tool → ToolDefinition
pub(crate) fn tool_to_definition(tool: &dyn DynTool) -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".into(),
        function: FunctionDefinition {
            name: tool.name().into(),
            description: tool.description().into(),
            parameters: tool.parameters_schema(),
        },
    }
}

pub mod registry;
