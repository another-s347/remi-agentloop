use remi_core::error::AgentError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvalError {
    #[error(transparent)]
    Agent(#[from] AgentError),

    #[error("capture does not contain a start request")]
    MissingStartRequest,

    #[error("capture contains a non-user start message")]
    InvalidStartMessage,
}