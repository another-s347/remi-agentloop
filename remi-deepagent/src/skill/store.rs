//! Skill store trait and implementations.
//!
//! Each skill is a named markdown document the model can create, read, list,
//! and delete.  The `FileSkillStore` persists skills to disk — one `.md` file
//! per skill — mirroring the Claude Code `.claude/commands/` convention.

use remi_core::error::AgentError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ── SkillStore trait ──────────────────────────────────────────────────────────

/// Persistent skill storage backend.
pub trait SkillStore: Send + Sync + 'static {
    /// Save (create or overwrite) a skill.  Returns the storage path/key.
    async fn save(&self, name: &str, content: &str) -> Result<String, AgentError>;
    /// Retrieve a skill's content.  Returns `None` if the skill doesn't exist.
    async fn get(&self, name: &str) -> Result<Option<String>, AgentError>;
    /// List all skill names.
    async fn list(&self) -> Result<Vec<String>, AgentError>;
    /// Delete a skill.  Returns `true` if it existed.
    async fn delete(&self, name: &str) -> Result<bool, AgentError>;
}

// ── FileSkillStore ────────────────────────────────────────────────────────────

/// Stores each skill as `<base_dir>/<name>.md`.
///
/// Defaults to `.deepagent/skills/` relative to the current working directory,
/// matching the Claude Code `.claude/commands/` convention.
#[derive(Clone)]
pub struct FileSkillStore {
    base_dir: PathBuf,
}

impl FileSkillStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self { base_dir: base_dir.into() }
    }

    /// Default location: `<cwd>/.deepagent/skills/`
    pub fn default_dir() -> Self {
        Self::new(".deepagent/skills")
    }

    fn path_for(&self, name: &str) -> PathBuf {
        // Sanitise: replace any path separators in name
        let safe = name.replace(['/', '\\', '.'], "_");
        self.base_dir.join(format!("{safe}.md"))
    }
}

impl SkillStore for FileSkillStore {
    async fn save(&self, name: &str, content: &str) -> Result<String, AgentError> {
        tokio::fs::create_dir_all(&self.base_dir)
            .await
            .map_err(|e| AgentError::Io(e.to_string()))?;
        let path = self.path_for(name);
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| AgentError::Io(e.to_string()))?;
        Ok(path.to_string_lossy().to_string())
    }

    async fn get(&self, name: &str) -> Result<Option<String>, AgentError> {
        let path = self.path_for(name);
        match tokio::fs::read_to_string(&path).await {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(AgentError::Io(e.to_string())),
        }
    }

    async fn list(&self) -> Result<Vec<String>, AgentError> {
        let dir = &self.base_dir;
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut names = vec![];
        let mut rd = tokio::fs::read_dir(dir)
            .await
            .map_err(|e| AgentError::Io(e.to_string()))?;
        while let Ok(Some(entry)) = rd.next_entry().await {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("md") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    names.push(stem.to_string());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    async fn delete(&self, name: &str) -> Result<bool, AgentError> {
        let path = self.path_for(name);
        match tokio::fs::remove_file(&path).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(AgentError::Io(e.to_string())),
        }
    }
}

// ── InMemorySkillStore ────────────────────────────────────────────────────────

/// In-memory skill store — useful for tests and WASM targets.
#[derive(Clone, Default)]
pub struct InMemorySkillStore {
    map: Arc<Mutex<HashMap<String, String>>>,
}

impl InMemorySkillStore {
    pub fn new() -> Self { Self::default() }
}

impl SkillStore for InMemorySkillStore {
    async fn save(&self, name: &str, content: &str) -> Result<String, AgentError> {
        self.map.lock().unwrap().insert(name.to_string(), content.to_string());
        Ok(format!("memory:{name}"))
    }

    async fn get(&self, name: &str) -> Result<Option<String>, AgentError> {
        Ok(self.map.lock().unwrap().get(name).cloned())
    }

    async fn list(&self) -> Result<Vec<String>, AgentError> {
        let mut names: Vec<String> = self.map.lock().unwrap().keys().cloned().collect();
        names.sort();
        Ok(names)
    }

    async fn delete(&self, name: &str) -> Result<bool, AgentError> {
        Ok(self.map.lock().unwrap().remove(name).is_some())
    }
}
