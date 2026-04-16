use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use uuid::Uuid;

fn uuid_v4() -> String {
    Uuid::new_v4().to_string()
}

// ── Identifiers ──────────────────────────────────────────────────────────────

/// Unique identifier for a conversation thread.
///
/// A thread holds multiple runs and messages.  Create one via
/// [`BuiltAgent::create_thread`](crate::builder::BuiltAgent) or
/// [`ContextStore::create_thread`](crate::context::ContextStore).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadId(pub String);

/// Unique identifier for a single run.
///
/// A run corresponds to one `agent.chat()` call.  The same `RunId` is
/// retained across interrupt/resume cycles that belong to the same logical
/// invocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

/// Unique identifier for a single message within a thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

/// Unique identifier for a single tool interrupt instance.
///
/// Returned inside [`InterruptRequest`](crate::tool::InterruptRequest) and
/// used in [`ResumePayload`] to match a resume response to its interrupt.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InterruptId(pub String);

/// Unique identifier for a tracing span node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpanId(pub String);

impl ThreadId {
    pub fn new() -> Self {
        Self(uuid_v4())
    }
}
impl RunId {
    pub fn new() -> Self {
        Self(uuid_v4())
    }
}
impl MessageId {
    pub fn new() -> Self {
        Self(uuid_v4())
    }
}
impl InterruptId {
    pub fn new() -> Self {
        Self(uuid_v4())
    }
}
impl SpanId {
    pub fn new() -> Self {
        Self(uuid_v4())
    }

    pub fn derived(namespace: Option<&SpanId>, name: impl AsRef<str>) -> Self {
        let ns = namespace
            .and_then(|id| Uuid::parse_str(&id.0).ok())
            .unwrap_or_else(Uuid::nil);
        Self(Uuid::new_v5(&ns, name.as_ref().as_bytes()).to_string())
    }
}

impl Default for ThreadId {
    fn default() -> Self {
        Self::new()
    }
}
impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}
impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}
impl Default for InterruptId {
    fn default() -> Self {
        Self::new()
    }
}
impl Default for SpanId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl fmt::Display for InterruptId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl fmt::Display for SpanId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ── Chat Context ────────────────────────────────────────────────────────────

/// Kind of tracing span represented by a [`SpanNode`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpanKind {
    Run,
    Model,
    Tool,
    Subagent,
    Custom { name: String },
}

impl SpanKind {
    pub fn stable_name(&self) -> String {
        match self {
            SpanKind::Run => "run".to_string(),
            SpanKind::Model => "model".to_string(),
            SpanKind::Tool => "tool".to_string(),
            SpanKind::Subagent => "subagent".to_string(),
            SpanKind::Custom { name } => format!("custom:{name}"),
        }
    }
}

/// Parent-linked tracing node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpanNode {
    pub span_id: SpanId,
    pub kind: SpanKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Box<SpanNode>>,
}

impl SpanNode {
    pub fn new(kind: SpanKind) -> Self {
        Self {
            span_id: SpanId::new(),
            kind,
            scope_key: None,
            parent: None,
        }
    }

    pub fn derived(
        kind: SpanKind,
        scope_key: impl Into<String>,
        parent: Option<&SpanNode>,
    ) -> Self {
        let scope_key = scope_key.into();
        let stable_name = format!("{}:{scope_key}", kind.stable_name());
        Self {
            span_id: SpanId::derived(parent.map(|node| &node.span_id), stable_name),
            kind,
            scope_key: Some(scope_key),
            parent: parent.cloned().map(Box::new),
        }
    }

    pub fn with_scope_key(mut self, scope_key: impl Into<String>) -> Self {
        self.scope_key = Some(scope_key.into());
        self
    }

    pub fn child(&self, kind: SpanKind) -> Self {
        Self {
            span_id: SpanId::new(),
            kind,
            scope_key: None,
            parent: Some(Box::new(self.clone())),
        }
    }

    pub fn derived_child(&self, kind: SpanKind, scope_key: impl Into<String>) -> Self {
        Self::derived(kind, scope_key, Some(self))
    }
}

/// One frame in the owning tool-call chain for nested routing and tracing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallFrame {
    pub tool_call_id: String,
    pub tool_name: String,
}

/// Serializable route used to resume an interrupt back into the correct nested owner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResumeRoute {
    pub interrupt_id: InterruptId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_chain: Vec<ToolCallFrame>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_thread_id: Option<ThreadId>,
}

impl ResumeRoute {
    pub fn new(interrupt_id: InterruptId) -> Self {
        Self {
            interrupt_id,
            call_chain: Vec::new(),
            target_run_id: None,
            target_thread_id: None,
        }
    }

