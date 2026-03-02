//! Checkpoint — durable snapshot of agent execution state.
//!
//! A [`Checkpoint`] captures the full [`AgentState`] plus the [`Action`]
//! needed to resume execution from that exact point.  The agent loop
//! emits checkpoints at key lifecycle boundaries:
//!
//! | Point                        | Status                        |
//! |------------------------------|-------------------------------|
//! | After `step()` text response | `StepDone`                    |
//! | Model requests tool calls    | `AwaitingToolExecution`       |
//! | After local tool execution   | `ToolsExecuted`               |
//! | Run completed normally       | `RunDone`                     |
//! | Run interrupted              | `Interrupted`                 |
//! | Run errored                  | `Errored`                     |
//!
//! ## Resuming from a checkpoint
//!
//! ```ignore
//! let cp = store.load_latest_by_thread(&thread_id).await?.unwrap();
//! let action = cp.pending_action.unwrap_or(Action::ToolResults(vec![]));
//! let stream = agent_loop.run(cp.state, action, false);
//! ```
//!
//! The [`CheckpointStore`] trait abstracts persistence.  An
//! [`InMemoryCheckpointStore`] is provided for testing; production
//! deployments can implement the trait over SQLite, Redis, DynamoDB, etc.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::error::AgentError;
use crate::state::{Action, AgentState};
use crate::types::{RunId, ThreadId};

// ── Checkpoint ────────────────────────────────────────────────────────────────

/// Unique identifier for a checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CheckpointId(pub String);

impl CheckpointId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for CheckpointId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Lifecycle status at the moment the checkpoint was captured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckpointStatus {
    /// `step()` completed with a text response (no tool calls).
    StepDone,
    /// Model requested tool calls; waiting for execution.
    AwaitingToolExecution,
    /// Local tools have been executed; about to start the next turn.
    ToolsExecuted,
    /// Run completed normally.
    RunDone,
    /// Run paused by a tool interrupt.
    Interrupted,
    /// An error occurred.
    Errored,
    /// Run was explicitly cancelled by the user.
    Cancelled,
}

