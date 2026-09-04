//! Wire protocol and replay buffer for terminale's persistent sessions.
//!
//! A PTY's child is sent `SIGHUP` when the master side closes, so a shell can
//! only outlive its window if some other process holds that master. That
//! process is the session daemon; this crate is the language it and the GUI
//! speak, and the buffer that lets a reattaching client rebuild a screen it
//! never saw being drawn.
//!
//! See `docs/design/persistent-sessions.md` for why the daemon relays bytes
//! rather than owning the emulation, and for the phases this crate is step one
//! of. It is deliberately transport-agnostic and side-effect free: no sockets,
//! no `unsafe`, no platform code — which is what makes the whole protocol
//! testable without a daemon to talk to.
//!
//! # Framing
//!
//! ```text
//! ┌──────────────┬─────┬───────────────┐
//! │ len: u32 be  │ tag │ body          │
//! └──────────────┴─────┴───────────────┘
//!   len = 1 + body.len()
//!   tag = 0x01 control (body is JSON)  |  0x02 data (body is raw PTY bytes)
//! ```
//!
//! Two frame kinds rather than one, because the two payloads want opposite
//! things. Control messages are rare and want to be readable and extensible, so
//! they are JSON. PTY traffic is the hot path and is arbitrary bytes, so it
//! travels as-is: base64 inside JSON would inflate every byte of a build log by
//! a third for no gain.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

// ── Frames ───────────────────────────────────────────────────────────────────

/// Frame tag for a JSON control message.
const TAG_CONTROL: u8 = 0x01;
/// Frame tag for raw PTY bytes.
const TAG_DATA: u8 = 0x02;

/// Largest frame this crate will decode.
///
/// A peer that announces more than this is either broken or hostile, and either
/// way the answer is to fail the connection rather than allocate what it asked
/// for. Sized well above a realistic control message and a PTY read.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// One decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// A control message, already parsed from its JSON body.
    Control(serde_json::Value),
    /// Raw PTY bytes: input when travelling client → daemon, output when
    /// travelling daemon → client.
    Data(Vec<u8>),
}

/// Why a frame could not be decoded.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    /// The buffer does not hold a whole frame yet. Not an error in a stream —
    /// read more and try again.
    #[error("frame incomplete")]
    Incomplete,
    /// The announced length exceeds [`MAX_FRAME_BYTES`].
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_BYTES} byte limit")]
    TooLarge(usize),
    /// A length of zero leaves no room for the tag byte.
    #[error("frame is empty")]
    Empty,
    /// The tag byte is neither control nor data.
    #[error("unknown frame tag {0:#04x}")]
    UnknownTag(u8),
    /// A control frame's body was not valid JSON.
    #[error("malformed control frame: {0}")]
    Malformed(String),
}

/// Encode a control message.
///
/// # Errors
///
/// Only if `msg` cannot be serialized, which for the protocol's own types
/// cannot happen.
pub fn encode_control<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(msg).map_err(|e| FrameError::Malformed(e.to_string()))?;
    Ok(frame(TAG_CONTROL, &body))
}

/// Encode raw PTY bytes.
#[must_use]
pub fn encode_data(bytes: &[u8]) -> Vec<u8> {
    frame(TAG_DATA, bytes)
}

/// Build one frame from a tag and a body.
fn frame(tag: u8, body: &[u8]) -> Vec<u8> {
    let len = u32::try_from(body.len() + 1).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(body.len() + 5);
    out.extend_from_slice(&len.to_be_bytes());
    out.push(tag);
    out.extend_from_slice(body);
    out
}

/// Try to decode one frame from the front of `buf`.
///
/// On success returns the frame and how many bytes it consumed, so a caller
/// draining a stream can drop exactly that much and loop. [`FrameError::Incomplete`]
/// means "not yet", and the caller keeps the buffer as it is.
///
/// # Errors
///
/// See [`FrameError`]. Every variant except `Incomplete` means the stream is no
/// longer trustworthy and the connection should be dropped.
pub fn decode_frame(buf: &[u8]) -> Result<(Frame, usize), FrameError> {
    if buf.len() < 4 {
        return Err(FrameError::Incomplete);
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if len == 0 {
        return Err(FrameError::Empty);
    }
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(len));
    }
    let total = 4 + len;
    if buf.len() < total {
        return Err(FrameError::Incomplete);
    }
    let tag = buf[4];
    let body = &buf[5..total];
    let f = match tag {
        TAG_CONTROL => Frame::Control(
            serde_json::from_slice(body).map_err(|e| FrameError::Malformed(e.to_string()))?,
        ),
        TAG_DATA => Frame::Data(body.to_vec()),
        other => return Err(FrameError::UnknownTag(other)),
    };
    Ok((f, total))
}