    pub fn push_frame(
        mut self,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
    ) -> Self {
        self.call_chain.push(ToolCallFrame {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
        });
        self
    }
}

/// Mutable serializable portion of the chat context.
///
/// This is intentionally **not** the agent trajectory itself. Conversation
/// messages, externally supplied tools, and resume inputs belong to the
/// request surface. `ChatCtxState` is reserved for cross-cutting context that
/// must flow through nested tools/layers/subagents, such as tracing lineage,
/// cancellation-adjacent metadata, and tool-managed shared user state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCtxState {
    #[serde(default)]
    pub user_state: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_tool_chain: Vec<ToolCallFrame>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<SpanNode>,
}

impl Default for ChatCtxState {
    fn default() -> Self {
        Self {
            user_state: Value::Null,
            metadata: None,
            active_tool_chain: Vec::new(),
            span: None,
        }
    }
}

impl ChatCtxState {
    pub fn with_user_state(mut self, user_state: Value) -> Self {
        self.user_state = user_state;
        self
    }

    pub fn child_span(&mut self, kind: SpanKind, scope_key: impl Into<String>) -> SpanNode {
        let scope_key = scope_key.into();
        let next = match &self.span {
            Some(span) => span.child(kind),
            None => SpanNode::new(kind),
        }
        .with_scope_key(scope_key);
        self.span = Some(next.clone());
        next
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChatCtxSnapshot {
    pub thread_id: ThreadId,
    pub run_id: RunId,
    #[serde(flatten)]
    pub state: ChatCtxState,
}

#[derive(Debug, Clone, Default)]
struct ChatCtxOverlay {
    active_tool_chain: Vec<ToolCallFrame>,
    span: Option<SpanNode>,
}

#[derive(Debug)]
struct CancellationInner {
    cancelled: AtomicBool,
    children: Mutex<Vec<Weak<CancellationInner>>>,
}

impl CancellationInner {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            children: Mutex::new(Vec::new()),
        }
    }
}

