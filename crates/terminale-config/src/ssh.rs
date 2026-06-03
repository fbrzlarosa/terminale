//! SSH host definitions — each entry is a named remote you can open as a
//! terminal tab (via the command palette: `SSH: <host name>`, or a "New SSH
//! tab" picker). Mirrors the shape of a `~/.ssh/config` host stanza but
//! kept deliberately small (no ProxyJump / per-host options yet).
//!
//! Also holds [`SshConfig`]: global SSH settings (known-hosts path, host-key
//! verification policy) that apply to every connection.
//!
//! # OpenSSH config import
//!
//! [`SshConfig`] now has three fields that control how the user's
//! `~/.ssh/config` file is surfaced inside the terminal:
//!
//! - `import_openssh_config` — the import mode:
//!   - [`Off`](ImportOpenSshConfig::Off): feature disabled (default).
//!   - [`ImportOnce`](ImportOpenSshConfig::ImportOnce): a one-shot button /
//!     palette command that appends new hosts to `ssh_hosts` and persists.
//!   - [`Live`](ImportOpenSshConfig::Live): on every startup / config-reload
//!     the parsed hosts are merged into the in-memory list; they are **not**
//!     written to `config.toml` (ephemeral read-only import).
//! - `openssh_config_path` — path to the OpenSSH client config file.
//!   Defaults to `~/.ssh/config`.
//!
//! The actual parsing is done by [`parse_ssh_config`], a pure function that
//! accepts the raw text and returns a `Vec<ParsedSshHost>`, one entry per
//! concrete `Host` stanza (wildcard `*` patterns are skipped). Callers map
//! those into [`SshHost`] values via [`ParsedSshHost::into_ssh_host`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── Host-key verification ─────────────────────────────────────────────────────

/// What to do when connecting to an SSH host whose key is not yet in the
/// known-hosts store, or whose key has changed since the last connection.
///
/// The default is [`accept_new`](HostKeyPolicy::AcceptNew): pin the key on
/// the first connection (trust-on-first-use, TOFU), and **refuse** if the key
/// changes afterwards. A changed key always produces an error regardless of
/// policy, except when the policy is [`off`](HostKeyPolicy::Off).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostKeyPolicy {
    /// **Default.** Accept and pin unknown host keys (TOFU); refuse a changed
    /// key with an error.
    #[default]
    AcceptNew,
    /// Refuse connections to hosts that are not already in the known-hosts
    /// store. Changed keys are also refused.
    Strict,
    /// Accept any host key without verification (the original behaviour).
    /// Disables MITM detection entirely — use only in isolated test environments.
    Off,
}

impl HostKeyPolicy {
    /// All variants in display order — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::AcceptNew, Self::Strict, Self::Off]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::AcceptNew => "Accept new (TOFU)",
            Self::Strict => "Strict (known hosts only)",
            Self::Off => "Off (no verification)",
        }
    }
}

/// Returns the OS-default path to the SSH known-hosts file.
///
/// - Unix: `~/.ssh/known_hosts`
/// - Windows: `%USERPROFILE%\.ssh\known_hosts`
#[must_use]
pub fn default_known_hosts_path() -> PathBuf {
    directories::BaseDirs::new().map_or_else(
        || PathBuf::from(".ssh/known_hosts"),
        |b| b.home_dir().join(".ssh").join("known_hosts"),
    )
}

fn default_known_hosts_path_serde() -> PathBuf {
    default_known_hosts_path()
}

// ── OpenSSH config import ─────────────────────────────────────────────────────

/// Returns the OS-default path to the OpenSSH client configuration file.
///
/// - Unix: `~/.ssh/config`
/// - Windows: `%USERPROFILE%\.ssh\config`
#[must_use]
pub fn default_openssh_config_path() -> PathBuf {
    directories::BaseDirs::new().map_or_else(
        || PathBuf::from(".ssh/config"),
        |b| b.home_dir().join(".ssh").join("config"),
    )
}

fn default_openssh_config_path_serde() -> PathBuf {
    default_openssh_config_path()
}

