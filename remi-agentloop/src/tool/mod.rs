use std::future::Future;
use std::pin::Pin;
use futures::Stream;
use serde::{Deserialize, Serialize};
use crate::error::AgentError;
use crate::types::InterruptId;

// ── ToolOutput ────────────────────────────────────────────────────────────────

/// Tool 执行的流式输出项
#[derive(Debug, Clone)]
pub enum ToolOutput {
    /// 进度/增量文本
    Delta(String),
    /// 工具执行最终结果
    Result(String),
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

    fn execute(
        &self,
        arguments: serde_json::Value,
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
    fn execute_boxed(
        &self,
        arguments: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<BoxedToolResult<'_>, AgentError>> + '_>>;
}

impl<T: Tool + Send + Sync> DynTool for T {
    fn name(&self) -> &str { Tool::name(self) }
    fn description(&self) -> &str { Tool::description(self) }
    fn parameters_schema(&self) -> serde_json::Value { Tool::parameters_schema(self) }

    fn execute_boxed(
        &self,
        arguments: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<BoxedToolResult<'_>, AgentError>> + '_>> {
        Box::pin(async move {
            let result = Tool::execute(self, arguments).await?;
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

#[cfg(feature = "tool-bash")]
pub mod bash;

#[cfg(feature = "tool-fs")]
pub mod fs;

#[cfg(feature = "tool-fs-virtual")]
pub mod vfs;
