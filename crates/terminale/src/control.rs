//! The control API — what a second process is allowed to ask a running
//! terminale to do, and the answers it gets back.
//!
//! This is the transport-independent half of the story: the request/reply
//! vocabulary, the permission gate, and the handler that runs on the UI thread.
//! The wire and the socket live in [`crate::ipc`] (a Unix socket today; a
//! Windows named pipe can be added there without touching anything here).
//!
//! # Why this exists
//!
//! Two reasons, and they reinforce each other.
//!
//! The first is Wayland: a desktop keybinding can only *run a command*, so
//! `terminale --toggle-quake` has to be able to reach the running instance.
//! That is the socket's original job.
//!
//! The second is that a terminal is increasingly not driven by a human alone.
//! An AI agent working in a repo wants to read what a command printed, know
//! whether it succeeded, and propose the next one. Doing that by scraping a
//! screenshot of a window is absurd when the terminal already has the grid, the
//! exit codes (via OSC 133), and every action the command palette can run. So
//! the same channel exposes them:
//!
//! ```text
//! terminale ctl list-tabs
//! terminale ctl last-command --json      # command + output + exit code
//! terminale ctl send-text "cargo test"   # types it at the prompt, does NOT run it
//! terminale ctl action SplitRight
//! ```
//!
//! # Trust model
//!
//! The socket is per-user and mode 0600, so the caller is already "something
//! running as you". That is the same bar as `~/.ssh`, but it is a *lower* bar
//! than the user's intent: an editor plugin or an agent runs as you too, and
//! neither should silently be able to execute commands in your shell.
//!
//! So the permission split is between typing and submitting.
//! [`ControlApiConfig::allow_input`] lets a caller compose text at your prompt;
//! [`ControlApiConfig::allow_submit`] — off by default — is what lets that text
//! carry a newline and actually run. Reading terminal content
//! ([`ControlApiConfig::allow_read`]) and grabbing a PNG
//! ([`ControlApiConfig::allow_screenshot`]) are separately switchable, because
//! scrollback holds whatever your commands printed.
//!
//! Everything is refusable, every refusal says which setting to flip, and the
//! whole surface can be turned off with `integration.control_socket = false`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use terminale_config::ControlApiConfig;

/// How many lines of command output `last-command` returns when the caller
/// does not ask for a specific limit. Generous enough for a test run or a
/// compiler error, small enough not to hand an agent a megabyte by accident.
const DEFAULT_OUTPUT_LINES: usize = 500;

/// The bare word that asks the running instance to toggle Quake visibility.
///
/// Predates the JSON protocol and is kept forever: it is what desktop
/// keybindings, `terminale --toggle-quake`, and every `socat` snippet in the
/// wild send.
pub(crate) const CMD_TOGGLE_QUAKE: &str = "toggle-quake";

/// The bare-word liveness probe. Answered with `ok`, off the UI thread, which
/// is what makes it a useful probe even when the app is busy.
pub(crate) const CMD_PING: &str = "ping";