/// Whether (and how) to incorporate the user's OpenSSH client config
/// (`~/.ssh/config`) into the terminal's SSH host list.
///
/// The default is [`off`](ImportOpenSshConfig::Off) so the feature is
/// strictly opt-in and existing configs are unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportOpenSshConfig {
    /// **Default.** The OpenSSH config file is not read.
    #[default]
    Off,
    /// A one-shot import: the "Import from SSH config" button (or command
    /// palette action) reads the file and appends any hosts that are not
    /// already in the saved list (deduplicated by name). The imported hosts
    /// are written to `config.toml` and remain there permanently.
    ImportOnce,
    /// At every startup and on every config reload the file is parsed and the
    /// resulting hosts are merged into the **in-memory** host list. They appear
    /// in the SSH picker but are **not** written to `config.toml`.
    Live,
}

impl ImportOpenSshConfig {
    /// All variants in display order — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::Off, Self::ImportOnce, Self::Live]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::ImportOnce => "Import once (one-shot)",
            Self::Live => "Live (merge on startup / reload)",
        }
    }
}

/// A single host stanza parsed from an OpenSSH client config file.
///
/// Glob-pattern hosts (those whose alias contains `*` or `?`) are **skipped**
/// because they don't represent concrete connection targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSshHost {
    /// The alias given on the `Host` line (e.g. `prod-db`). Used as the
    /// display name when mapping to an [`SshHost`].
    pub alias: String,
    /// The `HostName` value — the actual hostname/IP to connect to. Falls
    /// back to `alias` when `HostName` is absent.
    pub hostname: Option<String>,
    /// The `User` value.
    pub user: Option<String>,
    /// The `Port` value, if present and valid.
    pub port: Option<u16>,
    /// The first `IdentityFile` value (only the first is captured).
    pub identity_file: Option<PathBuf>,
    /// The `ProxyJump` value (informational; stored but not used by the SSH
    /// client yet — captured so it isn't silently discarded).
    pub proxy_jump: Option<String>,
    /// The `ForwardAgent` value, parsed as a boolean (yes/true/1 = true).
    pub forward_agent: bool,
}

impl ParsedSshHost {
    /// Map this parsed stanza to an [`SshHost`] ready to add to the config.
    ///
    /// - `address` = `hostname` if set, else `alias`.
    /// - `name` = `alias`.
    /// - `port` = parsed `port` or the SSH default (22).
    /// - `user` = parsed `user` or an empty string.
    /// - `auth` = `Key` when `identity_file` is set, else `Agent`.
    /// - `key_path` = `identity_file`.
    #[must_use]
    pub fn into_ssh_host(self) -> SshHost {
        let address = self.hostname.as_deref().unwrap_or(&self.alias).to_string();
        let auth = if self.identity_file.is_some() {
            SshAuthMethod::Key
        } else {
            SshAuthMethod::Agent
        };
        SshHost {
            id: SshHost::new_id(),
            name: self.alias,
            host: address,
            port: self.port.unwrap_or(default_ssh_port()),
            user: self.user.unwrap_or_default(),
            auth,
            key_path: self.identity_file,
        }
    }
}