/// Full snapshot of agent execution state at a lifecycle boundary.
///
/// Contains everything needed to resume execution:
/// - `state` — the [`AgentState`] (messages, config, phase, user_state, …)
/// - `pending_action` — the [`Action`] to feed into the *next* `step()` call
///   (e.g. `ToolResults` after tool execution).  `None` means the run
///   has reached a terminal state (`RunDone`, `Errored`, `Interrupted`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Unique checkpoint id.
    pub id: CheckpointId,
    /// Thread this checkpoint belongs to.
    pub thread_id: ThreadId,
    /// Run this checkpoint belongs to.
    pub run_id: RunId,
    /// Full agent state snapshot.
    pub state: AgentState,
    /// The action to resume with.  `None` for terminal checkpoints.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_action: Option<Action>,
    /// Turn counter at checkpoint time.
    pub turn: usize,
    /// Lifecycle status.
    pub status: CheckpointStatus,
    /// Monotonic sequence number within the run (0, 1, 2, …).
    pub sequence: u64,
    /// Timestamp (UTC).
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl Checkpoint {
    /// Create a new checkpoint.
    pub fn new(
        thread_id: ThreadId,
        run_id: RunId,
        state: AgentState,
        pending_action: Option<Action>,
        turn: usize,
        status: CheckpointStatus,
        sequence: u64,
    ) -> Self {
        Self {
            id: CheckpointId::new(),
            thread_id,
            run_id,
            state,
            pending_action,
            turn,
            status,
            sequence,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Whether this checkpoint represents a resumable (non-terminal) state.
    pub fn is_resumable(&self) -> bool {
        self.pending_action.is_some()
    }
}

// ── CheckpointStore trait ─────────────────────────────────────────────────────

/// Persistence backend for checkpoints.
///
/// Implementations can store checkpoints in memory, SQLite, Redis,
/// DynamoDB, or any other durable store.
///
/// All methods use RPITIT (return-position `impl Trait` in traits) —
/// no `Send` bound, compatible with wasm targets.
pub trait CheckpointStore {
    /// Save a checkpoint.
    fn save(&self, checkpoint: Checkpoint) -> impl Future<Output = Result<(), AgentError>>;

    /// Load the latest checkpoint for a given run.
    fn load_latest_by_run(
        &self,
        run_id: &RunId,
    ) -> impl Future<Output = Result<Option<Checkpoint>, AgentError>>;

    /// Load the latest checkpoint for a given thread (across all runs).
    fn load_latest_by_thread(
        &self,
        thread_id: &ThreadId,
    ) -> impl Future<Output = Result<Option<Checkpoint>, AgentError>>;

    /// List all checkpoints for a run, ordered by sequence number.
    fn list_by_run(
        &self,
        run_id: &RunId,
    ) -> impl Future<Output = Result<Vec<Checkpoint>, AgentError>>;
}

// ── NoCheckpointStore (marker) ────────────────────────────────────────────────

/// Marker store that discards all checkpoints.  Used by default when
/// no persistence is configured.
#[derive(Debug, Clone, Default)]
pub struct NoCheckpointStore;

impl CheckpointStore for NoCheckpointStore {
    fn save(&self, _checkpoint: Checkpoint) -> impl Future<Output = Result<(), AgentError>> {
        async { Ok(()) }
    }

    fn load_latest_by_run(
        &self,
        _run_id: &RunId,
    ) -> impl Future<Output = Result<Option<Checkpoint>, AgentError>> {
        async { Ok(None) }
    }

    fn load_latest_by_thread(
        &self,
        _thread_id: &ThreadId,
    ) -> impl Future<Output = Result<Option<Checkpoint>, AgentError>> {
        async { Ok(None) }
    }

    fn list_by_run(
        &self,
        _run_id: &RunId,
    ) -> impl Future<Output = Result<Vec<Checkpoint>, AgentError>> {
        async { Ok(vec![]) }
    }
}

// ── InMemoryCheckpointStore ───────────────────────────────────────────────────

/// In-memory checkpoint store — useful for testing and short-lived agents.
#[derive(Debug, Clone, Default)]
pub struct InMemoryCheckpointStore {
    inner: Arc<Mutex<CheckpointStoreInner>>,
}

#[derive(Debug, Default)]
struct CheckpointStoreInner {
    /// run_id → ordered list of checkpoints
    by_run: HashMap<String, Vec<Checkpoint>>,
    /// thread_id → latest checkpoint across all runs
    latest_by_thread: HashMap<String, Checkpoint>,
}

impl InMemoryCheckpointStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CheckpointStore for InMemoryCheckpointStore {
    fn save(&self, checkpoint: Checkpoint) -> impl Future<Output = Result<(), AgentError>> {
        let inner = self.inner.clone();
        async move {
            let mut guard = inner.lock().unwrap();
            // Update latest-by-thread
            guard
                .latest_by_thread
                .insert(checkpoint.thread_id.0.clone(), checkpoint.clone());
            // Append to run list
            guard
                .by_run
                .entry(checkpoint.run_id.0.clone())
                .or_default()
                .push(checkpoint);
            Ok(())
        }
    }

    fn load_latest_by_run(
        &self,
        run_id: &RunId,
    ) -> impl Future<Output = Result<Option<Checkpoint>, AgentError>> {
        let inner = self.inner.clone();
        let rid = run_id.clone();
        async move {
            let guard = inner.lock().unwrap();
            Ok(guard.by_run.get(&rid.0).and_then(|v| v.last()).cloned())
        }
    }

    fn load_latest_by_thread(
        &self,
        thread_id: &ThreadId,
    ) -> impl Future<Output = Result<Option<Checkpoint>, AgentError>> {
        let inner = self.inner.clone();
        let tid = thread_id.clone();
        async move {
            let guard = inner.lock().unwrap();
            Ok(guard.latest_by_thread.get(&tid.0).cloned())
        }
    }

    fn list_by_run(
        &self,
        run_id: &RunId,
    ) -> impl Future<Output = Result<Vec<Checkpoint>, AgentError>> {
        let inner = self.inner.clone();
        let rid = run_id.clone();
        async move {
            let guard = inner.lock().unwrap();
            Ok(guard.by_run.get(&rid.0).cloned().unwrap_or_default())
        }
    }
}

// ── Blanket impls for smart pointers ──────────────────────────────────────────

impl<S: CheckpointStore> CheckpointStore for Arc<S> {
    fn save(&self, checkpoint: Checkpoint) -> impl Future<Output = Result<(), AgentError>> {
        (**self).save(checkpoint)
    }
    fn load_latest_by_run(
        &self,
        run_id: &RunId,
    ) -> impl Future<Output = Result<Option<Checkpoint>, AgentError>> {
        (**self).load_latest_by_run(run_id)
    }
    fn load_latest_by_thread(
        &self,
        thread_id: &ThreadId,
    ) -> impl Future<Output = Result<Option<Checkpoint>, AgentError>> {
        (**self).load_latest_by_thread(thread_id)
    }
    fn list_by_run(
        &self,
        run_id: &RunId,
    ) -> impl Future<Output = Result<Vec<Checkpoint>, AgentError>> {
        (**self).list_by_run(run_id)
    }
}

impl<S: CheckpointStore> CheckpointStore for std::rc::Rc<S> {
    fn save(&self, checkpoint: Checkpoint) -> impl Future<Output = Result<(), AgentError>> {
        (**self).save(checkpoint)
    }
    fn load_latest_by_run(
        &self,
        run_id: &RunId,
    ) -> impl Future<Output = Result<Option<Checkpoint>, AgentError>> {
        (**self).load_latest_by_run(run_id)
    }
    fn load_latest_by_thread(
        &self,
        thread_id: &ThreadId,
    ) -> impl Future<Output = Result<Option<Checkpoint>, AgentError>> {
        (**self).load_latest_by_thread(thread_id)
    }
    fn list_by_run(
        &self,
        run_id: &RunId,
    ) -> impl Future<Output = Result<Vec<Checkpoint>, AgentError>> {
        (**self).list_by_run(run_id)
    }
}
