use async_stream::stream;
use futures::{Stream, StreamExt};
use std::future::Future;
use std::pin::Pin;

use crate::agent::Agent;
use crate::agent_loop::AgentLoop;
use crate::checkpoint::{CheckpointStore, NoCheckpointStore};
use crate::config::AgentConfig;
use crate::context::{ContextStore, NoStore};
use crate::error::AgentError;
use crate::model::ChatModel;
use crate::state::Action;
use crate::tool::registry::{DefaultToolRegistry, ToolRegistry};
use crate::tool::Tool;
use crate::tracing::{DynTracer, Tracer};
use crate::types::{AgentEvent, ChatInput, Message, Role, ThreadId, ToolCallOutcome};

// ── Typestate markers ─────────────────────────────────────────────────────────

pub struct NoModel;

// ── AgentBuilder ──────────────────────────────────────────────────────────────

pub struct AgentBuilder<M = NoModel, S = NoStore, C = NoCheckpointStore> {
    pub(crate) model: M,
    pub(crate) store: S,
    pub(crate) checkpoint_store: C,
    pub(crate) config: AgentConfig,
    pub(crate) tracer: Option<Box<dyn DynTracer>>,
    pub(crate) system_prompt: Option<String>,
    pub(crate) tools: DefaultToolRegistry,
    pub(crate) max_turns: usize,
}

impl AgentBuilder<NoModel, NoStore, NoCheckpointStore> {
    pub fn new() -> Self {
        AgentBuilder {
            model: NoModel,
            store: NoStore,
            checkpoint_store: NoCheckpointStore,
            config: AgentConfig::default(),
            tracer: None,
            system_prompt: None,
            tools: DefaultToolRegistry::new(),
            max_turns: 10,
        }
    }
}

impl<M, S, C> AgentBuilder<M, S, C> {
    /// Set the model — transitions typestate from NoModel to M
    pub fn model<NewM: ChatModel>(self, model: NewM) -> AgentBuilder<NewM, S, C> {
        AgentBuilder {
            model,
            store: self.store,
            checkpoint_store: self.checkpoint_store,
            config: self.config,
            tracer: self.tracer,
            system_prompt: self.system_prompt,
            tools: self.tools,
            max_turns: self.max_turns,
        }
    }

    /// Set the context store — enables stateful (thread-aware) mode
    pub fn context_store<NewS: ContextStore>(self, store: NewS) -> AgentBuilder<M, NewS, C> {
        AgentBuilder {
            model: self.model,
            store,
            checkpoint_store: self.checkpoint_store,
            config: self.config,
            tracer: self.tracer,
            system_prompt: self.system_prompt,
            tools: self.tools,
            max_turns: self.max_turns,
        }
    }

    /// Set the checkpoint store — enables durable state snapshots for resume.
    pub fn checkpoint_store<NewC: CheckpointStore>(self, cs: NewC) -> AgentBuilder<M, S, NewC> {
        AgentBuilder {
            model: self.model,
            store: self.store,
            checkpoint_store: cs,
            config: self.config,
            tracer: self.tracer,
            system_prompt: self.system_prompt,
            tools: self.tools,
            max_turns: self.max_turns,
        }
    }

    /// Replace the default in-process registry with a custom [`ToolRegistry`] implementation.
    ///
    /// Note: any tools previously registered via [`.tool()`](Self::tool) will be discarded.
    pub fn with_registry(
        self,
        registry: impl ToolRegistry + 'static,
    ) -> AgentBuilderWithRegistry<M, S, C> {
        AgentBuilderWithRegistry {
            inner: AgentBuilder {
                model: self.model,
                store: self.store,
                checkpoint_store: self.checkpoint_store,
                config: self.config,
                tracer: self.tracer,
                system_prompt: self.system_prompt,
                tools: DefaultToolRegistry::new(), // unused
                max_turns: self.max_turns,
            },
            registry: Box::new(registry),
        }
    }