/// Runtime-only cancellation token with parent-child propagation.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    inner: Arc<CancellationInner>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(CancellationInner::new()),
        }
    }

    pub fn child_token(&self) -> Self {
        let child = Self::new();
        if self.is_cancelled() {
            child.cancel();
            return child;
        }

        self.inner
            .children
            .lock()
            .unwrap()
            .push(Arc::downgrade(&child.inner));
        child
    }

    pub fn cancel(&self) {
        if self.inner.cancelled.swap(true, Ordering::SeqCst) {
            return;
        }

        let children = self.inner.children.lock().unwrap().clone();
        for child in children {
            if let Some(child) = child.upgrade() {
                CancellationToken { inner: child }.cancel();
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }
}

/// Runtime-only sidecar for [`ChatCtx`].
#[derive(Debug, Clone, Default)]
pub struct ChatRuntime {
    cancellation: CancellationToken,
}

impl ChatRuntime {
    pub fn new() -> Self {
        Self {
            cancellation: CancellationToken::new(),
        }
    }

    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub fn child(&self) -> Self {
        Self {
            cancellation: self.cancellation.child_token(),
        }
    }
}

/// Shared chat context handle with serializable state and runtime-only sidecar.
///
/// `ChatCtx` is the thread that ties a run together across nested calls. It is
/// for tracing, cancellation, shared metadata, and tool/layer-owned mutable
/// state, not for carrying the request payload that drives the next turn.
#[derive(Debug, Clone)]
pub struct ChatCtx {
    thread_id: ThreadId,
    run_id: RunId,
    state: Arc<RwLock<ChatCtxState>>,
    runtime: ChatRuntime,
    overlay: ChatCtxOverlay,
}

impl Default for ChatCtx {
    fn default() -> Self {
        Self::new(ChatCtxState::default())
    }
}

impl ChatCtx {
    pub fn new(state: ChatCtxState) -> Self {
        Self::with_ids(ThreadId::new(), RunId::new(), state)
    }

    pub fn with_ids(thread_id: ThreadId, run_id: RunId, state: ChatCtxState) -> Self {
        Self {
            thread_id,
            run_id,
            state: Arc::new(RwLock::new(state)),
            runtime: ChatRuntime::new(),
            overlay: ChatCtxOverlay::default(),
        }
    }

    pub fn from_parts(
        thread_id: ThreadId,
        run_id: RunId,
        state: ChatCtxState,
        runtime: ChatRuntime,
    ) -> Self {
        Self::from_shared_parts(thread_id, run_id, Arc::new(RwLock::new(state)), runtime)
    }

    fn from_shared_parts(
        thread_id: ThreadId,
        run_id: RunId,
        state: Arc<RwLock<ChatCtxState>>,
        runtime: ChatRuntime,
    ) -> Self {
        Self {
            thread_id,
            run_id,
            state,
            runtime,
            overlay: ChatCtxOverlay::default(),
        }
    }

    pub(crate) fn snapshot(&self) -> ChatCtxSnapshot {
        let mut state = self.state.read().unwrap().clone();
        if !self.overlay.active_tool_chain.is_empty() {
            state
                .active_tool_chain
                .extend(self.overlay.active_tool_chain.clone());
        }
        if self.overlay.span.is_some() {
            state.span = self.overlay.span.clone();
        }

        ChatCtxSnapshot {
            thread_id: self.thread_id.clone(),
            run_id: self.run_id.clone(),
            state,
        }
    }

    pub fn update(&self, f: impl FnOnce(&mut ChatCtxState)) {
        f(&mut self.state.write().unwrap());
    }

    pub fn runtime(&self) -> &ChatRuntime {
        &self.runtime
    }

    pub fn user_state(&self) -> Value {
        self.state.read().unwrap().user_state.clone()
    }

    pub fn with_user_state<T>(&self, f: impl FnOnce(&Value) -> T) -> T {
        f(&self.state.read().unwrap().user_state)
    }

    pub fn update_user_state<T>(&self, f: impl FnOnce(&mut Value) -> T) -> T {
        f(&mut self.state.write().unwrap().user_state)
    }

    pub fn set_user_state(&self, user_state: Value) {
        self.update(|state| state.user_state = user_state);
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread_id.clone()
    }

    pub fn run_id(&self) -> RunId {
        self.run_id.clone()
    }

    pub fn metadata(&self) -> Option<Value> {
        self.state.read().unwrap().metadata.clone()
    }

    pub fn cancel(&self) {
        self.runtime.cancellation.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.runtime.cancellation.is_cancelled()
    }

    pub fn child(&self) -> Self {
        self.fork()
    }

    pub fn fork(&self) -> Self {
        Self {
            thread_id: self.thread_id.clone(),
            run_id: self.run_id.clone(),
            state: Arc::clone(&self.state),
            runtime: self.runtime.child(),
            overlay: self.overlay.clone(),
        }
    }

    pub fn fork_for_tool(
        &self,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
    ) -> Self {
        let tool_call_id = tool_call_id.into();
        let tool_name = tool_name.into();
        let scope_key = format!("{tool_name}:{tool_call_id}");
        let parent_span = self.snapshot().state.span;
        let mut overlay = self.overlay.clone();

        overlay.active_tool_chain.push(ToolCallFrame {
            tool_call_id,
            tool_name,
        });
        overlay.span = Some(match parent_span.as_ref() {
            Some(span) => span.derived_child(SpanKind::Tool, scope_key),
            None => SpanNode::derived(SpanKind::Tool, scope_key, None),
        });

        Self {
            thread_id: self.thread_id.clone(),
            run_id: self.run_id.clone(),
            state: Arc::clone(&self.state),
            runtime: self.runtime.child(),
            overlay,
        }
    }
}

impl Serialize for ChatCtx {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.snapshot().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ChatCtx {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let snapshot = ChatCtxSnapshot::deserialize(deserializer)?;
        Ok(ChatCtx::with_ids(
            snapshot.thread_id,
            snapshot.run_id,
            snapshot.state,
        ))
    }
}

// ── Multimodal Content ────────────────────────────────────────────────────────

/// The content of a message, compatible with the OpenAI `content` field.
///
/// Can be plain text (`Text`) or a sequence of typed parts (`Parts`).
/// Use [`Content::text`] for the common text-only case and
/// [`Content::parts`] when mixing text with images or audio.
///
/// # Examples
///
/// ```ignore
/// use remi_agentloop_core::types::{Content, ContentPart};
///
/// // Plain text
/// let c = Content::text("Hello!");
///
/// // Mixed image + text
/// let c = Content::parts(vec![
///     ContentPart::text("Describe this image:"),
///     ContentPart::image_url("https://example.com/photo.jpg"),
/// ]);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl Content {
    pub fn text(s: impl Into<String>) -> Self {
        Content::Text(s.into())
    }
    pub fn parts(parts: Vec<ContentPart>) -> Self {
        Content::Parts(parts)
    }

    pub fn text_content(&self) -> String {
        match self {
            Content::Text(s) => s.clone(),
            Content::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    pub fn is_multimodal(&self) -> bool {
        matches!(self, Content::Parts(parts) if parts.iter().any(|p| !matches!(p, ContentPart::Text { .. })))
    }
}

/// A single typed content part inside a [`Content::Parts`] message.
///
/// Corresponds to an OpenAI multimodal `content` part object.
/// Construct parts using the associated helper methods:
/// [`ContentPart::text`], [`ContentPart::image_url`], [`ContentPart::image_base64`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlDetail },

    #[serde(rename = "image_base64")]
    ImageBase64 { media_type: String, data: String },

    #[serde(rename = "input_audio")]
    Audio { input_audio: AudioDetail },

    #[serde(rename = "file")]
    File {
        file_id: Option<String>,
        filename: Option<String>,
        media_type: Option<String>,
        data: Option<String>,
    },
}

impl ContentPart {
    pub fn text(s: impl Into<String>) -> Self {
        ContentPart::Text { text: s.into() }
    }
    pub fn image_url(url: impl Into<String>) -> Self {
        ContentPart::ImageUrl {
            image_url: ImageUrlDetail {
                url: url.into(),
                detail: None,
            },
        }
    }
    pub fn image_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        ContentPart::ImageBase64 {
            media_type: media_type.into(),
            data: data.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrlDetail {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDetail {
    pub data: String,
    pub format: String,
}

// ── Role & Message ────────────────────────────────────────────────────────────

/// The role of a participant in a conversation.
///
/// Matches the OpenAI Chat Completions `role` field names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// A system-level instruction (sent before the first user message).
    System,
    /// A message from the human user.
    User,
    /// A reply generated by the language model.
    Assistant,
    /// The result of a model-requested tool call.
    Tool,
}

/// A single message in a conversation thread.
///
/// Use the constructor helpers for the most common cases:
/// [`Message::user`], [`Message::assistant`], [`Message::system`],
/// [`Message::tool_result`].
///
/// # Example
///
/// ```ignore
/// use remi_agentloop_core::types::Message;
///
/// let history = vec![
///     Message::system("You are a concise assistant."),
///     Message::user("What is the capital of France?"),
///     Message::assistant("Paris."),
/// ];
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub id: MessageId,
    pub role: Role,
    pub content: Content,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Optional user identifier for `Role::User` messages.
    ///
    /// Maps to the `name` field in OpenAI-compatible request bodies.
    /// Useful for multi-user scenarios or end-user abuse monitoring.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Chain-of-thought / reasoning text returned by thinking models (e.g. Kimi K2.5).
    /// Must be echoed back verbatim when replaying the conversation history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// User-defined metadata attached to this message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::User,
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            metadata: None,
        }
    }

    /// Create a user message with an explicit user identifier.
    ///
    /// The `user_id` is serialised as the `name` field in OpenAI-compatible
    /// request bodies, useful for multi-user conversations.
    pub fn user_with_id(text: impl Into<String>, user_id: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::User,
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
            name: Some(user_id.into()),
            reasoning_content: None,
            metadata: None,
        }
    }

    pub fn user_content(content: Content) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::User,
            content,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            metadata: None,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::System,
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            metadata: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Assistant,
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            metadata: None,
        }
    }

    pub fn assistant_with_tool_calls(
        text: impl Into<String>,
        tool_calls: Vec<ToolCallMessage>,
        reasoning_content: Option<String>,
    ) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Assistant,
            content: Content::text(text),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
            reasoning_content,
            metadata: None,
        }
    }

    /// Set the user identifier on this message (maps to the `name` field).
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the user identifier from an `Option` — no-op if `None`.
    pub fn with_name_opt(mut self, name: Option<String>) -> Self {
        if let Some(n) = name {
            self.name = Some(n);
        }
        self
    }

    /// Attach user-defined metadata to this message.
    pub fn with_metadata(mut self, metadata: impl Into<Value>) -> Self {
        self.metadata = Some(metadata.into());
        self
    }

    pub fn tool_result(tool_call_id: impl Into<String>, result: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Tool,
            content: Content::text(result),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
            reasoning_content: None,
            metadata: None,
        }
    }

    /// Tool result with rich content (text and/or images).
    pub fn tool_result_content(tool_call_id: impl Into<String>, content: Content) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Tool,
            content,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
            reasoning_content: None,
            metadata: None,
        }
    }

    pub fn user_multimodal(parts: Vec<ContentPart>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::User,
            content: Content::parts(parts),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            metadata: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallMessage {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

// ── ModelRequest / ChatResponseChunk ─────────────────────────────────────────

use crate::config::RateLimitRetryPolicy;
use crate::tool::ToolDefinition;

#[derive(Debug, Clone, Serialize)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Internal retry policy for rate-limited model calls.
    #[serde(skip)]
    pub rate_limit_retry: Option<RateLimitRetryPolicy>,
    /// Provider-specific extra parameters merged into the top-level request body.
    ///
    /// Keys are flattened directly into the JSON object, enabling any
    /// OpenAI-compatible parameter not otherwise modelled here (e.g.
    /// `top_p`, `presence_penalty`, vendor extensions).
    /// Populated from [`AgentBuilder::extra_options`].
    #[serde(flatten)]
    pub extra_body: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub enum ChatResponseChunk {
    Delta {
        content: String,
        role: Option<Role>,
    },
    /// Chain-of-thought / thinking content from reasoning models (e.g. Kimi K2.5, DeepSeek-R1).
    ReasoningDelta {
        content: String,
    },
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    ToolCallDelta {
        index: usize,
        arguments_delta: String,
    },
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
    },
    Done,
}

// ── ChatRequest ──────────────────────────────────────────────────────────────

/// New top-level chat request used with `chat(ctx, request)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatRequest {
    Start {
        message: Message,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        history: Vec<Message>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        external_tools: Vec<crate::tool::ToolDefinition>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        temperature: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_tokens: Option<u32>,
        #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
        extra_body: serde_json::Map<String, Value>,
    },
    Resume {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        payloads: Vec<ResumePayload>,
    },
}

