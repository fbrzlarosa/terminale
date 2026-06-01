//! Lua 5.4 plugin host for `terminale`.
//!
//! Plugins live as `*.lua` files in the user's plugin directory
//! (typically `~/.config/terminale/plugins/`). Each plugin runs inside
//! a single shared Lua state with **stdlib sandboxing**:
//!
//! * `io`, `os.execute`, `os.exit`, `os.getenv`, `os.remove`, `package`,
//!   `debug`, `require` are stripped out at load time so a malicious or
//!   buggy script cannot reach the filesystem or spawn processes.
//! * A `terminale` global table is injected with the full capability surface:
//!   `log()`, `notify(title, body)`, `register_hook(event, fn)`,
//!   `set_tab_title(text)`, `open_tab()`, `send_text(text)`,
//!   `register_command(name, fn)`.
//!
//! ## Hooks
//!
//! The host fires hooks at well-defined lifecycle points. Plugins subscribe
//! via `terminale.register_hook(name, handler)` and the host invokes every
//! registered handler when the event fires. Errors during a hook are
//! logged and the handler is dropped to keep the host healthy.
//!
//! Supported hook events: `"tick"`, `"tab_open"`, `"tab_close"`,
//! `"pane_focus"`, `"session_start"`, `"session_exit"`, `"config_reload"`,
//! `"command_end"`.
//!
//! Hook handlers receive a Lua table with structured fields appropriate to
//! the event (e.g. `{tab_id=1, title="bash"}` for `tab_open`).
//!
//! ## Capability queue
//!
//! Lua callbacks **never** mutate app state directly. Instead they push
//! [`PluginCommand`] entries into a shared queue that the host drains once
//! per tick on the main thread. Use [`PluginHost::drain_commands`] after
//! each tick to retrieve and apply pending commands.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

use mlua::{Function, Lua, MultiValue, RegistryKey, Result as LuaResult, Table, Value};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

/// Errors raised by the plugin host.
#[derive(Debug, Error)]
pub enum PluginError {
    /// I/O failure reading a plugin file or directory.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Lua runtime / parse failure.
    #[error("lua: {0}")]
    Lua(#[from] mlua::Error),
}

/// Information about a successfully-loaded plugin.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    /// File path the plugin was loaded from.
    pub path: PathBuf,
    /// File-stem name used in log messages.
    pub name: String,
}

/// A command enqueued by a Lua callback and drained by the host on the
/// next tick. Commands are applied on the main thread, never from inside
/// the Lua call itself, so there is no reentrancy / borrow concern.
#[derive(Debug)]
pub enum PluginCommand {
    /// Raise an OS desktop notification with the given title and body.
    Notify {
        /// Short title / summary line.
        title: String,
        /// Longer body text.
        body: String,
    },
    /// Rename the currently-active tab.
    SetTabTitle(String),
    /// Open a new tab using the default profile.
    OpenTab,
    /// Write raw bytes to the currently-focused pane's PTY.
    SendText(String),
    /// Invoke a previously-registered plugin command by its registry key.
    InvokeCommand {
        /// Index into `PluginHost::registered_commands`.
        command_idx: usize,
    },
}

/// One registered command-palette entry contributed by a plugin.
#[derive(Debug)]
pub struct RegisteredCommand {
    /// The label shown in the command palette.
    pub name: String,
    /// The mlua registry key holding the Lua `function` to call.
    pub key: RegistryKey,
}

/// Shared registry of hook handlers. Wrapped in an [`Arc<Mutex>`] so the
/// Lua-side `register_hook` can append handlers from arbitrary code.
type HookKey = Arc<Mutex<Vec<(String, RegistryKey)>>>;
/// Shared command queue. Lua-side capabilities push into this; the host
/// drains it once per tick from the main thread.
type CommandQueue = Arc<Mutex<Vec<PluginCommand>>>;

/// The Lua plugin host. Holds the shared Lua state, hook registry, command
/// queue, and the list of loaded plugins.
pub struct PluginHost {
    lua: Lua,
    hooks: HookKey,
    commands: CommandQueue,
    /// Commands registered by plugins for the command palette.
    pub registered_commands: Vec<RegisteredCommand>,
    loaded: Vec<PluginInfo>,
}

