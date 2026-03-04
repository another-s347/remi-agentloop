//! TOML-based configuration for `deep-agent`.
//!
//! ## Config file lookup order
//! 1. `--config <path>` CLI argument
//! 2. `REMI_CONFIG` env var
//! 3. `./deep-agent.toml` in the current directory
//! 4. Fall back to defaults + env vars (backward compatibility)
//!
//! ## Example `deep-agent.toml`
//! ```toml
//! [model]
//! api_key  = "sk-..."
//! base_url = "https://api.openai.com/v1"   # optional
//! model    = "gpt-4o"
//!
//! [agent]
//! # system = "You are..."   # optional – uses built-in default if absent
//! max_turns              = 20
//! workspace_dir          = ".deepagent/workspace"
//! task_sub_agent_turns   = 10
//! result_spill_threshold = 4096
//!
//! [search]
//! tavily_api_key = "tvly-..."   # optional – omit to disable web_search
//! ```

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::agent::DeepAgentBuilder;
use remi_core::model::ChatModel;

// ── Sub-sections ──────────────────────────────────────────────────────────────

/// `[model]` section — LLM provider settings.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModelConfig {
    /// API key for the LLM provider.
    /// Falls back to `OPENAI_API_KEY` / `REMI_API_KEY` env vars if empty.
    #[serde(default)]
    pub api_key: String,

    /// OpenAI-compatible base URL.
    /// Falls back to `REMI_BASE_URL` / `OPENAI_BASE_URL` env vars.
    pub base_url: Option<String>,

    /// Model name (e.g. `"gpt-4o"`, `"kimi-k2.5"`).
    #[serde(default = "defaults::model")]
    pub model: String,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            api_key: std::env::var("OPENAI_API_KEY")
                .or_else(|_| std::env::var("REMI_API_KEY"))
                .unwrap_or_default(),
            base_url: std::env::var("REMI_BASE_URL")
                .or_else(|_| std::env::var("OPENAI_BASE_URL"))
                .ok(),
            model: std::env::var("REMI_MODEL").unwrap_or_else(|_| defaults::model()),
        }
    }
}

/// `[agent]` section — agent behaviour settings.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AgentSection {
    /// System prompt.  Uses the built-in default when absent.
    pub system: Option<String>,

    /// Maximum number of agent turns (default 20).
    #[serde(default = "defaults::max_turns")]
    pub max_turns: usize,

    /// Workspace root directory — bash cwd + fs tool root + parent of skills.
    #[serde(default = "defaults::workspace_dir")]
    pub workspace_dir: PathBuf,

    /// Max turns for sub-agent task delegation (default 10).
    #[serde(default = "defaults::task_turns")]
    pub task_sub_agent_turns: usize,

    /// Tool output byte threshold: outputs larger than this are spilled to disk
    /// instead of flooding the model context (default 4096).
    #[serde(default = "defaults::spill_threshold")]
    pub result_spill_threshold: usize,
}

impl Default for AgentSection {
    fn default() -> Self {
        Self {
            system: std::env::var("REMI_SYSTEM").ok(),
            max_turns: defaults::max_turns(),
            workspace_dir: defaults::workspace_dir(),
            task_sub_agent_turns: defaults::task_turns(),
            result_spill_threshold: defaults::spill_threshold(),
        }
    }
}

/// `[search]` section — web search settings.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct SearchConfig {
    /// Tavily API key.  If absent, the `web_search` tool is not registered.
    pub tavily_api_key: Option<String>,
}

// ── Root config ───────────────────────────────────────────────────────────────

/// Complete deep-agent configuration loaded from `deep-agent.toml`.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct DeepAgentConfig {
    #[serde(default)]
    pub model: ModelConfig,
    #[serde(default)]
    pub agent: AgentSection,
    #[serde(default)]
    pub search: SearchConfig,
}

impl DeepAgentConfig {
    // ── Loading ───────────────────────────────────────────────────────────