/// A request from a control-socket client.
///
/// Serialized as a JSON object with a `cmd` discriminant, one per line:
/// `{"cmd":"send-text","text":"cargo test"}`. The two commands that predate
/// this enum (`ping`, `toggle-quake`) are also accepted as bare words so the
/// `--toggle-quake` client and anyone's existing `socat` one-liner keep working
/// — see [`parse_line`].
// NB: no `deny_unknown_fields` — serde ignores it on an internally-tagged enum,
// so asking for it would only look like strictness. Unknown keys are therefore
// skipped; a *missing* required field is still an error, which is the case that
// actually catches a malformed request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub(crate) enum ControlRequest {
    /// Liveness probe. Always allowed, even with the command surface disabled.
    Ping,
    /// Report the running version and which permissions are in effect, so a
    /// client can tell "refused" apart from "not supported by this build".
    Version,
    /// Show/hide the Quake drop-down. Always allowed: this is the command the
    /// socket exists for, and a window-manager keybinding must keep working
    /// regardless of the automation switches.
    ToggleQuake,
    /// Every action name the palette and keybindings can dispatch, with its
    /// label and current bindings.
    ListActions,
    /// Dispatch a named action — anything in the command palette, e.g.
    /// `SplitRight`, `NewTab`, `ToggleZenMode`. Names match
    /// [`crate::keymap::action_from_name`] and are case-insensitive.
    Action {
        /// The action name.
        name: String,
    },
    /// One entry per tab of every window: index, title, size, and status.
    ListTabs,
    /// One entry per split pane of the addressed tab.
    ListPanes {
        /// Tab index; omitted means the active tab.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<usize>,
    },
    /// Read a pane's text — the visible screen by default, the whole buffer
    /// with `scrollback`.
    GetText {
        /// Tab index; omitted means the active tab.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<usize>,
        /// Pane id within that tab; omitted means the tab's focused pane.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<u32>,
        /// Include scrollback history, not just the viewport.
        #[serde(default)]
        scrollback: bool,
    },
    /// The most recent finished command in a pane: what was typed, what it
    /// printed, and what it exited with. Needs shell integration (OSC 133),
    /// which is what supplies the marks.
    LastCommand {
        /// Tab index; omitted means the active tab.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<usize>,
        /// Pane id within that tab; omitted means the tab's focused pane.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<u32>,
        /// Cap on returned output lines. Defaults to
        /// [`DEFAULT_OUTPUT_LINES`]; the reply flags whether it truncated.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_lines: Option<usize>,
    },
    /// Type text into a pane's shell, exactly as if it had been typed.
    ///
    /// A trailing newline is stripped unless `submit` is set *and*
    /// [`ControlApiConfig::allow_submit`] is on, so the default behaviour is to
    /// leave the command sitting at the prompt for a human to read and press
    /// Enter — the same contract the AI "Inject" button uses.
    SendText {
        /// The text to type.
        text: String,
        /// Tab index; omitted means the active tab.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<usize>,
        /// Pane id within that tab; omitted means the tab's focused pane.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<u32>,
        /// Also press Enter. Refused unless `allow_submit` is enabled.
        #[serde(default)]
        submit: bool,
    },
    /// Send key *presses* — `"ctrl+c"`, `"escape"`, `"down down enter"` — with
    /// the encoding the pane's current modes call for (application cursor keys,
    /// the kitty keyboard protocol when a program has engaged it).
    ///
    /// `enter`/`return` count as submitting, so they need `allow_submit`.
    SendKeys {
        /// Space-separated key specs.
        keys: String,
        /// Tab index; omitted means the active tab.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<usize>,
        /// Pane id within that tab; omitted means the tab's focused pane.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<u32>,
    },
    /// Write a PNG of the focused window's next frame to `path`.
    Screenshot {
        /// Destination file. Must be absolute — the running instance's working
        /// directory is not the caller's.
        path: PathBuf,
    },
}

impl ControlRequest {
    /// Short name used in logs and refusal messages.
    fn name(&self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::Version => "version",
            Self::ToggleQuake => "toggle-quake",
            Self::ListActions => "list-actions",
            Self::Action { .. } => "action",
            Self::ListTabs => "list-tabs",
            Self::ListPanes { .. } => "list-panes",
            Self::GetText { .. } => "get-text",
            Self::LastCommand { .. } => "last-command",
            Self::SendText { .. } => "send-text",
            Self::SendKeys { .. } => "send-keys",
            Self::Screenshot { .. } => "screenshot",
        }
    }

    /// Whether this request has to be answered by the UI thread. `ping` is the
    /// only one that does not — and answering it off the UI thread is the
    /// point, since it is how a client probes a *busy* instance.
    pub(crate) fn needs_ui_thread(&self) -> bool {
        !matches!(self, Self::Ping)
    }
}

/// Which permission a request needs, so the gate reads as a table rather than
/// a pile of `match` arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Always served — liveness and the Quake toggle a keybinding depends on.
    Always,
    /// Needs the command surface enabled, nothing more (metadata only).
    Meta,
    /// Reads terminal content.
    Read,
    /// Drives the app or types into a shell.
    Input,
    /// Types something that submits — a newline, or Enter.
    Submit,
    /// Renders a PNG to a caller-chosen path.
    Screenshot,
}

/// The permission a request needs.
fn scope_of(req: &ControlRequest) -> Scope {
    match req {
        ControlRequest::Ping | ControlRequest::ToggleQuake | ControlRequest::Version => {
            Scope::Always
        }
        ControlRequest::ListActions => Scope::Meta,
        ControlRequest::ListTabs | ControlRequest::ListPanes { .. } => Scope::Read,
        ControlRequest::GetText { .. } | ControlRequest::LastCommand { .. } => Scope::Read,
        ControlRequest::Action { .. } => Scope::Input,
        ControlRequest::SendText { text, submit, .. } => {
            if *submit || ends_with_newline(text) {
                Scope::Submit
            } else {
                Scope::Input
            }
        }
        ControlRequest::SendKeys { keys, .. } => {
            if crate::keyspec::any_submits(keys) {
                Scope::Submit
            } else {
                Scope::Input
            }
        }
        ControlRequest::Screenshot { .. } => Scope::Screenshot,
    }
}

/// Whether `text` would submit a command as typed.
fn ends_with_newline(text: &str) -> bool {
    text.ends_with('\n') || text.ends_with('\r')
}

