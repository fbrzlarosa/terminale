//! Error types shared across `terminale-core`.

use thiserror::Error;

/// Errors that can occur in the core layer.
#[derive(Debug, Error)]
pub enum CoreError {
    /// The underlying PTY layer returned an error.
    #[error("pty error: {0}")]
    Pty(String),

    /// I/O error while reading from or writing to a session.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A remote (e.g. SSH) backend returned an error or has closed.
    #[error("remote session error: {0}")]
    Remote(String),

    /// A session was queried after it had been dropped.
    #[error("session no longer exists")]
    SessionGone,
}

/// Convenience alias used throughout this crate.
pub type CoreResult<T> = Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_converts() {
        let io_err = std::io::Error::other("boom");
        let core_err: CoreError = io_err.into();
        assert!(matches!(core_err, CoreError::Io(_)));
    }
}
