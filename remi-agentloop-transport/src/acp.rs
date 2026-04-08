/// ACP (Agent Communication Protocol) — A complete protocol for agent-to-agent communication.
///
/// ACP defines a standalone wire format for multi-agent systems with:
/// - Agent discovery and capability negotiation
/// - Task delegation and coordination
/// - Streaming bidirectional communication
/// - State synchronization and context sharing
///
/// # Design Principles
///
/// 1. **Agent-Centric**: Built for agent collaboration, not just LLM chat
/// 2. **Capability-Driven**: Agents advertise skills; routing is automatic
/// 3. **Composable**: Agents can delegate to other agents recursively
/// 4. **Observable**: Full tracing of delegation chains and cross-agent calls
///
/// # Protocol Flow
///
/// ```text
/// Client                Router              Search Agent        Code Agent
///   |                     |                      |                  |
///   |-- AcpRequest ------>|                      |                  |
///   |   (route by cap)    |                      |                  |
///   |                     |-- AgentStart ------->|                  |
///   |<-- AgentStart ------|                      |                  |
///   |<-- ContentDelta ----|<-- ContentDelta -----|                  |
///   |<-- ToolCall --------|<-- ToolCall ---------|                  |
///   |                     |   (delegate to code) |                  |
///   |                     |-- DelegateRequest ---|----------------->|
///   |<-- DelegateStart ---|                      |                  |
///   |<-- ContentDelta ----|<--------------------- ContentDelta -----|
///   |<-- DelegateEnd ------|<--------------------- AgentEnd ---------|
///   |<-- ToolResult ------|<-- ToolResult -------|                  |
///   |<-- AgentEnd --------|<-- AgentEnd ---------|                  |
/// ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Core ACP Types ────────────────────────────────────────────────────────────

/// Unique identifier for an agent instance in the system.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(pub String);

impl AgentId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a conversation session (may span multiple agents).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a single task execution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

impl TaskId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a delegation operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DelegationId(pub String);

impl DelegationId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for DelegationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for DelegationId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── Agent Capabilities ────────────────────────────────────────────────────────

/// Tool parameter definition (JSON Schema compatible).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolParameter {
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub param_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
}

/// Tool definition exposed by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Vec<ToolParameter>,
    /// Additional metadata (e.g., cost, latency, rate limits)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Agent capabilities — what an agent can do.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCapabilities {
    /// Unique agent identifier
    pub agent_id: AgentId,
    /// Human-readable name
    pub name: String,
    /// Agent description / purpose
    pub description: String,
    /// Version string (e.g., "1.0.0")
    pub version: String,
    /// Available tools
    pub tools: Vec<AcpToolDefinition>,
    /// Domain expertise tags (e.g., ["code", "python", "debugging"])
    pub domains: Vec<String>,
    /// Languages supported (ISO 639-1 codes)
    #[serde(default)]
    pub languages: Vec<String>,
    /// Performance characteristics
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performance: Option<AgentPerformance>,
    /// Cost information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<AgentCost>,
    /// Arbitrary metadata
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Agent performance characteristics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPerformance {
    /// Average response time in milliseconds
    pub avg_latency_ms: u64,
    /// Maximum concurrent tasks supported
    pub max_concurrency: u32,
    /// Rate limit (requests per minute)
    pub rate_limit_rpm: u32,
}

/// Agent cost information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCost {
    /// Cost per 1K input tokens (or equivalent)
    pub input_cost: f64,
    /// Cost per 1K output tokens (or equivalent)
    pub output_cost: f64,
    /// Currency code (ISO 4217)
    pub currency: String,
}

// ── ACP Messages (Content) ────────────────────────────────────────────────────

/// Content part — text, image, audio, or file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AcpContentPart {
    #[serde(rename = "text")]
    Text {
        text: String,
    },
    #[serde(rename = "image")]
    Image {
        /// URL or base64 data URI
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    #[serde(rename = "audio")]
    Audio {
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    #[serde(rename = "file")]
    File {
        filename: String,
        source: String,
        mime_type: String,
    },
    #[serde(rename = "structured")]
    Structured {
        data: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        schema: Option<serde_json::Value>,
    },
}

/// Message content (multimodal).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AcpContent {
    Text(String),
    Parts(Vec<AcpContentPart>),
}