/// Parse an OpenSSH client config file (`~/.ssh/config`) into a list of
/// concrete host stanzas.
///
/// The parser is lenient: unknown keywords are silently skipped, blank lines
/// and `#` comments are ignored, and `Include` directives are not followed
/// (only the top-level file is parsed). Glob-pattern host aliases (containing
/// `*` or `?`) are excluded from the result because they do not represent
/// specific connection targets.
///
/// # Example
///
/// ```rust
/// use terminale_config::parse_ssh_config;
///
/// let text = r#"
/// Host prod-db
///     HostName 10.0.0.5
///     User deploy
///     Port 2222
///     IdentityFile ~/.ssh/id_ed25519
///
/// Host jump
///     HostName jump.example.com
///     User admin
/// "#;
///
/// let hosts = parse_ssh_config(text);
/// assert_eq!(hosts.len(), 2);
/// assert_eq!(hosts[0].alias, "prod-db");
/// assert_eq!(hosts[0].port, Some(2222));
/// assert_eq!(hosts[1].alias, "jump");
/// assert_eq!(hosts[1].user.as_deref(), Some("admin"));
/// ```
#[must_use]
pub fn parse_ssh_config(text: &str) -> Vec<ParsedSshHost> {
    let mut results: Vec<ParsedSshHost> = Vec::new();
    // The stanza currently being built. `None` until the first `Host` line.
    let mut current: Option<ParsedSshHost> = None;

    for line in text.lines() {
        // Strip leading whitespace and comments.
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Split on the first whitespace or `=` (both forms are valid in
        // OpenSSH config: `Key Value` and `Key=Value`).
        let (keyword_raw, value_raw) = match split_kv(trimmed) {
            Some(kv) => kv,
            None => continue,
        };
        let keyword = keyword_raw.to_ascii_lowercase();
        let value = value_raw.trim().to_string();

        if keyword == "host" {
            // Commit the previous stanza (if it isn't a wildcard).
            if let Some(prev) = current.take() {
                if !is_glob_pattern(&prev.alias) {
                    results.push(prev);
                }
            }
            // Start a new stanza. Multiple aliases on one `Host` line are
            // split by whitespace; we create one entry per concrete alias.
            let aliases: Vec<&str> = value.split_whitespace().collect();
            // Begin with the first alias; we'll duplicate for the others below.
            if let Some(first) = aliases.first() {
                current = Some(ParsedSshHost {
                    alias: (*first).to_string(),
                    hostname: None,
                    user: None,
                    port: None,
                    identity_file: None,
                    proxy_jump: None,
                    forward_agent: false,
                });
            }
            // For every additional alias, commit the current one and create a
            // fresh sibling with the same alias (no values yet — they inherit
            // the lines that follow, which we haven't read yet; the simplest
            // correct approach is to duplicate the first alias as multiple
            // independents). We use the "flush + push sibling" pattern so
            // subsequent keyword lines fill the *last* started stanza.
            for extra_alias in aliases.iter().skip(1) {
                if let Some(prev) = current.take() {
                    if !is_glob_pattern(&prev.alias) {
                        results.push(prev);
                    }
                }
                current = Some(ParsedSshHost {
                    alias: (*extra_alias).to_string(),
                    hostname: None,
                    user: None,
                    port: None,
                    identity_file: None,
                    proxy_jump: None,
                    forward_agent: false,
                });
            }
            continue;
        }

        // All other keywords fill the current stanza; ignore if no stanza is
        // open yet (top-of-file defaults before any `Host` line).
        let Some(stanza) = current.as_mut() else {
            continue;
        };

        match keyword.as_str() {
            "hostname" => stanza.hostname = Some(value),
            "user" => stanza.user = Some(value),
            "port" => {
                if let Ok(p) = value.parse::<u16>() {
                    stanza.port = Some(p);
                }
            }
            "identityfile" => {
                if stanza.identity_file.is_none() {
                    // Expand a leading `~` to the home directory.
                    let expanded = expand_tilde(&value);
                    stanza.identity_file = Some(PathBuf::from(expanded));
                }
            }
            "proxyjump" => stanza.proxy_jump = Some(value),
            "forwardagent" => {
                stanza.forward_agent =
                    matches!(value.to_ascii_lowercase().as_str(), "yes" | "true" | "1");
            }
            _ => {
                // Unknown keywords are silently ignored (lenient parsing).
            }
        }
    }

    // Flush the last stanza.
    if let Some(last) = current.take() {
        if !is_glob_pattern(&last.alias) {
            results.push(last);
        }
    }

    results
}

/// Returns `true` when `alias` looks like a glob pattern (contains `*` or
/// `?`). Those stanzas are global defaults, not concrete hosts.
#[must_use]
fn is_glob_pattern(alias: &str) -> bool {
    alias.contains('*') || alias.contains('?')
}

/// Split `line` on the first whitespace run **or** the first `=`, returning
/// `(keyword, rest)`. Returns `None` for a line with no separable keyword.
#[must_use]
fn split_kv(line: &str) -> Option<(&str, &str)> {
    // Prefer `=` when present (handles `Key=Value` without spaces).
    if let Some(idx) = line.find('=') {
        let kw = line[..idx].trim();
        let val = line[idx + 1..].trim();
        if !kw.is_empty() {
            return Some((kw, val));
        }
    }
    // Fall back to first whitespace split (`Key Value`).
    let mut iter = line.splitn(2, |c: char| c.is_whitespace());
    let kw = iter.next()?.trim();
    let val = iter.next().unwrap_or("").trim();
    if kw.is_empty() {
        None
    } else {
        Some((kw, val))
    }
}

/// Expand a leading `~` in an SSH config path to the user's home directory.
/// Leaves the string unchanged when it doesn't start with `~`.
#[must_use]
fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
            return home.join(rest).display().to_string();
        }
    } else if path == "~" {
        if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
            return home.display().to_string();
        }
    }
    path.to_string()
}