    pub fn system(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn tool(mut self, tool: impl Tool + Send + Sync + 'static) -> Self {
        self.tools.register(tool);
        self
    }

    pub fn max_turns(mut self, n: usize) -> Self {
        self.max_turns = n;
        self
    }

    pub fn config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    pub fn tracer(mut self, tracer: impl Tracer + Send + Sync + 'static) -> Self {
        self.tracer = Some(Box::new(tracer));
        self
    }
}

impl<M: ChatModel, S, C> AgentBuilder<M, S, C> {
    /// Build into just the [`AgentLoop`] (no store / memory layer).
    ///
    /// Use this when you need composable external tool calling:
    /// ```ignore
    /// let inner = AgentBuilder::new()
    ///     .model(oai)
    ///     .tool(Add::new())
    ///     .build_loop();
    ///
    /// let mut state = inner.build_state(vec![]);
    /// state.tool_definitions.extend(outer_defs);
    /// let stream = inner.run(state, action, true);
    /// ```
    pub fn build_loop(self) -> AgentLoop<M> {
        AgentLoop {
            model: self.model,
            tools: Box::new(self.tools),
            config: self.config,
            tracer: self.tracer,
            system_prompt: self.system_prompt.unwrap_or_default(),
            max_turns: self.max_turns,
        }
    }

    /// Build into a BuiltAgent (only available once model is set)
    pub fn build(self) -> BuiltAgent<M, S, C> {
        let system_prompt = self.system_prompt.unwrap_or_default();
        let inner = AgentLoop {
            model: self.model,
            tools: Box::new(self.tools),
            config: self.config,
            tracer: self.tracer,
            system_prompt: system_prompt.clone(),
            max_turns: self.max_turns,
        };
        BuiltAgent {
            inner,
            store: self.store,
            checkpoint_store: self.checkpoint_store,
            system_prompt,
        }
    }
}

// ── AgentBuilderWithRegistry ──────────────────────────────────────────────────

/// A builder variant that holds a custom [`ToolRegistry`] implementation.
pub struct AgentBuilderWithRegistry<M = NoModel, S = NoStore, C = NoCheckpointStore> {
    pub(crate) inner: AgentBuilder<M, S, C>,
    pub(crate) registry: Box<dyn ToolRegistry>,
}

impl<M: ChatModel, S, C> AgentBuilderWithRegistry<M, S, C> {
    pub fn build(self) -> BuiltAgent<M, S, C> {
        let system_prompt = self.inner.system_prompt.unwrap_or_default();
        let inner = AgentLoop {
            model: self.inner.model,
            tools: self.registry,
            config: self.inner.config,
            tracer: self.inner.tracer,
            system_prompt: system_prompt.clone(),
            max_turns: self.inner.max_turns,
        };
        BuiltAgent {
            inner,
            store: self.inner.store,
            checkpoint_store: self.inner.checkpoint_store,
            system_prompt,
        }
    }
}

// ── BuiltAgent ────────────────────────────────────────────────────────────────

/// Agent with optional memory persistence and checkpoint support.
///
/// Wraps an [`AgentLoop`] to add context-store persistence, checkpoint
/// persistence, and run lifecycle:
///
/// ```text
/// BuiltAgent (memory + checkpoints + run lifecycle)
///   └── AgentLoop (step + tools + tracing)  ← composable core
///         └── step()
/// ```
///
/// The inner `AgentLoop` emits [`AgentEvent::Checkpoint`] at key lifecycle
/// boundaries.  `BuiltAgent` intercepts these, persists messages to the
/// [`ContextStore`] and snapshots to the [`CheckpointStore`], then filters
/// them out of the stream forwarded to the caller.
pub struct BuiltAgent<M: ChatModel, S = NoStore, C = NoCheckpointStore> {
    pub(crate) inner: AgentLoop<M>,
    pub(crate) store: S,
    pub(crate) checkpoint_store: C,
    pub(crate) system_prompt: String,
}

impl<M: ChatModel, S: ContextStore, C: CheckpointStore> BuiltAgent<M, S, C> {
    /// Thin wrapper around [`AgentLoop::run`] that adds memory + checkpoint persistence.
    ///
    /// Observes the inner stream:
    /// - `Checkpoint(cp)` → persists messages to context store + checkpoint to
    ///   checkpoint store, then swallows the event
    /// - `Done` / `Interrupt` → marks run as complete in store
    /// - everything else → forwarded as-is
    fn run_loop<'a>(
        &'a self,
        state: crate::state::AgentState,
        action: Action,
        emit_run_start: bool,
    ) -> impl Stream<Item = AgentEvent> + 'a {
        let thread_id = state.thread_id.clone();
        let run_id = state.run_id.clone();
        let inner_stream = self.inner.run(state, action, emit_run_start);

