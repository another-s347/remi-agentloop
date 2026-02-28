use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;
use crate::error::AgentError;
use crate::types::{Message, MessageId, RunId, ThreadId};

/// 上下文存储 trait — RPITIT, no Send bound
pub trait ContextStore {
    fn create_thread(&self) -> impl Future<Output = Result<ThreadId, AgentError>>;

    fn get_messages(
        &self,
        thread_id: &ThreadId,
    ) -> impl Future<Output = Result<Vec<Message>, AgentError>>;

    fn get_recent_messages(
        &self,
        thread_id: &ThreadId,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<Message>, AgentError>>;

    fn append_message(
        &self,
        thread_id: &ThreadId,
        message: Message,
    ) -> impl Future<Output = Result<MessageId, AgentError>>;

    fn append_messages(
        &self,
        thread_id: &ThreadId,
        messages: Vec<Message>,
    ) -> impl Future<Output = Result<Vec<MessageId>, AgentError>>;

    fn delete_thread(
        &self,
        thread_id: &ThreadId,
    ) -> impl Future<Output = Result<(), AgentError>>;

    fn create_run(
        &self,
        thread_id: &ThreadId,
    ) -> impl Future<Output = Result<RunId, AgentError>>;

    fn complete_run(
        &self,
        run_id: &RunId,
    ) -> impl Future<Output = Result<(), AgentError>>;
}

// ── NoStore —— placeholder when no store is needed ───────────────────────────

/// Marker: no context store (stateless mode)
pub struct NoStore;

impl ContextStore for NoStore {
    fn create_thread(&self) -> impl Future<Output = Result<ThreadId, AgentError>> {
        async { Ok(ThreadId::new()) }
    }
    fn get_messages(&self, _: &ThreadId) -> impl Future<Output = Result<Vec<Message>, AgentError>> {
        async { Ok(vec![]) }
    }
    fn get_recent_messages(&self, _: &ThreadId, _: usize) -> impl Future<Output = Result<Vec<Message>, AgentError>> {
        async { Ok(vec![]) }
    }
    fn append_message(&self, _: &ThreadId, msg: Message) -> impl Future<Output = Result<MessageId, AgentError>> {
        let id = msg.id.clone();
        async move { Ok(id) }
    }
    fn append_messages(&self, _: &ThreadId, msgs: Vec<Message>) -> impl Future<Output = Result<Vec<MessageId>, AgentError>> {
        let ids: Vec<_> = msgs.iter().map(|m| m.id.clone()).collect();
        async move { Ok(ids) }
    }
    fn delete_thread(&self, _: &ThreadId) -> impl Future<Output = Result<(), AgentError>> {
        async { Ok(()) }
    }
    fn create_run(&self, _: &ThreadId) -> impl Future<Output = Result<RunId, AgentError>> {
        async { Ok(RunId::new()) }
    }
    fn complete_run(&self, _: &RunId) -> impl Future<Output = Result<(), AgentError>> {
        async { Ok(()) }
    }
}

// ── InMemoryStore ─────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Default, Clone)]
pub struct InMemoryStore {
    inner: Arc<Mutex<InMemoryStoreInner>>,
}

#[derive(Debug, Default)]
struct InMemoryStoreInner {
    threads: HashMap<String, Vec<Message>>,
    runs: HashMap<String, String>,  // run_id → thread_id
}

impl InMemoryStore {
    pub fn new() -> Self { Self::default() }
}

impl ContextStore for InMemoryStore {
    fn create_thread(&self) -> impl Future<Output = Result<ThreadId, AgentError>> {
        let inner = self.inner.clone();
        async move {
            let tid = ThreadId::new();
            inner.lock().unwrap().threads.insert(tid.0.clone(), vec![]);
            Ok(tid)
        }
    }

    fn get_messages(&self, thread_id: &ThreadId) -> impl Future<Output = Result<Vec<Message>, AgentError>> {
        let inner = self.inner.clone();
        let tid = thread_id.clone();
        async move {
            let guard = inner.lock().unwrap();
            guard.threads.get(&tid.0)
                .cloned()
                .ok_or(AgentError::ThreadNotFound(tid))
        }
    }

    fn get_recent_messages(&self, thread_id: &ThreadId, limit: usize) -> impl Future<Output = Result<Vec<Message>, AgentError>> {
        let inner = self.inner.clone();
        let tid = thread_id.clone();
        async move {
            let guard = inner.lock().unwrap();
            let msgs = guard.threads.get(&tid.0)
                .ok_or(AgentError::ThreadNotFound(tid))?;
            let skip = msgs.len().saturating_sub(limit);
            Ok(msgs[skip..].to_vec())
        }
    }

    fn append_message(&self, thread_id: &ThreadId, message: Message) -> impl Future<Output = Result<MessageId, AgentError>> {
        let inner = self.inner.clone();
        let tid = thread_id.clone();
        async move {
            let mut guard = inner.lock().unwrap();
            let msgs = guard.threads.entry(tid.0.clone()).or_default();
            let id = message.id.clone();
            msgs.push(message);
            Ok(id)
        }
    }

    fn append_messages(&self, thread_id: &ThreadId, messages: Vec<Message>) -> impl Future<Output = Result<Vec<MessageId>, AgentError>> {
        let inner = self.inner.clone();
        let tid = thread_id.clone();
        async move {
            let mut guard = inner.lock().unwrap();
            let msgs = guard.threads.entry(tid.0.clone()).or_default();
            let ids: Vec<_> = messages.iter().map(|m| m.id.clone()).collect();
            msgs.extend(messages);
            Ok(ids)
        }
    }

