use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::error::AgentError;
use std::future::Future;

/// Runtime configuration for an agent invocation.
///
/// All fields are `Option` so that this struct can be serialised across
/// WASM guest/host boundaries: unset fields indicate “use the implementation
/// default”.  Build a config using the fluent setter methods or load from
/// environment variables with [`AgentConfig::from_env`].
///
/// # Example
///
/// ```ignore
/// use remi_agentloop_core::config::AgentConfig;
///
/// // Programmatic construction
/// let config = AgentConfig::new()
///     .with_api_key("sk-...")
///     .with_model("gpt-4o")
///     .with_temperature(0.7)
///     .with_max_tokens(2048);
///
/// // Load from REMI_* environment variables
/// let config = AgentConfig::from_env();
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,

    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub extra: serde_json::Value,
}

impl AgentConfig {
    pub fn new() -> Self { Self::default() }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into()); self
    }
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into()); self
    }
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into()); self
    }
    pub fn with_temperature(mut self, temp: f64) -> Self {
        self.temperature = Some(temp); self
    }
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n); self
    }
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into()); self
    }
    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = Some(ms); self
    }
    pub fn with_extra(mut self, extra: serde_json::Value) -> Self {
        self.extra = extra; self
    }

    /// Load from REMI_* environment variables
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_env() -> Self {
        Self {
            api_key: std::env::var("REMI_API_KEY").ok(),
            model: std::env::var("REMI_MODEL").ok(),
            base_url: std::env::var("REMI_BASE_URL").ok(),
            temperature: std::env::var("REMI_TEMPERATURE").ok().and_then(|s| s.parse().ok()),
            max_tokens: std::env::var("REMI_MAX_TOKENS").ok().and_then(|s| s.parse().ok()),
            timeout_ms: std::env::var("REMI_TIMEOUT_MS").ok().and_then(|s| s.parse().ok()),
            ..Default::default()
        }
    }

    /// Merge: fields from `other` override `self` when Some
    pub fn merge(mut self, other: &AgentConfig) -> Self {
        if other.api_key.is_some()    { self.api_key    = other.api_key.clone(); }
        if other.model.is_some()      { self.model      = other.model.clone(); }
        if other.base_url.is_some()   { self.base_url   = other.base_url.clone(); }
        if other.temperature.is_some(){ self.temperature = other.temperature; }
        if other.max_tokens.is_some() { self.max_tokens  = other.max_tokens; }
        if other.timeout_ms.is_some() { self.timeout_ms  = other.timeout_ms; }
        for (k, v) in &other.headers  { self.headers.insert(k.clone(), v.clone()); }
        if !other.extra.is_null()     { self.extra = other.extra.clone(); }
        self
    }
}

/// Dynamic config provider — called on each chat() invocation
pub trait ConfigProvider {
    fn resolve(&self) -> impl Future<Output = Result<AgentConfig, AgentError>>;
}

impl ConfigProvider for AgentConfig {
    fn resolve(&self) -> impl Future<Output = Result<AgentConfig, AgentError>> {
        let cfg = self.clone();
        async move { Ok(cfg) }
    }
}
