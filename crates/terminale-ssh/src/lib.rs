//! SSH client wrapper for `terminale`.
//!
//! Wraps [`russh`] to expose an API close to `terminale_core::Session`:
//! you `connect`, hand back an output receiver + a writer, and the
//! background task pumps bytes between the local emulator and the
//! remote PTY.
//!
//! ## Host-key verification
//!
//! The handler consults a `known_hosts` file on every connection:
//!
//! - **Known** (host + key in store): accepted silently.
//! - **Unknown** (host not in store), policy `accept_new` (default): the key
//!   is pinned and the connection proceeds (trust-on-first-use / TOFU).
//! - **Unknown**, policy `strict`: the connection is refused.
//! - **Changed** (host known, key differs): **always** refused as a possible
//!   MITM, unless policy is `off`.
//! - Policy `off`: any key is accepted without verification (legacy behaviour).

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

use async_trait::async_trait;
use bytes::Bytes;
use russh::client::{self, Handler};
use russh::keys::agent::client::AgentClient;
use russh::keys::{key, load_secret_key, PublicKeyBase64};
use russh::ChannelMsg;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

pub use terminale_config::{HostKeyPolicy, SshConfig};

/// What credentials to present to the server.
#[derive(Debug, Clone)]
pub enum AuthMethod {
    /// Use the running SSH agent (`ssh-agent` / `gpg-agent`). Preferred —
    /// no key material is handled by terminale. Not supported on Windows
    /// with the current `russh-keys` (no Pageant/named-pipe backend); use
    /// [`AuthMethod::Key`] there.
    Agent,
    /// Plain password.
    Password(String),
    /// Private-key file on disk. Optional passphrase for encrypted keys.
    /// Prefer ed25519 keys — RSA keys link the `rsa` crate
    /// (RUSTSEC-2023-0071, a Marvin-attack timing side-channel).
    Key {
        /// Path to the private key file (OpenSSH or PEM).
        path: PathBuf,
        /// Passphrase to decrypt the key, if any.
        passphrase: Option<String>,
    },
}

/// Where to connect and as whom.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// Hostname or IP.
    pub host: String,
    /// TCP port (typically 22).
    pub port: u16,
    /// Remote username.
    pub user: String,
    /// Credentials.
    pub auth: AuthMethod,
    /// Host-key verification policy.
    pub host_key_policy: HostKeyPolicy,
    /// Path to the SSH known-hosts file used for verification.
    pub known_hosts: PathBuf,
}

/// Errors produced by the SSH client wrapper.
#[derive(Debug, Error)]
pub enum SshError {
    /// `russh` protocol error.
    #[error("ssh: {0}")]
    Ssh(#[from] russh::Error),
    /// Key parsing / decoding error.
    #[error("ssh key: {0}")]
    Key(#[from] russh::keys::Error),
    /// I/O on the underlying transport.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Authentication was rejected by the server.
    #[error("authentication rejected")]
    AuthRejected,
    /// No usable identity was available (e.g. agent auth requested but the
    /// agent is unreachable or holds no keys).
    #[error("ssh agent unavailable or empty: {0}")]
    Agent(String),
    /// The background channel-pump task has exited.
    #[error("ssh session closed")]
    Closed,
    /// The server presented a host key that differs from the one recorded in
    /// the known-hosts store. This is a possible MITM attack. The connection
    /// was refused. The user must remove the old entry from `known_hosts`
    /// manually if the key change is legitimate.
    #[error(
        "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED for {host}:{port}!\n\
         The server's host key no longer matches the entry in {known_hosts}.\n\
         This may indicate a man-in-the-middle attack. Connection refused.\n\
         To accept the new key, remove the old entry from {known_hosts} and reconnect."
    )]
    HostKeyChanged {
        /// The hostname that was being connected to.
        host: String,
        /// The TCP port.
        port: u16,
        /// Path to the known-hosts file that holds the conflicting entry.
        known_hosts: PathBuf,
    },
    /// The server host key is not yet in the known-hosts store and the policy
    /// is `strict` — the connection was refused.
    #[error(
        "Host key for {host}:{port} is not in the known-hosts file ({known_hosts}).\n\
         Connection refused (host_key_policy = strict).\n\
         To allow this host, change the policy to `accept_new` or add the key manually."
    )]
    HostKeyUnknown {
        /// The hostname that was being connected to.
        host: String,
        /// The TCP port.
        port: u16,
        /// Path to the known-hosts file that was consulted.
        known_hosts: PathBuf,
    },
}

