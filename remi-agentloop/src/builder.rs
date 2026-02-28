use async_stream::stream;
use futures::{Stream, StreamExt};
use std::future::Future;

use crate::agent::Agent;
use crate::agent_loop::AgentLoop;
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

pub struct AgentBuilder<M = NoModel, S = NoStore> {
    pub(crate) model: M,
    pub(crate) store: S,
    pub(crate) config: AgentConfig,
    pub(crate) tracer: Option<Box<dyn DynTracer>>,
    pub(crate) system_prompt: Option<String>,
    pub(crate) tools: DefaultToolRegistry,
    pub(crate) max_turns: usize,
}

impl AgentBuilder<NoModel, NoStore> {
    pub fn new() -> Self {
        AgentBuilder {
            model: NoModel,
            store: NoStore,
            config: AgentConfig::default(),
            tracer: None,
            system_prompt: None,
            tools: DefaultToolRegistry::new(),
            max_turns: 10,
        }
    }
}

impl<M, S> AgentBuilder<M, S> {
    /// Set the model — transitions typestate from NoModel to M
    pub fn model<NewM: ChatModel>(self, model: NewM) -> AgentBuilder<NewM, S> {
        AgentBuilder {
            model,
            store: self.store,
            config: self.config,
            tracer: self.tracer,
            system_prompt: self.system_prompt,
            tools: self.tools,
            max_turns: self.max_turns,
        }
    }

    /// Set the context store — enables stateful (thread-aware) mode
    pub fn context_store<NewS: ContextStore>(self, store: NewS) -> AgentBuilder<M, NewS> {
        AgentBuilder {
            model: self.model,
            store,
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
    ) -> AgentBuilderWithRegistry<M, S> {
        AgentBuilderWithRegistry {
            inner: AgentBuilder {
                model: self.model,
                store: self.store,
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

impl<M: ChatModel, S> AgentBuilder<M, S> {
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
    pub fn build(self) -> BuiltAgent<M, S> {
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
            system_prompt,
        }
    }
}

// ── AgentBuilderWithRegistry ──────────────────────────────────────────────────

/// A builder variant that holds a custom [`ToolRegistry`] implementation.
pub struct AgentBuilderWithRegistry<M = NoModel, S = NoStore> {
    pub(crate) inner: AgentBuilder<M, S>,
    pub(crate) registry: Box<dyn ToolRegistry>,
}

impl<M: ChatModel, S> AgentBuilderWithRegistry<M, S> {
    pub fn build(self) -> BuiltAgent<M, S> {
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
            system_prompt,
        }
    }
}

// ── BuiltAgent ────────────────────────────────────────────────────────────────

/// Agent with optional memory persistence.
///
/// Wraps an [`AgentLoop`] to add context-store persistence and run lifecycle:
///
/// ```text
/// BuiltAgent (memory + run lifecycle)
///   └── AgentLoop (step + tools + tracing)  ← composable core
///         └── step()
/// ```
///
/// The inner `AgentLoop` emits [`AgentEvent::NewMessages`] whenever new
/// messages are added to the agent state.  `BuiltAgent` intercepts those
/// events, persists them to the [`ContextStore`], and filters them out of
/// the stream forwarded to the caller.
pub struct BuiltAgent<M: ChatModel, S = NoStore> {
    pub(crate) inner: AgentLoop<M>,
    pub(crate) store: S,
    pub(crate) system_prompt: String,
}

impl<M: ChatModel, S: ContextStore> BuiltAgent<M, S> {
    /// Thin wrapper around [`AgentLoop::run`] that adds memory persistence.
    ///
    /// Observes the inner stream:
    /// - `NewMessages(msgs)` → persists to store, then swallows the event
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
            let mut inner_stream = std::pin::pin!(inner_stream);
            while let Some(event) = inner_stream.next().await {
                match event {
                    AgentEvent::NewMessages(msgs) => {
                        // Persist new messages to store — swallow event
                        let _ = self.store.append_messages(&thread_id, msgs).await;
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
}

// ── Agent impl (stateless) ────────────────────────────────────────────────────

impl<M: ChatModel> Agent for BuiltAgent<M, NoStore> {
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

impl<M: ChatModel, S: ContextStore> BuiltAgent<M, S> {
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
    ) -> Result<impl Stream<Item = AgentEvent> + '_, AgentError> {
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
                Ok(self.run_loop(state, Action::ToolResults(vec![]), false))
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
                Ok(self.run_loop(state, Action::ToolResults(outcomes), false))
            }
        }
    }
}

impl Default for AgentBuilder<NoModel, NoStore> {
    fn default() -> Self {
        Self::new()
    }
}