impl ChatRequest {
    pub fn start(msg: impl Into<String>) -> Self {
        Self::Start {
            message: Message::user(msg),
            history: vec![],
            external_tools: vec![],
            model: None,
            temperature: None,
            max_tokens: None,
            extra_body: serde_json::Map::new(),
        }
    }

    pub fn resume(payloads: Vec<ResumePayload>) -> Self {
        Self::Resume { payloads }
    }
}

// ── AgentEvent ────────────────────────────────────────────────────────────────

use crate::error::AgentError;

/// Events streamed from the agent loop to the caller.
///
/// Consume these by polling the stream returned from `agent.chat(…)`.
/// The stream always terminates with [`AgentEvent::Done`],
/// [`AgentEvent::Interrupt`], or [`AgentEvent::Error`].
///
/// # Pattern match reference
///
/// ```ignore
/// while let Some(event) = stream.next().await {
///     match event {
///         AgentEvent::TextDelta(chunk)  => print!("{chunk}"),
///         AgentEvent::ToolCallStart { name, .. } => println!("[{name}]"),
///         AgentEvent::ToolResult { name, result, .. } => println!("→ {result}"),
///         AgentEvent::Done              => break,
///         AgentEvent::Error(e)          => return Err(e.into()),
///         _                             => {}
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub enum AgentEvent {
    RunStart {
        thread_id: ThreadId,
        run_id: RunId,
        metadata: Option<serde_json::Value>,
    },
    TextDelta(String),
    /// Emitted once when a thinking model begins its chain-of-thought.
    /// All events until `ThinkingEnd` occur conceptually inside the thinking phase.
    ThinkingStart,
    /// Emitted when the thinking phase ends. Carries the full accumulated reasoning text.
    ThinkingEnd {
        content: String,
    },
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        id: String,
        delta: String,
    },
    ToolDelta {
        id: String,
        name: String,
        delta: String,
    },
    ToolResult {
        id: String,
        name: String,
        result: String,
    },
    SubSession(SubSessionEvent),
    Interrupt {
        interrupts: Vec<InterruptInfo>,
    },
    TurnStart {
        turn: usize,
    },
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
    },
    Custom {
        event_type: String,
        extra: serde_json::Value,
    },
    Done,
    /// The run was cancelled by the user.  A `Cancelled` checkpoint has been
    /// saved; the conversation can be resumed from where it was interrupted.
    Cancelled,
    Error(AgentError),
    /// Full state checkpoint emitted at key lifecycle boundaries.
    /// Outer layers (e.g. `BuiltAgent`) intercept this for durable persistence
    /// and filter it out before reaching the consumer.
    ///
    /// Contains everything needed to resume execution after a crash or restart.
    Checkpoint(crate::checkpoint::Checkpoint),
    /// Tool calls that the inner agent loop cannot execute (not in its registry).
    /// The outer layer should execute these externally, then resume via
    /// `AgentLoop::run(state, Action::ToolResults(all_outcomes), false)`.
    ///
    /// `completed_results` contains outcomes of tools that **were** executed
    /// internally by this loop. The outer layer must merge its own results
    /// with these before resuming.
    NeedToolExecution {
        state: crate::state::AgentState,
        tool_calls: Vec<ParsedToolCall>,
        completed_results: Vec<ToolCallOutcome>,
    },
}