    /// Find, load, and parse the config file.
    ///
    /// Search order:
    /// 1. `--config <path>` CLI arg
    /// 2. `REMI_CONFIG` env var
    /// 3. `./deep-agent.toml`
    /// 4. Defaults derived from environment variables (backward compat).
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(path) = Self::find_config_path() {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let mut cfg: DeepAgentConfig = toml::from_str(&content)
                .map_err(|e| format!("invalid TOML in {}: {e}", path.display()))?;
            // If api_key was left blank in the file, fall back to env
            if cfg.model.api_key.is_empty() {
                cfg.model.api_key = std::env::var("OPENAI_API_KEY")
                    .or_else(|_| std::env::var("REMI_API_KEY"))
                    .unwrap_or_default();
            }
            if cfg.model.base_url.is_none() {
                cfg.model.base_url = std::env::var("REMI_BASE_URL")
                    .or_else(|_| std::env::var("OPENAI_BASE_URL"))
                    .ok();
            }
            Ok(cfg)
        } else {
            Ok(Self::default())
        }
    }

    /// Return the config file path that would be used, without loading it.
    pub fn find_config_path() -> Option<PathBuf> {
        // 1. --config <path>
        let args: Vec<String> = std::env::args().collect();
        if let Some(idx) = args.iter().position(|a| a == "--config") {
            if let Some(p) = args.get(idx + 1) {
                return Some(PathBuf::from(p));
            }
        }
        // 2. REMI_CONFIG env
        if let Ok(p) = std::env::var("REMI_CONFIG") {
            return Some(PathBuf::from(p));
        }
        // 3. ./deep-agent.toml
        let local = PathBuf::from("deep-agent.toml");
        if local.exists() {
            return Some(local);
        }
        None
    }

    // ── Validation ────────────────────────────────────────────────────────

    /// Return an error if no API key is available.
    pub fn require_api_key(&self) -> Result<(), String> {
        if self.model.api_key.is_empty() {
            return Err(
                "No API key configured.\n\
                 Set api_key in deep-agent.toml [model] section,\n\
                 or set the OPENAI_API_KEY environment variable.\n\
                 Tip: run with --init to create an example deep-agent.toml."
                    .to_string(),
            );
        }
        Ok(())
    }

    // ── Builder application ───────────────────────────────────────────────

    /// Apply all agent settings from this config to a [`DeepAgentBuilder`].
    pub fn apply_to_builder<M>(&self, mut builder: DeepAgentBuilder<M>) -> DeepAgentBuilder<M>
    where
        M: ChatModel + Clone + Send + Sync + 'static,
    {
        builder = builder
            .max_turns(self.agent.max_turns)
            .task_sub_agent_turns(self.agent.task_sub_agent_turns)
            .result_spill_threshold(self.agent.result_spill_threshold)
            .workspace_dir(self.agent.workspace_dir.clone());

        if let Some(system) = &self.agent.system {
            builder = builder.system(system);
        }
        if let Some(key) = &self.search.tavily_api_key {
            builder = builder.tavily_api_key(key.clone());
        }
        builder
    }

    // ── Init helper ───────────────────────────────────────────────────────

    /// Return the content of an example `deep-agent.toml` with comments.
    pub fn example_toml() -> &'static str {
        r#"# deep-agent.toml — remi-deepagent configuration
# Run `deep-agent --init` to (re)generate this file.

[model]
# LLM provider API key.
# Can also be set via OPENAI_API_KEY or REMI_API_KEY environment variable.
api_key = ""

# OpenAI-compatible base URL (omit for api.openai.com).
# Examples:
#   base_url = "https://api.moonshot.cn/v1"    # Kimi / Moonshot
#   base_url = "https://api.deepseek.com/v1"   # DeepSeek
# base_url = "https://api.openai.com/v1"

# Model name.
model = "gpt-4o"

[agent]
# System prompt override (uses built-in default when commented out).
# system = "You are a helpful coding assistant."

# Maximum number of agent turns per chat.
max_turns = 20

# Workspace root — bash runs here, all fs paths are relative to this.
# Skills and tool-result spills are stored inside this directory.
workspace_dir = ".deepagent/workspace"

# Maximum turns for sub-agent task delegation.
task_sub_agent_turns = 10

# Tool outputs larger than this (bytes) are spilled to disk and a pointer
# is returned to the model instead of the full content.
result_spill_threshold = 4096

[search]
# Tavily web search API key. Omit or leave empty to disable web_search.
# Get one at https://tavily.com
# tavily_api_key = "tvly-..."
"#
    }

    /// Write `example_toml()` to `./deep-agent.toml`, refusing to overwrite.
    pub fn write_example(overwrite: bool) -> Result<(), Box<dyn std::error::Error>> {
        let path = PathBuf::from("deep-agent.toml");
        if path.exists() && !overwrite {
            return Err(format!(
                "deep-agent.toml already exists. Use --init --force to overwrite."
            )
            .into());
        }
        std::fs::write(&path, Self::example_toml())?;
        println!("Created {}", path.display());
        Ok(())
    }
}

// ── Defaults ──────────────────────────────────────────────────────────────────

mod defaults {
    use std::path::PathBuf;
    pub fn model() -> String { "gpt-4o".to_string() }
    pub fn max_turns() -> usize { 20 }
    pub fn workspace_dir() -> PathBuf { PathBuf::from(".deepagent/workspace") }
    pub fn task_turns() -> usize { 10 }
    pub fn spill_threshold() -> usize { 4096 }
}