/// Bytes notifier — fired after each PTY chunk arrives.
pub type DataNotifier = Arc<dyn Fn() + Send + Sync>;

/// Commands shipped to the background channel-pump task.
enum SshCmd {
    Write(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Close(oneshot::Sender<()>),
}

/// A live SSH session backed by a remote shell behind a PTY.
pub struct SshSession {
    cmd_tx: mpsc::UnboundedSender<SshCmd>,
    cols: u16,
    rows: u16,
    output_rx: Option<mpsc::UnboundedReceiver<Bytes>>,
}

impl SshSession {
    /// Connect, authenticate, request a PTY + shell.
    ///
    /// # Errors
    ///
    /// Returns [`SshError`] on socket failure, auth rejection, or any
    /// russh protocol error. Returns [`SshError::HostKeyChanged`] when the
    /// server key has changed (possible MITM). Returns
    /// [`SshError::HostKeyUnknown`] when the policy is `strict` and the host
    /// is not yet in the known-hosts store.
    pub async fn connect(
        opts: ConnectOptions,
        cols: u16,
        rows: u16,
        notifier: Option<DataNotifier>,
    ) -> Result<Self, SshError> {
        let cfg = Arc::new(client::Config {
            inactivity_timeout: Some(std::time::Duration::from_secs(3600)),
            ..Default::default()
        });
        let handler = KnownHostsHandler {
            host: opts.host.clone(),
            port: opts.port,
            policy: opts.host_key_policy,
            known_hosts: opts.known_hosts.clone(),
        };
        let mut session = client::connect(cfg, (opts.host.as_str(), opts.port), handler).await?;

        let ok = match opts.auth {
            AuthMethod::Agent => authenticate_with_agent(&mut session, &opts.user).await?,
            AuthMethod::Password(pw) => session.authenticate_password(&opts.user, pw).await?,
            AuthMethod::Key { path, passphrase } => {
                let key = load_secret_key(&path, passphrase.as_deref())?;
                session
                    .authenticate_publickey(&opts.user, Arc::new(key))
                    .await?
            }
        };
        if !ok {
            return Err(SshError::AuthRejected);
        }

        let mut channel = session.channel_open_session().await?;
        channel
            .request_pty(
                true,
                "xterm-256color",
                u32::from(cols),
                u32::from(rows),
                0,
                0,
                &[],
            )
            .await?;
        channel.request_shell(true).await?;

        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SshCmd>();
        let (output_tx, output_rx) = mpsc::unbounded_channel::<Bytes>();

        // Single pump task: drains incoming ChannelMsg events AND
        // services outgoing commands. Channel methods take &self for
        // writes but `wait()` needs &mut, so we can't trivially split
        // — `tokio::select!` keeps it single-task.
        tokio::spawn(async move {
            // Keep `session` alive so the underlying TCP/protocol task
            // doesn't drop. We never call methods on it after this.
            let _session = session;
            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(SshCmd::Write(data)) => {
                                if let Err(e) = channel.data(&data[..]).await {
                                    tracing::warn!(?e, "ssh write failed");
                                }
                            }
                            Some(SshCmd::Resize { cols, rows }) => {
                                let _ = channel
                                    .window_change(u32::from(cols), u32::from(rows), 0, 0)
                                    .await;
                            }
                            Some(SshCmd::Close(ack)) => {
                                let _ = channel.close().await;
                                let _ = ack.send(());
                                break;
                            }
                            None => break,
                        }
                    }
                    msg = channel.wait() => {
                        match msg {
                            Some(ChannelMsg::Data { data }) => {
                                let _ = output_tx.send(Bytes::copy_from_slice(&data));
                                if let Some(n) = notifier.as_ref() { n(); }
                            }
                            Some(ChannelMsg::ExtendedData { data, .. }) => {
                                // stderr also lands in the terminal stream.
                                let _ = output_tx.send(Bytes::copy_from_slice(&data));
                                if let Some(n) = notifier.as_ref() { n(); }
                            }
                            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                            _ => {}
                        }
                    }
                }
            }
        });

        Ok(Self {
            cmd_tx,
            cols,
            rows,
            output_rx: Some(output_rx),
        })
    }

    /// Take the channel receiving remote-output bytes. Callable once.
    pub fn take_output(&mut self) -> Option<mpsc::UnboundedReceiver<Bytes>> {
        self.output_rx.take()
    }

    /// Current remote PTY dimensions.
    #[must_use]
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Resize the remote PTY (server-side SIGWINCH).
    ///
    /// # Errors
    ///
    /// Returns [`SshError::Closed`] when the pump task has exited.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), SshError> {
        self.cols = cols;
        self.rows = rows;
        self.cmd_tx
            .send(SshCmd::Resize { cols, rows })
            .map_err(|_| SshError::Closed)
    }

    /// Send bytes to the remote shell's stdin.
    ///
    /// # Errors
    ///
    /// Returns [`SshError::Closed`] when the pump task has exited.
    pub fn write_input(&self, data: &[u8]) -> Result<(), SshError> {
        self.cmd_tx
            .send(SshCmd::Write(data.to_vec()))
            .map_err(|_| SshError::Closed)
    }

    /// Cleanly close the channel and disconnect.
    pub async fn close(self) {
        let (tx, rx) = oneshot::channel();
        let _ = self.cmd_tx.send(SshCmd::Close(tx));
        let _ = rx.await;
    }
}