pub fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubSessionEvent {
    pub parent_tool_call_id: String,
    pub sub_thread_id: ThreadId,
    pub sub_run_id: RunId,
    pub agent_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub depth: u32,
    #[serde(flatten)]
    pub payload: SubSessionEventPayload,
}

impl SubSessionEvent {
    pub fn new(
        parent_tool_call_id: impl Into<String>,
        sub_thread_id: ThreadId,
        sub_run_id: RunId,
        agent_name: impl Into<String>,
        title: Option<String>,
        depth: u32,
        payload: SubSessionEventPayload,
    ) -> Self {
        Self {
            parent_tool_call_id: parent_tool_call_id.into(),
            sub_thread_id,
            sub_run_id,
            agent_name: agent_name.into(),
            title,
            depth,
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "sub_type", rename_all = "snake_case")]
pub enum SubSessionEventPayload {
    Start,
    Delta {
        content: String,
    },
    ThinkingStart,
    ThinkingEnd {
        content: String,
    },
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        id: String,
        delta: String,
    },
    ToolDelta {
        id: String,
        name: String,
        delta: String,
    },
    ToolResult {
        id: String,
        name: String,
        result: String,
    },
    TurnStart {
        turn: usize,
    },
    Done {
        #[serde(skip_serializing_if = "Option::is_none")]
        final_output: Option<String>,
    },
    Error {
        message: String,
    },
}