/// Decide whether `cfg` permits `req`, with a message naming the setting to
/// change when it does not.
///
/// Kept as a free function over plain config so the policy is unit-testable
/// without a window, a GPU, or an event loop.
pub(crate) fn permits(cfg: &ControlApiConfig, req: &ControlRequest) -> Result<(), String> {
    let scope = scope_of(req);
    if scope == Scope::Always {
        return Ok(());
    }
    if !cfg.enabled {
        return Err(format!(
            "`{}` refused: the control API is disabled \
             (set `integration.control_api.enabled = true`)",
            req.name()
        ));
    }
    let (allowed, setting) = match scope {
        Scope::Always | Scope::Meta => (true, ""),
        Scope::Read => (cfg.allow_read, "integration.control_api.allow_read"),
        Scope::Input => (cfg.allow_input, "integration.control_api.allow_input"),
        // Submitting needs *both*: the right to type, and the right to press
        // Enter. Reporting the missing one specifically keeps the message
        // actionable when only one of the two is off.
        Scope::Submit => {
            if cfg.allow_input {
                (cfg.allow_submit, "integration.control_api.allow_submit")
            } else {
                (false, "integration.control_api.allow_input")
            }
        }
        Scope::Screenshot => (
            cfg.allow_screenshot,
            "integration.control_api.allow_screenshot",
        ),
    };
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "`{}` refused: set `{setting} = true` to allow it",
            req.name()
        ))
    }
}

/// A reply to a [`ControlRequest`], serialized as one JSON line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ControlReply {
    /// Whether the request was carried out.
    pub(crate) ok: bool,
    /// Why not, when `ok` is `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// Command-specific payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) data: Option<serde_json::Value>,
}

impl ControlReply {
    /// A bare success, for commands whose effect is the answer.
    pub(crate) fn ok() -> Self {
        Self {
            ok: true,
            error: None,
            data: None,
        }
    }

    /// A success carrying a payload.
    pub(crate) fn with(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            error: None,
            data: Some(data),
        }
    }

    /// A refusal or failure.
    pub(crate) fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            data: None,
        }
    }

    /// Serialize to the single line that goes on the wire. Infallible in
    /// practice; a serializer failure degrades to a hand-built error object
    /// rather than dropping the connection.
    pub(crate) fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            format!(r#"{{"ok":false,"error":"reply serialization failed: {e}"}}"#)
        })
    }
}

/// Parse one wire line into a request.
///
/// Accepts either a JSON object or one of the two legacy bare words. The bare
/// words are load-bearing: `terminale --toggle-quake` on an older binary, a
/// keybinding someone wired to `echo toggle-quake | socat …`, and the liveness
/// probe [`crate::ipc::serve`] uses to tell a live socket from a stale file all
/// speak them.
///
/// # Errors
///
/// Returns a human-readable message when the line is neither.
pub(crate) fn parse_line(line: &str) -> Result<ControlRequest, String> {
    let line = line.trim();
    match line {
        "" => return Err("empty command".into()),
        CMD_PING => return Ok(ControlRequest::Ping),
        CMD_TOGGLE_QUAKE => return Ok(ControlRequest::ToggleQuake),
        _ => {}
    }
    if !line.starts_with('{') {
        return Err(format!(
            "unknown command `{}`; expected a JSON request or `ping` / `toggle-quake`. \
             Use the `terminale ctl` subcommands to build one.",
            line.chars().take(40).collect::<String>()
        ));
    }
    serde_json::from_str(line).map_err(|e| format!("malformed request: {e}"))
}

/// Whether a reply to `req` should be sent in the legacy bare-`ok` form.
///
/// The old protocol answered exactly `ok`, and the `--toggle-quake` client
/// string-compares against it. A client that spoke JSON gets JSON back.
pub(crate) fn wants_legacy_reply(line: &str) -> bool {
    let line = line.trim();
    line == CMD_PING || line == CMD_TOGGLE_QUAKE
}

// ── The UI-thread handler ─────────────────────────────────────────────────────