// ── Control messages ─────────────────────────────────────────────────────────

/// A session's stable identity.
///
/// Opaque on purpose: the daemon mints it and nothing else may assume a shape.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a client asks the daemon to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum ClientControl {
    /// Create a session: spawn `command` under a new PTY the daemon owns.
    Create {
        /// Program to run, plus its arguments. Empty means the daemon's idea of
        /// a login shell.
        command: Vec<String>,
        /// Working directory for the child.
        cwd: Option<String>,
        /// Profile name, carried so a reattaching client can restore the tab's
        /// look without keeping its own side table.
        profile: Option<String>,
        /// Initial grid size.
        cols: u16,
        /// Initial grid size.
        rows: u16,
    },
    /// Every session the daemon holds, alive or exited-but-unreaped.
    List,
    /// Attach to a session: the daemon answers [`DaemonControl::Attached`] and
    /// then streams data frames until detach.
    Attach {
        /// Which session.
        id: SessionId,
        /// Client's grid size, applied to the PTY before the replay is sent so
        /// the replayed bytes were produced for the size they will be rendered
        /// at.
        cols: u16,
        /// Client's grid size.
        rows: u16,
    },
    /// Stop streaming, leave the session running. What closing a window does.
    Detach,
    /// The grid changed size.
    Resize {
        /// New size.
        cols: u16,
        /// New size.
        rows: u16,
    },
    /// End a session and its child.
    Kill {
        /// Which session.
        id: SessionId,
    },
}

/// What the daemon says back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "kebab-case")]
pub enum DaemonControl {
    /// A session was created.
    Created {
        /// Its id.
        id: SessionId,
    },
    /// Answer to [`ClientControl::List`].
    Sessions {
        /// One entry per session, oldest first.
        sessions: Vec<SessionInfo>,
    },
    /// Attach succeeded. The replay follows as data frames, so a client can
    /// start rendering before it has all of them.
    Attached {
        /// Which session.
        id: SessionId,
        /// How many bytes of replay are on the way, so a client can show
        /// progress instead of appearing to hang on a large ring.
        replay_bytes: usize,
    },
    /// The replay is finished; everything after this frame is live output.
    ///
    /// A client needs the boundary: until it arrives, output is history being
    /// re-rendered, and things like the bell, notifications and the OSC 133
    /// "command finished" hooks must not fire again for commands that ended
    /// hours ago.
    ReplayDone,
    /// The child exited. The session stays listed until reaped, so a client that
    /// reattaches to a finished command can still read what it printed.
    Exited {
        /// Which session.
        id: SessionId,
        /// Exit status, where the platform reported one.
        status: Option<i32>,
    },
    /// The request could not be served.
    Error {
        /// Why, in a form worth showing a user.
        message: String,
    },
}

/// One session, as reported by [`ClientControl::List`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Stable id.
    pub id: SessionId,
    /// Last title the child set (OSC 0/2), when it set one.
    pub title: Option<String>,
    /// Last reported working directory (OSC 7), when the shell reports it.
    pub cwd: Option<String>,
    /// Profile the session was created with.
    pub profile: Option<String>,
    /// Child pid, for the user's own `ps`.
    pub pid: Option<u32>,
    /// Current grid size.
    pub cols: u16,
    /// Current grid size.
    pub rows: u16,
    /// Whether a client is streaming it right now.
    pub attached: bool,
    /// Whether the child is still running.
    pub alive: bool,
}

// ── Replay buffer ────────────────────────────────────────────────────────────

/// A bounded ring of recent PTY output, kept so a reattaching client can
/// rebuild the screen.
///
/// The emulator is a pure function of the bytes it is fed, so replaying these
/// reconstructs the grid exactly — provided the client starts from a reset
/// emulator, which is why [`ReplayRing::snapshot`] documents that requirement
/// rather than trying to encode a "current state" the daemon does not model.
#[derive(Debug)]
pub struct ReplayRing {
    /// Bytes held, oldest first.
    buf: VecDeque<u8>,
    /// Hard cap.
    capacity: usize,
    /// Whether anything has ever been dropped. A ring that never overflowed can
    /// be replayed from its very first byte, and that first byte is the child's
    /// first output — the one case where the replay is not just close but exact.
    truncated: bool,
}