/// Details of a single tool interrupt — part of [`AgentEvent::Interrupt`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptInfo {
    pub interrupt_id: InterruptId,
    pub tool_call_id: String,
    pub tool_name: String,
    pub kind: String,
    pub data: serde_json::Value,
}

/// Data provided by the caller when resuming after an [`InterruptRequest`](crate::tool::InterruptRequest).
///
/// # Example
///
/// ```ignore
/// use remi_agentloop_core::types::ResumePayload;
///
/// // After collecting the user's confirmation:
/// let payloads = vec![
///     ResumePayload {
///         interrupt_id: interrupt_info.interrupt_id.clone(),
///         result: serde_json::json!({ "approved": true }),
///     },
/// ];
/// agent.chat(ChatInput::resume(payloads)).await?;
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumePayload {
    pub interrupt_id: InterruptId,
    pub result: serde_json::Value,
}

/// Versioned export of a chat/debug session for later replay or inspection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSessionBundle {
    pub version: u32,
    pub exported_at: chrono::DateTime<chrono::Utc>,
    pub thread_id: ThreadId,
    pub run_id: RunId,
    pub replay: ChatReplayCursor,
    pub state: crate::state::AgentState,
    #[serde(default)]
    pub checkpoints: Vec<crate::checkpoint::Checkpoint>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, Value>,
}

impl ChatSessionBundle {
    pub const VERSION: u32 = 1;

    pub fn new(state: crate::state::AgentState) -> Self {
        let replay = ChatReplayCursor::from_state(&state);
        Self {
            version: Self::VERSION,
            exported_at: chrono::Utc::now(),
            thread_id: state.thread_id.clone(),
            run_id: state.run_id.clone(),
            replay,
            state,
            checkpoints: Vec::new(),
            metadata: serde_json::Map::new(),
        }
    }

    pub fn with_checkpoints(mut self, checkpoints: Vec<crate::checkpoint::Checkpoint>) -> Self {
        self.checkpoints = checkpoints;
        self
    }

    pub fn with_metadata(mut self, metadata: serde_json::Map<String, Value>) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Location in the exported conversation history from which replay should start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatReplayCursor {
    pub start_message_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_message_id: Option<MessageId>,
}

impl ChatReplayCursor {
    pub fn from_state(state: &crate::state::AgentState) -> Self {
        let start_message_index = state.messages.len().saturating_sub(1);
        let start_message_id = state.messages.get(start_message_index).map(|msg| msg.id.clone());
        Self {
            start_message_index,
            start_message_id,
        }
    }
}

// ── Internal loop types (pub(crate)) ─────────────────────────────────────────

/// Parsed and fully accumulated tool call ready for execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Single tool call execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub id: String,
    pub name: String,
    pub result: String,
}

/// Outcome of executing a tool call externally.
///
/// Used with [`Action::ToolResults`](crate::state::Action::ToolResults) and
/// [`LoopInput::Resume`] to feed tool results back into the agent loop when
/// tool execution happens outside `AgentLoop` (e.g. in a composable outer layer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolCallOutcome {
    /// Tool executed successfully (content may include text and/or images)
    Result {
        tool_call_id: String,
        tool_name: String,
        content: Content,
    },
    /// Tool execution failed
    Error {
        tool_call_id: String,
        tool_name: String,
        error: String,
    },
}

// ── LoopInput ─────────────────────────────────────────────────────────────────

