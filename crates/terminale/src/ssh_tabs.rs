//! SSH tab lifecycle: connecting, finishing, secret detection, host helpers.
//! Also: SSH save-prompt handling, parsed-ssh endpoint, track_input_line.

use crate::{ParsedSsh, RunningState, SshConnectOutcome, UserEvent};
use std::sync::Arc;
use terminale_core::Session;
use terminale_term::Emulator;

// ── ssh_secret_needed ─────────────────────────────────────────────────────────

/// Whether opening `host` needs a secret that isn't available yet, so the
/// caller should pop the in-window credential prompt before connecting.
///
/// Returns:
/// - `Ok(None)` — ready to connect without a prompt (agent auth, key auth with
///   an unencrypted key or a passphrase already in the keychain, or a password
///   already in the keychain).
/// - `Ok(Some(is_passphrase))` — a secret is required but absent; the bool
///   says whether we're asking for a key passphrase (`true`) vs a login
///   password (`false`).
/// - `Err(msg)` — misconfiguration (e.g. key auth with no `key_path`).
///
/// Key auth is treated as "ready" when no passphrase is stored: an unencrypted
/// key needs none, and an encrypted one surfaces a clear error at connect time
/// rather than us guessing. (We only prompt for a key passphrase when the user
/// previously stored one, to refresh it — kept simple by always treating a
/// missing key passphrase as "try without".)
pub(crate) fn ssh_secret_needed(host: &terminale_config::SshHost) -> Result<Option<bool>, String> {
    use terminale_config::SshAuthMethod;
    match host.auth {
        SshAuthMethod::Agent => Ok(None),
        SshAuthMethod::Key => {
            if host.key_path.is_none() {
                return Err(format!(
                    "host '{}' uses key auth but no `key_path` is set",
                    host.name
                ));
            }
            // Key auth proceeds with whatever passphrase (if any) is in the
            // keychain; we don't pre-prompt for it.
            Ok(None)
        }
        SshAuthMethod::Password => {
            let stored = terminale_config::get_secret(&host.secret_id())
                .map_err(|e| format!("keychain unavailable: {e}"))?;
            // Prompt only when there's nothing stored yet.
            Ok(if stored.is_some() { None } else { Some(false) })
        }
    }
}

// ── ssh_connect_options ───────────────────────────────────────────────────────

/// Map a configured [`terminale_config::SshHost`] onto the SSH client's
/// [`terminale_ssh::ConnectOptions`].
///
/// Secrets are **never** read from `config.toml`. Password auth and encrypted
/// key passphrases come from the OS keychain (keyed by the host's `secret_id`),
/// unless `secret_override` is supplied — that's the freshly-prompted secret on
/// a first connect, used before (optionally) persisting it to the keychain.
/// When a password host has neither an override nor a stored secret, the
/// connect attempt fails with a clear message.
///
/// The `ssh_cfg` parameter supplies the host-key verification policy and the
/// path to the known-hosts file from the global `[ssh]` config section.
pub(crate) fn ssh_connect_options(
    host: &terminale_config::SshHost,
    secret_override: Option<&str>,
    ssh_cfg: &terminale_config::SshConfig,
) -> Result<terminale_ssh::ConnectOptions, String> {
    use terminale_config::SshAuthMethod;
    let auth = match host.auth {
        SshAuthMethod::Agent => terminale_ssh::AuthMethod::Agent,
        SshAuthMethod::Key => {
            let path = host.key_path.clone().ok_or_else(|| {
                format!(
                    "host '{}' uses key auth but no `key_path` is set",
                    host.name
                )
            })?;
            // Passphrase (for an encrypted key) comes from the override first,
            // else the keychain. An unencrypted key resolves to `None`.
            let passphrase = match secret_override {
                Some(s) => Some(s.to_string()),
                None => terminale_config::get_secret(&host.secret_id())
                    .map_err(|e| format!("keychain unavailable: {e}"))?,
            };
            terminale_ssh::AuthMethod::Key { path, passphrase }
        }
        SshAuthMethod::Password => {
            let pw = match secret_override {
                Some(s) => Some(s.to_string()),
                None => terminale_config::get_secret(&host.secret_id())
                    .map_err(|e| format!("keychain unavailable: {e}"))?,
            };
            let pw = pw.filter(|s| !s.is_empty()).ok_or_else(|| {
                format!(
                    "host '{}' uses password auth but no password is stored — \
                     open it to be prompted, or use agent/ed25519-key auth",
                    host.name
                )
            })?;
            terminale_ssh::AuthMethod::Password(pw)
        }
    };
    Ok(terminale_ssh::ConnectOptions {
        host: host.host.clone(),
        port: host.port,
        user: host.user.clone(),
        auth,
        host_key_policy: ssh_cfg.host_key_policy,
        known_hosts: ssh_cfg.known_hosts.clone(),
    })
}

