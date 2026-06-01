//! Events emitted by a [`Session`](crate::Session) toward higher layers
//! (terminal engine, UI, plugins).

use crate::session::SessionId;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// An event emitted by a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionEvent {
    /// New output read from the PTY master.
    Output {
        /// Session this output belongs to.
        session: SessionId,
        /// Raw bytes received.
        data: Bytes,
    },
    /// The underlying process exited.
    Exited {
        /// Session that exited.
        session: SessionId,
        /// Exit status (platform-dependent).
        code: Option<i32>,
    },
    /// The session was resized (cols x rows).
    Resized {
        /// Session that was resized.
        session: SessionId,
        /// New column count.
        cols: u16,
        /// New row count.
        rows: u16,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_event_carries_session_id() {
        let id = SessionId::new();
        let ev = SessionEvent::Output {
            session: id,
            data: Bytes::from_static(b"hello"),
        };
        match ev {
            SessionEvent::Output { session, data } => {
                assert_eq!(session, id);
                assert_eq!(data.as_ref(), b"hello");
            }
            _ => panic!("wrong variant"),
        }
    }
}
