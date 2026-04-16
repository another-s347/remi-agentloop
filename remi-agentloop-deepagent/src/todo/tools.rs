//! Todo tools: add, list, complete, update, remove.
//!
//! State lives in `ctx.user_state["__todos"]` as a JSON array so it is
//! automatically serialised into every `AgentState` checkpoint.

use async_stream::stream;
use futures::Stream;
use remi_core::error::AgentError;
use remi_core::tool::{parse_arguments, schema_for_type, Tool, ToolOutput, ToolResult};
use remi_core::types::{ChatCtx, ResumePayload};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

// ── Todo item ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: u64,
    pub content: String,
    pub done: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TodoAddArgs {
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TodoListArgs {}

#[derive(Debug, Deserialize, JsonSchema)]
struct TodoIdArgs {
    id: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TodoUpdateArgs {
    id: u64,
    content: String,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Read the todo list from user_state, defaulting to empty.
fn read_todos(ctx: &ChatCtx) -> Vec<TodoItem> {
    ctx.with_user_state(|us| match us.get("__todos") {
        Some(v) => serde_json::from_value::<Vec<TodoItem>>(v.clone()).unwrap_or_default(),
        None => vec![],
    })
}

/// Atomically modify the todo list under a single write lock to prevent
/// interleaving when multiple todo tools run in parallel.
///
/// `f` receives the current list and returns `(updated_list, return_value)`.
fn modify_todos<T>(ctx: &ChatCtx, f: impl FnOnce(Vec<TodoItem>) -> (Vec<TodoItem>, T)) -> T {
    ctx.update_user_state(|us| {
        let todos: Vec<TodoItem> = match us.get("__todos") {
            Some(v) => serde_json::from_value::<Vec<TodoItem>>(v.clone()).unwrap_or_default(),
            None => vec![],
        };
        let (updated, ret) = f(todos);
        us["__todos"] = serde_json::to_value(&updated).unwrap_or(json!([]));
        ret
    })
}

/// Next ID = max existing + 1 (or 1 if empty).
fn next_id(todos: &[TodoItem]) -> u64 {
    todos.iter().map(|t| t.id).max().unwrap_or(0) + 1
}

fn fmt_todos(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        return "No todos.".to_string();
    }
    todos
        .iter()
        .map(|t| {
            let mark = if t.done { "✓" } else { "○" };
            format!("[{}] {} {}", mark, t.id, t.content)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ── TodoAddTool ───────────────────────────────────────────────────────────────

/// Add a new todo item to the todo list. Returns the new item ID.
pub struct TodoAddTool;

impl Tool for TodoAddTool {
    fn name(&self) -> &str {
        "todo__add"
    }
    fn description(&self) -> &str {
        "Add a new todo item. Returns the assigned numeric ID."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        schema_for_type::<TodoAddArgs>()
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        ctx: ChatCtx,
    ) -> Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError> {
        let TodoAddArgs { content } = parse_arguments("todo__add", arguments)?;

        let (id, content2) = modify_todos(&ctx, |mut todos| {
            let id = next_id(&todos);
            todos.push(TodoItem {
                id,
                content: content.clone(),
                done: false,
            });
            (todos, (id, content))
        });
        let (id, content) = (id, content2);

        Ok(ToolResult::Output(stream! {
            yield ToolOutput::text(format!("Added todo #{id}: {content}"));
        }))
    }
}

// ── TodoListTool ──────────────────────────────────────────────────────────────

/// List all todo items with their completion status.
pub struct TodoListTool;

impl Tool for TodoListTool {
    fn name(&self) -> &str {
        "todo__list"
    }
    fn description(&self) -> &str {
        "List all todo items with their completion status."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        schema_for_type::<TodoListArgs>()
    }

    async fn execute(
        &self,
        _arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        ctx: ChatCtx,
    ) -> Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError> {
        let todos = read_todos(&ctx);
        let text = fmt_todos(&todos);
        Ok(ToolResult::Output(stream! {
            yield ToolOutput::text(text);
        }))
    }
}

// ── TodoCompleteTool ──────────────────────────────────────────────────────────

/// Mark a todo item as done.
pub struct TodoCompleteTool;

impl Tool for TodoCompleteTool {
    fn name(&self) -> &str {
        "todo__complete"
    }
    fn description(&self) -> &str {
        "Mark a todo item as completed by its ID."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        schema_for_type::<TodoIdArgs>()
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        ctx: ChatCtx,
    ) -> Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError> {
        let TodoIdArgs { id } = parse_arguments("todo__complete", arguments)?;

        let msg = modify_todos(&ctx, |mut todos| {
            let msg = match todos.iter_mut().find(|t| t.id == id) {
                Some(t) => {
                    t.done = true;
                    format!("Todo #{id} marked as done.")
                }
                None => format!("Todo #{id} not found."),
            };
            (todos, msg)
        });
        Ok(ToolResult::Output(stream! { yield ToolOutput::text(msg); }))
    }
}

// ── TodoUpdateTool ────────────────────────────────────────────────────────────

/// Update the text of an existing todo item.
pub struct TodoUpdateTool;

impl Tool for TodoUpdateTool {
    fn name(&self) -> &str {
        "todo__update"
    }
    fn description(&self) -> &str {
        "Update the content text of an existing todo item by its ID."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        schema_for_type::<TodoUpdateArgs>()
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        ctx: ChatCtx,
    ) -> Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError> {
        let TodoUpdateArgs { id, content } = parse_arguments("todo__update", arguments)?;

        let msg = modify_todos(&ctx, |mut todos| {
            let msg = match todos.iter_mut().find(|t| t.id == id) {
                Some(t) => {
                    t.content = content.clone();
                    format!("Updated todo #{id}: {content}")
                }
                None => format!("Todo #{id} not found."),
            };
            (todos, msg)
        });
        Ok(ToolResult::Output(stream! { yield ToolOutput::text(msg); }))
    }
}

// ── TodoRemoveTool ────────────────────────────────────────────────────────────

/// Remove a todo item by ID.
pub struct TodoRemoveTool;

impl Tool for TodoRemoveTool {
    fn name(&self) -> &str {
        "todo__remove"
    }
    fn description(&self) -> &str {
        "Permanently remove a todo item by its ID."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        schema_for_type::<TodoIdArgs>()
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        ctx: ChatCtx,
    ) -> Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError> {
        let TodoIdArgs { id } = parse_arguments("todo__remove", arguments)?;

        let removed = modify_todos(&ctx, |mut todos| {
            let before = todos.len();
            todos.retain(|t| t.id != id);
            let removed = before != todos.len();
            (todos, removed)
        });

        Ok(ToolResult::Output(stream! {
            if removed {
                yield ToolOutput::text(format!("Removed todo #{id}."));
            } else {
                yield ToolOutput::text(format!("Todo #{id} not found."));
            }
        }))
    }
}

/// All five todo tools as a convenience group.
pub struct TodoToolkit;

impl TodoToolkit {
    pub fn add(&self) -> TodoAddTool {
        TodoAddTool
    }
    pub fn list(&self) -> TodoListTool {
        TodoListTool
    }
    pub fn complete(&self) -> TodoCompleteTool {
        TodoCompleteTool
    }
    pub fn update(&self) -> TodoUpdateTool {
        TodoUpdateTool
    }
    pub fn remove(&self) -> TodoRemoveTool {
        TodoRemoveTool
    }
}