        stream! {
            let mut known_msg_count = 0usize;
            let mut inner_stream = std::pin::pin!(inner_stream);
            while let Some(event) = inner_stream.next().await {
                match event {
                    AgentEvent::Checkpoint(cp) => {
                        // Persist new messages to context store
                        if cp.state.messages.len() > known_msg_count {
                            let new_msgs = cp.state.messages[known_msg_count..].to_vec();
                            let _ = self.store.append_messages(&thread_id, new_msgs).await;
                            known_msg_count = cp.state.messages.len();
                        }
                        // Persist checkpoint — swallow event
                        let _ = self.checkpoint_store.save(cp).await;
                    }
                    AgentEvent::Done => {
                        let _ = self.store.complete_run(&run_id).await;
                        yield AgentEvent::Done;
                    }
                    AgentEvent::Interrupt { interrupts } => {
                        let _ = self.store.complete_run(&run_id).await;
                        yield AgentEvent::Interrupt { interrupts };
                    }
                    other => {
                        yield other;
                    }
                }
            }
        }
    }

    /// Cancel a run and produce a `Cancelled` checkpoint.
    ///
    /// Wraps the inner `AgentLoop::cancel()` with persistence:
    /// persists checkpoint + context, yields `Cancelled`.
    fn cancel_loop<'a>(
        &'a self,
        state: crate::state::AgentState,
    ) -> impl Stream<Item = AgentEvent> + 'a {
        let thread_id = state.thread_id.clone();
        let run_id = state.run_id.clone();
        let inner_stream = self.inner.cancel_run(state);

        stream! {
            let mut known_msg_count = 0usize;
            let mut inner_stream = std::pin::pin!(inner_stream);
            while let Some(event) = inner_stream.next().await {
                match event {
                    AgentEvent::Checkpoint(cp) => {
                        if cp.state.messages.len() > known_msg_count {
                            let new_msgs = cp.state.messages[known_msg_count..].to_vec();
                            let _ = self.store.append_messages(&thread_id, new_msgs).await;
                            known_msg_count = cp.state.messages.len();
                        }
                        let _ = self.checkpoint_store.save(cp).await;
                    }
                    AgentEvent::Cancelled => {
                        let _ = self.store.complete_run(&run_id).await;
                        yield AgentEvent::Cancelled;
                    }
                    other => {
                        yield other;
                    }
                }
            }
        }
    }

    /// Resume execution from the latest checkpoint for a given thread.
    ///
    /// Loads the most recent checkpoint, extracts the agent state and pending
    /// action, and resumes the agent loop from that point.
    ///
    /// Returns `None` if no checkpoint exists for the thread.
    pub async fn resume_from_checkpoint(
        &self,
        thread_id: &ThreadId,
    ) -> Result<Option<impl Stream<Item = AgentEvent> + '_>, AgentError> {
        let cp = self.checkpoint_store.load_latest_by_thread(thread_id).await?;
        match cp {
            None => Ok(None),
            Some(cp) => {
                if !cp.is_resumable() {
                    // Terminal checkpoint — nothing to resume
                    return Ok(None);
                }
                let action = cp.pending_action.unwrap();
                Ok(Some(self.run_loop(cp.state, action, false)))
            }
        }
    }
}

// ── Agent impl (stateless) ────────────────────────────────────────────────────

impl<M: ChatModel> Agent for BuiltAgent<M, NoStore, NoCheckpointStore> {
    type Request = crate::types::LoopInput;
    type Response = AgentEvent;
    type Error = AgentError;

    /// Stateless mode: delegates to the inner [`AgentLoop`].
    fn chat(
        &self,
        input: crate::types::LoopInput,
    ) -> impl Future<Output = Result<impl Stream<Item = AgentEvent>, AgentError>> {
        self.inner.chat(input)
    }
}

// ── Stateful mode ─────────────────────────────────────────────────────────────

impl<M: ChatModel, S: ContextStore, C: CheckpointStore> BuiltAgent<M, S, C> {
    /// Create a new Thread
    pub async fn create_thread(&self) -> Result<ThreadId, AgentError> {
        self.store.create_thread().await
    }

