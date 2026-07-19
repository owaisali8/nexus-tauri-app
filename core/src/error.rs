use thiserror::Error;

/// Errors surfaced by `core`. Engine-specific errors are flattened into
/// [`Error::Engine`] so that callers never depend on a concrete engine crate.
#[derive(Debug, Error)]
pub enum Error {
    #[error("provider not found: {0}")]
    ProviderNotFound(String),

    #[error("provider `{provider_id}` is misconfigured: {reason}")]
    ProviderMisconfigured { provider_id: String, reason: String },

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("engine error: {0}")]
    Engine(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("run was cancelled")]
    Cancelled,

    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;
