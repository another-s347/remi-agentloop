use std::future::Future;
use async_stream::stream;
use futures::Stream;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::agent::Agent;
use crate::protocol::{ProtocolEvent, ProtocolError};
use crate::types::LoopInput;
use crate::transport::sse::decode_sse_data;

/// HTTP SSE client — connects to a remote Agent service exposing standard protocol
pub struct HttpSseClient {
    client: reqwest::Client,
    endpoint: String,
    headers: HeaderMap,
}

impl HttpSseClient {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint: endpoint.into(),
            headers: HeaderMap::new(),
        }
    }

    pub fn with_header(mut self, key: &str, value: &str) -> Self {
        if let (Ok(k), Ok(v)) = (
            HeaderName::from_bytes(key.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            self.headers.insert(k, v);
        }
        self
    }

    pub fn with_bearer_token(self, token: &str) -> Self {
        self.with_header("Authorization", &format!("Bearer {token}"))
    }
}

impl Agent for HttpSseClient {
    type Request = LoopInput;
    type Response = ProtocolEvent;
    type Error = ProtocolError;

    fn chat(
        &self,
        req: LoopInput,
    ) -> impl Future<Output = Result<impl Stream<Item = ProtocolEvent>, ProtocolError>> {
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let headers = self.headers.clone();

        async move {
            let response = client
                .post(&endpoint)
                .headers(headers)
                .json(&req)
                .send()
                .await
                .map_err(|e| ProtocolError {
                    code: "http_error".into(),
                    message: e.to_string(),
                })?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(ProtocolError {
                    code: "http_error".into(),
                    message: format!("HTTP {}: {}", status, body),
                });
            }

            use tokio_util::io::StreamReader;
            use tokio::io::AsyncBufReadExt;
            use futures::TryStreamExt;

            let byte_stream = response
                .bytes_stream()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
            let reader = StreamReader::new(byte_stream);
            let mut lines = reader.lines();

            Ok(stream! {
                let mut _current_event: Option<String> = None;
                loop {
                    match lines.next_line().await {
                        Err(_) | Ok(None) => break,
                        Ok(Some(line)) => {
                            let line = line.trim().to_string();
                            if line.is_empty() {
                                _current_event = None;
                                continue;
                            }
                            if line.starts_with(':') { continue; }
                            if let Some(event_type) = line.strip_prefix("event: ") {
                                _current_event = Some(event_type.to_string());
                                continue;
                            }
                            if let Some(data) = line.strip_prefix("data: ") {
                                match decode_sse_data(data) {
                                    Ok(event) => {
                                        let is_done = matches!(event, ProtocolEvent::Done);
                                        yield event;
                                        if is_done { break; }
                                    }
                                    Err(_) => continue,
                                }
                            }
                        }
                    }
                }
            })
        }
    }
}