/// Try every identity the SSH agent holds, returning `Ok(true)` on the
/// first one the server accepts. Errors when the agent can't be reached
/// (e.g. Windows, where `russh-keys` has no agent backend, or a missing
/// `SSH_AUTH_SOCK` on Unix). `Ok(false)` means the agent was reachable
/// but no key was accepted — surfaced to the caller as `AuthRejected`.
async fn authenticate_with_agent(
    session: &mut client::Handle<KnownHostsHandler>,
    user: &str,
) -> Result<bool, SshError> {
    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|e| SshError::Agent(format!("could not connect to ssh-agent: {e}")))?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| SshError::Agent(format!("could not list agent identities: {e}")))?;
    if identities.is_empty() {
        return Err(SshError::Agent("ssh-agent holds no keys".into()));
    }
    for key in identities {
        // `authenticate_future` consumes + returns the agent so it can sign
        // again for the next key if this one is rejected.
        let (returned, result) = session.authenticate_future(user, key, agent).await;
        agent = returned;
        match result {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(e) => return Err(SshError::Agent(format!("agent sign failed: {e}"))),
        }
    }
    Ok(false)
}

/// Host-key verification handler backed by a `known_hosts` store.
///
/// Decision table (see module-level docs):
///
/// | Verdict  | `accept_new` | `strict` | `off` |
/// |----------|-------------|---------|-------|
/// | Known    | accept      | accept  | accept |
/// | Unknown  | pin + accept | refuse | accept |
/// | Changed  | refuse      | refuse  | accept |
struct KnownHostsHandler {
    host: String,
    port: u16,
    policy: HostKeyPolicy,
    known_hosts: PathBuf,
}

#[async_trait]
impl Handler for KnownHostsHandler {
    type Error = SshError;