/// Deduplicate `new_hosts` against `existing`: a host is a duplicate when an
/// existing entry already has the same `name` (alias) or the same
/// `(host, user, port)` triple. Returns only the entries from `new_hosts`
/// that are not already represented.
#[must_use]
pub fn dedupe_imported_hosts<'a>(
    new_hosts: &'a [SshHost],
    existing: &[SshHost],
) -> Vec<&'a SshHost> {
    new_hosts
        .iter()
        .filter(|n| {
            !existing.iter().any(|e| {
                // Duplicate by name.
                e.name == n.name
                    // … or by connection target (same host + user + port).
                    || (e.host == n.host
                        && e.user == n.user
                        && e.port == n.port)
            })
        })
        .collect()
}

/// Global SSH settings — known-hosts store location and host-key verification
/// policy. These settings apply to every SSH connection made by the terminal.
///
/// The defaults are deliberately safe: new hosts are trusted once and pinned
/// (TOFU), and a changed key always produces a visible error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SshConfig {
    /// Path to the SSH known-hosts file.
    /// Defaults to `~/.ssh/known_hosts` (the OS-standard location).
    #[serde(default = "default_known_hosts_path_serde")]
    pub known_hosts: PathBuf,
    /// Host-key verification policy applied to every SSH connection.
    /// See [`HostKeyPolicy`] for the options.
    #[serde(default)]
    pub host_key_policy: HostKeyPolicy,
    /// Whether (and how) to incorporate the user's OpenSSH client config into
    /// the terminal's SSH host list. Off by default.
    #[serde(default)]
    pub import_openssh_config: ImportOpenSshConfig,
    /// Path to the OpenSSH client config file to import.
    /// Defaults to `~/.ssh/config`.
    #[serde(default = "default_openssh_config_path_serde")]
    pub openssh_config_path: PathBuf,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            known_hosts: default_known_hosts_path(),
            host_key_policy: HostKeyPolicy::default(),
            import_openssh_config: ImportOpenSshConfig::default(),
            openssh_config_path: default_openssh_config_path(),
        }
    }
}

impl SshConfig {
    /// Validate this config block.
    ///
    /// Currently there are no hard constraints on the path (the file need not
    /// exist yet). The signature matches all other config `validate` methods
    /// so future constraints can be added without a breaking API change.
    #[allow(clippy::unnecessary_wraps)]
    pub fn validate(&self) -> Result<(), crate::ConfigError> {
        Ok(())
    }
}

// ── How to authenticate to an [`SshHost`] ────────────────────────────────────

/// How to authenticate to an [`SshHost`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SshAuthMethod {
    /// Use the running SSH agent (`ssh-agent` / Pageant / `gpg-agent`).
    /// Preferred — no key material touches the config file.
    #[default]
    Agent,
    /// Use the private key at `key_path` (OpenSSH or PEM). Prefer ed25519
    /// keys: RSA keys pull in the `rsa` crate, which is subject to
    /// RUSTSEC-2023-0071 (a Marvin-attack timing side-channel).
    Key,
    /// Interactive password — prompted on connect (never stored in config).
    Password,
}

impl SshAuthMethod {
    /// All variants in display order — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::Agent, Self::Key, Self::Password]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Agent => "SSH agent",
            Self::Key => "Private key",
            Self::Password => "Password",
        }
    }
}

/// Default TCP port for SSH (used when an entry omits `port`).
#[must_use]
pub const fn default_ssh_port() -> u16 {
    22
}

