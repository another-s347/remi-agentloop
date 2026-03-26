//! MCP Client Implementation
//!
//! Feature-rich HTTP-based MCP client with:
//! - Dynamic server discovery
//! - Schema introspection
//! - Connection state management
//! - Caching capabilities

use crate::protocol::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(String),
    
    #[error("JSON-RPC error [{code}]: {message}")]
    JsonRpc { code: i32, message: String },
    
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    
    #[error("Invalid response")]
    InvalidResponse,
    
    #[error("Not initialized. Call initialize() first.")]
    NotInitialized,
    
    #[error("Already initialized")]
    AlreadyInitialized,
}

#[cfg(feature = "client")]
impl From<reqwest::Error> for ClientError {
    fn from(e: reqwest::Error) -> Self {
        ClientError::Http(e.to_string())
    }
}

/// Connection state
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

/// Client statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientStats {
    pub requests_sent: u64,
    pub responses_received: u64,
    pub errors: u64,
    pub tools_called: u64,
    pub resources_read: u64,
    pub prompts_fetched: u64,
}

/// Cached server information
#[derive(Debug, Clone)]
struct ServerInfo {
    capabilities: ServerCapabilities,
    implementation: Implementation,
    protocol_version: String,
}

/// MCP HTTP Client with advanced features
#[cfg(feature = "client")]
pub struct McpClient {
    endpoint: String,
    client: reqwest::Client,
    next_id: AtomicI64,
    
    // State management
    state: Arc<RwLock<ConnectionState>>,
    server_info: Arc<RwLock<Option<ServerInfo>>>,
    
    // Caching
    tools_cache: Arc<RwLock<Option<Vec<Tool>>>>,
    resources_cache: Arc<RwLock<Option<Vec<Resource>>>>,
    prompts_cache: Arc<RwLock<Option<Vec<Prompt>>>>,
    
    // Statistics
    stats: Arc<RwLock<ClientStats>>,
}