/// Unified input for `Agent::chat()` — used by `AgentLoop`, composable layers,
/// and the protocol/transport layer.
///
/// This is the caller-driven surface that advances the agent trajectory.
/// Conversation history, externally visible tool definitions, and resume data
/// all live here because they are part of the user's effective input.
///
/// Internal execution state is tracked separately in [`crate::state::AgentState`],
/// while [`ChatCtx`] carries cross-cutting context through the full run.
///
/// Merges the previous `LoopInput` and `ProtocolRequest` into a single
/// serialisable type that supports:
/// - Starting a new turn with text or multimodal content
/// - Resuming after `NeedToolExecution`
/// - Protocol-level overrides (model, temperature, max_tokens, metadata)
///
/// ```ignore
/// // Start a new conversation (String converts automatically):
/// agent.chat("hello".into()).await?;
///
/// // Start with multimodal content:
/// agent.chat(Content::parts(vec![
///     ContentPart::text("describe this image"),
///     ContentPart::image_url("https://example.com/img.png"),
/// ]).into()).await?;
///
/// // Start with history + extra tool definitions + overrides:
/// agent.chat(
///     LoopInput::start("hello")
///         .history(msgs)
///         .extra_tools(defs)
///         .model("gpt-4o")
///         .temperature(0.5)
/// ).await?;
///
/// // Resume after NeedToolExecution:
/// agent.chat(LoopInput::resume(state, outcomes)).await?;
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LoopInput {
    /// Start a new conversation turn
    #[serde(rename = "start")]
    Start {
        /// The user message that starts this turn.
        ///
        /// Request-level message creation lives here so the request fully
        /// describes the user input that drives the next trajectory step.
        message: Message,
        /// Conversation history from prior turns
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        history: Vec<Message>,
        /// Additional tool definitions injected by outer layers
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extra_tools: Vec<crate::tool::ToolDefinition>,
        /// Override model name for this request
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Override temperature for this request
        #[serde(skip_serializing_if = "Option::is_none")]
        temperature: Option<f64>,
        /// Override max tokens for this request
        #[serde(skip_serializing_if = "Option::is_none")]
        max_tokens: Option<u32>,
        /// Request metadata
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },
    /// Resume from a `NeedToolExecution` with completed tool results.
    ///
    /// `state` is the resumable internal runtime snapshot for the loop and its
    /// layers. `pending_interrupts` and `results` are still request data because
    /// they represent caller input that determines how execution continues.
    #[serde(rename = "resume")]
    Resume {
        state: crate::state::AgentState,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_interrupts: Vec<InterruptInfo>,
        results: Vec<ToolCallOutcome>,
    },
}

impl LoopInput {
    /// Create a `Start` input with a text message.
    pub fn start(msg: impl Into<String>) -> Self {
        Self::Start {
            message: Message::user(msg),
            history: vec![],
            extra_tools: vec![],
            model: None,
            temperature: None,
            max_tokens: None,
            metadata: None,
        }
    }

    /// Create a `Start` input with multimodal content.
    pub fn start_content(content: Content) -> Self {
        Self::Start {
            message: Message::user_content(content),
            history: vec![],
            extra_tools: vec![],
            model: None,
            temperature: None,
            max_tokens: None,
            metadata: None,
        }
    }

    pub fn start_message(message: Message) -> Self {
        Self::Start {
            message,
            history: vec![],
            extra_tools: vec![],
            model: None,
            temperature: None,
            max_tokens: None,
            metadata: None,
        }
    }

    /// Create a `Resume` input from state + tool results.
    pub fn resume(
        state: crate::state::AgentState,
        pending_interrupts: Vec<InterruptInfo>,
        results: Vec<ToolCallOutcome>,
    ) -> Self {
        Self::Resume {
            state,
            pending_interrupts,
            results,
        }
    }

    /// Builder: attach conversation history (only applies to `Start`).
    pub fn history(mut self, msgs: Vec<Message>) -> Self {
        if let Self::Start { history, .. } = &mut self {
            *history = msgs;
        }
        self
    }

    /// Builder: attach extra tool definitions (only applies to `Start`).
    pub fn extra_tools(mut self, defs: Vec<crate::tool::ToolDefinition>) -> Self {
        if let Self::Start { extra_tools, .. } = &mut self {
            *extra_tools = defs;
        }
        self
    }

    /// Builder: override model name (only applies to `Start`).
    pub fn model(mut self, m: impl Into<String>) -> Self {
        if let Self::Start { model, .. } = &mut self {
            *model = Some(m.into());
        }
        self
    }

    /// Builder: override temperature (only applies to `Start`).
    pub fn temperature(mut self, t: f64) -> Self {
        if let Self::Start { temperature, .. } = &mut self {
            *temperature = Some(t);
        }
        self
    }

    /// Builder: override max tokens (only applies to `Start`).
    pub fn max_tokens(mut self, n: u32) -> Self {
        if let Self::Start { max_tokens, .. } = &mut self {
            *max_tokens = Some(n);
        }
        self
    }

