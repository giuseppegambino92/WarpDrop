//! Error type exposed across the FFI boundary.
//!
//! Internally the engine uses `anyhow` for context-rich errors. Anything that
//! must be distinguishable by the UI (bad code, rejection, cancellation…) is
//! raised as a [`WarpError`] so it survives `anyhow` downcasting intact.

#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum WarpError {
    #[error("I/O error: {msg}")]
    Io { msg: String },
    #[error("cryptographic failure: {msg}")]
    Crypto { msg: String },
    #[error("protocol error: {msg}")]
    Protocol { msg: String },
    #[error("invalid configuration: {msg}")]
    Config { msg: String },
    #[error("device not found")]
    PeerNotFound,
    #[error("invalid code")]
    InvalidCode,
    #[error("no device is currently showing this code")]
    CodeNotFound,
    #[error("wrong code")]
    WrongCode,
    #[error("rejected by the other device: {reason}")]
    Rejected { reason: String },
    #[error("timed out")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    #[error("transfer not found")]
    TransferNotFound,
    #[error("unreachable: {msg}")]
    Unreachable { msg: String },
}

impl WarpError {
    pub fn io(e: impl std::fmt::Display) -> Self {
        WarpError::Io { msg: e.to_string() }
    }
    pub fn protocol(e: impl std::fmt::Display) -> Self {
        WarpError::Protocol { msg: e.to_string() }
    }
    pub fn crypto(e: impl std::fmt::Display) -> Self {
        WarpError::Crypto { msg: e.to_string() }
    }
}

impl From<anyhow::Error> for WarpError {
    fn from(e: anyhow::Error) -> Self {
        if let Some(w) = e.downcast_ref::<WarpError>() {
            return w.clone();
        }
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            return WarpError::Io { msg: io.to_string() };
        }
        if e.downcast_ref::<tokio::time::error::Elapsed>().is_some() {
            return WarpError::Timeout;
        }
        WarpError::Protocol { msg: format!("{e:#}") }
    }
}

impl From<std::io::Error> for WarpError {
    fn from(e: std::io::Error) -> Self {
        WarpError::Io { msg: e.to_string() }
    }
}

pub type WarpResult<T> = Result<T, WarpError>;