#[cfg(feature = "client")]
impl McpClient {
    /// Create a new MCP client
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            client: reqwest::Client::new(),
            next_id: AtomicI64::new(1),
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            server_info: Arc::new(RwLock::new(None)),
            tools_cache: Arc::new(RwLock::new(None)),
            resources_cache: Arc::new(RwLock::new(None)),
            prompts_cache: Arc::new(RwLock::new(None)),
            stats: Arc::new(RwLock::new(ClientStats::default())),
        }
    }

    // ── Connection Management ──────────────────────────────────────────────────

    /// Get current connection state
    pub async fn state(&self) -> ConnectionState {
        self.state.read().await.clone()
    }

    /// Check if client is connected
    pub async fn is_connected(&self) -> bool {
        matches!(*self.state.read().await, ConnectionState::Connected)
    }

    /// Get server information (if connected)
    pub async fn server_info(&self) -> Option<Implementation> {
        self.server_info.read().await.as_ref().map(|info| info.implementation.clone())
    }

    /// Get server capabilities (if connected)
    pub async fn server_capabilities(&self) -> Option<ServerCapabilities> {
        self.server_info.read().await.as_ref().map(|info| info.capabilities.clone())
    }

    /// Initialize the connection
    pub async fn initialize(
        &self,
        client_name: impl Into<String>,
        client_version: impl Into<String>,
    ) -> Result<InitializeResult, ClientError> {
        // Check if already initialized
        {
            let state = self.state.read().await;
            if *state == ConnectionState::Connected {
                return Err(ClientError::AlreadyInitialized);
            }
        }

        // Set connecting state
        *self.state.write().await = ConnectionState::Connecting;

        let params = InitializeParams {
            protocol_version: MCP_VERSION.into(),
            capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: client_name.into(),
                version: client_version.into(),
            },
        };

        match self.call("initialize", Some(serde_json::to_value(params)?)).await {
            Ok(result) => {
                let init_result: InitializeResult = serde_json::from_value(result)?;
                
                // Cache server info
                *self.server_info.write().await = Some(ServerInfo {
                    capabilities: init_result.capabilities.clone(),
                    implementation: init_result.server_info.clone(),
                    protocol_version: init_result.protocol_version.clone(),
                });
                
                // Set connected state
                *self.state.write().await = ConnectionState::Connected;
                
                Ok(init_result)
            }
            Err(e) => {
                *self.state.write().await = ConnectionState::Error(e.to_string());
                Err(e)
            }
        }
    }

    /// Disconnect and clear all caches
    pub async fn disconnect(&self) {
        *self.state.write().await = ConnectionState::Disconnected;
        *self.server_info.write().await = None;
        self.clear_cache().await;
    }

    // ── Cache Management ───────────────────────────────────────────────────────

    /// Clear all cached data
    pub async fn clear_cache(&self) {
        *self.tools_cache.write().await = None;
        *self.resources_cache.write().await = None;
        *self.prompts_cache.write().await = None;
    }

    /// Clear specific cache
    pub async fn clear_tools_cache(&self) {
        *self.tools_cache.write().await = None;
    }

    pub async fn clear_resources_cache(&self) {
        *self.resources_cache.write().await = None;
    }

    pub async fn clear_prompts_cache(&self) {
        *self.prompts_cache.write().await = None;
    }

    // ── Statistics ─────────────────────────────────────────────────────────────

    /// Get client statistics
    pub async fn stats(&self) -> ClientStats {
        self.stats.read().await.clone()
    }

    /// Reset statistics
    pub async fn reset_stats(&self) {
        *self.stats.write().await = ClientStats::default();
    }

    // ── Tools ──────────────────────────────────────────────────────────────────

    /// List available tools (with caching)
    pub async fn list_tools(&self) -> Result<ListToolsResult, ClientError> {
        self.check_initialized().await?;
        
        // Check cache
        {
            let cache = self.tools_cache.read().await;
            if let Some(tools) = cache.as_ref() {
                return Ok(ListToolsResult {
                    tools: tools.clone(),
                    next_cursor: None,
                });
            }
        }
        
        // Fetch from server
        let result = self.call("tools/list", None).await?;
        let tools_result: ListToolsResult = serde_json::from_value(result)?;
        
        // Update cache
        *self.tools_cache.write().await = Some(tools_result.tools.clone());
        
        Ok(tools_result)
    }

    /// List tools without caching (force refresh)
    pub async fn list_tools_fresh(&self) -> Result<ListToolsResult, ClientError> {
        self.clear_tools_cache().await;
        self.list_tools().await
    }

    /// Get a specific tool by name
    pub async fn get_tool(&self, name: &str) -> Result<Option<Tool>, ClientError> {
        let tools = self.list_tools().await?;
        Ok(tools.tools.into_iter().find(|t| t.name == name))
    }

    /// Check if a tool exists
    pub async fn has_tool(&self, name: &str) -> Result<bool, ClientError> {
        Ok(self.get_tool(name).await?.is_some())
    }

    /// Get tool schema
    pub async fn get_tool_schema(&self, name: &str) -> Result<serde_json::Value, ClientError> {
        let tool = self.get_tool(name).await?
            .ok_or_else(|| ClientError::JsonRpc {
                code: mcp_error_codes::TOOL_NOT_FOUND,
                message: format!("Tool not found: {}", name),
            })?;
        Ok(tool.input_schema)
    }

    /// Call a tool
    pub async fn call_tool(
        &self,
        name: impl Into<String>,
        arguments: Option<serde_json::Value>,
    ) -> Result<CallToolResult, ClientError> {
        self.check_initialized().await?;
        
        let params = CallToolParams {
            name: name.into(),
            arguments,
        };
        
        let result = self.call("tools/call", Some(serde_json::to_value(params)?)).await?;
        
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.tools_called += 1;
        }
        
        Ok(serde_json::from_value(result)?)
    }

    // ── Resources ──────────────────────────────────────────────────────────────

    /// List available resources (with caching)
    pub async fn list_resources(&self) -> Result<ListResourcesResult, ClientError> {
        self.check_initialized().await?;
        
        // Check cache
        {
            let cache = self.resources_cache.read().await;
            if let Some(resources) = cache.as_ref() {
                return Ok(ListResourcesResult {
                    resources: resources.clone(),
                    next_cursor: None,
                });
            }
        }
        
        // Fetch from server
        let result = self.call("resources/list", None).await?;
        let resources_result: ListResourcesResult = serde_json::from_value(result)?;
        
        // Update cache
        *self.resources_cache.write().await = Some(resources_result.resources.clone());
        
        Ok(resources_result)
    }

    /// List resources without caching (force refresh)
    pub async fn list_resources_fresh(&self) -> Result<ListResourcesResult, ClientError> {
        self.clear_resources_cache().await;
        self.list_resources().await
    }

    /// Get a specific resource by URI
    pub async fn get_resource(&self, uri: &str) -> Result<Option<Resource>, ClientError> {
        let resources = self.list_resources().await?;
        Ok(resources.resources.into_iter().find(|r| r.uri == uri))
    }

    /// Check if a resource exists
    pub async fn has_resource(&self, uri: &str) -> Result<bool, ClientError> {
        Ok(self.get_resource(uri).await?.is_some())
    }

    /// Read a resource
    pub async fn read_resource(&self, uri: impl Into<String>) -> Result<ReadResourceResult, ClientError> {
        self.check_initialized().await?;
        
        let params = ReadResourceParams { uri: uri.into() };
        let result = self.call("resources/read", Some(serde_json::to_value(params)?)).await?;
        
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.resources_read += 1;
        }
        
        Ok(serde_json::from_value(result)?)
    }

    // ── Prompts ────────────────────────────────────────────────────────────────

    /// List available prompts (with caching)
    pub async fn list_prompts(&self) -> Result<ListPromptsResult, ClientError> {
        self.check_initialized().await?;
        
        // Check cache
        {
            let cache = self.prompts_cache.read().await;
            if let Some(prompts) = cache.as_ref() {
                return Ok(ListPromptsResult {
                    prompts: prompts.clone(),
                    next_cursor: None,
                });
            }
        }
        
        // Fetch from server
        let result = self.call("prompts/list", None).await?;
        let prompts_result: ListPromptsResult = serde_json::from_value(result)?;
        
        // Update cache
        *self.prompts_cache.write().await = Some(prompts_result.prompts.clone());
        
        Ok(prompts_result)
    }

    /// List prompts without caching (force refresh)
    pub async fn list_prompts_fresh(&self) -> Result<ListPromptsResult, ClientError> {
        self.clear_prompts_cache().await;
        self.list_prompts().await
    }

    /// Get a specific prompt by name
    pub async fn get_prompt_info(&self, name: &str) -> Result<Option<Prompt>, ClientError> {
        let prompts = self.list_prompts().await?;
        Ok(prompts.prompts.into_iter().find(|p| p.name == name))
    }

    /// Check if a prompt exists
    pub async fn has_prompt(&self, name: &str) -> Result<bool, ClientError> {
        Ok(self.get_prompt_info(name).await?.is_some())
    }

    /// Get a prompt with arguments
    pub async fn get_prompt(
        &self,
        name: impl Into<String>,
        arguments: Option<HashMap<String, String>>,
    ) -> Result<GetPromptResult, ClientError> {
        self.check_initialized().await?;
        
        let params = GetPromptParams {
            name: name.into(),
            arguments,
        };
        
        let result = self.call("prompts/get", Some(serde_json::to_value(params)?)).await?;
        
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.prompts_fetched += 1;
        }
        
        Ok(serde_json::from_value(result)?)
    }

    // ── Schema Introspection ───────────────────────────────────────────────────

    /// Get all schemas (tools, resources, prompts)
    pub async fn get_all_schemas(&self) -> Result<ServerSchemas, ClientError> {
        self.check_initialized().await?;
        
        let (tools, resources, prompts) = tokio::join!(
            self.list_tools(),
            self.list_resources(),
            self.list_prompts()
        );

        Ok(ServerSchemas {
            tools: tools?.tools,
            resources: resources?.resources,
            prompts: prompts?.prompts,
        })
    }

    /// Export all schemas as JSON
    pub async fn export_schemas_json(&self) -> Result<String, ClientError> {
        let schemas = self.get_all_schemas().await?;
        Ok(serde_json::to_string_pretty(&schemas)?)
    }

    /// Get server capabilities summary
    pub async fn get_capabilities_summary(&self) -> Result<CapabilitiesSummary, ClientError> {
        let info = self.server_info.read().await;
        let info = info.as_ref().ok_or(ClientError::NotInitialized)?;
        
        let schemas = self.get_all_schemas().await?;
        
        Ok(CapabilitiesSummary {
            server_name: info.implementation.name.clone(),
            server_version: info.implementation.version.clone(),
            protocol_version: info.protocol_version.clone(),
            tools_count: schemas.tools.len(),
            resources_count: schemas.resources.len(),
            prompts_count: schemas.prompts.len(),
            has_logging: info.capabilities.logging.is_some(),
            tools: schemas.tools.iter().map(|t| t.name.clone()).collect(),
            resources: schemas.resources.iter().map(|r| r.uri.clone()).collect(),
            prompts: schemas.prompts.iter().map(|p| p.name.clone()).collect(),
        })
    }

    // ── Health Check ───────────────────────────────────────────────────────────

    /// Ping the server to check if it's alive
    pub async fn ping(&self) -> Result<bool, ClientError> {
        // Try to list tools as a lightweight ping
        match self.call("tools/list", None).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Health check with detailed information
    pub async fn health_check(&self) -> HealthCheckResult {
        let state = self.state().await;
        let stats = self.stats().await;
        
        let server_reachable = self.ping().await.unwrap_or(false);
        
        let server_info = self.server_info().await;
        
        HealthCheckResult {
            state,
            server_reachable,
            server_info,
            stats,
        }
    }

    // ── Batch Operations ───────────────────────────────────────────────────────

    /// Call multiple tools in sequence
    pub async fn call_tools_batch(
        &self,
        calls: Vec<(String, Option<serde_json::Value>)>,
    ) -> Vec<Result<CallToolResult, ClientError>> {
        let mut results = Vec::new();
        for (name, args) in calls {
            results.push(self.call_tool(name, args).await);
        }
        results
    }

    /// Read multiple resources in parallel
    pub async fn read_resources_batch(
        &self,
        uris: Vec<String>,
    ) -> Vec<Result<ReadResourceResult, ClientError>> {
        let futures: Vec<_> = uris.into_iter()
            .map(|uri| self.read_resource(uri))
            .collect();
        
        futures::future::join_all(futures).await
    }

    // ── Dynamic Discovery ──────────────────────────────────────────────────────

    /// Search tools by name pattern
    pub async fn search_tools(&self, pattern: &str) -> Result<Vec<Tool>, ClientError> {
        let tools = self.list_tools().await?;
        Ok(tools.tools.into_iter()
            .filter(|t| t.name.contains(pattern) || 
                   t.description.as_ref().map_or(false, |d| d.contains(pattern)))
            .collect())
    }

    /// Search resources by URI or name pattern
    pub async fn search_resources(&self, pattern: &str) -> Result<Vec<Resource>, ClientError> {
        let resources = self.list_resources().await?;
        Ok(resources.resources.into_iter()
            .filter(|r| r.uri.contains(pattern) || 
                   r.name.contains(pattern) ||
                   r.description.as_ref().map_or(false, |d| d.contains(pattern)))
            .collect())
    }

    /// Search prompts by name pattern
    pub async fn search_prompts(&self, pattern: &str) -> Result<Vec<Prompt>, ClientError> {
        let prompts = self.list_prompts().await?;
        Ok(prompts.prompts.into_iter()
            .filter(|p| p.name.contains(pattern) || 
                   p.description.as_ref().map_or(false, |d| d.contains(pattern)))
            .collect())
    }

    /// Get tools by domain/category (based on name prefix)
    pub async fn get_tools_by_prefix(&self, prefix: &str) -> Result<Vec<Tool>, ClientError> {
        let tools = self.list_tools().await?;
        Ok(tools.tools.into_iter()
            .filter(|t| t.name.starts_with(prefix))
            .collect())
    }

    // ── Low-level API ──────────────────────────────────────────────────────────

    /// Low-level JSON-RPC call
    async fn call(
        &self,
        method: impl Into<String>,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, ClientError> {
        let id = RequestId::new_number(self.next_id.fetch_add(1, Ordering::SeqCst));
        let request = JsonRpcRequest::new(id.clone(), method, params);

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.requests_sent += 1;
        }

        let response = self
            .client
            .post(&self.endpoint)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let mut stats = self.stats.write().await;
            stats.errors += 1;
            return Err(ClientError::Http(format!("HTTP {}", response.status())));
        }

        let body = response.text().await?;
        
        // Try to parse as success response
        if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&body) {
            if resp.id == id {
                let mut stats = self.stats.write().await;
                stats.responses_received += 1;
                return Ok(resp.result);
            }
        }

        // Try to parse as error response
        if let Ok(err) = serde_json::from_str::<JsonRpcError>(&body) {
            let mut stats = self.stats.write().await;
            stats.errors += 1;
            return Err(ClientError::JsonRpc {
                code: err.error.code,
                message: err.error.message,
            });
        }

        Err(ClientError::InvalidResponse)
    }

    async fn check_initialized(&self) -> Result<(), ClientError> {
        let state = self.state.read().await;
        if *state != ConnectionState::Connected {
            return Err(ClientError::NotInitialized);
        }
        Ok(())
    }
}