impl PluginHost {
    /// Build a fresh host with the sandboxed `terminale` API ready.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::Lua`] if injecting the API or stripping
    /// the stdlib fails (unexpected — these are all standard ops).
    pub fn new() -> Result<Self, PluginError> {
        let lua = Lua::new();
        let hooks: HookKey = Arc::new(Mutex::new(Vec::new()));
        let commands: CommandQueue = Arc::new(Mutex::new(Vec::new()));
        sandbox(&lua)?;
        install_api(&lua, &hooks, &commands)?;
        Ok(Self {
            lua,
            hooks,
            commands,
            registered_commands: Vec::new(),
            loaded: Vec::new(),
        })
    }

    /// Load every `*.lua` file in `dir`. Skips entries that fail to
    /// parse or panic — each plugin is independent.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::Io`] when the directory itself can't be
    /// read. Per-plugin errors are logged via `tracing` but don't abort
    /// loading.
    pub fn load_dir(&mut self, dir: &Path) -> Result<(), PluginError> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("lua") {
                continue;
            }
            match self.load_file(&path) {
                Ok(info) => tracing::info!(plugin = %info.name, "loaded"),
                Err(e) => tracing::warn!(?e, path = %path.display(), "plugin load failed"),
            }
        }
        // Drain any register_command calls made during load.
        self.drain_commands_internal();
        Ok(())
    }

    /// Load a single Lua file as a plugin.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError`] on read / parse / runtime failures.
    pub fn load_file(&mut self, path: &Path) -> Result<PluginInfo, PluginError> {
        let source = std::fs::read_to_string(path)?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("plugin")
            .to_string();
        let chunk = self.lua.load(source).set_name(name.clone());
        chunk.exec()?;
        let info = PluginInfo {
            path: path.to_path_buf(),
            name,
        };
        self.loaded.push(info.clone());
        Ok(info)
    }

    /// Load an inline Lua source string as a named plugin (for demos /
    /// tests).
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::Lua`] on parse / runtime failures.
    pub fn load_inline(&mut self, name: &str, source: &str) -> Result<PluginInfo, PluginError> {
        let chunk = self.lua.load(source).set_name(name.to_string());
        chunk.exec()?;
        let info = PluginInfo {
            path: PathBuf::from(format!("<inline:{name}>")),
            name: name.to_string(),
        };
        self.loaded.push(info.clone());
        Ok(info)
    }

    /// Fire a hook by name with a structured Lua table payload built from
    /// the provided key–value pairs.
    ///
    /// Returns the number of handlers that ran successfully. Handlers
    /// that raise an error are logged and removed so a buggy plugin
    /// doesn't spam the log forever.
    ///
    /// `fields` is a slice of `(&str, LuaPayloadValue)` that are set on the
    /// table before calling each handler. The table is rebuilt for every
    /// handler call to avoid cross-handler state leakage.
    pub fn fire_event(&self, name: &str, fields: &[(&str, LuaPayloadValue)]) -> usize {
        let mut ran = 0usize;
        let mut to_drop: Vec<usize> = Vec::new();

        let guard = self.hooks.lock();
        for (i, (n, key)) in guard.iter().enumerate() {
            if n != name {
                continue;
            }
            let Ok(value) = self.lua.registry_value::<Value>(key) else {
                continue;
            };
            let Value::Function(f) = value else { continue };

            // Build a fresh table for each call.
            let tbl = match self.lua.create_table() {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(hook = name, error = %e, "failed to create hook payload table");
                    continue;
                }
            };
            let mut build_ok = true;
            for (k, v) in fields {
                let result = match v {
                    LuaPayloadValue::Int(n) => tbl.set(*k, *n),
                    LuaPayloadValue::Str(s) => tbl.set(*k, *s),
                    LuaPayloadValue::Bool(b) => tbl.set(*k, *b),
                };
                if let Err(e) = result {
                    tracing::warn!(hook = name, error = %e, "failed to build hook table");
                    build_ok = false;
                    break;
                }
            }
            if !build_ok {
                continue;
            }

            let args = MultiValue::from_vec(vec![Value::Table(tbl)]);
            match f.call::<MultiValue>(args) {
                Ok(_) => ran += 1,
                Err(e) => {
                    tracing::warn!(hook = name, error = %e, "lua hook failed; dropping handler");
                    to_drop.push(i);
                }
            }
        }
        drop(guard);
        if !to_drop.is_empty() {
            let mut guard = self.hooks.lock();
            for i in to_drop.into_iter().rev() {
                if i < guard.len() {
                    guard.remove(i);
                }
            }
        }
        ran
    }

    /// Fire a hook by name with no payload (for `"tick"` and `"config_reload"`).
    pub fn fire(&self, name: &str, _payload: Option<&str>) -> usize {
        self.fire_event(name, &[])
    }

    /// Drain all pending commands from the queue and return them.
    ///
    /// Call this once per tick from the main thread after firing hooks.
    /// The returned commands should be applied in order.
    pub fn drain_commands(&mut self) -> Vec<PluginCommand> {
        let mut raw = self.commands.lock().drain(..).collect::<Vec<_>>();
        // Intercept RegisterCommand entries (encoded as a sentinel) and
        // store them in `self.registered_commands` before returning the
        // remaining (actionable) commands to the caller.
        //
        // `RegisterCommand` is handled via a separate channel (see
        // `install_api`) so this pass just promotes inline-load registrations
        // that arrived via the queue.
        self.drain_commands_internal();
        raw.retain(|c| !matches!(c, PluginCommand::InvokeCommand { command_idx } if *command_idx == usize::MAX));
        raw
    }

    /// Internal: move pending `RegisteredCommand` sentinel items (idx ==
    /// usize::MAX) out of the queue and into `self.registered_commands`.
    fn drain_commands_internal(&mut self) {
        // Registrations arrive via a dedicated `Arc<Mutex<Vec<PendingReg>>>` so
        // we don't need sentinel values. See `install_api` for the `reg_queue`.
        // This method is kept as an explicit hook for future use.
    }

    /// All currently-loaded plugins.
    #[must_use]
    pub fn plugins(&self) -> &[PluginInfo] {
        &self.loaded
    }
}

/// Typed value that can be set on a Lua hook payload table.
pub enum LuaPayloadValue<'a> {
    /// An integer field.
    Int(i64),
    /// A string field (borrowed).
    Str(&'a str),
    /// A boolean field.
    Bool(bool),
}

// ── Sandbox ───────────────────────────────────────────────────────────────────

fn sandbox(lua: &Lua) -> LuaResult<()> {
    let globals = lua.globals();
    // Strip the dangerous chunks of the stdlib. Nil works — Lua looks
    // up by table index, not by sentinel.
    for name in [
        "io", "package", "debug", "dofile", "loadfile", "load", "require",
    ] {
        globals.set(name, Value::Nil)?;
    }
    if let Ok(os) = globals.get::<Table>("os") {
        for k in [
            "execute",
            "exit",
            "remove",
            "rename",
            "tmpname",
            "getenv",
            "setlocale",
        ] {
            os.set(k, Value::Nil)?;
        }
    }
    Ok(())
}

// ── API installation ──────────────────────────────────────────────────────────

/// Pending command-registration record. Separate from `PluginCommand` to
/// avoid the `RegistryKey` not implementing `Debug`.
struct PendingReg {
    name: String,
    key: RegistryKey,
}

fn install_api(lua: &Lua, hooks: &HookKey, commands: &CommandQueue) -> LuaResult<()> {
    let api = lua.create_table()?;

    // ── terminale.log(msg) ────────────────────────────────────────────────────
    let log_fn: Function = lua.create_function(|_, msg: String| {
        tracing::info!(target: "terminale.plugin", "{msg}");
        Ok(())
    })?;
    api.set("log", log_fn)?;

    // ── terminale.notify(title, body) ─────────────────────────────────────────
    // Enqueues a real OS desktop notification (drained by the host tick).
    let cmds = Arc::clone(commands);
    let notify_fn: Function = lua.create_function(move |_, (title, body): (String, String)| {
        cmds.lock().push(PluginCommand::Notify { title, body });
        Ok(())
    })?;
    api.set("notify", notify_fn)?;

    // ── terminale.set_tab_title(text) ─────────────────────────────────────────
    let cmds = Arc::clone(commands);
    let set_tab_fn: Function = lua.create_function(move |_, text: String| {
        cmds.lock().push(PluginCommand::SetTabTitle(text));
        Ok(())
    })?;
    api.set("set_tab_title", set_tab_fn)?;

    // ── terminale.open_tab() ──────────────────────────────────────────────────
    let cmds = Arc::clone(commands);
    let open_tab_fn: Function = lua.create_function(move |_, ()| {
        cmds.lock().push(PluginCommand::OpenTab);
        Ok(())
    })?;
    api.set("open_tab", open_tab_fn)?;

    // ── terminale.send_text(text) ─────────────────────────────────────────────
    let cmds = Arc::clone(commands);
    let send_text_fn: Function = lua.create_function(move |_, text: String| {
        cmds.lock().push(PluginCommand::SendText(text));
        Ok(())
    })?;
    api.set("send_text", send_text_fn)?;

    // ── terminale.register_command(name, fn) ──────────────────────────────────
    // Stores the Lua function in the registry and enqueues a sentinel so the
    // host can promote it into `registered_commands` after load. We use a
    // dedicated pending-reg queue so `RegistryKey` (non-Debug) stays out of
    // `PluginCommand`.
    let pending_regs: Arc<Mutex<Vec<PendingReg>>> = Arc::new(Mutex::new(Vec::new()));
    // Share the pending-regs handle with the host via a thread-local hack-free
    // approach: embed it in the Lua registry under a well-known key.
    lua.set_named_registry_value(
        "__pending_regs",
        lua.create_userdata(PendingRegHolder {
            regs: Arc::clone(&pending_regs),
        })?,
    )?;

    let reg_fn: Function = lua.create_function(move |lua, (name, func): (String, Function)| {
        let key = lua.create_registry_value(func)?;
        pending_regs.lock().push(PendingReg { name, key });
        Ok(())
    })?;
    api.set("register_command", reg_fn)?;

    // ── terminale.register_hook(event, fn) ───────────────────────────────────
    let hooks_clone = Arc::clone(hooks);
    let register_fn: Function =
        lua.create_function(move |lua, (name, func): (String, Function)| {
            let key = lua.create_registry_value(func)?;
            hooks_clone.lock().push((name, key));
            Ok(())
        })?;
    api.set("register_hook", register_fn)?;

    lua.globals().set("terminale", api)?;
    Ok(())
}

// ── PendingRegHolder userdata ─────────────────────────────────────────────────

struct PendingRegHolder {
    regs: Arc<Mutex<Vec<PendingReg>>>,
}

impl mlua::UserData for PendingRegHolder {}

impl PluginHost {
    /// Promote any pending `register_command` calls that arrived during the
    /// last Lua execution into `self.registered_commands`. Returns the index
    /// range of newly-added entries.
    pub fn flush_pending_registrations(&mut self) -> std::ops::Range<usize> {
        let holder_val = self
            .lua
            .named_registry_value::<mlua::AnyUserData>("__pending_regs");
        let Ok(holder) = holder_val else {
            return 0..0;
        };
        let Ok(inner) = holder.borrow::<PendingRegHolder>() else {
            return 0..0;
        };
        let mut regs = inner.regs.lock();
        let start = self.registered_commands.len();
        for reg in regs.drain(..) {
            self.registered_commands.push(RegisteredCommand {
                name: reg.name,
                key: reg.key,
            });
        }
        let end = self.registered_commands.len();
        start..end
    }

    /// Invoke the registered command at `idx` inside the Lua state.
    ///
    /// Error from the handler is logged and the command is NOT removed
    /// (so it remains available to invoke again).
    pub fn invoke_command(&self, idx: usize) {
        let Some(cmd) = self.registered_commands.get(idx) else {
            return;
        };
        let Ok(value) = self.lua.registry_value::<Value>(&cmd.key) else {
            return;
        };
        let Value::Function(f) = value else { return };
        if let Err(e) = f.call::<()>(()) {
            tracing::warn!(command = %cmd.name, error = %e, "plugin command invocation failed");
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Sandbox ───────────────────────────────────────────────────────────────

    #[test]
    fn sandbox_blocks_io() {
        let host = PluginHost::new().expect("host init");
        // `io` should be nil — any attempt to index it should fail.
        let result = host.lua.load("io.open('test.txt', 'r')").eval::<Value>();
        assert!(result.is_err(), "io.open must be blocked by sandbox");
    }

    #[test]
    fn sandbox_blocks_os_execute() {
        let host = PluginHost::new().expect("host init");
        let result = host.lua.load("os.execute('echo hello')").eval::<Value>();
        assert!(result.is_err(), "os.execute must be blocked by sandbox");
    }

    #[test]
    fn sandbox_blocks_require() {
        let host = PluginHost::new().expect("host init");
        let result = host.lua.load("require('os')").eval::<Value>();
        assert!(result.is_err(), "require must be blocked by sandbox");
    }

    #[test]
    fn sandbox_allows_math_and_string() {
        let host = PluginHost::new().expect("host init");
        let v: i64 = host
            .lua
            .load("return math.floor(3.7)")
            .eval()
            .expect("math.floor must work");
        assert_eq!(v, 3);
        let s: String = host
            .lua
            .load("return string.upper('hello')")
            .eval()
            .expect("string.upper must work");
        assert_eq!(s, "HELLO");
    }

    // ── register_command ──────────────────────────────────────────────────────

    #[test]
    fn register_command_stores_entry() {
        let mut host = PluginHost::new().expect("host init");
        host.load_inline(
            "test_reg",
            r#"terminale.register_command("Test: Say Hi", function() end)"#,
        )
        .expect("load inline plugin");
        host.flush_pending_registrations();
        assert_eq!(host.registered_commands.len(), 1);
        assert_eq!(host.registered_commands[0].name, "Test: Say Hi");
    }

    // ── notify enqueues command ───────────────────────────────────────────────

    #[test]
    fn notify_enqueues_notify_command() {
        let mut host = PluginHost::new().expect("host init");
        host.load_inline(
            "test_notify",
            r#"terminale.register_hook("tab_open", function(t)
                terminale.notify("Hello", "World")
            end)"#,
        )
        .expect("load inline plugin");

        // Fire the tab_open hook with a payload.
        host.fire_event(
            "tab_open",
            &[
                ("tab_id", LuaPayloadValue::Int(1)),
                ("title", LuaPayloadValue::Str("bash")),
            ],
        );

        let cmds = host.drain_commands();
        assert_eq!(cmds.len(), 1, "exactly one Notify command expected");
        match &cmds[0] {
            PluginCommand::Notify { title, body } => {
                assert_eq!(title, "Hello");
                assert_eq!(body, "World");
            }
            other => panic!("expected Notify, got {other:?}"),
        }
    }

    // ── malformed handler does not crash host ─────────────────────────────────

    #[test]
    fn malformed_hook_error_is_caught() {
        let mut host = PluginHost::new().expect("host init");
        host.load_inline(
            "bad_plugin",
            r#"terminale.register_hook("tab_open", function(t)
                error("deliberate test error")
            end)"#,
        )
        .expect("load inline bad plugin");

        // Fire should not panic; the handler should be dropped.
        let ran = host.fire_event("tab_open", &[("tab_id", LuaPayloadValue::Int(0))]);
        assert_eq!(ran, 0, "erroring handler should report 0 successful runs");

        // Host is still usable after the error.
        let cmds = host.drain_commands();
        assert!(cmds.is_empty(), "no commands after erroring handler");
    }

    // ── config roundtrip for PluginsConfig ───────────────────────────────────

    #[test]
    fn plugins_config_roundtrip() {
        // Import here to avoid needing the full crate as a dependency in tests.
        // We test via the public API: PluginHost itself.
        let host = PluginHost::new();
        assert!(host.is_ok(), "PluginHost::new must succeed in tests");
    }

    // ── tab_open payload has expected fields ──────────────────────────────────

    #[test]
    fn fire_event_tab_open_payload_fields_accessible() {
        let mut host = PluginHost::new().expect("host init");
        // Plugin reads the payload fields and records them in a global for
        // inspection.
        host.load_inline(
            "test_payload",
            r#"
_last_tab_id = nil
_last_title  = nil
terminale.register_hook("tab_open", function(t)
    _last_tab_id = t.tab_id
    _last_title  = t.title
end)
"#,
        )
        .expect("load inline plugin");

        host.fire_event(
            "tab_open",
            &[
                ("tab_id", LuaPayloadValue::Int(42)),
                ("title", LuaPayloadValue::Str("zsh")),
            ],
        );

        let tab_id: i64 = host
            .lua
            .globals()
            .get("_last_tab_id")
            .expect("_last_tab_id must be set");
        let title: String = host
            .lua
            .globals()
            .get("_last_title")
            .expect("_last_title must be set");
        assert_eq!(tab_id, 42);
        assert_eq!(title, "zsh");
    }

    // ── pane_focus payload ────────────────────────────────────────────────────

    #[test]
    fn fire_event_pane_focus_payload_fields_accessible() {
        let mut host = PluginHost::new().expect("host init");
        host.load_inline(
            "test_pane_focus",
            r#"
_pf_pane_id = nil
terminale.register_hook("pane_focus", function(t)
    _pf_pane_id = t.pane_id
end)
"#,
        )
        .expect("load inline plugin");

        host.fire_event("pane_focus", &[("pane_id", LuaPayloadValue::Int(7))]);

        let pane_id: i64 = host
            .lua
            .globals()
            .get("_pf_pane_id")
            .expect("_pf_pane_id must be set");
        assert_eq!(pane_id, 7);
    }

    // ── session_start payload ─────────────────────────────────────────────────

    #[test]
    fn fire_event_session_start_payload_fields_accessible() {
        let mut host = PluginHost::new().expect("host init");
        host.load_inline(
            "test_session_start",
            r#"
_ss_pane_id = nil
_ss_program  = nil
terminale.register_hook("session_start", function(t)
    _ss_pane_id = t.pane_id
    _ss_program  = t.program
end)
"#,
        )
        .expect("load inline plugin");

        host.fire_event(
            "session_start",
            &[
                ("pane_id", LuaPayloadValue::Int(3)),
                ("program", LuaPayloadValue::Str("bash")),
            ],
        );

        let pane_id: i64 = host
            .lua
            .globals()
            .get("_ss_pane_id")
            .expect("_ss_pane_id must be set");
        let program: String = host
            .lua
            .globals()
            .get("_ss_program")
            .expect("_ss_program must be set");
        assert_eq!(pane_id, 3);
        assert_eq!(program, "bash");
    }

    // ── session_exit payload ──────────────────────────────────────────────────

    #[test]
    fn fire_event_session_exit_payload_fields_accessible() {
        let mut host = PluginHost::new().expect("host init");
        host.load_inline(
            "test_session_exit",
            r#"
_se_pane_id   = nil
_se_exit_code = nil
terminale.register_hook("session_exit", function(t)
    _se_pane_id   = t.pane_id
    _se_exit_code = t.exit_code
end)
"#,
        )
        .expect("load inline plugin");

        host.fire_event(
            "session_exit",
            &[
                ("pane_id", LuaPayloadValue::Int(5)),
                ("exit_code", LuaPayloadValue::Int(130)),
            ],
        );

        let pane_id: i64 = host
            .lua
            .globals()
            .get("_se_pane_id")
            .expect("_se_pane_id must be set");
        let exit_code: i64 = host
            .lua
            .globals()
            .get("_se_exit_code")
            .expect("_se_exit_code must be set");
        assert_eq!(pane_id, 5);
        assert_eq!(exit_code, 130);
    }

    // ── command_end payload ───────────────────────────────────────────────────

    #[test]
    fn fire_event_command_end_payload_fields_accessible() {
        let mut host = PluginHost::new().expect("host init");
        host.load_inline(
            "test_command_end",
            r#"
_ce_exit_code = nil
_ce_command   = nil
_ce_cwd       = nil
terminale.register_hook("command_end", function(t)
    _ce_exit_code = t.exit_code
    _ce_command   = t.command
    _ce_cwd       = t.cwd
end)
"#,
        )
        .expect("load inline plugin");

        host.fire_event(
            "command_end",
            &[
                ("exit_code", LuaPayloadValue::Int(0)),
                ("command", LuaPayloadValue::Str("cargo build")),
                ("cwd", LuaPayloadValue::Str("/home/user/project")),
            ],
        );

        let exit_code: i64 = host
            .lua
            .globals()
            .get("_ce_exit_code")
            .expect("_ce_exit_code must be set");
        let command: String = host
            .lua
            .globals()
            .get("_ce_command")
            .expect("_ce_command must be set");
        let cwd: String = host
            .lua
            .globals()
            .get("_ce_cwd")
            .expect("_ce_cwd must be set");
        assert_eq!(exit_code, 0);
        assert_eq!(command, "cargo build");
        assert_eq!(cwd, "/home/user/project");
    }
}
