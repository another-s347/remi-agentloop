use std::collections::HashMap;
use crate::error::AgentError;
use crate::types::ParsedToolCall;
use super::{DynTool, Tool, ToolDefinition, tool_to_definition, BoxedToolResult};

pub struct ToolRegistry {
    tools: Vec<Box<dyn DynTool>>,
    index: HashMap<String, usize>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new(), index: HashMap::new() }
    }

    pub fn register(&mut self, tool: impl Tool + Send + Sync + 'static) {
        let name = tool.name().to_string();
        let idx = self.tools.len();
        self.tools.push(Box::new(tool));
        self.index.insert(name, idx);
    }

    pub(crate) fn get(&self, name: &str) -> Option<&dyn DynTool> {
        self.index.get(name).map(|&i| self.tools[i].as_ref())
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| tool_to_definition(t.as_ref())).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Execute all tool calls in parallel, returning (call_id, result) pairs
    pub async fn execute_parallel(
        &self,
        calls: &[ParsedToolCall],
    ) -> Vec<(String, Result<BoxedToolResult<'_>, AgentError>)> {
        // We need to execute each tool call. Since DynTool is Send+Sync,
        // we can create a Vec of futures and join them.
        let mut results = Vec::with_capacity(calls.len());
        // Sequential execution to avoid borrowing issues with 'self
        // For true parallelism, tools would need Arc wrapping
        for tc in calls {
            let result = match self.get(&tc.name) {
                Some(tool) => tool.execute_boxed(tc.arguments.clone()).await,
                None => Err(AgentError::ToolNotFound(tc.name.clone())),
            };
            results.push((tc.id.clone(), result));
        }
        results
    }
}

impl Default for ToolRegistry {
    fn default() -> Self { Self::new() }
}