    /// Stateful mode: chat or resume within a Thread.
    ///
    /// This is the single entry point for stateful interactions.
    /// It is compositional: load messages from store → run_loop → persist.
    ///
    /// ```ignore
    /// // New user message:
    /// agent.chat_in_thread(&tid, "hello").await?;
    ///
    /// // Resume from interrupt:
    /// agent.chat_in_thread(&tid, ChatInput::Resume { run_id, ... }).await?;
    /// ```
    pub async fn chat_in_thread(
        &self,
        thread_id: &ThreadId,
        input: impl Into<ChatInput>,
    ) -> Result<Pin<Box<dyn Stream<Item = AgentEvent> + '_>>, AgentError> {
        let input = input.into();

        // ── Load existing messages from store ─────────────────────────
        let mut messages = self.store.get_messages(thread_id).await?;

        // Ensure system prompt is first
        if !self.system_prompt.is_empty() {
            if !messages
                .first()
                .is_some_and(|m| matches!(m.role, Role::System))
            {
                messages.insert(0, Message::system(&self.system_prompt));
            }
        }

        match input {
            ChatInput::Message(user_input) => {
                let run_id = self.store.create_run(thread_id).await?;

                // Append user message to store + state
                let user_msg = Message::user(&user_input);
                self.store
                    .append_message(thread_id, user_msg.clone())
                    .await?;
                messages.push(user_msg);

                let mut state = self.inner.build_state(messages);
                state.thread_id = thread_id.clone();
                state.run_id = run_id;
                state.system_prompt = None;

                // Action is empty ToolResults because user message is already in state.messages;
                // step() will see it and call the model.
                Ok(Box::pin(self.run_loop(state, Action::ToolResults(vec![]), false)))
            }

            ChatInput::Resume {
                run_id,
                completed_results,
                pending_interrupts,
                payloads,
            } => {
                // ── Validate payloads match pending interrupts ─────────
                if payloads.len() != pending_interrupts.len() {
                    return Err(AgentError::ResumeIncomplete {
                        expected: pending_interrupts.len(),
                        got: payloads.len(),
                    });
                }
                for intr in &pending_interrupts {
                    if !payloads.iter().any(|p| p.interrupt_id == intr.interrupt_id) {
                        return Err(AgentError::InterruptNotFound(intr.interrupt_id.clone()));
                    }
                }

                // ── Build tool outcomes (don't add messages manually —
                //    step() will do that via Action::ToolResults, and
                //    run_loop will persist them automatically) ──────────
                let mut outcomes: Vec<ToolCallOutcome> = Vec::new();

                for tr in &completed_results {
                    outcomes.push(ToolCallOutcome::Result {
                        tool_call_id: tr.id.clone(),
                        tool_name: tr.name.clone(),
                        result: tr.result.clone(),
                    });
                }

                for payload in &payloads {
                    let intr = pending_interrupts
                        .iter()
                        .find(|i| i.interrupt_id == payload.interrupt_id)
                        .unwrap();
                    let result_str = serde_json::to_string(&payload.result).unwrap_or_default();
                    outcomes.push(ToolCallOutcome::Result {
                        tool_call_id: intr.tool_call_id.clone(),
                        tool_name: intr.tool_name.clone(),
                        result: result_str,
                    });
                }

                let mut state = self.inner.build_state(messages);
                state.thread_id = thread_id.clone();
                state.run_id = run_id;
                state.system_prompt = None;

                // step() will append tool_result messages to state.messages,
                // run_loop will detect & persist the new messages automatically.
                    Ok(Box::pin(self.run_loop(state, Action::ToolResults(outcomes), false)))
            }

            ChatInput::Cancel { run_id } => {
                // Load the latest checkpoint for this run to get the current state
                let cp = self.checkpoint_store.load_latest_by_run(&run_id).await?;
                let state = match cp {
                    Some(cp) => cp.state,
                    None => {
                        // No checkpoint yet — build state from stored messages
                        let mut state = self.inner.build_state(messages);
                        state.thread_id = thread_id.clone();
                        state.run_id = run_id;
                        state.system_prompt = None;
                        state
                    }
                };
                Ok(Box::pin(self.cancel_loop(state)))
            }
        }
    }
}

impl Default for AgentBuilder<NoModel, NoStore, NoCheckpointStore> {
    fn default() -> Self {
        Self::new()
    }
}