impl AcpContent {
    pub fn text(s: impl Into<String>) -> Self {
        AcpContent::Text(s.into())
    }

    pub fn parts(parts: Vec<AcpContentPart>) -> Self {
        AcpContent::Parts(parts)
    }

    pub fn text_content(&self) -> String {
        match self {
            AcpContent::Text(s) => s.clone(),
            AcpContent::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    AcpContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}

impl From<String> for AcpContent {
    fn from(s: String) -> Self {
        AcpContent::Text(s)
    }
}

impl From<&str> for AcpContent {
    fn from(s: &str) -> Self {
        AcpContent::Text(s.to_string())
    }
}

// ── ACP Request ───────────────────────────────────────────────────────────────

/// Main ACP request — sent from client to agent system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRequest {
    /// Session identifier (optional, created if not provided)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    
    /// User message content
    pub content: AcpContent,
    
    /// Target agent (optional, router will select if not provided)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_agent: Option<AgentId>,
    
    /// Routing hints for agent selection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingHints>,
    
    /// Conversation history (for stateless agents)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<AcpMessage>,
    
    /// Execution constraints
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraints: Option<ExecutionConstraints>,
    
    /// Request metadata
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Routing hints for agent selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingHints {
    /// Preferred domains
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<String>,
    
    /// Required tools
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_tools: Vec<String>,
    
    /// Preferred language
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    
    /// Cost preference ("low", "medium", "high", "any")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_preference: Option<String>,
    
    /// Latency preference ("low", "medium", "high", "any")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_preference: Option<String>,
}

/// Execution constraints for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConstraints {
    /// Maximum execution time in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    
    /// Maximum delegation depth
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_delegation_depth: Option<u32>,
    
    /// Maximum total cost allowed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_cost: Option<f64>,
    
    /// Whether tool execution requires approval
    #[serde(default)]
    pub require_tool_approval: bool,
}

/// A single message in conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpMessage {
    pub role: MessageRole,
    pub content: AcpContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Agent,
    System,
    Tool,
}

// ── ACP Response (Streaming Events) ──────────────────────────────────────────

/// ACP streaming response event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AcpEvent {
    /// Agent execution started
    #[serde(rename = "agent_start")]
    AgentStart {
        session_id: SessionId,
        task_id: TaskId,
        agent_id: AgentId,
        agent_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },

    /// Content delta (streaming text)
    #[serde(rename = "content_delta")]
    ContentDelta {
        task_id: TaskId,
        delta: String,
    },

    /// Thinking/reasoning phase started
    #[serde(rename = "thinking_start")]
    ThinkingStart {
        task_id: TaskId,
    },

    /// Thinking/reasoning content delta
    #[serde(rename = "thinking_delta")]
    ThinkingDelta {
        task_id: TaskId,
        delta: String,
    },

    /// Thinking/reasoning phase ended
    #[serde(rename = "thinking_end")]
    ThinkingEnd {
        task_id: TaskId,
        content: String,
    },

    /// Tool call started
    #[serde(rename = "tool_call_start")]
    ToolCallStart {
        task_id: TaskId,
        tool_call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },

    /// Tool execution progress
    #[serde(rename = "tool_progress")]
    ToolProgress {
        task_id: TaskId,
        tool_call_id: String,
        delta: String,
    },

    /// Tool call completed
    #[serde(rename = "tool_result")]
    ToolResult {
        task_id: TaskId,
        tool_call_id: String,
        result: AcpContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Tool approval required (interrupt)
    #[serde(rename = "tool_approval_required")]
    ToolApprovalRequired {
        task_id: TaskId,
        tool_call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
        reason: String,
    },

    /// Delegation started (agent calling another agent)
    #[serde(rename = "delegate_start")]
    DelegateStart {
        delegation_id: DelegationId,
        parent_task_id: TaskId,
        target_agent_id: AgentId,
        target_agent_name: String,
        task_description: String,
    },

    /// Delegation event (forwarded from delegated agent)
    #[serde(rename = "delegate_event")]
    DelegateEvent {
        delegation_id: DelegationId,
        event: Box<AcpEvent>,
    },

    /// Delegation completed
    #[serde(rename = "delegate_end")]
    DelegateEnd {
        delegation_id: DelegationId,
        result: AcpContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Agent query/discovery request
    #[serde(rename = "agent_query")]
    AgentQuery {
        query: AgentQueryRequest,
    },

    /// Agent query response
    #[serde(rename = "agent_query_response")]
    AgentQueryResponse {
        agents: Vec<AgentCapabilities>,
    },

    /// State synchronization between agents
    #[serde(rename = "state_sync")]
    StateSync {
        key: String,
        value: serde_json::Value,
        version: u64,
    },

    /// Usage/cost information
    #[serde(rename = "usage")]
    Usage {
        task_id: TaskId,
        input_tokens: u64,
        output_tokens: u64,
        cost: f64,
        currency: String,
    },

    /// Agent execution completed
    #[serde(rename = "agent_end")]
    AgentEnd {
        task_id: TaskId,
        status: TaskStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<AcpContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<AcpError>,
    },

    /// Error event (non-fatal)
    #[serde(rename = "error")]
    Error {
        task_id: TaskId,
        error: AcpError,
    },

    /// Trace event for observability
    #[serde(rename = "trace")]
    Trace {
        task_id: TaskId,
        level: TraceLevel,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },

    /// Heartbeat/keepalive
    #[serde(rename = "heartbeat")]
    Heartbeat {
        timestamp: i64,
    },
}

/// Task completion status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Success,
    Error,
    Cancelled,
    Timeout,
}