/// One configured SSH host. Listed in the command palette as `SSH: <host name>`
/// and in the "New SSH tab" picker.
///
/// This struct is **metadata only** — no password or key passphrase is ever
/// stored here (and therefore never written to `config.toml`). The actual
/// secret lives in the OS keychain (see [`crate::secrets`]), keyed by this
/// host's stable [`id`](SshHost::id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SshHost {
    /// Stable identifier used to key this host's secret in the OS keychain.
    /// Generated once when the host is created and never changes afterwards,
    /// so renaming / re-pointing the host keeps its stored credential. Older
    /// config files predating this field deserialize with an empty id;
    /// [`SshHost::secret_id`] falls back to a name-derived key in that case,
    /// and the UI backfills a fresh id on next save.
    #[serde(default)]
    pub id: String,
    /// Display name shown in pickers / palette / tab titles (e.g. "prod-db").
    pub name: String,
    /// Hostname or IP address to connect to.
    pub host: String,
    /// TCP port. Defaults to 22.
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// Remote username to log in as.
    pub user: String,
    /// Which credential to present. Defaults to the SSH agent.
    #[serde(default)]
    pub auth: SshAuthMethod,
    /// Path to the private key file. Required only when `auth = "key"`;
    /// ignored otherwise. Prefer an ed25519 key.
    #[serde(default)]
    pub key_path: Option<PathBuf>,
}

impl SshHost {
    /// `user@host:port` rendering for tab titles / logs.
    #[must_use]
    pub fn endpoint(&self) -> String {
        if self.port == default_ssh_port() {
            format!("{}@{}", self.user, self.host)
        } else {
            format!("{}@{}:{}", self.user, self.host, self.port)
        }
    }

    /// Stable keychain key for this host's secret. Uses the explicit
    /// [`id`](SshHost::id) when set; otherwise falls back to a deterministic
    /// `legacy:<name>` key so credentials referenced by pre-`id` configs still
    /// resolve. The keychain "username" component is namespaced (`ssh:<id>`)
    /// so SSH secrets never collide with any other terminale keychain use.
    #[must_use]
    pub fn secret_id(&self) -> String {
        if self.id.is_empty() {
            format!("ssh:legacy:{}", self.name)
        } else {
            format!("ssh:{}", self.id)
        }
    }