/// Carry out `req` and produce its reply.
///
/// Runs on the winit thread — every caller reaches it through
/// [`crate::UserEvent::Control`], because touching tabs, panes, emulators or the
/// renderer from the socket thread would race the render loop. That is also why
/// this is the only place the permission gate needs to live: config is here.
pub(crate) fn handle(app: &mut crate::TerminaleApp, req: &ControlRequest) -> ControlReply {
    if let Err(refusal) = permits(&app.config.integration.control_api, req) {
        tracing::debug!(command = req.name(), "control request refused");
        return ControlReply::err(refusal);
    }

    match req {
        ControlRequest::Ping => ControlReply::ok(),
        ControlRequest::Version => ControlReply::with(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "windows": app.windows.len(),
            "permissions": {
                "enabled": app.config.integration.control_api.enabled,
                "read": app.config.integration.control_api.allow_read,
                "input": app.config.integration.control_api.allow_input,
                "submit": app.config.integration.control_api.allow_submit,
                "screenshot": app.config.integration.control_api.allow_screenshot,
            },
        })),
        ControlRequest::ToggleQuake => {
            app.toggle_quake_all();
            ControlReply::ok()
        }
        ControlRequest::ListActions => list_actions(app),
        ControlRequest::Action { name } => run_action(app, name),
        ControlRequest::ListTabs => list_tabs(app),
        ControlRequest::ListPanes { tab } => list_panes(app, *tab),
        ControlRequest::GetText {
            tab,
            pane,
            scrollback,
        } => get_text(app, *tab, *pane, *scrollback),
        ControlRequest::LastCommand {
            tab,
            pane,
            max_lines,
        } => last_command(app, *tab, *pane, *max_lines),
        ControlRequest::SendText {
            text,
            tab,
            pane,
            submit,
        } => send_text(app, text, *tab, *pane, *submit),
        ControlRequest::SendKeys { keys, tab, pane } => send_keys(app, keys, *tab, *pane),
        ControlRequest::Screenshot { path } => screenshot(app, path),
    }
}

/// Index of the window a request addresses — the focused one.
///
/// Errors rather than defaulting when there is no window at all, which happens
/// while the app is shutting down.
fn focused_window(app: &crate::TerminaleApp) -> Result<usize, ControlReply> {
    app.focused_window_index()
        .filter(|i| app.windows.get(*i).is_some())
        .ok_or_else(|| ControlReply::err("no terminal window is open"))
}

/// Resolve `tab` (defaulting to the active one) to an index in `state.tabs`.
fn resolve_tab(state: &crate::TermWindow, tab: Option<usize>) -> Result<usize, ControlReply> {
    let idx = tab.unwrap_or(state.active_tab);
    if state.tabs.get(idx).is_some() {
        Ok(idx)
    } else {
        Err(ControlReply::err(format!(
            "no tab {idx}: this window has {} ({}..={})",
            state.tabs.len(),
            0,
            state.tabs.len().saturating_sub(1)
        )))
    }
}

/// Resolve `pane` (defaulting to the tab's focused pane) to a pane id.
fn resolve_pane(tab: &crate::TabState, pane: Option<u32>) -> Result<u32, ControlReply> {
    let id = pane.unwrap_or(tab.focused);
    if tab.panes.contains_key(&id) {
        Ok(id)
    } else {
        let ids: Vec<String> = tab.panes.keys().map(u32::to_string).collect();
        Err(ControlReply::err(format!(
            "no pane {id} in this tab (have: {})",
            ids.join(", ")
        )))
    }
}

/// Resolve a request's `tab`/`pane` pair to concrete coordinates in one step,
/// since every content and input command needs exactly this.
fn coords(
    app: &crate::TerminaleApp,
    tab: Option<usize>,
    pane: Option<u32>,
) -> Result<(usize, usize, u32), ControlReply> {
    let w = focused_window(app)?;
    let state = &app.windows[w];
    let t = resolve_tab(state, tab)?;
    let p = resolve_pane(&state.tabs[t], pane)?;
    Ok((w, t, p))
}

/// Every dispatchable action with its label and current binding.
fn list_actions(app: &crate::TerminaleApp) -> ControlReply {
    let shortcuts = app
        .focused_window_index()
        .and_then(|i| app.windows.get(i))
        .map_or_else(
            || app.config.keybinds.shortcuts.clone(),
            |s| s.shortcuts.clone(),
        );
    let actions: Vec<serde_json::Value> = crate::palette::PALETTE_ACTIONS
        .iter()
        .map(|(action, label)| {
            let binding = crate::shortcuts::binding_for(*action, &shortcuts);
            serde_json::json!({
                // The `{:?}` form is the action's canonical name and is exactly
                // what `action_from_name` parses back, so a client can feed a
                // listed name straight to `ctl action`.
                "name": format!("{action:?}"),
                "label": label,
                "binding": binding,
            })
        })
        .collect();
    ControlReply::with(serde_json::json!({ "actions": actions }))
}

/// Dispatch a named action into the focused window.
fn run_action(app: &mut crate::TerminaleApp, name: &str) -> ControlReply {
    let Some(action) = crate::keymap::action_from_name(name) else {
        return ControlReply::err(format!(
            "unknown action `{name}` — `terminale ctl list-actions` prints the names"
        ));
    };
    let w = match focused_window(app) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let state = &mut app.windows[w];
    crate::dispatch_shortcut(state, action);
    state.window.request_redraw();
    ControlReply::with(serde_json::json!({ "dispatched": format!("{action:?}") }))
}