// ── open_ssh_tab / finish_ssh_tab ─────────────────────────────────────────────

/// Kick off an asynchronous SSH connection for `host` and return immediately.
///
/// The TCP + auth + PTY handshake runs on the shared Tokio runtime so the
/// winit event-loop thread is never blocked. When the attempt completes (or
/// times out) a [`UserEvent::SshConnected`] is sent back; the event handler
/// calls [`finish_ssh_tab`] to build and push the new tab on the UI thread.
pub(crate) fn open_ssh_tab(
    state: &mut RunningState,
    host: &terminale_config::SshHost,
    secret_override: Option<&str>,
    ssh_cfg: &terminale_config::SshConfig,
    runtime: &tokio::runtime::Handle,
    window_idx: usize,
) {
    let size = state.window.inner_size();
    let (cols, rows) = state.renderer.pixels_to_cells(size.width, size.height);

    let connect_result = ssh_connect_options(host, secret_override, ssh_cfg);
    let host_name = host.name.clone();
    let host_endpoint = host.endpoint();
    let proxy = state.proxy.clone();

    let notifier: terminale_ssh::DataNotifier = {
        let proxy = proxy.clone();
        Arc::new(move || {
            let _ = proxy.send_event(UserEvent::PtyDataReady);
        })
    };

    match connect_result {
        Err(msg) => {
            // Misconfiguration: no network call needed, surface the error
            // immediately as a (crashed) tab without spawning a task.
            tracing::warn!(host = %host_name, %msg, "ssh host misconfigured");
            let _ = proxy.send_event(UserEvent::SshConnected(Box::new(SshConnectOutcome {
                window_idx,
                host_name,
                host_endpoint,
                cols,
                rows,
                result: Err(msg),
            })));
        }
        Ok(opts) => {
            runtime.spawn(async move {
                let result = tokio::time::timeout(
                    crate::SSH_CONNECT_TIMEOUT,
                    terminale_ssh::SshSession::connect(opts, cols, rows, Some(notifier)),
                )
                .await
                .map_err(|_| format!("timed out after {}s", crate::SSH_CONNECT_TIMEOUT.as_secs()))
                .and_then(|r| r.map_err(|e| e.to_string()));

                let _ = proxy.send_event(UserEvent::SshConnected(Box::new(SshConnectOutcome {
                    window_idx,
                    host_name,
                    host_endpoint,
                    cols,
                    rows,
                    result,
                })));
            });
        }
    }
}