    async fn check_server_key(
        &mut self,
        server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key.public_key_base64();

        // `off` — accept everything without consulting the store.
        if self.policy == HostKeyPolicy::Off {
            tracing::info!(
                host = %self.host,
                port = self.port,
                %fingerprint,
                "accepting server host key (host_key_policy = off, no verification)"
            );
            return Ok(true);
        }

        // Consult the known-hosts file.
        let verdict =
            check_host_key_verdict(&self.known_hosts, &self.host, self.port, server_public_key)?;

        match verdict {
            HostKeyVerdict::Known => {
                tracing::debug!(
                    host = %self.host,
                    port = self.port,
                    %fingerprint,
                    "server host key matches known_hosts"
                );
                Ok(true)
            }
            HostKeyVerdict::Unknown => {
                match self.policy {
                    HostKeyPolicy::AcceptNew => {
                        // Pin the key: append it to the known-hosts file so
                        // future connections can detect a key change.
                        if let Err(e) = russh::keys::learn_known_hosts_path(
                            &self.host,
                            self.port,
                            server_public_key,
                            &self.known_hosts,
                        ) {
                            tracing::warn!(
                                host = %self.host,
                                port = self.port,
                                err = %e,
                                "could not write to known_hosts; key will NOT be pinned"
                            );
                        } else {
                            tracing::info!(
                                host = %self.host,
                                port = self.port,
                                %fingerprint,
                                known_hosts = %self.known_hosts.display(),
                                "new server host key pinned (TOFU)"
                            );
                        }
                        Ok(true)
                    }
                    HostKeyPolicy::Strict => {
                        tracing::warn!(
                            host = %self.host,
                            port = self.port,
                            %fingerprint,
                            "refusing unknown host key (host_key_policy = strict)"
                        );
                        Err(SshError::HostKeyUnknown {
                            host: self.host.clone(),
                            port: self.port,
                            known_hosts: self.known_hosts.clone(),
                        })
                    }
                    HostKeyPolicy::Off => unreachable!("handled above"),
                }
            }
            HostKeyVerdict::Changed => {
                // Changed key is always refused (unless policy = off, handled
                // at the top). This is the critical MITM-detection path.
                tracing::error!(
                    host = %self.host,
                    port = self.port,
                    %fingerprint,
                    known_hosts = %self.known_hosts.display(),
                    "REMOTE HOST KEY HAS CHANGED — possible MITM attack!"
                );
                Err(SshError::HostKeyChanged {
                    host: self.host.clone(),
                    port: self.port,
                    known_hosts: self.known_hosts.clone(),
                })
            }
        }
    }
}

// ── Host-key verdict logic ────────────────────────────────────────────────────

/// Result of consulting the known-hosts store for a specific `(host, port, key)`
/// triple.
#[derive(Debug, PartialEq, Eq)]
pub enum HostKeyVerdict {
    /// The host is in the store and the key matches.
    Known,
    /// The host is not in the store at all.
    Unknown,
    /// The host is in the store but the key is different — possible MITM.
    Changed,
}