/// ACP error information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl std::fmt::Display for AcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for AcpError {}

/// Trace level for observability events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceLevel {
    Debug,
    Info,
    Warn,
    Error,
}

// ── Agent Discovery ───────────────────────────────────────────────────────────

/// Agent query request for discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentQueryRequest {
    /// Domain filter (OR logic)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<String>,
    
    /// Required tools (AND logic)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_tools: Vec<String>,
    
    /// Free-form search query
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    
    /// Language filter
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

// ── Agent Registry ────────────────────────────────────────────────────────────

use std::sync::{Arc, RwLock};

/// Agent registry for discovery and routing.
pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<AgentId, AgentCapabilities>>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register an agent with its capabilities.
    pub fn register(&self, capabilities: AgentCapabilities) {
        let mut agents = self.agents.write().unwrap();
        agents.insert(capabilities.agent_id.clone(), capabilities);
    }

    /// Unregister an agent.
    pub fn unregister(&self, agent_id: &AgentId) -> Option<AgentCapabilities> {
        let mut agents = self.agents.write().unwrap();
        agents.remove(agent_id)
    }

    /// Get agent capabilities by ID.
    pub fn get(&self, agent_id: &AgentId) -> Option<AgentCapabilities> {
        let agents = self.agents.read().unwrap();
        agents.get(agent_id).cloned()
    }

    /// List all registered agents.
    pub fn list(&self) -> Vec<AgentCapabilities> {
        let agents = self.agents.read().unwrap();
        agents.values().cloned().collect()
    }

    /// Query agents by criteria.
    pub fn query(&self, query: &AgentQueryRequest) -> Vec<AgentCapabilities> {
        let agents = self.agents.read().unwrap();
        agents
            .values()
            .filter(|cap| self.matches_query(cap, query))
            .cloned()
            .collect()
    }

    /// Select the best agent for a request.
    pub fn select_agent(&self, request: &AcpRequest) -> Option<AgentCapabilities> {
        // If target specified, use it
        if let Some(target) = &request.target_agent {
            return self.get(target);
        }

        // Use routing hints if provided
        if let Some(routing) = &request.routing {
            let query = AgentQueryRequest {
                domains: routing.domains.clone(),
                required_tools: routing.required_tools.clone(),
                query: None,
                language: routing.language.clone(),
            };
            let mut candidates = self.query(&query);
            
            // Sort by cost/latency preference
            if !candidates.is_empty() {
                // TODO: implement preference-based sorting
                return Some(candidates.remove(0));
            }
        }

        // Fallback: keyword-based routing
        let text = request.content.text_content().to_lowercase();
        let agents = self.agents.read().unwrap();
        
        for (_, cap) in agents.iter() {
            for domain in &cap.domains {
                if text.contains(&domain.to_lowercase()) {
                    return Some(cap.clone());
                }
            }
        }

        // Default: first registered agent
        agents.values().next().cloned()
    }

    fn matches_query(&self, cap: &AgentCapabilities, query: &AgentQueryRequest) -> bool {
        // Domain match (OR)
        let domain_match = query.domains.is_empty()
            || query
                .domains
                .iter()
                .any(|d| cap.domains.iter().any(|cd| cd.eq_ignore_ascii_case(d)));

        // Tool match (AND)
        let tool_match = query.required_tools.is_empty()
            || query
                .required_tools
                .iter()
                .all(|t| cap.tools.iter().any(|tool| tool.name == *t));

        // Language match
        let lang_match = query.language.as_ref().map_or(true, |lang| {
            cap.languages.is_empty() || cap.languages.iter().any(|l| l.eq_ignore_ascii_case(lang))
        });

        // Text search
        let text_match = query.query.as_ref().map_or(true, |q| {
            let q_lower = q.to_lowercase();
            cap.name.to_lowercase().contains(&q_lower)
                || cap.description.to_lowercase().contains(&q_lower)
                || cap.domains.iter().any(|d| d.to_lowercase().contains(&q_lower))
        });

        domain_match && tool_match && lang_match && text_match
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for AgentRegistry {
    fn clone(&self) -> Self {
        Self {
            agents: Arc::clone(&self.agents),
        }
    }
}

// ── ACP Agent Trait ───────────────────────────────────────────────────────────

use futures::Stream;
use std::future::Future;
use std::pin::Pin;

/// Core trait for ACP-compatible agents.
///
/// Any agent implementing this trait can participate in the ACP ecosystem.
pub trait AcpAgent: Send + Sync {
    /// Get this agent's capabilities.
    fn capabilities(&self) -> AgentCapabilities;

    /// Execute a task and stream events.
    fn execute(
        &self,
        request: AcpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Pin<Box<dyn Stream<Item = AcpEvent> + Send>>, AcpError>> + Send>>;
}

// ── ACP Router ────────────────────────────────────────────────────────────────

/// Agent router — dispatches requests to registered agents.
pub struct AcpRouter {
    registry: AgentRegistry,
    handlers: Arc<RwLock<HashMap<AgentId, Box<dyn AcpAgent>>>>,
}

impl Clone for AcpRouter {
    fn clone(&self) -> Self {
        Self {
            registry: self.registry.clone(),
            handlers: Arc::clone(&self.handlers),
        }
    }
}

impl AcpRouter {
    pub fn new(registry: AgentRegistry) -> Self {
        Self {
            registry,
            handlers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register an agent handler.
    pub fn register_agent(self, agent: Box<dyn AcpAgent>) -> Self {
        let capabilities = agent.capabilities();
        self.registry.register(capabilities.clone());
        
        let mut handlers = self.handlers.write().unwrap();
        handlers.insert(capabilities.agent_id, agent);
        drop(handlers);
        
        self
    }

    /// Get the registry (for external queries).
    pub fn registry(&self) -> &AgentRegistry {
        &self.registry
    }

    /// Route and execute a request.
    pub async fn execute(
        &self,
        request: AcpRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = AcpEvent> + Send>>, AcpError> {
        use async_stream::stream;

        // Select agent
        let capabilities = self.registry.select_agent(&request).ok_or_else(|| AcpError {
            code: "no_agent".into(),
            message: "No suitable agent found for this request".into(),
            details: None,
        })?;

        let agent_id = capabilities.agent_id.clone();

        // Get handler
        let handlers = self.handlers.read().unwrap();
        let handler = handlers.get(&agent_id).ok_or_else(|| AcpError {
            code: "agent_not_found".into(),
            message: format!("Agent '{}' not registered", agent_id),
            details: None,
        })?;

        // Execute via handler (need to work around borrow checker)
        let handler_ptr: *const dyn AcpAgent = &**handler as *const dyn AcpAgent;
        drop(handlers);

        // SAFETY: handler is stored in Arc<RwLock> and won't be dropped while executing
        let result_stream = unsafe { (*handler_ptr).execute(request).await? };

        Ok(result_stream)
    }
}

// ── ACP Client ────────────────────────────────────────────────────────────────

/// ACP HTTP client for remote agents.
pub struct AcpClient {
    endpoint: String,
    headers: Vec<(String, String)>,
}

impl AcpClient {
    #[cfg(feature = "http-client")]
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            headers: Vec::new(),
        }
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    pub fn with_bearer_token(self, token: &str) -> Self {
        self.with_header("Authorization", format!("Bearer {token}"))
    }

    /// Execute a request and stream events.
    #[cfg(feature = "http-client")]
    pub async fn execute(
        &self,
        request: AcpRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = AcpEvent> + Send>>, AcpError> {
        use async_stream::stream;
        use futures::StreamExt;

        let client = reqwest::Client::new();
        let body = serde_json::to_vec(&request).map_err(|e| AcpError {
            code: "serialize_error".into(),
            message: e.to_string(),
            details: None,
        })?;

        let mut req_builder = client
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .body(body);

        for (k, v) in &self.headers {
            req_builder = req_builder.header(k, v);
        }

        let response = req_builder.send().await.map_err(|e| AcpError {
            code: "http_error".into(),
            message: e.to_string(),
            details: None,
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AcpError {
                code: "http_error".into(),
                message: format!("HTTP {}: {}", status, body),
                details: None,
            });
        }

        // Parse SSE stream using bytes_stream
        let body_stream = response.bytes_stream();
        
        // Convert reqwest BytesStream to our HttpTransportError
        let converted_stream = body_stream.map(|result| {
            result
                .map(|bytes| bytes.to_vec())
                .map_err(|e| remi_core::error::HttpTransportError::new(e.to_string()))
        });

        let mut lines = crate::http::sse_lines(Box::pin(converted_stream));

        Ok(Box::pin(stream! {
            while let Some(line) = lines.next().await {
                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                if let Some(data) = line.strip_prefix("data: ") {
                    match serde_json::from_str::<AcpEvent>(data) {
                        Ok(event) => yield event,
                        Err(_) => continue,
                    }
                }
            }
        }))
    }

    /// Discover agents.
    #[cfg(feature = "http-client")]
    pub async fn discover(&self, query: AgentQueryRequest) -> Result<Vec<AgentCapabilities>, AcpError> {
        let request = AcpRequest {
            session_id: None,
            content: AcpContent::text("__acp_discover__"),
            target_agent: None,
            routing: None,
            history: vec![],
            constraints: None,
            metadata: {
                let mut map = HashMap::new();
                map.insert(
                    "acp_query".into(),
                    serde_json::to_value(&query).unwrap_or_default(),
                );
                map
            },
        };

        let mut stream = self.execute(request).await?;
        use futures::StreamExt;

        let mut agents = Vec::new();
        while let Some(event) = stream.next().await {
            if let AcpEvent::AgentQueryResponse { agents: found } = event {
                agents.extend(found);
            }
        }

        Ok(agents)
    }
}

// ── ACP Server ────────────────────────────────────────────────────────────────

#[cfg(feature = "http-server")]
pub mod server {
    use super::*;
    use axum::{
        extract::State,
        response::sse::{Event, Sse},
        routing::post,
        Json, Router,
    };
    use futures::{Stream, StreamExt};
    use std::convert::Infallible;
    use std::future::Future;
    use std::net::SocketAddr;
    use std::pin::Pin;
    use std::sync::Arc;

    type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

    /// ACP HTTP server — closure-based API like HttpSseServer.
    pub struct AcpServer<F> {
        handler: Arc<F>,
        bind_addr: SocketAddr,
    }

    impl<F, Fut, S> AcpServer<F>
    where
        F: Fn(AcpRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<S, AcpError>> + Send + 'static,
        S: Stream<Item = AcpEvent> + Send + 'static,
    {
        pub fn new(handler: F) -> Self {
            Self {
                handler: Arc::new(handler),
                bind_addr: ([0, 0, 0, 0], 8080).into(),
            }
        }

        pub fn bind(mut self, addr: impl Into<SocketAddr>) -> Self {
            self.bind_addr = addr.into();
            self
        }

        pub fn into_router(self) -> Router {
            let handler = self.handler;
            Router::new()
                .route("/acp", post(handle_acp_request::<F, Fut, S>))
                .with_state(handler)
        }

        pub async fn serve(self) -> Result<(), std::io::Error> {
            let addr = self.bind_addr;
            let app = self.into_router();
            let listener = tokio::net::TcpListener::bind(addr).await?;
            println!("🚀 ACP Server listening on http://{}", addr);
            axum::serve(listener, app).await
        }
    }

    async fn handle_acp_request<F, Fut, S>(
        State(handler): State<Arc<F>>,
        Json(request): Json<AcpRequest>,
    ) -> Sse<SseStream>
    where
        F: Fn(AcpRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<S, AcpError>> + Send + 'static,
        S: Stream<Item = AcpEvent> + Send + 'static,
    {
        match handler(request).await {
            Ok(stream) => {
                let event_stream: SseStream = Box::pin(stream.map(|event| {
                    Ok::<Event, Infallible>(
                        Event::default()
                            .event(event_type(&event))
                            .data(serde_json::to_string(&event).unwrap_or_default()),
                    )
                }));
                Sse::new(event_stream)
            }
            Err(e) => {
                let error_event = AcpEvent::Error {
                    task_id: TaskId::new(),
                    error: e,
                };
                let event_stream: SseStream = Box::pin(futures::stream::once(async move {
                    Ok::<Event, Infallible>(
                        Event::default()
                            .event("error")
                            .data(serde_json::to_string(&error_event).unwrap_or_default()),
                    )
                }));
                Sse::new(event_stream)
            }
        }
    }

    async fn handle_discover(
        State(router): State<Arc<AcpRouter>>,
        Json(query): Json<AgentQueryRequest>,
    ) -> Json<Vec<AgentCapabilities>> {
        let agents = router.registry().query(&query);
        Json(agents)
    }

    fn event_type(event: &AcpEvent) -> &'static str {
        match event {
            AcpEvent::AgentStart { .. } => "agent_start",
            AcpEvent::ContentDelta { .. } => "content_delta",
            AcpEvent::ThinkingStart { .. } => "thinking_start",
            AcpEvent::ThinkingDelta { .. } => "thinking_delta",
            AcpEvent::ThinkingEnd { .. } => "thinking_end",
            AcpEvent::ToolCallStart { .. } => "tool_call_start",
            AcpEvent::ToolProgress { .. } => "tool_progress",
            AcpEvent::ToolResult { .. } => "tool_result",
            AcpEvent::ToolApprovalRequired { .. } => "tool_approval_required",
            AcpEvent::DelegateStart { .. } => "delegate_start",
            AcpEvent::DelegateEvent { .. } => "delegate_event",
            AcpEvent::DelegateEnd { .. } => "delegate_end",
            AcpEvent::AgentQuery { .. } => "agent_query",
            AcpEvent::AgentQueryResponse { .. } => "agent_query_response",
            AcpEvent::StateSync { .. } => "state_sync",
            AcpEvent::Usage { .. } => "usage",
            AcpEvent::AgentEnd { .. } => "agent_end",
            AcpEvent::Error { .. } => "error",
            AcpEvent::Trace { .. } => "trace",
            AcpEvent::Heartbeat { .. } => "heartbeat",
        }
    }
}

#[cfg(feature = "http-server")]
pub use server::AcpServer;

// ── Example Implementation ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_registry() {
        let registry = AgentRegistry::new();
        
        let cap = AgentCapabilities {
            agent_id: AgentId::new("test"),
            name: "Test Agent".into(),
            description: "A test agent".into(),
            version: "1.0.0".into(),
            tools: vec![],
            domains: vec!["test".into()],
            languages: vec!["en".into()],
            performance: None,
            cost: None,
            metadata: HashMap::new(),
        };

        registry.register(cap.clone());
        assert_eq!(registry.get(&AgentId::new("test")).unwrap().name, "Test Agent");
    }

    #[test]
    fn test_agent_query() {
        let registry = AgentRegistry::new();
        
        registry.register(AgentCapabilities {
            agent_id: AgentId::new("search"),
            name: "Search Agent".into(),
            description: "Web search".into(),
            version: "1.0.0".into(),
            tools: vec![],
            domains: vec!["search".into(), "web".into()],
            languages: vec!["en".into()],
            performance: None,
            cost: None,
            metadata: HashMap::new(),
        });

        let results = registry.query(&AgentQueryRequest {
            domains: vec!["search".into()],
            required_tools: vec![],
            query: None,
            language: None,
        });

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].agent_id.0, "search");
    }

    #[test]
    fn test_content_serialization() {
        let content = AcpContent::text("hello");
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("hello"));

        let parts = AcpContent::parts(vec![
            AcpContentPart::Text { text: "test".into() },
        ]);
        let json = serde_json::to_string(&parts).unwrap();
        assert!(json.contains("test"));
    }
}