    /// Generate a fresh, process-unique stable id. Combines the wall-clock
    /// time (nanoseconds since the Unix epoch) with a monotonic per-process
    /// counter so two hosts created in the same nanosecond still differ — no
    /// external `uuid` dependency required.
    #[must_use]
    pub fn new_id() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{nanos:x}-{seq:x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_defaults_to_agent() {
        assert_eq!(SshAuthMethod::default(), SshAuthMethod::Agent);
    }

    #[test]
    fn host_parses_with_port_default() {
        let toml_src = r#"
            name = "prod"
            host = "10.0.0.5"
            user = "deploy"
        "#;
        let h: SshHost = toml::from_str(toml_src).unwrap();
        assert_eq!(h.port, 22);
        assert_eq!(h.auth, SshAuthMethod::Agent);
        assert_eq!(h.endpoint(), "deploy@10.0.0.5");
    }

    #[test]
    fn host_parses_key_auth() {
        let toml_src = r#"
            name = "build"
            host = "ci.example.com"
            port = 2222
            user = "runner"
            auth = "key"
            key_path = "/home/me/.ssh/id_ed25519"
        "#;
        let h: SshHost = toml::from_str(toml_src).unwrap();
        assert_eq!(h.port, 2222);
        assert_eq!(h.auth, SshAuthMethod::Key);
        assert!(h.key_path.is_some());
        assert_eq!(h.endpoint(), "runner@ci.example.com:2222");
    }

    #[test]
    fn secret_id_uses_explicit_id_when_set() {
        let h = SshHost {
            id: "abc123".into(),
            name: "prod".into(),
            host: "h".into(),
            port: 22,
            user: "u".into(),
            auth: SshAuthMethod::Password,
            key_path: None,
        };
        assert_eq!(h.secret_id(), "ssh:abc123");
    }

    #[test]
    fn secret_id_falls_back_to_name_for_legacy_configs() {
        // A pre-`id` config deserializes with an empty id; the keychain key
        // then falls back to a deterministic name-derived form so an existing
        // stored credential still resolves.
        let h: SshHost = toml::from_str(
            r#"
            name = "old-host"
            host = "h"
            user = "u"
        "#,
        )
        .unwrap();
        assert!(h.id.is_empty());
        assert_eq!(h.secret_id(), "ssh:legacy:old-host");
    }

    #[test]
    fn new_id_is_unique_per_call() {
        let a = SshHost::new_id();
        let b = SshHost::new_id();
        assert_ne!(a, b, "generated ids must be unique");
        assert!(!a.is_empty());
    }

    #[test]
    fn host_never_serializes_a_secret_field() {
        // Metadata-only invariant: serializing a host must not emit any key
        // that holds a credential. Guards against a future field accidentally
        // re-introducing on-disk secrets. We check for assignment *keys*
        // (`password =`, …) — the auth *value* `"password"` is fine, it's just
        // a method selector, not the secret itself.
        let h = SshHost {
            id: "x".into(),
            name: "n".into(),
            host: "h".into(),
            port: 22,
            user: "u".into(),
            auth: SshAuthMethod::Password,
            key_path: None,
        };
        let s = toml::to_string(&h).unwrap();
        for forbidden_key in ["password =", "passphrase =", "secret ="] {
            assert!(
                !s.contains(forbidden_key),
                "serialized host must not contain a `{forbidden_key}` key: {s}"
            );
        }
        // The only auth-related key on disk is the method selector.
        assert!(s.contains("auth = \"password\""));
    }

    // ── HostKeyPolicy ─────────────────────────────────────────────────────────

    #[test]
    fn host_key_policy_default_is_accept_new() {
        assert_eq!(HostKeyPolicy::default(), HostKeyPolicy::AcceptNew);
    }

    #[test]
    fn host_key_policy_all_returns_three_distinct_variants() {
        let all = HostKeyPolicy::all();
        assert_eq!(all.len(), 3);
        let mut seen = std::collections::HashSet::new();
        for v in all {
            let label = v.label();
            assert!(seen.insert(label), "duplicate label: {label}");
        }
    }

    #[test]
    fn host_key_policy_parses_all_variants() {
        for (raw, want) in [
            ("accept_new", HostKeyPolicy::AcceptNew),
            ("strict", HostKeyPolicy::Strict),
            ("off", HostKeyPolicy::Off),
        ] {
            let toml_src = format!("host_key_policy = \"{raw}\"");
            #[derive(serde::Deserialize)]
            struct W {
                host_key_policy: HostKeyPolicy,
            }
            let w: W = toml::from_str(&toml_src)
                .unwrap_or_else(|e| panic!("policy `{raw}` must parse: {e}"));
            assert_eq!(w.host_key_policy, want, "policy `{raw}`");
        }
    }

    #[test]
    fn host_key_policy_roundtrips() {
        for policy in HostKeyPolicy::all() {
            #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
            struct W {
                host_key_policy: HostKeyPolicy,
            }
            let w = W {
                host_key_policy: policy,
            };
            let s = toml::to_string(&w).unwrap();
            let back: W = toml::from_str(&s).unwrap();
            assert_eq!(back.host_key_policy, policy, "{policy:?} must roundtrip");
        }
    }

    // ── SshConfig ─────────────────────────────────────────────────────────────

    #[test]
    fn ssh_config_default_policy_is_accept_new() {
        assert_eq!(
            SshConfig::default().host_key_policy,
            HostKeyPolicy::AcceptNew
        );
    }

    #[test]
    fn ssh_config_default_validates() {
        SshConfig::default()
            .validate()
            .expect("default SshConfig must validate");
    }

    #[test]
    fn ssh_config_parses_and_roundtrips() {
        let toml_src = r#"
known_hosts = "/home/me/.ssh/known_hosts"
host_key_policy = "strict"
"#;
        let cfg: SshConfig = toml::from_str(toml_src).expect("SshConfig must parse");
        cfg.validate().expect("must validate");
        assert_eq!(cfg.host_key_policy, HostKeyPolicy::Strict);
        assert_eq!(cfg.known_hosts, PathBuf::from("/home/me/.ssh/known_hosts"));
        let s = toml::to_string(&cfg).unwrap();
        let back: SshConfig = toml::from_str(&s).unwrap();
        assert_eq!(back.host_key_policy, HostKeyPolicy::Strict);
        assert_eq!(back.known_hosts, PathBuf::from("/home/me/.ssh/known_hosts"));
    }

    #[test]
    fn ssh_config_absent_fields_fall_back_to_defaults() {
        // An empty table must give us the defaults (forward-compatibility for
        // users who never set [ssh]).
        let cfg: SshConfig = toml::from_str("").expect("empty SshConfig must parse");
        assert_eq!(cfg.host_key_policy, HostKeyPolicy::default());
        // known_hosts path is non-empty (depends on home dir, but must be set)
        assert!(!cfg.known_hosts.as_os_str().is_empty());
    }

    // ── ImportOpenSshConfig ───────────────────────────────────────────────────

    #[test]
    fn import_mode_default_is_off() {
        assert_eq!(ImportOpenSshConfig::default(), ImportOpenSshConfig::Off);
    }

    #[test]
    fn import_mode_roundtrips() {
        for mode in ImportOpenSshConfig::all() {
            #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
            struct W {
                import_openssh_config: ImportOpenSshConfig,
            }
            let w = W {
                import_openssh_config: mode,
            };
            let s = toml::to_string(&w).unwrap();
            let back: W = toml::from_str(&s).unwrap();
            assert_eq!(back.import_openssh_config, mode, "{mode:?} must roundtrip");
        }
    }

    #[test]
    fn ssh_config_new_fields_have_defaults() {
        let cfg: SshConfig = toml::from_str("").expect("empty SshConfig must parse");
        assert_eq!(cfg.import_openssh_config, ImportOpenSshConfig::Off);
        assert!(!cfg.openssh_config_path.as_os_str().is_empty());
    }

    // ── parse_ssh_config ──────────────────────────────────────────────────────

    const SAMPLE_CONFIG: &str = r#"
# Main bastion
Host bastion
    HostName 10.0.1.1
    User ubuntu
    Port 2222
    IdentityFile ~/.ssh/id_ed25519
    ForwardAgent yes

Host prod-db
    HostName db.internal
    User postgres
    ProxyJump bastion

Host dev
    HostName 192.168.56.10
    User vagrant
    # No IdentityFile — defaults to agent

# Wildcard — should be excluded
Host *
    ServerAliveInterval 60

Host extra-server
    HostName extra.example.com
    User admin
"#;

    #[test]
    fn parse_representative_config() {
        let hosts = parse_ssh_config(SAMPLE_CONFIG);
        // Wildcard host excluded; 4 concrete hosts.
        assert_eq!(hosts.len(), 4, "expected 4 concrete hosts, got {hosts:?}");

        let bastion = &hosts[0];
        assert_eq!(bastion.alias, "bastion");
        assert_eq!(bastion.hostname.as_deref(), Some("10.0.1.1"));
        assert_eq!(bastion.user.as_deref(), Some("ubuntu"));
        assert_eq!(bastion.port, Some(2222));
        assert!(
            bastion.identity_file.is_some(),
            "bastion must have an identity_file"
        );
        assert!(bastion.forward_agent, "bastion.ForwardAgent = yes");

        let prod_db = &hosts[1];
        assert_eq!(prod_db.alias, "prod-db");
        assert_eq!(prod_db.hostname.as_deref(), Some("db.internal"));
        assert_eq!(prod_db.user.as_deref(), Some("postgres"));
        assert_eq!(prod_db.port, None);
        assert_eq!(prod_db.proxy_jump.as_deref(), Some("bastion"));
        assert!(!prod_db.forward_agent);

        let dev = &hosts[2];
        assert_eq!(dev.alias, "dev");
        assert!(dev.identity_file.is_none(), "dev has no IdentityFile");

        let extra = &hosts[3];
        assert_eq!(extra.alias, "extra-server");
        assert_eq!(extra.hostname.as_deref(), Some("extra.example.com"));
    }

    #[test]
    fn parse_empty_config() {
        let hosts = parse_ssh_config("");
        assert!(hosts.is_empty());
    }

    #[test]
    fn parse_comments_only() {
        let hosts = parse_ssh_config("# just a comment\n# another line\n");
        assert!(hosts.is_empty());
    }

    #[test]
    fn parse_unknown_keywords_are_ignored() {
        let cfg = "Host test\n    HostName test.example.com\n    SomeUnknownOption value\n    User alice\n";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].user.as_deref(), Some("alice"));
    }

    #[test]
    fn parse_wildcard_only_returns_empty() {
        let cfg = "Host *\n    ServerAliveInterval 60\n";
        let hosts = parse_ssh_config(cfg);
        assert!(
            hosts.is_empty(),
            "wildcard-only config must produce no hosts"
        );
    }

    #[test]
    fn parse_host_without_hostname_uses_alias_as_address() {
        let cfg = "Host myserver\n    User bob\n";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        assert!(hosts[0].hostname.is_none());
        let ssh_host = hosts[0].clone().into_ssh_host();
        // Without HostName the address falls back to the alias.
        assert_eq!(ssh_host.host, "myserver");
        assert_eq!(ssh_host.name, "myserver");
    }

    #[test]
    fn into_ssh_host_with_identity_file_uses_key_auth() {
        let cfg = "Host ci\n    HostName ci.example.com\n    User runner\n    IdentityFile ~/.ssh/id_ed25519\n";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        let h = hosts[0].clone().into_ssh_host();
        assert_eq!(h.auth, SshAuthMethod::Key);
        assert!(h.key_path.is_some());
        assert_eq!(h.host, "ci.example.com");
        assert_eq!(h.user, "runner");
    }

    #[test]
    fn into_ssh_host_without_identity_file_uses_agent_auth() {
        let cfg = "Host ci\n    HostName ci.example.com\n    User runner\n";
        let hosts = parse_ssh_config(cfg);
        let h = hosts[0].clone().into_ssh_host();
        assert_eq!(h.auth, SshAuthMethod::Agent);
        assert!(h.key_path.is_none());
    }

    #[test]
    fn into_ssh_host_port_default() {
        let cfg = "Host ci\n    HostName ci.example.com\n";
        let hosts = parse_ssh_config(cfg);
        let h = hosts[0].clone().into_ssh_host();
        assert_eq!(h.port, default_ssh_port());
    }

    // ── dedupe_imported_hosts ─────────────────────────────────────────────────

    fn make_host(name: &str, host: &str, user: &str, port: u16) -> SshHost {
        SshHost {
            id: SshHost::new_id(),
            name: name.to_string(),
            host: host.to_string(),
            port,
            user: user.to_string(),
            auth: SshAuthMethod::Agent,
            key_path: None,
        }
    }

    #[test]
    fn dedupe_no_existing_keeps_all() {
        let new = vec![make_host("a", "a.example.com", "u", 22)];
        let existing: Vec<SshHost> = vec![];
        let result = dedupe_imported_hosts(&new, &existing);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn dedupe_duplicate_name_excluded() {
        let new = vec![make_host("prod", "prod.example.com", "u", 22)];
        let existing = vec![make_host("prod", "other.example.com", "u", 22)];
        let result = dedupe_imported_hosts(&new, &existing);
        assert!(result.is_empty(), "same name must be deduped");
    }

    #[test]
    fn dedupe_duplicate_connection_target_excluded() {
        let new = vec![make_host("alias2", "same.example.com", "user", 22)];
        let existing = vec![make_host("alias1", "same.example.com", "user", 22)];
        let result = dedupe_imported_hosts(&new, &existing);
        assert!(result.is_empty(), "same host+user+port must be deduped");
    }

    #[test]
    fn dedupe_different_port_not_deduped() {
        let new = vec![make_host("prod-2222", "h.example.com", "u", 2222)];
        let existing = vec![make_host("prod", "h.example.com", "u", 22)];
        let result = dedupe_imported_hosts(&new, &existing);
        assert_eq!(result.len(), 1, "different port must not be deduped");
    }

    #[test]
    fn dedupe_multiple_new_some_kept() {
        let new = vec![
            make_host("new1", "n1.example.com", "u", 22),
            make_host("existing", "e.example.com", "u", 22),
        ];
        let existing = vec![make_host("existing", "e.example.com", "u", 22)];
        let result = dedupe_imported_hosts(&new, &existing);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "new1");
    }

    // ── SshConfig roundtrip with new fields ───────────────────────────────────

    #[test]
    fn ssh_config_roundtrips_with_import_fields() {
        let toml_src = r#"
known_hosts = "/home/me/.ssh/known_hosts"
host_key_policy = "strict"
import_openssh_config = "live"
openssh_config_path = "/home/me/.ssh/config"
"#;
        let cfg: SshConfig = toml::from_str(toml_src).expect("SshConfig must parse");
        cfg.validate().expect("must validate");
        assert_eq!(cfg.import_openssh_config, ImportOpenSshConfig::Live);
        assert_eq!(
            cfg.openssh_config_path,
            PathBuf::from("/home/me/.ssh/config")
        );
        let s = toml::to_string(&cfg).unwrap();
        let back: SshConfig = toml::from_str(&s).unwrap();
        assert_eq!(back.import_openssh_config, ImportOpenSshConfig::Live);
    }
}
