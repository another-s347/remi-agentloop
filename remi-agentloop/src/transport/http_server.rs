use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::routing::post;
use axum::Json;
use futures::{Stream, StreamExt};

use crate::agent::Agent;
use crate::protocol::{ProtocolAgent, ProtocolEvent};
use crate::transport::sse::encode_sse_event;
use crate::types::LoopInput;

/// Wraps a ProtocolAgent as an HTTP SSE server (axum-based)
pub struct HttpSseServer<A: ProtocolAgent + Send + Sync + 'static> {
    agent: Arc<A>,
    bind_addr: SocketAddr,
}

impl<A: ProtocolAgent + Send + Sync + 'static> HttpSseServer<A>
where
    A::Error: Send + 'static,
{
    pub fn new(agent: A) -> Self {
        Self {
            agent: Arc::new(agent),
            bind_addr: ([0, 0, 0, 0], 8080).into(),
        }
    }

    pub fn bind(mut self, addr: impl Into<SocketAddr>) -> Self {
        self.bind_addr = addr.into();
        self
    }

    /// Start the HTTP server
    pub async fn serve(self) -> Result<(), std::io::Error> {
        let agent = self.agent.clone();

        let app = axum::Router::new()
            .route("/chat", post(handle_chat::<A>))
            .with_state(agent);

        let listener = tokio::net::TcpListener::bind(self.bind_addr).await?;
        axum::serve(listener, app).await
    }
}

async fn handle_chat<A: ProtocolAgent + Send + Sync + 'static>(
    State(agent): State<Arc<A>>,
    Json(req): Json<LoopInput>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>>
where
    A::Error: Send,
{
    let stream = match agent.chat(req).await {
        Err(e) => {
            let err_event = ProtocolEvent::Error {
                message: e.message.clone(),
                code: Some(e.code.clone()),
            };
            let data = encode_sse_event(&err_event);
            return Sse::new(futures::stream::once(async move {
                Ok::<Event, Infallible>(Event::default().data(data))
            }));
        }
        Ok(s) => s,
    };

    let sse_stream = stream.map(|event| {
        let data = encode_sse_event(&event);
        Ok::<Event, Infallible>(Event::default().data(data))
    });

    Sse::new(sse_stream)
}
