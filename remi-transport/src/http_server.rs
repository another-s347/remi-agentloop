use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::routing::post;
use axum::Json;
use futures::{Stream, StreamExt};

use remi_core::protocol::{ProtocolError, ProtocolEvent};
use crate::sse::encode_sse_event;
use remi_core::types::LoopInput;

/// HTTP SSE server for exposing an agent over HTTP.
///
/// Uses a closure-based API so the compiler can verify `Send` bounds
/// on the concrete stream type (which RPITIT trait bounds cannot express
/// across crate boundaries).
///
/// # Example
///
/// ```ignore
/// let agent = Arc::new(my_agent);
/// let server = HttpSseServer::new(move |req| {
///     let agent = agent.clone();
///     async move { agent.chat(req).await }
/// });
/// server.bind(([0, 0, 0, 0], 8080)).serve().await?;
/// ```
pub struct HttpSseServer<F> {
    handler: Arc<F>,
    bind_addr: SocketAddr,
}

impl<F, Fut, S> HttpSseServer<F>
where
    F: Fn(LoopInput) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<S, ProtocolError>> + Send + 'static,
    S: Stream<Item = ProtocolEvent> + Send + 'static,
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

    /// Start the HTTP server.
    pub async fn serve(self) -> Result<(), std::io::Error> {
        let handler = self.handler;

        let app = axum::Router::new()
            .route("/chat", post(handle_chat::<F, Fut, S>))
            .with_state(handler);

        let listener = tokio::net::TcpListener::bind(self.bind_addr).await?;
        axum::serve(listener, app).await
    }
}

async fn handle_chat<F, Fut, S>(
    State(handler): State<Arc<F>>,
    Json(req): Json<LoopInput>,
) -> Sse<Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>>
where
    F: Fn(LoopInput) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<S, ProtocolError>> + Send + 'static,
    S: Stream<Item = ProtocolEvent> + Send + 'static,
{
    match handler(req).await {
        Err(e) => {
            let err_event = ProtocolEvent::Error {
                message: e.message.clone(),
                code: Some(e.code.clone()),
            };
            let data = encode_sse_event(&err_event);
            Sse::new(Box::pin(futures::stream::once(async move {
                Ok::<Event, Infallible>(Event::default().data(data))
            })))
        }
        Ok(stream) => Sse::new(Box::pin(stream.map(|event| {
            let data = encode_sse_event(&event);
            Ok::<Event, Infallible>(Event::default().data(data))
        }))),
    }
}