impl ReplayRing {
    /// A ring holding at most `capacity` bytes. A capacity of zero keeps
    /// nothing, which is a legitimate way to turn replay off.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: VecDeque::new(),
            capacity,
            truncated: false,
        }
    }

    /// Record output, dropping the oldest bytes to stay within capacity.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.capacity == 0 {
            self.truncated |= !bytes.is_empty();
            return;
        }
        // A write larger than the ring can only leave its own tail.
        if bytes.len() >= self.capacity {
            self.buf.clear();
            self.buf.extend(&bytes[bytes.len() - self.capacity..]);
            self.truncated = true;
            return;
        }
        let overflow = (self.buf.len() + bytes.len()).saturating_sub(self.capacity);
        if overflow > 0 {
            self.buf.drain(..overflow);
            self.truncated = true;
        }
        self.buf.extend(bytes);
    }

    /// The bytes to replay, and whether they start at the child's first output.
    ///
    /// The caller must reset its emulator before feeding these: a ring that has
    /// wrapped begins mid-stream, so whatever modes, colours or cursor position
    /// were set before the cut are not in here.
    ///
    /// When the ring *has* wrapped, the replay starts just after the first
    /// newline rather than at the raw cut. A cut lands anywhere — including
    /// inside a UTF-8 sequence or halfway through an escape sequence — and a
    /// partial escape at the head of a replay does not stay a partial escape: it
    /// swallows the bytes after it and corrupts the first visible line. Starting
    /// at a line boundary costs at most one line and cannot mis-parse.
    #[must_use]
    pub fn snapshot(&self) -> (Vec<u8>, bool) {
        let bytes: Vec<u8> = self.buf.iter().copied().collect();
        if !self.truncated {
            return (bytes, true);
        }
        let start = bytes
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |i| i + 1);
        (bytes[start..].to_vec(), false)
    }

    /// Bytes currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether nothing is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Whether any output has been dropped, i.e. a replay would be partial.
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Framing ──────────────────────────────────────────────────────────────

    /// A control message must survive the round trip unchanged; this is the
    /// protocol's basic promise.
    #[test]
    fn a_control_frame_round_trips() {
        let msg = ClientControl::Attach {
            id: SessionId("abc".into()),
            cols: 120,
            rows: 40,
        };
        let bytes = encode_control(&msg).unwrap();
        let (frame, used) = decode_frame(&bytes).unwrap();
        assert_eq!(used, bytes.len());
        let Frame::Control(v) = frame else {
            panic!("expected a control frame")
        };
        assert_eq!(serde_json::from_value::<ClientControl>(v).unwrap(), msg);
    }

    /// PTY bytes must arrive byte-identical, including the ones that would not
    /// survive being put through JSON — an escape sequence and invalid UTF-8.
    #[test]
    fn data_frames_carry_arbitrary_bytes() {
        let payload = b"\x1b[31mred\x1b[0m \xff\xfe\x00binary";
        let bytes = encode_data(payload);
        let (frame, used) = decode_frame(&bytes).unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(frame, Frame::Data(payload.to_vec()));
    }

    /// A stream hands over whatever arrived, which is rarely a frame boundary.
    /// Every prefix of a frame must read as `Incomplete` rather than as
    /// garbage.
    #[test]
    fn every_partial_frame_is_incomplete() {
        let bytes = encode_data(b"hello");
        for cut in 0..bytes.len() {
            assert_eq!(
                decode_frame(&bytes[..cut]),
                Err(FrameError::Incomplete),
                "prefix of {cut} bytes"
            );
        }
        assert!(decode_frame(&bytes).is_ok());
    }

    /// Frames decode one at a time, and the reported length is what lets the
    /// caller find the next one.
    #[test]
    fn frames_decode_back_to_back() {
        let mut stream = encode_data(b"one");
        stream.extend(encode_control(&ClientControl::Detach).unwrap());
        stream.extend(encode_data(b"two"));

        let (f1, n1) = decode_frame(&stream).unwrap();
        assert_eq!(f1, Frame::Data(b"one".to_vec()));
        let (f2, n2) = decode_frame(&stream[n1..]).unwrap();
        assert!(matches!(f2, Frame::Control(_)));
        let (f3, _) = decode_frame(&stream[n1 + n2..]).unwrap();
        assert_eq!(f3, Frame::Data(b"two".to_vec()));
    }

    /// An absurd announced length must be refused before anything is allocated
    /// for it: the peer is broken or hostile, and either way this connection is
    /// over.
    #[test]
    fn an_oversized_length_is_refused_not_allocated() {
        let mut bytes = (MAX_FRAME_BYTES as u32 + 1).to_be_bytes().to_vec();
        bytes.push(TAG_DATA);
        assert_eq!(
            decode_frame(&bytes),
            Err(FrameError::TooLarge(MAX_FRAME_BYTES + 1))
        );
    }

    /// A zero length has no room for the tag byte, so it is malformed rather
    /// than an empty data frame.
    #[test]
    fn a_zero_length_frame_is_rejected() {
        let bytes = 0u32.to_be_bytes().to_vec();
        assert_eq!(decode_frame(&bytes), Err(FrameError::Empty));
    }

    /// An unknown tag is a version skew or a wrong peer; guessing would be
    /// worse than failing.
    #[test]
    fn an_unknown_tag_is_rejected() {
        let mut bytes = 1u32.to_be_bytes().to_vec();
        bytes.push(0x7f);
        assert_eq!(decode_frame(&bytes), Err(FrameError::UnknownTag(0x7f)));
    }

    /// A control frame whose body is not JSON is malformed — and must not take
    /// the process down.
    #[test]
    fn a_malformed_control_body_is_an_error() {
        let mut bytes = 5u32.to_be_bytes().to_vec();
        bytes.push(TAG_CONTROL);
        bytes.extend(b"{oops");
        assert!(matches!(
            decode_frame(&bytes),
            Err(FrameError::Malformed(_))
        ));
    }

    /// Control messages are tagged, so a new variant added later does not
    /// change the meaning of the existing ones on a peer that predates it.
    #[test]
    fn control_messages_are_tagged_on_the_wire() {
        let json = serde_json::to_string(&ClientControl::List).unwrap();
        assert_eq!(json, r#"{"op":"list"}"#);
        let json = serde_json::to_string(&DaemonControl::ReplayDone).unwrap();
        assert_eq!(json, r#"{"ev":"replay-done"}"#);
    }

    // ── Replay ring ──────────────────────────────────────────────────────────

    /// Under capacity, nothing is lost and the replay is exact — the case where
    /// a reattached session is indistinguishable from one never detached.
    #[test]
    fn a_ring_under_capacity_replays_everything() {
        let mut ring = ReplayRing::new(64);
        ring.push(b"first line\n");
        ring.push(b"second line\n");
        let (bytes, exact) = ring.snapshot();
        assert_eq!(bytes, b"first line\nsecond line\n");
        assert!(
            exact,
            "nothing was dropped, so the replay starts at byte one"
        );
        assert!(!ring.truncated());
    }

    /// Over capacity, the newest output is what survives.
    #[test]
    fn a_full_ring_keeps_the_tail() {
        let mut ring = ReplayRing::new(10);
        ring.push(b"aaaaa\n");
        ring.push(b"bbbbb\n");
        assert_eq!(ring.len(), 10);
        assert!(ring.truncated());
        // Trimmed to the line boundary inside what is left.
        let (bytes, exact) = ring.snapshot();
        assert!(!exact);
        assert_eq!(bytes, b"bbbbb\n");
    }

    /// A single write larger than the whole ring keeps its own tail rather than
    /// overflowing or panicking — a `cat` of a big file is exactly this.
    #[test]
    fn one_huge_write_keeps_its_tail() {
        let mut ring = ReplayRing::new(8);
        let big: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        ring.push(&big);
        assert_eq!(ring.len(), 8, "only the last 8 bytes fit");
        assert_eq!(ring.buf.iter().copied().collect::<Vec<_>>(), big[992..]);
        assert!(ring.truncated());
        // This data has no newline at all, so the line trim leaves nothing:
        // better an empty replay than one that starts mid-escape. A real
        // terminal stream has newlines; a binary blob replayed verbatim is
        // precisely what would corrupt the first screen.
        assert!(ring.snapshot().0.is_empty());
    }

    /// The trim must cut at the *first* newline, so nothing before a line
    /// boundary — where a half escape sequence could be hiding — is replayed.
    #[test]
    fn a_wrapped_ring_starts_after_a_newline() {
        let mut ring = ReplayRing::new(16);
        // The `\x1b[3` at the head is a truncated escape sequence: replayed as
        // is, it would eat the letters after it.
        ring.push(b"XXXXXXXXXXXXXXXX");
        ring.push(b"\x1b[3m\nvisible\n");
        let (bytes, exact) = ring.snapshot();
        assert!(!exact);
        assert_eq!(bytes, b"visible\n");
        assert!(!bytes.starts_with(b"\x1b"), "no partial escape may survive");
    }

    /// A capacity of zero is a valid way to say "no replay", and must not
    /// pretend the output was kept.
    #[test]
    fn a_zero_capacity_ring_keeps_nothing() {
        let mut ring = ReplayRing::new(0);
        ring.push(b"output");
        assert!(ring.is_empty());
        assert!(ring.truncated(), "it was dropped, and callers must know");
        assert_eq!(ring.snapshot().0, Vec::<u8>::new());
    }
}