    fn delete_thread(&self, thread_id: &ThreadId) -> impl Future<Output = Result<(), AgentError>> {
        let inner = self.inner.clone();
        let tid = thread_id.clone();
        async move {
            inner.lock().unwrap().threads.remove(&tid.0);
            Ok(())
        }
    }

    fn create_run(&self, thread_id: &ThreadId) -> impl Future<Output = Result<RunId, AgentError>> {
        let inner = self.inner.clone();
        let tid = thread_id.clone();
        async move {
            let rid = RunId::new();
            inner.lock().unwrap().runs.insert(rid.0.clone(), tid.0);
            Ok(rid)
        }
    }

    fn complete_run(&self, run_id: &RunId) -> impl Future<Output = Result<(), AgentError>> {
        let inner = self.inner.clone();
        let rid = run_id.clone();
        async move {
            inner.lock().unwrap().runs.remove(&rid.0);
            Ok(())
        }
    }
}

// ── Rc<S> blanket impl — enables shared store for Sub-Agent pattern ──────────

impl<S: ContextStore> ContextStore for Rc<S> {
    fn create_thread(&self) -> impl Future<Output = Result<ThreadId, AgentError>> {
        (**self).create_thread()
    }
    fn get_messages(&self, thread_id: &ThreadId) -> impl Future<Output = Result<Vec<Message>, AgentError>> {
        (**self).get_messages(thread_id)
    }
    fn get_recent_messages(&self, thread_id: &ThreadId, limit: usize) -> impl Future<Output = Result<Vec<Message>, AgentError>> {
        (**self).get_recent_messages(thread_id, limit)
    }
    fn append_message(&self, thread_id: &ThreadId, message: Message) -> impl Future<Output = Result<MessageId, AgentError>> {
        (**self).append_message(thread_id, message)
    }
    fn append_messages(&self, thread_id: &ThreadId, messages: Vec<Message>) -> impl Future<Output = Result<Vec<MessageId>, AgentError>> {
        (**self).append_messages(thread_id, messages)
    }
    fn delete_thread(&self, thread_id: &ThreadId) -> impl Future<Output = Result<(), AgentError>> {
        (**self).delete_thread(thread_id)
    }
    fn create_run(&self, thread_id: &ThreadId) -> impl Future<Output = Result<RunId, AgentError>> {
        (**self).create_run(thread_id)
    }
    fn complete_run(&self, run_id: &RunId) -> impl Future<Output = Result<(), AgentError>> {
        (**self).complete_run(run_id)
    }
}

// ── Arc<S> blanket impl — enables shared store across Send boundaries ────────

impl<S: ContextStore> ContextStore for Arc<S> {
    fn create_thread(&self) -> impl Future<Output = Result<ThreadId, AgentError>> {
        (**self).create_thread()
    }
    fn get_messages(&self, thread_id: &ThreadId) -> impl Future<Output = Result<Vec<Message>, AgentError>> {
        (**self).get_messages(thread_id)
    }
    fn get_recent_messages(&self, thread_id: &ThreadId, limit: usize) -> impl Future<Output = Result<Vec<Message>, AgentError>> {
        (**self).get_recent_messages(thread_id, limit)
    }
    fn append_message(&self, thread_id: &ThreadId, message: Message) -> impl Future<Output = Result<MessageId, AgentError>> {
        (**self).append_message(thread_id, message)
    }
    fn append_messages(&self, thread_id: &ThreadId, messages: Vec<Message>) -> impl Future<Output = Result<Vec<MessageId>, AgentError>> {
        (**self).append_messages(thread_id, messages)
    }
    fn delete_thread(&self, thread_id: &ThreadId) -> impl Future<Output = Result<(), AgentError>> {
        (**self).delete_thread(thread_id)
    }
    fn create_run(&self, thread_id: &ThreadId) -> impl Future<Output = Result<RunId, AgentError>> {
        (**self).create_run(thread_id)
    }
    fn complete_run(&self, run_id: &RunId) -> impl Future<Output = Result<(), AgentError>> {
        (**self).complete_run(run_id)
    }
}

// ── ContextStoreExt — convenience methods ────────────────────────────────────

/// Extension trait for [`ContextStore`] — provides convenience methods
/// like conversation forking.
pub trait ContextStoreExt: ContextStore {
    /// Fork a thread — copy messages up to (and including) `up_to_message`
    /// into a new thread.
    ///
    /// The new thread shares history up to the fork point and then evolves
    /// independently. Each copied message gets a fresh [`MessageId`].
    fn fork_thread(
        &self,
        source: &ThreadId,
        up_to_message: &MessageId,
    ) -> impl Future<Output = Result<ThreadId, AgentError>> {
        async {
            let messages = self.get_messages(source).await?;
            let idx = messages.iter()
                .position(|m| m.id == *up_to_message)
                .ok_or(AgentError::MessageNotFound(up_to_message.clone()))?;
            let forked: Vec<Message> = messages[..=idx]
                .iter()
                .map(|m| Message { id: MessageId::new(), ..m.clone() })
                .collect();
            let new_thread = self.create_thread().await?;
            self.append_messages(&new_thread, forked).await?;
            Ok(new_thread)
        }
    }
}

// blanket impl — every ContextStore gets ContextStoreExt for free
impl<S: ContextStore> ContextStoreExt for S {}