/// Complete an SSH connection attempt on the UI thread.
///
/// Called from the [`UserEvent::SshConnected`] handler after the Tokio task
/// that performed the TCP + auth handshake delivers its result. Builds and
/// pushes the new tab (success → live session, failure → crashed tab showing
/// the error), then switches to it and requests a redraw. The emulator is
/// created here, on the UI thread, so no `Send` bound is needed on the
/// terminal grid or renderer types.
pub(crate) fn finish_ssh_tab(state: &mut RunningState, outcome: SshConnectOutcome) {
    let SshConnectOutcome {
        host_name,
        host_endpoint,
        cols,
        rows,
        result,
        ..
    } = outcome;

    use parking_lot::Mutex;

    let mut emu = Emulator::new(cols, rows);
    emu.set_scrollback(state.scrollback_lines);
    emu.set_command_blocks(state.command_blocks_enabled, state.max_command_blocks);

    let (mut session, crashed) = match result {
        Ok(mut ssh) => {
            let output_rx = ssh
                .take_output()
                .expect("fresh SshSession must have an output channel");
            // Share the SshSession across the write/resize closures. Both
            // operations just push onto the SSH wrapper's command channel,
            // so the lock is held only momentarily.
            let ssh = Arc::new(Mutex::new(ssh));
            let ssh_w = Arc::clone(&ssh);
            let write: terminale_core::RemoteWriter = Arc::new(move |data: &[u8]| {
                ssh_w
                    .lock()
                    .write_input(data)
                    .map_err(|e| terminale_core::CoreError::Remote(e.to_string()))
            });
            let ssh_r = Arc::clone(&ssh);
            let resize: terminale_core::RemoteResizer = Arc::new(move |c: u16, r: u16| {
                ssh_r
                    .lock()
                    .resize(c, r)
                    .map_err(|e| terminale_core::CoreError::Remote(e.to_string()))
            });
            let session = Session::from_remote(cols, rows, output_rx, write, resize);
            (session, false)
        }
        Err(msg) => {
            tracing::warn!(host = %host_name, %msg, "ssh connect failed");
            emu.advance(format!("\r\n\x1b[31mSSH connection failed:\x1b[0m {msg}\r\n").as_bytes());
            // A failed connection still produces a (crashed) tab so the user
            // can read the error. Back it with a never-fed receiver so
            // write_input / resize are harmless no-ops.
            let (_dead_tx, dead_rx) = tokio::sync::mpsc::unbounded_channel();
            let noop_w: terminale_core::RemoteWriter = Arc::new(|_: &[u8]| Ok(()));
            let noop_r: terminale_core::RemoteResizer = Arc::new(|_: u16, _: u16| Ok(()));
            (
                Session::from_remote(cols, rows, dead_rx, noop_w, noop_r),
                true,
            )
        }
    };

    let output_rx = session
        .take_output()
        .expect("remote session must expose an output channel");

    let mut tab = crate::TabState::new_single(crate::Pane {
        profile_name: format!("SSH: {host_name}"),
        icon: Some("\u{1F310}".into()), // 🌐 GLOBE
        custom_title: Some(host_endpoint),
        user_title: None,
        session,
        output_rx,
        emulator: Arc::new(Mutex::new(emu)),
        cols,
        rows,
        scroll_lines: 0,
        crashed,
        autodetect_links: Vec::new(),
        last_output_at: None,
        last_input_at: None,
    });
    // Record the SSH host name so context-rule matching can use it immediately.
    tab.ssh_host_name = host_name;
    state.tabs.push(tab);
    state.active_tab = state.tabs.len() - 1;
    if let Some(t) = state.tabs.last() {
        t.emulator.lock().set_palette(state.palette);
    }
    // Evaluate context rules for the new tab immediately so the tint/badge
    // appears on the first frame rather than waiting for the next PTY drain.
    if crate::osc_handlers::refresh_context_rules(state) {
        crate::tabs::refresh_tab_bar(state);
    }
    state.renderer.set_selection(None);
    state.window.request_redraw();
}

// ── ssh host helpers ──────────────────────────────────────────────────────────

/// Construct a fresh [`terminale_config::SshHost`] from a parsed `ssh`
/// command. Metadata only — the auth method defaults to the SSH agent and no
/// secret is stored (the keychain prompt handles that on first connect). The
/// display name is the endpoint so the new host is recognisable in pickers.
pub(crate) fn ssh_host_from_parsed(parsed: &ParsedSsh) -> terminale_config::SshHost {
    let user = parsed.user.clone().unwrap_or_default();
    terminale_config::SshHost {
        id: terminale_config::SshHost::new_id(),
        name: parsed_ssh_endpoint(parsed),
        host: parsed.host.clone(),
        port: parsed.port,
        user,
        auth: terminale_config::SshAuthMethod::default(),
        key_path: None,
    }
}

/// Build the `(host, user, port)` dedupe targets from the configured SSH
/// hosts. An empty `user` maps to `None` so it compares equal to a typed
/// `ssh host` (no explicit user), matching OpenSSH's default-user behaviour.
pub(crate) fn ssh_host_targets_from(
    cfg: &terminale_config::Config,
) -> Vec<(String, Option<String>, u16)> {
    cfg.ssh_hosts
        .iter()
        .map(|h| {
            let user = if h.user.is_empty() {
                None
            } else {
                Some(h.user.clone())
            };
            (h.host.clone(), user, h.port)
        })
        .collect()
}

/// Build the cached snippet display list from the config. Each element is
/// `(name, description)` — the description is an empty string when absent.
pub(crate) fn snippet_names_from(cfg: &terminale_config::Config) -> Vec<(String, String)> {
    cfg.snippets
        .iter()
        .map(|s| {
            let desc = s.description.clone().unwrap_or_default();
            (s.name.clone(), desc)
        })
        .collect()
}

/// Open the command palette scoped to the SSH hosts. No-op when no hosts
/// are configured — guarding here keeps the path safe if a future caller
/// reaches it with an empty host list.
pub(crate) fn open_ssh_quick_connect(state: &mut RunningState) {
    if state.ssh_host_names.is_empty() {
        return;
    }
    let mut pal = crate::CommandPaletteState::new();
    pal.mode = crate::PaletteMode::SshQuickConnect;
    state.command_palette = Some(pal);
    crate::refresh_palette(state);
    state.window.request_redraw();
}