/// Describe every tab of every window.
fn list_tabs(app: &crate::TerminaleApp) -> ControlReply {
    let focused = app.focused_window_index();
    let windows: Vec<serde_json::Value> = app
        .windows
        .iter()
        .enumerate()
        .map(|(w, state)| {
            let tabs: Vec<serde_json::Value> = state
                .tabs
                .iter()
                .enumerate()
                .map(|(t, tab)| {
                    // Compute the label *before* locking: `tab_label` takes the
                    // same emulator lock, and parking_lot mutexes are not
                    // reentrant, so doing it inside the guard's scope wedges the
                    // UI thread permanently.
                    let title = crate::panes::tab_label(tab);
                    let pane = tab.focused_pane();
                    let em = pane.emulator.lock();
                    serde_json::json!({
                        "index": t,
                        "title": title,
                        "cwd": em.current_dir(),
                        "active": t == state.active_tab,
                        "panes": tab.panes.len(),
                        "cols": pane.cols,
                        "rows": pane.rows,
                        // The signals a supervising agent actually wants: has
                        // this tab produced output I haven't seen, and did the
                        // program in it ring the bell to ask for attention
                        // (which is how a finished agent turn shows up).
                        "unread": tab.unread,
                        "attention": tab.attention,
                        "alt_screen": em.is_alt_screen(),
                        "crashed": pane.crashed,
                    })
                })
                .collect();
            serde_json::json!({
                "index": w,
                "focused": focused == Some(w),
                "tabs": tabs,
            })
        })
        .collect();
    ControlReply::with(serde_json::json!({ "windows": windows }))
}

/// Describe the split panes of one tab.
fn list_panes(app: &crate::TerminaleApp, tab: Option<usize>) -> ControlReply {
    let w = match focused_window(app) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let state = &app.windows[w];
    let t = match resolve_tab(state, tab) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let tab_state = &state.tabs[t];
    let panes: Vec<serde_json::Value> = tab_state
        .panes
        .iter()
        .map(|(id, pane)| {
            // Same reentrancy trap as in `list_tabs`: `pane_label` locks this
            // pane's emulator, so it has to happen before the guard exists.
            let title = crate::panes::pane_label(pane);
            let em = pane.emulator.lock();
            serde_json::json!({
                "id": id,
                "title": title,
                "cwd": em.current_dir(),
                "focused": *id == tab_state.focused,
                "zoomed": tab_state.zoomed_pane == Some(*id),
                "cols": pane.cols,
                "rows": pane.rows,
                "alt_screen": em.is_alt_screen(),
                "crashed": pane.crashed,
            })
        })
        .collect();
    ControlReply::with(serde_json::json!({ "tab": t, "panes": panes }))
}

/// Read a pane's text.
fn get_text(
    app: &crate::TerminaleApp,
    tab: Option<usize>,
    pane: Option<u32>,
    scrollback: bool,
) -> ControlReply {
    let (w, t, p) = match coords(app, tab, pane) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let pane_ref = &app.windows[w].tabs[t].panes[&p];
    let em = pane_ref.emulator.lock();
    let lines = if scrollback {
        em.buffer_lines_text()
    } else {
        em.visible_lines_text()
    };
    let (cols, rows) = em.size();
    let (cursor_col, cursor_row) = em.cursor();
    ControlReply::with(serde_json::json!({
        "tab": t,
        "pane": p,
        "text": lines.join("\n"),
        "lines": lines.len(),
        "cols": cols,
        "rows": rows,
        "cursor": { "col": cursor_col, "row": cursor_row },
        "scrollback_lines": em.history_size(),
    }))
}