    /// Builder: set metadata (only applies to `Start`).
    pub fn metadata(mut self, v: serde_json::Value) -> Self {
        if let Self::Start { metadata, .. } = &mut self {
            *metadata = Some(v);
        }
        self
    }

    /// Builder: set metadata on the start message (only applies to `Start`).
    pub fn message_metadata(mut self, v: serde_json::Value) -> Self {
        if let Self::Start { message, .. } = &mut self {
            message.metadata = Some(v);
        }
        self
    }

    /// Builder: set the user identifier on the start message (only applies to `Start`).
    ///
    /// Serialised as the `name` field in OpenAI-compatible request bodies.
    pub fn user_name(mut self, name: impl Into<String>) -> Self {
        if let Self::Start { message, .. } = &mut self {
            message.name = Some(name.into());
        }
        self
    }

}

impl From<String> for LoopInput {
    fn from(s: String) -> Self {
        Self::start(s)
    }
}

impl From<&str> for LoopInput {
    fn from(s: &str) -> Self {
        Self::start(s)
    }
}

impl From<Content> for LoopInput {
    fn from(c: Content) -> Self {
        Self::start_content(c)
    }
}

// ── ChatInput ─────────────────────────────────────────────────────────────────

/// Unified input for `chat_in_thread` — covers both new messages and resume from interrupt.
///
/// ```ignore
/// // New user message (String converts automatically):
/// agent.chat_in_thread(&tid, "hello").await?;
///
/// // Resume from interrupt:
/// agent.chat_in_thread(&tid, ChatInput::Resume {
///     run_id,
///     completed_results: vec![],
///     pending_interrupts: interrupts,
///     payloads: vec![payload],
/// }).await?;
/// ```
#[derive(Debug, Clone)]
pub enum ChatInput {
    /// A new user message.
    Message { message: Message },
    /// Resume a previously interrupted run
    Resume {
        run_id: RunId,
        /// Tool calls that completed normally (before the interrupt)
        completed_results: Vec<ToolCallResult>,
        /// The interrupt(s) that were returned by the agent
        pending_interrupts: Vec<InterruptInfo>,
        /// User-provided payloads resolving each interrupt
        payloads: Vec<ResumePayload>,
    },
}

impl ChatInput {
    /// Create a plain text message input.
    pub fn text(msg: impl Into<String>) -> Self {
        ChatInput::Message {
            message: Message::user(msg),
        }
    }

    /// Create a multimodal message input (text + images, audio, etc.).
    pub fn multimodal(parts: Vec<ContentPart>) -> Self {
        ChatInput::Message {
            message: Message::user_multimodal(parts),
        }
    }

    /// Attach a user identifier to a `Message` input.
    pub fn with_user_name(self, name: impl Into<String>) -> Self {
        match self {
            ChatInput::Message { message } => ChatInput::Message {
                message: message.with_name(name),
            },
            other => other,
        }
    }

    pub fn with_message_metadata(self, metadata: impl Into<Value>) -> Self {
        match self {
            ChatInput::Message { message } => ChatInput::Message {
                message: message.with_metadata(metadata),
            },
            other => other,
        }
    }
}

impl From<String> for ChatInput {
    fn from(s: String) -> Self {
        ChatInput::Message {
            message: Message::user(s),
        }
    }
}

impl From<&str> for ChatInput {
    fn from(s: &str) -> Self {
        ChatInput::Message {
            message: Message::user(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_ctx_serialization_drops_runtime_cancel_state() {
        let ctx = ChatCtx::new(ChatCtxState::default().with_user_state(serde_json::json!({
            "todos": ["one"]
        })));
        ctx.cancel();

        let json = serde_json::to_value(&ctx).unwrap();
        assert!(json.get("thread_id").is_some());
        assert!(json.get("run_id").is_some());
        assert_eq!(json.get("user_state").unwrap()["todos"][0], "one");
        assert!(json.get("cancellation").is_none());

        let restored: ChatCtx = serde_json::from_value(json).unwrap();
        assert!(!restored.is_cancelled());
        assert_eq!(restored.user_state()["todos"][0], "one");
    }

    #[test]
    fn cancellation_token_propagates_to_children() {
        let parent = CancellationToken::new();
        let child = parent.child_token();

        parent.cancel();

        assert!(parent.is_cancelled());
        assert!(child.is_cancelled());
    }

    #[test]
    fn span_nodes_are_parent_linked() {
        let root = SpanNode::new(SpanKind::Run).with_scope_key("run:root");
        let child = root.child(SpanKind::Tool).with_scope_key("tool:search");

        assert_eq!(child.parent.as_ref().unwrap().span_id, root.span_id);
        assert_eq!(
            child.parent.as_ref().unwrap().scope_key.as_deref(),
            Some("run:root")
        );
    }
}