/// Open the command palette scoped to the user's snippet library. When
/// no snippets are configured the picker still opens (it shows an empty
/// list rather than silently doing nothing, so the user knows the feature
/// exists and can configure it).
pub(crate) fn open_snippet_picker(state: &mut RunningState) {
    let mut pal = crate::CommandPaletteState::new();
    pal.mode = crate::PaletteMode::Snippets;
    state.command_palette = Some(pal);
    crate::refresh_palette(state);
    state.window.request_redraw();
}

// ── ParsedSsh / parse_ssh_command ─────────────────────────────────────────────

/// Best-effort parse of a typed `ssh` command line into a [`ParsedSsh`].
///
/// Handles the common shapes — `ssh [user@]host`, `ssh host -p PORT`,
/// `ssh -p PORT user@host`, and `ssh -l user host` — and ignores other flags
/// gracefully (e.g. `-A`, `-X`, `-i keyfile`). Returns `None` for anything
/// that isn't an `ssh` command, has no destination, or carries an
/// unparsable port. Flag values that take an argument (`-p`, `-l`, `-i`, …)
/// consume the following token so it isn't mistaken for the host.
pub(crate) fn parse_ssh_command(line: &str) -> Option<ParsedSsh> {
    let mut tokens = line.split_whitespace();
    // First token must be the ssh binary (allow a path like /usr/bin/ssh).
    let prog = tokens.next()?;
    let prog_name = prog.rsplit(['/', '\\']).next().unwrap_or(prog);
    if prog_name != "ssh" {
        return None;
    }

    // Options that take a value in the *next* token; we only care about
    // `-p` (port) and `-l` (login user), but every value-taking flag must
    // be skipped so its argument isn't mistaken for the destination.
    const VALUE_FLAGS: &[char] = &[
        'p', 'l', 'i', 'F', 'o', 'b', 'c', 'D', 'L', 'R', 'W', 'J', 'e', 'm', 'O', 'Q', 'S', 'w',
    ];

    let mut user: Option<String> = None;
    let mut host: Option<String> = None;
    let mut port: u16 = terminale_config::default_ssh_port();

    let mut tokens = tokens.peekable();
    while let Some(tok) = tokens.next() {
        if let Some(rest) = tok.strip_prefix('-') {
            // A bare "-" is not a valid flag; ignore it.
            let Some(flag) = rest.chars().next() else {
                continue;
            };
            if VALUE_FLAGS.contains(&flag) {
                // Value may be glued (`-p2222`) or in the next token.
                let value = if rest.len() > 1 {
                    Some(rest[1..].to_string())
                } else {
                    tokens.next().map(str::to_string)
                };
                if let Some(v) = value {
                    match flag {
                        'p' => port = v.parse().ok()?,
                        'l' => user = Some(v),
                        _ => {}
                    }
                }
            }
            // Boolean flags (and any combined cluster like `-AX`) carry no
            // value — nothing to consume.
            continue;
        }
        // First non-flag token is the destination. A later one would be the
        // remote command to run; we ignore it.
        if host.is_none() {
            let (u, h) = match tok.rsplit_once('@') {
                Some((u, h)) => (Some(u.to_string()), h.to_string()),
                None => (None, tok.to_string()),
            };
            // `user@host` wins over a `-l user` for the login name (matches
            // OpenSSH, which lets the destination override `-l`).
            if u.is_some() {
                user = u;
            }
            if h.is_empty() {
                return None;
            }
            host = Some(h);
        }
    }

    Some(ParsedSsh {
        user,
        host: host?,
        port,
    })
}

// ── track_input_line / maybe_offer_save_ssh_host ─────────────────────────────