/// The last finished command in a pane, as command + output + exit code.
///
/// This is the single most useful thing an agent can ask a terminal, and it is
/// only answerable because OSC 133 marks tell the emulator where the command
/// ended and how it exited. Without shell integration there are no marks, so
/// the reply says so rather than guessing from the grid.
fn last_command(
    app: &crate::TerminaleApp,
    tab: Option<usize>,
    pane: Option<u32>,
    max_lines: Option<usize>,
) -> ControlReply {
    let (w, t, p) = match coords(app, tab, pane) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let pane_ref = &app.windows[w].tabs[t].panes[&p];
    let em = pane_ref.emulator.lock();
    let Some(block) = em.last_command_block().cloned() else {
        return ControlReply::err(
            "no command block recorded — this needs shell integration (OSC 133); \
             check `terminal.shell_integration` and `terminal.command_blocks`",
        );
    };
    let all = em.buffer_lines_text();
    let hist = i32::try_from(em.history_size()).unwrap_or(i32::MAX);
    // A still-running command has no D mark yet; read to the end of the buffer
    // so a caller watching a long build sees its output so far.
    let end = block
        .end_line
        .unwrap_or_else(|| all.len() as i32 - hist - 1);
    let output =
        crate::shortcuts::extract_block_output_text(&all, hist, block.output_start_line, end);
    let limit = max_lines.unwrap_or(DEFAULT_OUTPUT_LINES);
    let mut out_lines: Vec<&str> = output.lines().collect();
    let total = out_lines.len();
    let truncated = total > limit;
    if truncated {
        // Keep the tail: the interesting part of a failed build is the end.
        out_lines = out_lines.split_off(total - limit);
    }
    ControlReply::with(serde_json::json!({
        "tab": t,
        "pane": p,
        "command": block.command_text,
        "cwd": block.cwd,
        "exit_code": block.exit_code,
        "running": block.end_line.is_none(),
        "output": out_lines.join("\n"),
        "output_lines": out_lines.len(),
        "output_truncated": truncated,
        "output_total_lines": total,
    }))
}

/// Type text into a pane's shell.
fn send_text(
    app: &mut crate::TerminaleApp,
    text: &str,
    tab: Option<usize>,
    pane: Option<u32>,
    submit: bool,
) -> ControlReply {
    let (w, t, p) = match coords(app, tab, pane) {
        Ok(c) => c,
        Err(e) => return e,
    };
    // Normalise: the payload's own trailing newline and the `submit` flag mean
    // the same thing, and the permission gate has already treated them alike.
    let body = text.trim_end_matches(['\r', '\n']);
    let mut bytes = body.as_bytes().to_vec();
    if submit || ends_with_newline(text) {
        bytes.push(b'\r');
    }
    write_to_pane(app, w, t, p, &bytes).unwrap_or_else(|e| e)
}

/// Send key presses to a pane, encoded for its current modes.
fn send_keys(
    app: &mut crate::TerminaleApp,
    keys: &str,
    tab: Option<usize>,
    pane: Option<u32>,
) -> ControlReply {
    let (w, t, p) = match coords(app, tab, pane) {
        Ok(c) => c,
        Err(e) => return e,
    };
    // Read the modes off the pane's own emulator so the encoding matches what
    // the program running there is expecting right now.
    let (app_cursor, kitty) = {
        let em = app.windows[w].tabs[t].panes[&p].emulator.lock();
        (em.app_cursor_mode(), em.kitty_keyboard_flags())
    };
    let bytes = match crate::keyspec::encode_keys(keys, app_cursor, kitty) {
        Ok(b) => b,
        Err(e) => return ControlReply::err(e),
    };
    if bytes.is_empty() {
        return ControlReply::err(format!("`{keys}` encodes to nothing in the current mode"));
    }
    write_to_pane(app, w, t, p, &bytes).unwrap_or_else(|e| e)
}

/// Write bytes to a pane's PTY and refresh its window.
///
/// Stamps `last_input_at` exactly as a keystroke does, so the busy-indicator
/// heuristics keep telling command output apart from input echo.
fn write_to_pane(
    app: &mut crate::TerminaleApp,
    w: usize,
    t: usize,
    p: u32,
    bytes: &[u8],
) -> Result<ControlReply, ControlReply> {
    let state = &mut app.windows[w];
    let pane = state.tabs[t]
        .panes
        .get_mut(&p)
        .ok_or_else(|| ControlReply::err("the pane went away"))?;
    if pane.crashed {
        return Err(ControlReply::err(
            "that pane has crashed — restart it before sending input",
        ));
    }
    pane.session
        .write_input(bytes)
        .map_err(|e| ControlReply::err(format!("could not write to the pty: {e}")))?;
    pane.last_input_at = Some(std::time::Instant::now());
    state.window.request_redraw();
    Ok(ControlReply::with(serde_json::json!({
        "tab": t,
        "pane": p,
        "bytes": bytes.len(),
    })))
}

