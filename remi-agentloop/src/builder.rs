use std::future::Future;
use futures::{Stream, StreamExt};

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::context::{ContextStore, NoStore};
use crate::error::AgentError;
use crate::model::ChatModel;
use crate::state::AgentLoop;
use crate::tool::registry::ToolRegistry;
use crate::tool::Tool;
use crate::tracing::{DynTracer, Tracer};
use crate::types::{AgentEvent, Message, Role, RunId, ThreadId};

// ── Typestate markers ─────────────────────────────────────────────────────────

pub struct NoModel;

// ── AgentBuilder ──────────────────────────────────────────────────────────────

pub struct AgentBuilder<M = NoModel, S = NoStore> {
    pub(crate) model: M,
    pub(crate) store: S,
    pub(crate) config: AgentConfig,
    pub(crate) tracer: Option<Box<dyn DynTracer>>,
    pub(crate) system_prompt: Option<String>,
    pub(crate) tools: ToolRegistry,
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
            tools: ToolRegistry::new(),
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
    /// Build into a BuiltAgent (only available once model is set)
    pub fn build(self) -> BuiltAgent<M, S> {
        BuiltAgent {
            model: self.model,
            store: self.store,
            config: self.config,
            tracer: self.tracer,
            system_prompt: self.system_prompt.unwrap_or_default(),
            tools: self.tools,
            max_turns: self.max_turns,
        }
    }
}

// ── BuiltAgent ────────────────────────────────────────────────────────────────

pub struct BuiltAgent<M: ChatModel, S = NoStore> {
    pub(crate) model: M,
    pub(crate) store: S,
    pub(crate) config: AgentConfig,
    pub(crate) tracer: Option<Box<dyn DynTracer>>,
    pub(crate) system_prompt: String,
    pub(crate) tools: ToolRegistry,
    pub(crate) max_turns: usize,
}

impl<M: ChatModel> Agent for BuiltAgent<M, NoStore> {
    type Request = String;
    type Response = AgentEvent;
    type Error = AgentError;

    /// Stateless mode: each call is independent, no context persistence
    fn chat(&self, user_input: String) -> impl Future<Output = Result<impl Stream<Item = AgentEvent>, AgentError>> {
        async move {
            let mut messages = Vec::new();
            if !self.system_prompt.is_empty() {
                messages.push(Message::system(&self.system_prompt));
            }
            messages.push(Message::user(&user_input));

            let model_name = self.config.model.clone().unwrap_or_else(|| "gpt-4o".into());

            let loop_ = AgentLoop::new(&self.model, &self.tools, messages, self.max_turns, model_name);
            let loop_ = if let Some(t) = &self.tracer {
                loop_.with_tracer(t.as_ref())
            } else {
                loop_
            };

            Ok(loop_.into_stream())
        }
    }
}

impl<M: ChatModel, S: ContextStore> BuiltAgent<M, S> {
    /// Create a new Thread
    pub async fn create_thread(&self) -> Result<ThreadId, AgentError> {
        self.store.create_thread().await
    }

    /// Stateful mode: chat within a Thread with history
    pub async fn chat_in_thread(
        &self,
        thread_id: &ThreadId,
        user_input: String,
    ) -> Result<impl Stream<Item = AgentEvent> + '_, AgentError> {
        let run_id = self.store.create_run(thread_id).await?;

        let mut messages = self.store.get_messages(thread_id).await?;

        // Ensure system prompt is first
        if !self.system_prompt.is_empty() {
            if !messages.first().is_some_and(|m| matches!(m.role, Role::System)) {
                messages.insert(0, Message::system(&self.system_prompt));
            }
        }

        let user_msg = Message::user(&user_input);
        self.store.append_message(thread_id, user_msg.clone()).await?;
        messages.push(user_msg);

        let model_name = self.config.model.clone().unwrap_or_else(|| "gpt-4o".into());

        let loop_ = AgentLoop::new(&self.model, &self.tools, messages, self.max_turns, model_name)
            .with_thread(thread_id.clone())
            .with_run_id(run_id);

        let loop_ = if let Some(t) = &self.tracer {
            loop_.with_tracer(t.as_ref())
        } else {
            loop_
        };

        Ok(loop_.into_stream())
    }

    /// Resume from an Interrupt — RunId unchanged, stream continues from CallingModel
    ///
    /// Caller must provide ResumePayload for every InterruptId in the Interrupt event.
    pub async fn resume_run(
        &self,
        thread_id: &ThreadId,
        _run_id: &RunId,
        completed_results: Vec<crate::types::ToolCallResult>,
        pending_interrupts: Vec<crate::types::InterruptInfo>,
        payloads: Vec<crate::types::ResumePayload>,
    ) -> Result<impl Stream<Item = AgentEvent> + '_, AgentError> {
        // Validate
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

        // Reload existing messages from store
        let mut messages = self.store.get_messages(thread_id).await?;

        // Append completed tool results
        for tr in &completed_results {
            let msg = Message::tool_result(&tr.id, &tr.result);
            self.store.append_message(thread_id, msg.clone()).await?;
            messages.push(msg);
        }

        // Append resume payloads as tool results
        for payload in &payloads {
            let intr = pending_interrupts.iter()
                .find(|i| i.interrupt_id == payload.interrupt_id)
                .unwrap();
            let result_str = serde_json::to_string(&payload.result).unwrap_or_default();
            let msg = Message::tool_result(&intr.tool_call_id, &result_str);
            self.store.append_message(thread_id, msg.clone()).await?;
            messages.push(msg);
        }

        let model_name = self.config.model.clone().unwrap_or_else(|| "gpt-4o".into());

        // Continue in same run — don't yield RunStart again
        // We use a variant of into_stream that doesn't emit RunStart
        let loop_ = AgentLoop::new(&self.model, &self.tools, messages, self.max_turns, model_name)
            .with_thread(thread_id.clone())
            .with_run_id(_run_id.clone());

        let loop_ = if let Some(t) = &self.tracer {
            loop_.with_tracer(t.as_ref())
        } else {
            loop_
        };

        // Wrap to skip the leading RunStart event (resume continues the same run)
        Ok(loop_.into_stream().filter(|e| {
            std::future::ready(!matches!(e, AgentEvent::RunStart { .. }))
        }))
    }
}

impl Default for AgentBuilder<NoModel, NoStore> {
    fn default() -> Self { Self::new() }
}