// ── Helper Types ───────────────────────────────────────────────────────────────

/// All server schemas
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSchemas {
    pub tools: Vec<Tool>,
    pub resources: Vec<Resource>,
    pub prompts: Vec<Prompt>,
}

/// Capabilities summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesSummary {
    pub server_name: String,
    pub server_version: String,
    pub protocol_version: String,
    pub tools_count: usize,
    pub resources_count: usize,
    pub prompts_count: usize,
    pub has_logging: bool,
    pub tools: Vec<String>,
    pub resources: Vec<String>,
    pub prompts: Vec<String>,
}

/// Health check result
#[derive(Debug, Clone)]
pub struct HealthCheckResult {
    pub state: ConnectionState,
    pub server_reachable: bool,
    pub server_info: Option<Implementation>,
    pub stats: ClientStats,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_id_generation() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let client = McpClient::new("http://localhost:8080");
            let id1 = client.next_id.load(Ordering::SeqCst);
            let id2 = client.next_id.fetch_add(1, Ordering::SeqCst);
            assert_eq!(id1, 1);
            assert_eq!(id2, 1);
            assert_eq!(client.next_id.load(Ordering::SeqCst), 2);
        });
    }

    #[tokio::test]
    async fn test_connection_state() {
        let client = McpClient::new("http://localhost:8080");
        assert_eq!(client.state().await, ConnectionState::Disconnected);
        assert!(!client.is_connected().await);
    }

    #[tokio::test]
    async fn test_stats() {
        let client = McpClient::new("http://localhost:8080");
        let stats = client.stats().await;
        assert_eq!(stats.requests_sent, 0);
        assert_eq!(stats.tools_called, 0);
    }
}