/// Render the focused window's next frame to a PNG.
fn screenshot(app: &mut crate::TerminaleApp, path: &std::path::Path) -> ControlReply {
    if !path.is_absolute() {
        return ControlReply::err(format!(
            "`{}` is relative — pass an absolute path, since the running \
             instance's working directory is not yours",
            path.display()
        ));
    }
    let w = match focused_window(app) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let state = &mut app.windows[w];
    // Capture happens on the next presented frame, not this instant: the
    // renderer copies the frame it is about to show, which is the only way the
    // PNG matches what the user sees (including any animation mid-flight).
    state.renderer.request_capture(path.to_path_buf());
    state.window.request_redraw();
    ControlReply::with(serde_json::json!({
        "path": path.display().to_string(),
        "pending": true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_all_on() -> ControlApiConfig {
        ControlApiConfig {
            enabled: true,
            allow_read: true,
            allow_input: true,
            allow_submit: true,
            allow_screenshot: true,
        }
    }

    // ── parse_line ───────────────────────────────────────────────────────────

    /// The two pre-JSON commands must keep parsing, or a desktop keybinding
    /// wired to `terminale --toggle-quake` breaks on upgrade.
    #[test]
    fn legacy_bare_words_still_parse() {
        assert_eq!(parse_line("ping").expect("ping"), ControlRequest::Ping);
        assert_eq!(
            parse_line("toggle-quake").expect("toggle"),
            ControlRequest::ToggleQuake
        );
        assert_eq!(
            parse_line("  toggle-quake \n").expect("whitespace-tolerant"),
            ControlRequest::ToggleQuake
        );
        assert!(wants_legacy_reply("ping"));
        assert!(wants_legacy_reply("toggle-quake\n"));
        assert!(!wants_legacy_reply(r#"{"cmd":"ping"}"#));
    }

    #[test]
    fn json_requests_parse() {
        assert_eq!(
            parse_line(r#"{"cmd":"list-tabs"}"#).expect("list-tabs"),
            ControlRequest::ListTabs
        );
        assert_eq!(
            parse_line(r#"{"cmd":"send-text","text":"ls","submit":true}"#).expect("send-text"),
            ControlRequest::SendText {
                text: "ls".into(),
                tab: None,
                pane: None,
                submit: true,
            }
        );
        assert_eq!(
            parse_line(r#"{"cmd":"get-text","tab":2,"scrollback":true}"#).expect("get-text"),
            ControlRequest::GetText {
                tab: Some(2),
                pane: None,
                scrollback: true,
            }
        );
    }

    #[test]
    fn bad_lines_are_rejected_with_a_message() {
        assert!(parse_line("").is_err());
        assert!(parse_line("do-something-else").is_err());
        assert!(parse_line("{not json}").is_err());
        assert!(parse_line(r#"{"cmd":"no-such-command"}"#).is_err());
        // A required field left out is the malformation that matters, and it is
        // caught.
        assert!(parse_line(r#"{"cmd":"send-text"}"#).is_err());
        assert!(parse_line(r#"{"cmd":"action"}"#).is_err());
        // An unknown key is tolerated: serde cannot deny extra fields on an
        // internally-tagged enum, so this documents the real behaviour rather
        // than pretending otherwise.
        assert_eq!(
            parse_line(r#"{"cmd":"list-tabs","nope":1}"#).expect("tolerated"),
            ControlRequest::ListTabs
        );
    }

    /// Round-tripping matters: the `ctl` client serializes, the server parses.
    #[test]
    fn every_request_round_trips() {
        let reqs = vec![
            ControlRequest::Ping,
            ControlRequest::Version,
            ControlRequest::ToggleQuake,
            ControlRequest::ListActions,
            ControlRequest::Action {
                name: "NewTab".into(),
            },
            ControlRequest::ListTabs,
            ControlRequest::ListPanes { tab: Some(1) },
            ControlRequest::GetText {
                tab: None,
                pane: Some(3),
                scrollback: true,
            },
            ControlRequest::LastCommand {
                tab: None,
                pane: None,
                max_lines: Some(10),
            },
            ControlRequest::SendText {
                text: "echo hi".into(),
                tab: None,
                pane: None,
                submit: false,
            },
            ControlRequest::SendKeys {
                keys: "ctrl+c".into(),
                tab: None,
                pane: None,
            },
            ControlRequest::Screenshot {
                path: PathBuf::from("/tmp/x.png"),
            },
        ];
        for req in reqs {
            let line = serde_json::to_string(&req).expect("serialize");
            assert_eq!(parse_line(&line).expect("parse back"), req, "line: {line}");
        }
    }

    // ── permits ──────────────────────────────────────────────────────────────

    /// Quake toggling and liveness must survive every switch being off — they
    /// are how a window manager and a health check reach the app.
    #[test]
    fn always_allowed_commands_ignore_every_switch() {
        let off = ControlApiConfig {
            enabled: false,
            allow_read: false,
            allow_input: false,
            allow_submit: false,
            allow_screenshot: false,
        };
        for req in [
            ControlRequest::Ping,
            ControlRequest::ToggleQuake,
            ControlRequest::Version,
        ] {
            permits(&off, &req).expect("must stay allowed");
        }
    }

    #[test]
    fn disabled_api_refuses_everything_else() {
        let off = ControlApiConfig {
            enabled: false,
            ..cfg_all_on()
        };
        let err = permits(&off, &ControlRequest::ListTabs).expect_err("refused");
        assert!(err.contains("control_api.enabled"), "{err}");
    }

    #[test]
    fn read_scope_gated_by_allow_read() {
        let cfg = ControlApiConfig {
            allow_read: false,
            ..cfg_all_on()
        };
        for req in [
            ControlRequest::ListTabs,
            ControlRequest::ListPanes { tab: None },
            ControlRequest::GetText {
                tab: None,
                pane: None,
                scrollback: false,
            },
            ControlRequest::LastCommand {
                tab: None,
                pane: None,
                max_lines: None,
            },
        ] {
            let err = permits(&cfg, &req).expect_err("refused");
            assert!(err.contains("allow_read"), "{err}");
        }
        // Metadata is not content, so it stays available.
        permits(&cfg, &ControlRequest::ListActions).expect("actions are metadata");
    }

    #[test]
    fn input_scope_gated_by_allow_input() {
        let cfg = ControlApiConfig {
            allow_input: false,
            ..cfg_all_on()
        };
        let err = permits(
            &cfg,
            &ControlRequest::Action {
                name: "NewTab".into(),
            },
        )
        .expect_err("refused");
        assert!(err.contains("allow_input"), "{err}");
    }

    /// The heart of the trust model: typing is allowed, pressing Enter is not.
    #[test]
    fn submit_needs_allow_submit_even_with_input_on() {
        let cfg = ControlApiConfig {
            allow_submit: false,
            ..cfg_all_on()
        };
        let typing = ControlRequest::SendText {
            text: "rm -rf /".into(),
            tab: None,
            pane: None,
            submit: false,
        };
        permits(&cfg, &typing).expect("composing at the prompt is allowed");

        for req in [
            ControlRequest::SendText {
                text: "rm -rf /".into(),
                tab: None,
                pane: None,
                submit: true,
            },
            // A newline in the payload is submitting by another name — the gate
            // must not be bypassable by smuggling one in.
            ControlRequest::SendText {
                text: "rm -rf /\n".into(),
                tab: None,
                pane: None,
                submit: false,
            },
            ControlRequest::SendText {
                text: "rm -rf /\r".into(),
                tab: None,
                pane: None,
                submit: false,
            },
            ControlRequest::SendKeys {
                keys: "enter".into(),
                tab: None,
                pane: None,
            },
            ControlRequest::SendKeys {
                keys: "ctrl+c enter".into(),
                tab: None,
                pane: None,
            },
        ] {
            let err = permits(&cfg, &req).expect_err("must refuse submitting");
            assert!(err.contains("allow_submit"), "{req:?} -> {err}");
        }
    }

    /// With input off entirely, a submit attempt should name `allow_input` —
    /// the switch the caller actually has to flip first.
    #[test]
    fn submit_with_input_off_names_input() {
        let cfg = ControlApiConfig {
            allow_input: false,
            allow_submit: true,
            ..cfg_all_on()
        };
        let err = permits(
            &cfg,
            &ControlRequest::SendKeys {
                keys: "enter".into(),
                tab: None,
                pane: None,
            },
        )
        .expect_err("refused");
        assert!(err.contains("allow_input"), "{err}");
    }

    #[test]
    fn screenshot_has_its_own_switch() {
        let cfg = ControlApiConfig {
            allow_screenshot: false,
            ..cfg_all_on()
        };
        let err = permits(
            &cfg,
            &ControlRequest::Screenshot {
                path: PathBuf::from("/tmp/a.png"),
            },
        )
        .expect_err("refused");
        assert!(err.contains("allow_screenshot"), "{err}");
        // Reading text is a different switch and stays on.
        permits(
            &cfg,
            &ControlRequest::GetText {
                tab: None,
                pane: None,
                scrollback: false,
            },
        )
        .expect("reading is unaffected");
    }

    #[test]
    fn ping_is_answerable_without_the_ui_thread() {
        assert!(!ControlRequest::Ping.needs_ui_thread());
        assert!(ControlRequest::ListTabs.needs_ui_thread());
        assert!(ControlRequest::ToggleQuake.needs_ui_thread());
    }

    #[test]
    fn reply_lines_are_single_line_json() {
        let long = ControlReply::with(serde_json::json!({ "text": "a\nb\nc" }));
        let line = long.to_line();
        assert!(!line.contains('\n'), "newlines must be escaped: {line}");
        assert!(line.contains("a\\nb\\nc"));
        let err = ControlReply::err("nope").to_line();
        assert!(err.contains(r#""ok":false"#));
        assert!(err.contains("nope"));
    }
}