/// Consult the known-hosts file at `path` and return a [`HostKeyVerdict`] for
/// `(host, port, server_key)`.
///
/// If the file does not exist the host is considered `Unknown`. I/O errors
/// other than "file not found" propagate as [`SshError::Io`].
///
/// This is a pure function (no policy decisions here) — the caller applies
/// the policy.
pub fn check_host_key_verdict(
    path: &std::path::Path,
    host: &str,
    port: u16,
    server_key: &key::PublicKey,
) -> Result<HostKeyVerdict, SshError> {
    use russh::keys::{check_known_hosts_path, Error as KErr};
    match check_known_hosts_path(host, port, server_key, path) {
        Ok(true) => Ok(HostKeyVerdict::Known),
        Ok(false) => Ok(HostKeyVerdict::Unknown),
        Err(KErr::KeyChanged { .. }) => Ok(HostKeyVerdict::Changed),
        // File not found → treat as empty known-hosts (Unknown).
        Err(KErr::IO(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(HostKeyVerdict::Unknown)
        }
        Err(e) => Err(SshError::Key(e)),
    }
}

/// Append `server_key` for `(host, port)` to the known-hosts file at `path`.
/// Creates the file (and parent directories) if they do not yet exist.
///
/// # Errors
///
/// Returns [`SshError::Key`] on I/O or encoding errors.
pub fn add_host_key(
    path: &std::path::Path,
    host: &str,
    port: u16,
    server_key: &key::PublicKey,
) -> Result<(), SshError> {
    russh::keys::learn_known_hosts_path(host, port, server_key, path)
        .map_err(SshError::Key)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::parse_public_key_base64;
    use std::io::Write;

    /// A real ed25519 public key blob (base64) — from the russh-keys test suite.
    const ED25519_KEY_A: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    /// A different ed25519 key.
    const ED25519_KEY_B: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G1sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";

    fn key_a() -> key::PublicKey {
        parse_public_key_base64(ED25519_KEY_A).expect("key A must parse")
    }

    fn key_b() -> key::PublicKey {
        parse_public_key_base64(ED25519_KEY_B).expect("key B must parse")
    }

    fn tmp_known_hosts(content: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("tempfile");
        f.write_all(content).expect("write");
        f
    }

    // ── check_host_key_verdict ────────────────────────────────────────────────

    #[test]
    fn verdict_unknown_when_file_missing() {
        let path = std::path::Path::new("/nonexistent/path/to/known_hosts_that_does_not_exist");
        let v = check_host_key_verdict(path, "example.com", 22, &key_a()).unwrap();
        assert_eq!(v, HostKeyVerdict::Unknown);
    }

    #[test]
    fn verdict_unknown_when_file_empty() {
        let f = tmp_known_hosts(b"");
        let v = check_host_key_verdict(f.path(), "example.com", 22, &key_a()).unwrap();
        assert_eq!(v, HostKeyVerdict::Unknown);
    }

    #[test]
    fn verdict_known_when_key_matches() {
        // Write a known_hosts entry for example.com with key_a.
        let f = tmp_known_hosts(b"");
        add_host_key(f.path(), "example.com", 22, &key_a()).unwrap();
        let v = check_host_key_verdict(f.path(), "example.com", 22, &key_a()).unwrap();
        assert_eq!(v, HostKeyVerdict::Known);
    }

    #[test]
    fn verdict_changed_when_key_differs() {
        // Pin key_a, then present key_b — should be Changed.
        let f = tmp_known_hosts(b"");
        add_host_key(f.path(), "example.com", 22, &key_a()).unwrap();
        let v = check_host_key_verdict(f.path(), "example.com", 22, &key_b()).unwrap();
        assert_eq!(v, HostKeyVerdict::Changed);
    }

    #[test]
    fn verdict_unknown_for_different_host() {
        // key_a is pinned for example.com; querying other.com → Unknown.
        let f = tmp_known_hosts(b"");
        add_host_key(f.path(), "example.com", 22, &key_a()).unwrap();
        let v = check_host_key_verdict(f.path(), "other.com", 22, &key_a()).unwrap();
        assert_eq!(v, HostKeyVerdict::Unknown);
    }

    #[test]
    fn verdict_known_non_standard_port() {
        // Non-default ports are stored as `[host]:port` in known_hosts.
        let f = tmp_known_hosts(b"");
        add_host_key(f.path(), "example.com", 2222, &key_a()).unwrap();
        let v = check_host_key_verdict(f.path(), "example.com", 2222, &key_a()).unwrap();
        assert_eq!(v, HostKeyVerdict::Known);
        // Port 22 for the same host should be Unknown (different entry).
        let v22 = check_host_key_verdict(f.path(), "example.com", 22, &key_a()).unwrap();
        assert_eq!(v22, HostKeyVerdict::Unknown);
    }

    #[test]
    fn verdict_changed_only_for_same_port() {
        // Pin key_a at port 2222; present key_b at port 2222 → Changed.
        // Port 22 with key_b → Unknown (no entry for port 22).
        let f = tmp_known_hosts(b"");
        add_host_key(f.path(), "example.com", 2222, &key_a()).unwrap();
        let changed = check_host_key_verdict(f.path(), "example.com", 2222, &key_b()).unwrap();
        assert_eq!(changed, HostKeyVerdict::Changed);
        let unknown = check_host_key_verdict(f.path(), "example.com", 22, &key_b()).unwrap();
        assert_eq!(unknown, HostKeyVerdict::Unknown);
    }

    // ── add_host_key ──────────────────────────────────────────────────────────

    #[test]
    fn add_host_key_creates_file_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        // File does not exist yet.
        assert!(!path.exists());
        add_host_key(&path, "example.com", 22, &key_a()).unwrap();
        assert!(path.exists());
        // The verdict should now be Known.
        let v = check_host_key_verdict(&path, "example.com", 22, &key_a()).unwrap();
        assert_eq!(v, HostKeyVerdict::Known);
    }

    #[test]
    fn add_host_key_appends_without_destroying_existing_entries() {
        let f = tmp_known_hosts(b"");
        add_host_key(f.path(), "host-a.example.com", 22, &key_a()).unwrap();
        add_host_key(f.path(), "host-b.example.com", 22, &key_b()).unwrap();
        // Both entries must be resolvable.
        assert_eq!(
            check_host_key_verdict(f.path(), "host-a.example.com", 22, &key_a()).unwrap(),
            HostKeyVerdict::Known
        );
        assert_eq!(
            check_host_key_verdict(f.path(), "host-b.example.com", 22, &key_b()).unwrap(),
            HostKeyVerdict::Known
        );
    }

    // ── policy gating (via KnownHostsHandler) ────────────────────────────────

    /// Drive `KnownHostsHandler::check_server_key` synchronously (Tokio
    /// single-thread runtime) to verify policy gating without a live server.
    fn run_handler(
        handler: &mut KnownHostsHandler,
        key: &key::PublicKey,
    ) -> Result<bool, SshError> {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(handler.check_server_key(key))
    }

    #[test]
    fn policy_off_accepts_anything() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let mut h = KnownHostsHandler {
            host: "example.com".into(),
            port: 22,
            policy: HostKeyPolicy::Off,
            known_hosts: path.clone(),
        };
        // Unknown host → accepted.
        assert!(
            matches!(run_handler(&mut h, &key_a()), Ok(true)),
            "Off policy must accept unknown host"
        );
        // Pin key_a, then present key_b (changed) → still accepted with Off.
        add_host_key(&path, "example.com", 22, &key_a()).unwrap();
        assert!(
            matches!(run_handler(&mut h, &key_b()), Ok(true)),
            "Off policy must accept changed key"
        );
    }

    #[test]
    fn policy_accept_new_pins_and_accepts_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let mut h = KnownHostsHandler {
            host: "example.com".into(),
            port: 22,
            policy: HostKeyPolicy::AcceptNew,
            known_hosts: path.clone(),
        };
        // Unknown host → accepted (and pinned).
        assert!(
            matches!(run_handler(&mut h, &key_a()), Ok(true)),
            "AcceptNew must accept unknown host"
        );
        // The key must now be recorded.
        assert_eq!(
            check_host_key_verdict(&path, "example.com", 22, &key_a()).unwrap(),
            HostKeyVerdict::Known
        );
        // Known host with the same key → still accepted.
        assert!(
            matches!(run_handler(&mut h, &key_a()), Ok(true)),
            "AcceptNew must accept known host with matching key"
        );
    }

    #[test]
    fn policy_accept_new_refuses_changed_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        // Pre-pin key_a.
        add_host_key(&path, "example.com", 22, &key_a()).unwrap();
        let mut h = KnownHostsHandler {
            host: "example.com".into(),
            port: 22,
            policy: HostKeyPolicy::AcceptNew,
            known_hosts: path,
        };
        // Present key_b (changed) → must be refused.
        let result = run_handler(&mut h, &key_b());
        assert!(
            matches!(result, Err(SshError::HostKeyChanged { .. })),
            "changed key must be refused with AcceptNew: {result:?}"
        );
    }

    #[test]
    fn policy_strict_refuses_unknown_host() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts"); // does not exist
        let mut h = KnownHostsHandler {
            host: "example.com".into(),
            port: 22,
            policy: HostKeyPolicy::Strict,
            known_hosts: path,
        };
        let result = run_handler(&mut h, &key_a());
        assert!(
            matches!(result, Err(SshError::HostKeyUnknown { .. })),
            "unknown host must be refused with Strict: {result:?}"
        );
    }

    #[test]
    fn policy_strict_accepts_known_host() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        // Pre-pin key_a.
        add_host_key(&path, "example.com", 22, &key_a()).unwrap();
        let mut h = KnownHostsHandler {
            host: "example.com".into(),
            port: 22,
            policy: HostKeyPolicy::Strict,
            known_hosts: path,
        };
        assert!(
            matches!(run_handler(&mut h, &key_a()), Ok(true)),
            "Strict must accept a key that is already in known_hosts"
        );
    }

    #[test]
    fn policy_strict_refuses_changed_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        add_host_key(&path, "example.com", 22, &key_a()).unwrap();
        let mut h = KnownHostsHandler {
            host: "example.com".into(),
            port: 22,
            policy: HostKeyPolicy::Strict,
            known_hosts: path,
        };
        let result = run_handler(&mut h, &key_b());
        assert!(
            matches!(result, Err(SshError::HostKeyChanged { .. })),
            "changed key must be refused with Strict: {result:?}"
        );
    }
}