/// Maintain [`RunningState::input_line`] from a keystroke that's about to be
/// sent to the PTY, and — on Enter — try to offer a save prompt for a typed
/// `ssh …` command. Deliberately a best-effort reconstruction (it doesn't
/// model cursor movement or history recall); it only needs to be right for
/// the common "type `ssh user@host`, press Enter" case.
pub(crate) fn track_input_line(
    state: &mut RunningState,
    logical_key: &winit::keyboard::Key,
    text: Option<winit::keyboard::SmolStr>,
) {
    use winit::keyboard::Key;
    // Line-clearing control combos: Ctrl+C (SIGINT), Ctrl+U (kill line),
    // Ctrl+L (clear). Reset the tracked line so a stale prefix can't leak.
    if state.modifiers.control_key() {
        if let Key::Character(s) = logical_key {
            if matches!(s.as_str(), "c" | "C" | "u" | "U" | "l" | "L") {
                state.input_line.clear();
                return;
            }
        }
    }
    match logical_key {
        Key::Named(winit::keyboard::NamedKey::Enter) => {
            let line = std::mem::take(&mut state.input_line);
            maybe_offer_save_ssh_host(state, &line);
        }
        Key::Named(winit::keyboard::NamedKey::Backspace) => {
            state.input_line.pop();
        }
        Key::Named(winit::keyboard::NamedKey::Space) => state.input_line.push(' '),
        _ => {
            if let Some(t) = text {
                // Append only genuine printable text (skip control chars).
                if !t.is_empty() && t.chars().all(|c| !c.is_control()) {
                    state.input_line.push_str(&t);
                }
            }
        }
    }
}

/// If `line` parses as an `ssh` command for a host that isn't already saved
/// — and the feature is enabled and no prompt is already showing — pop the
/// "Save this SSH host?" toast. No-op otherwise.
pub(crate) fn maybe_offer_save_ssh_host(state: &mut RunningState, line: &str) {
    if !state.offer_save_ssh_hosts || state.save_host_prompt.is_some() {
        return;
    }
    let Some(parsed) = parse_ssh_command(line) else {
        return;
    };
    // Already saved? Match host + (effective) user + port against the cached
    // targets so an exact duplicate doesn't nag, but a different user/port on
    // the same host still re-offers. The App keeps `ssh_host_targets` in sync
    // with `config.ssh_hosts`.
    if state.ssh_host_targets.iter().any(|(h, u, p)| {
        h == &parsed.host && u.as_deref() == parsed.user.as_deref() && *p == parsed.port
    }) {
        return;
    }
    let endpoint = parsed_ssh_endpoint(&parsed);
    state.save_host_prompt = Some(crate::SaveHostPromptState {
        parsed,
        dont_ask_again: true,
    });
    state
        .renderer
        .set_save_host_prompt(Some(terminale_render::SaveHostPrompt {
            endpoint,
            dont_ask_again: true,
        }));
    state.window.request_redraw();
}

/// Hide the save-host toast (both in state and in the renderer).
pub(crate) fn close_save_prompt(state: &mut RunningState) {
    state.save_host_prompt = None;
    state.renderer.set_save_host_prompt(None);
    state.window.request_redraw();
}

/// Handle a click on the save-host toast: toggle the checkbox in place, or
/// resolve Save / Dismiss. Save stashes the parsed host for the App to add
/// (it owns `config`); both Save and a checked "don't ask again" stash the
/// suppression flag to persist. The actual config mutation + save happens on
/// the next loop tick where `&mut self` (the App) is in scope.
pub(crate) fn handle_save_prompt_click(
    state: &mut RunningState,
    hit: terminale_render::SavePromptHit,
) {
    use terminale_render::SavePromptHit;
    let Some(prompt) = state.save_host_prompt.as_mut() else {
        return;
    };
    match hit {
        SavePromptHit::DontAskAgain => {
            prompt.dont_ask_again = !prompt.dont_ask_again;
            let endpoint = parsed_ssh_endpoint(&prompt.parsed);
            let checked = prompt.dont_ask_again;
            state
                .renderer
                .set_save_host_prompt(Some(terminale_render::SaveHostPrompt {
                    endpoint,
                    dont_ask_again: checked,
                }));
            state.window.request_redraw();
        }
        SavePromptHit::Save => {
            let parsed = prompt.parsed.clone();
            let dont_ask = prompt.dont_ask_again;
            state.pending_save_ssh_host = Some(parsed);
            if dont_ask {
                state.pending_dont_ask_again = Some(true);
            }
            close_save_prompt(state);
        }
        SavePromptHit::Dismiss => {
            // Honour a checked "don't ask again" even when dismissing.
            if prompt.dont_ask_again {
                state.pending_dont_ask_again = Some(true);
            }
            close_save_prompt(state);
        }
    }
}

/// `user@host[:port]` rendering of a parsed ssh destination, matching the
/// style of [`terminale_config::SshHost::endpoint`].
pub(crate) fn parsed_ssh_endpoint(p: &ParsedSsh) -> String {
    let user = p.user.as_deref().unwrap_or("");
    let at = if user.is_empty() { "" } else { "@" };
    if p.port == terminale_config::default_ssh_port() {
        format!("{user}{at}{}", p.host)
    } else {
        format!("{user}{at}{}:{}", p.host, p.port)
    }
}
