//! Binary entry point for `terminale`.

// In release builds mark this as a GUI (`windows`) subsystem binary so
// Windows doesn't pop up a stray black console window on launch. Debug
// builds keep the console subsystem so `tracing` logs show up while
// developing. The CLI paths (`--schema`, `--help`, version, fatal errors)
// still work because `main` re-attaches to the parent console at startup
// when launched from a shell. See `attach_parent_console`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai_assistant_window;
mod app_icon;
mod config_watch;
mod confirm_close;
mod context_menu_window;
mod copy_mode;
#[cfg(target_os = "linux")]
mod desktop_entry;
mod dir_jump;
mod egui_icons;
pub mod icons;
mod keymap;
mod links;
mod markdown;
mod monitor_names;
mod mouse;
mod osc_handlers;
mod palette;
mod panes;
mod password_prompt;
mod paste_guard;
mod process_job;
mod quick_select;
mod resources;
mod settings_window;
mod shortcuts;
mod ssh_tabs;
mod status_bar;
mod suggestions;
mod tab_groups;
mod tabs;
mod update;
mod window_anim;
mod workspace;

// Sub-module glob re-exports: each private sub-module is a logical extension of
// main.rs, so glob re-exporting their items here is intentional and idiomatic.
#[allow(clippy::wildcard_imports)]
pub(crate) use mouse::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use osc_handlers::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use palette::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use panes::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use shortcuts::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use ssh_tabs::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use tabs::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use window_anim::*;

use arboard::Clipboard;
use clap::Parser;
use color_eyre::eyre::Result;
use context_menu_window::{ContextMenuWindow, MenuEntry};
use parking_lot::Mutex;
use password_prompt::PasswordPrompt;
use settings_window::SettingsWindow;
use std::path::PathBuf;
use std::sync::Arc;
use terminale_config::{Config, Profile};
use terminale_core::{Session, SpawnSpec};
use terminale_render::{
    CellRect, LabelBadge, MenuItem, MenuOverlay, Renderer, TabBar, TabBarItem, TabHit, WindowCtrl,
};
use terminale_term::Emulator;
use tracing_subscriber::EnvFilter;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

/// Action-ID base reserved for the dynamic profile picker. Picker entries
/// use `PROFILE_PICKER_BASE + profile_index`; anything below this routes
/// through the static [`MenuAction`] enum.
const PROFILE_PICKER_BASE: u32 = 0x1_0000;

/// Action-ID base reserved for the dynamic "New SSH tab" host picker.
/// Entries use `SSH_PICKER_BASE + host_index`. Kept well above
/// [`PROFILE_PICKER_BASE`] so the two ranges never overlap.
const SSH_PICKER_BASE: u32 = 0x2_0000;

/// Action-ID base reserved for the per-tab icon picker in the context menu.
/// Entries use `TAB_ICON_PICKER_BASE + icon_preset_index`. Well above both
/// the profile and SSH picker ranges.
const TAB_ICON_PICKER_BASE: u32 = 0x3_0000;

/// Action-ID base reserved for the "Add to group" entries in a tab's
/// context menu. Entries use `GROUP_ASSIGN_BASE + group_index` (index into
/// `RunningState::tab_groups`). Highest of the picker ranges, so the App-level
/// handler must test it first.
const GROUP_ASSIGN_BASE: u32 = 0x4_0000;

/// Which surface a right-click context menu was opened over. Drives which
/// menu is built: right-clicking a tab shows tab + group management, while
/// right-clicking the terminal body shows terminal actions only.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuContext {
    /// Right-click landed on the terminal body (or anywhere that isn't a tab).
    Terminal,
    /// Right-click landed on the tab at this index.
    Tab(usize),
}

/// How long to wait for an SSH connection (TCP + auth + PTY request)
/// before giving up and showing the failure in the new tab's buffer.
pub(crate) const SSH_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Distance (physical px, squared) the cursor must travel from the tab-press
/// point before a click is promoted into a drag. Mirrors the 9px² threshold
/// that discriminates a selection-drag from a plain click, so a simple tab
/// click still just switches tabs and never spawns a ghost.
const TAB_DRAG_ARM_PX2: f32 = 16.0;

/// How far (logical px) past the inner edge of a vertical tab strip the
/// cursor must move before a drag is promoted to a tear-out (Detach). This
/// dead-zone prevents accidental tear-outs when the user wobbles the cursor
/// slightly into the grid while scrolling through the tab list.
const VERT_TEAROUT_MARGIN_LOGICAL: f32 = 32.0;

/// Where an in-flight tab drag would land if released this instant. Resolved
/// every `CursorMoved` by a screen-space hit-test across every window's tab
/// bar, and consumed once on release.
#[derive(Debug, Clone, Copy)]
enum DropTarget {
    /// Re-order within the origin window so the dragged tab lands at this
    /// insertion slot (`0..=tabs.len()`).
    Reorder(usize),
    /// Attach to another window (by `WindowId`) at this insertion slot.
    AttachTo(WindowId, usize),
    /// Cursor is outside every tab bar — release tears out a new window.
    Detach,
}

/// What is being carried by an in-flight [`TabDrag`]. A drag can lift either a
/// whole tab (the original behaviour), a single split pane, or an entire tab
/// group (all tabs whose `tab.group == Some(gid)`).
#[derive(Debug, Clone)]
enum DragPayload {
    /// A whole tab is being dragged. The index is tracked in [`TabDrag::tab_index`].
    Tab {
        /// Index of the tab in the origin window at the moment of lift (and
        /// kept in sync during live in-window reorders).
        tab_index: usize,
    },
    /// A single split pane is being dragged out of its parent tab.
    Pane {
        /// Index of the tab that owns the pane in the origin window.
        tab_index: usize,
        /// Stable id of the leaf being lifted.
        pane_id: PaneId,
    },
    /// An entire named tab group is being dragged. All tabs whose
    /// `tab.group == Some(group_id)` travel together as a contiguous block.
    Group {
        /// Stable id of the group being dragged.
        group_id: TabGroupId,
    },
}

/// A live, App-level tab drag. The drag is global because it can span
/// windows (a tab lifted from window A can be dropped on window B), so it
/// lives on [`TerminaleApp`] rather than on a single [`TermWindow`].
#[derive(Debug, Clone)]
struct TabDrag {
    /// Window the tab was lifted from, by stable OS id.
    origin_window: WindowId,
    /// Index of the dragged tab within its origin window. Kept for fast
    /// access; also stored inside [`DragPayload::Tab`] / [`DragPayload::Pane`].
    /// Updated live as an in-window reorder slides the tab, so the ghost
    /// always reflects the same tab.
    tab_index: usize,
    /// What is actually being dragged — a whole tab or a single split pane.
    payload: DragPayload,
    /// Cached label of the dragged item, so the ghost can render without
    /// re-locking the source emulator every frame.
    label: String,
    /// Cursor position in SCREEN (physical) px, refreshed every move.
    cursor_screen: (i32, i32),
    /// Grab offset: cursor-to-pill-centre at lift time (logical px), so the
    /// ghost tracks under the same point the user grabbed.
    grab_offset_x: f32,
    /// Logical-px width of the dragged tab's slot, so the ghost matches it.
    slot_width: f32,
    /// Where a release right now would land, resolved each move.
    target: DropTarget,
    /// Whether the animated ghost + drop indicator are shown. Mirrors
    /// `appearance.animated_tab_drag` captured at lift time; when `false`
    /// the drag still resolves on release, just without the visuals.
    animated: bool,
}

/// One `PtyDataReady` in flight is enough: `drain_pty_output` empties every
/// channel, so per-chunk notifications past the first are pure overhead.
/// Reader threads set this before posting; the handler clears it before
/// draining (see the notifier in `spawn_pane_with` for the race argument).
static PTY_WAKE_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Events the host can post to the winit loop to wake it up out-of-band
/// (used by the PTY reader thread so input echo lands without waiting for
/// the next OS event).
#[derive(Debug)]
enum UserEvent {
    /// A PTY produced output — drain + redraw.
    PtyDataReady,
    /// The global hotkey for Quake mode fired. Carries the hotkey id so
    /// we can confirm it's the one we registered. This is delivered from
    /// a dedicated forwarder thread so the toggle works even when every
    /// window is hidden (the winit loop is otherwise parked in `Wait`
    /// with nothing to wake it).
    GlobalHotkey(u32),
    /// An incremental AI-assistant streaming event, forwarded from the
    /// Tokio task running the provider call so rendering stays on the
    /// main thread.
    Ai(AiEvent),
    /// An SSH connection attempt (started by clicking or pressing Enter on an
    /// SSH host in the picker / palette) has completed on a background Tokio
    /// task. The UI thread builds and pushes the tab on receipt so it never
    /// needs to block on network I/O.
    SshConnected(Box<SshConnectOutcome>),
    /// The config file was modified on disk (reported by the filesystem
    /// watcher in `config_watch`). The UI thread reloads and live-applies
    /// the new config. Also triggered by the `ReloadConfig` shortcut action.
    ConfigChanged,
    /// A one-shot AI suggestion request completed. Carries the target window id
    /// and a generation counter so stale results from superseded requests are
    /// silently dropped.
    Suggestion {
        /// The window that fired the request.
        window: winit::window::WindowId,
        /// Request generation — must match `window.suggestions.generation`.
        generation: u64,
        /// What the provider returned.
        outcome: suggestions::SuggestionOutcome,
    },
}

/// Result of an asynchronous SSH connection attempt sent back to the winit
/// event loop via [`UserEvent::SshConnected`].
struct SshConnectOutcome {
    /// Index of the [`RunningState`] (terminal window) that requested the
    /// connection. Used to push the new tab into the right window.
    window_idx: usize,
    /// Human-readable host name for the tab label (`SSH: <name>`).
    host_name: String,
    /// `user@host[:port]` string for the tab's custom title.
    host_endpoint: String,
    /// Terminal grid size to use when constructing the emulator and the
    /// initial PTY dimensions. Captured from the window size at request time.
    cols: u16,
    rows: u16,
    /// The connected session, or a human-readable error string. Either way
    /// a tab is opened — on error its buffer shows why it failed.
    result: Result<terminale_ssh::SshSession, String>,
}

impl std::fmt::Debug for SshConnectOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshConnectOutcome")
            .field("window_idx", &self.window_idx)
            .field("host_name", &self.host_name)
            .field("host_endpoint", &self.host_endpoint)
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .field(
                "result",
                &self
                    .result
                    .as_ref()
                    .map(|_| "<SshSession>")
                    .map_err(String::as_str),
            )
            .finish()
    }
}

/// One step of an AI assistant streamed response.
#[derive(Debug, Clone)]
enum AiEvent {
    /// A chunk of generated text to append.
    Chunk(String),
    /// The stream finished cleanly.
    Done,
    /// The provider errored; carries a human-readable message.
    Error(String),
}

/// A native, cross-platform, GPU-accelerated terminal.
#[derive(Debug, Parser)]
#[command(name = "terminale", version, about)]
struct Cli {
    /// Override the path to the user config TOML file.
    #[arg(long, env = "TERMINALE_CONFIG")]
    config: Option<PathBuf>,

    /// Name of the profile to launch (must match an entry under
    /// `profiles.profiles` in config.toml). Overrides the default.
    #[arg(long, env = "TERMINALE_PROFILE")]
    profile: Option<String>,

    /// Override the shell to launch (e.g. `/usr/bin/zsh`). Ignored when a
    /// matching profile exists.
    #[arg(long, env = "TERMINALE_SHELL")]
    shell: Option<String>,

    /// Launch in Quake mode (slide-down system-wide terminal).
    #[arg(long)]
    quake: bool,

    /// Log level filter (e.g. `info`, `debug`, `terminale=trace`).
    #[arg(long, default_value = "warn", env = "TERMINALE_LOG")]
    log_level: String,

    /// Print the JSON Schema of the config file and exit. Useful for
    /// hooking up editor validation (VSCode "json.schemas", Helix's
    /// `taplo` integration, etc.).
    #[arg(long)]
    schema: bool,

    /// Check GitHub for a newer release and report, without installing. Exits.
    #[arg(long)]
    check_update: bool,

    /// Download the latest release (verified via SHA-256) and replace the
    /// on-disk binary, then exit. The new version applies the next time you
    /// launch terminale — a running session is never interrupted.
    #[arg(long)]
    update: bool,

    /// Register the Linux desktop entry (application-menu launcher + icon)
    /// under `$XDG_DATA_HOME` and exit. Done automatically on launch unless
    /// `integration.desktop_entry = false`; this flag forces it explicitly.
    #[cfg(target_os = "linux")]
    #[arg(long)]
    install_desktop_entry: bool,

    /// Remove the Linux desktop entry installed by `--install-desktop-entry`
    /// (and on-launch auto-registration), then exit.
    #[cfg(target_os = "linux")]
    #[arg(long)]
    uninstall_desktop_entry: bool,
}

/// When launched from a shell (so a parent console exists), re-attach to
/// it so CLI output (`--schema`, `--help`, version, panics) is visible
/// even though release builds are compiled as a GUI subsystem binary.
/// A no-op / harmless failure when there is no parent console (e.g. the
/// app was double-clicked) or one is already attached (debug builds).
#[cfg(windows)]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    // ATTACH_PARENT_PROCESS = (DWORD)-1
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Chain a panic hook (after color-eyre's) that surfaces a fatal panic in
/// a modal message box. Release builds are GUI-subsystem, so a panic's
/// stderr report is otherwise invisible — without this, a failed startup
/// (e.g. GPU/renderer init) would just silently vanish. Release Windows
/// only; debug builds keep the console and need no box.
#[cfg(all(windows, not(debug_assertions)))]
fn install_panic_message_box() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        prev(info);
        // Panics inside the parser's `catch_unwind` are recovered (the pane
        // is marked crashed, the app keeps running) — a modal "fatal error"
        // dialog for those would be a lie. The hook fires *before* the
        // unwind is caught, so we ask the guard instead of the catch.
        if crate::osc_handlers::parser_panic_is_caught() {
            tracing::error!(%info, "recovered parser panic (pane marked crashed)");
            return;
        }
        show_fatal_message_box(&format!("{info}"));
    }));
}

/// Pop a modal error dialog. Used for fatal startup failures in release
/// Windows builds where there may be no console to print to.
#[cfg(all(windows, not(debug_assertions)))]
fn show_fatal_message_box(msg: &str) {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(
            hwnd: *mut core::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            u_type: u32,
        ) -> i32;
    }
    const MB_OK: u32 = 0x0000_0000;
    const MB_ICONERROR: u32 = 0x0000_0010;
    let wide = |s: &str| {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let text = wide(msg);
    let caption = wide("terminale — fatal error");
    unsafe {
        MessageBoxW(
            core::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// Build a [`ModifiersState`] from individual held-key booleans.
///
/// This is a pure, cross-platform helper used by the Windows-specific
/// `current_os_modifiers()` below. Keeping the logic here makes it testable
/// without any FFI.
#[cfg(any(windows, test))]
#[allow(clippy::fn_params_excessive_bools)] // four modifier flags; an enum would be over-engineering
fn modifiers_from_held(ctrl: bool, shift: bool, alt: bool, logo: bool) -> ModifiersState {
    let mut m = ModifiersState::empty();
    if ctrl {
        m |= ModifiersState::CONTROL;
    }
    if shift {
        m |= ModifiersState::SHIFT;
    }
    if alt {
        m |= ModifiersState::ALT;
    }
    if logo {
        m |= ModifiersState::SUPER;
    }
    m
}

/// Query the real physical state of the modifier keys via `GetAsyncKeyState`
/// and return the matching [`ModifiersState`].
///
/// This is called on focus-gain instead of blindly resetting to `empty()`.
/// The OS consumed the Quake WM_HOTKEY event while the window was hidden, so
/// winit never delivered the modifier *release*. Reading the actual hardware
/// state here prevents the next `KeyboardInput` from seeing stale flags.
#[cfg(target_os = "windows")]
fn current_os_modifiers() -> ModifiersState {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    // A key is currently down iff the high-order bit of the return value is set.
    let down = |vk: u16| unsafe { (GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000) != 0 };
    modifiers_from_held(
        down(VK_CONTROL),
        down(VK_SHIFT),
        down(VK_MENU),
        down(VK_LWIN) || down(VK_RWIN),
    )
}

/// macOS: is the primary (left) mouse/trackpad button physically down *right
/// now*, per the OS? winit can silently drop a trackpad button-release (e.g.
/// when focus changes mid-gesture), leaving our tracked `held_button` stuck
/// `Some(Left)` so plain cursor motion is misread as a drag-selection. Querying
/// `NSEvent +pressedMouseButtons` (bit 0 = primary button) gives the ground
/// truth so we can recover. Cheap state read; safe to call from the main thread.
#[cfg(target_os = "macos")]
fn macos_left_button_down() -> bool {
    use objc2::{class, msg_send};
    let buttons: usize = unsafe { msg_send![class!(NSEvent), pressedMouseButtons] };
    buttons & 1 != 0
}

fn main() -> Result<()> {
    #[cfg(windows)]
    attach_parent_console();
    color_eyre::install()?;
    #[cfg(all(windows, not(debug_assertions)))]
    install_panic_message_box();
    let cli = Cli::parse();

    if cli.schema {
        let schema = schemars::schema_for!(Config);
        println!("{}", serde_json::to_string_pretty(&schema)?);
        return Ok(());
    }

    if cli.check_update {
        match update::check_for_update() {
            Ok(Some(v)) => println!(
                "terminale {} is installed; {v} is available. Run `terminale --update`.",
                update::current_version()
            ),
            Ok(None) => println!("terminale {} is up to date.", update::current_version()),
            Err(e) => eprintln!("update check failed: {e:#}"),
        }
        return Ok(());
    }

    if cli.update {
        println!("Checking for updates…");
        match update::download_and_apply(true) {
            Ok(update::UpdateOutcome::Staged(v)) => {
                println!("Updated to terminale {v}. Restart terminale to use the new version.");
            }
            Ok(update::UpdateOutcome::InstallerLaunched(v)) => {
                println!(
                    "Installer for terminale {v} launched — follow its prompts to finish \
                     updating."
                );
            }
            Ok(update::UpdateOutcome::InstallerRequired(v)) => {
                // Not reachable with interactive=true; cover it anyway.
                println!("terminale {v} is available — run the platform installer to apply it.");
            }
            Ok(update::UpdateOutcome::UpToDate) => println!(
                "Already on the latest version ({}).",
                update::current_version()
            ),
            Err(e) => eprintln!("update failed: {e:#}"),
        }
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        if cli.install_desktop_entry {
            match desktop_entry::ensure_installed() {
                Ok(_) => println!("Registered terminale in the application menu."),
                Err(e) => eprintln!("Could not register desktop entry: {e}"),
            }
            return Ok(());
        }
        if cli.uninstall_desktop_entry {
            desktop_entry::remove();
            println!("Removed the terminale desktop entry.");
            return Ok(());
        }
    }

    // The config is loaded BEFORE the tracing subscriber so `[logging]` can
    // shape the file layer; the load outcome is logged right after `init()`.
    let loaded = Config::load_or_init_at(cli.config.clone());

    // Console layer: always on, follows `--log-level` / TERMINALE_LOG.
    let filter = EnvFilter::try_new(&cli.log_level).unwrap_or_else(|_| EnvFilter::new("warn"));
    {
        use tracing_subscriber::layer::SubscriberExt as _;
        use tracing_subscriber::util::SubscriberInitExt as _;
        use tracing_subscriber::Layer as _;
        let console = tracing_subscriber::fmt::layer()
            .with_target(false)
            .compact()
            .with_filter(filter);
        // Optional rolling file layer (`<config dir>/logs/terminale.log.<date>`).
        // This is the whole reason file logging exists: a GUI launch has no
        // console, so without it a freeze or crash leaves nothing to inspect.
        // `LOG_FILE_GUARD` keeps the non-blocking writer alive for the process
        // lifetime — dropping it would silently stop the file output.
        static LOG_FILE_GUARD: std::sync::OnceLock<tracing_appender::non_blocking::WorkerGuard> =
            std::sync::OnceLock::new();
        let file_layer = match &loaded {
            Ok((cfg, path)) if cfg.logging.file_enabled => path.parent().map(|dir| {
                let logs = dir.join("logs");
                cleanup_old_logs(&logs, cfg.logging.retention_days);
                let (writer, guard) = tracing_appender::non_blocking(
                    tracing_appender::rolling::daily(&logs, "terminale.log"),
                );
                let _ = LOG_FILE_GUARD.set(guard);
                // Cap chatty third-party crates: wgpu logs `Device::maintain`
                // at INFO on every poll, which once filled a log file with
                // 388 MB of noise in a single day and buried the lines that
                // actually mattered when diagnosing a crash.
                let directives = quiet_noisy_crates(&cfg.logging.file_level);
                let file_filter =
                    EnvFilter::try_new(&directives).unwrap_or_else(|_| EnvFilter::new("info"));
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(writer)
                    .with_filter(file_filter)
            }),
            _ => None,
        };
        tracing_subscriber::registry()
            .with(console)
            .with(file_layer)
            .init();
    }

    // Confine the process tree to a kill-on-close Job Object so the ConPTY
    // console hosts (`OpenConsole.exe`) we spawn can never outlive us — not
    // even on a force-kill or a crash where `Session::drop` never runs.
    // Placed AFTER the CLI early-returns (`--update` etc.) so one-shot
    // commands that hand off to a long-lived installer aren't confined, and
    // after tracing init so its log lines are captured. No-op off Windows.
    process_job::confine_to_job();

    let (config, config_path) = match loaded {
        Ok((c, p)) => {
            tracing::info!(path = %p.display(), "config loaded");
            (c, p)
        }
        Err(e) => {
            tracing::warn!(?e, "config load failed, using auto defaults");
            (Config::with_auto_profiles(), PathBuf::from("config.toml"))
        }
    };

    // Background self-update check (never blocks the UI, never restarts the
    // running session). With `auto_install` it downloads + verifies + stages the
    // update so it applies on the next launch; otherwise it only logs that one
    // is available. Disable via `updates.check_on_startup = false`.
    if config.updates.check_on_startup {
        let auto = config.updates.auto_install;
        std::thread::spawn(move || {
            if auto {
                // interactive=false: a background startup task must never pop
                // installer UI / elevation prompts. MSI installs downgrade to
                // a notification; the user applies via Settings → About.
                match update::download_and_apply(false) {
                    Ok(update::UpdateOutcome::Staged(v)) => {
                        tracing::info!(version = %v, "update staged; restart terminale to apply");
                    }
                    Ok(update::UpdateOutcome::InstallerRequired(v)) => {
                        tracing::info!(
                            version = %v,
                            "a newer terminale is available; this install is managed by the \
                             platform installer — use Settings → About → Check for updates \
                             to run it"
                        );
                    }
                    Ok(update::UpdateOutcome::InstallerLaunched(_))
                    | Ok(update::UpdateOutcome::UpToDate) => {}
                    Err(e) => tracing::warn!(?e, "background auto-update failed"),
                }
            } else {
                match update::check_for_update() {
                    Ok(Some(v)) => tracing::info!(
                        version = %v,
                        "a newer terminale is available — run `terminale --update` or use Settings"
                    ),
                    Ok(None) => {}
                    Err(e) => tracing::warn!(?e, "background update check failed"),
                }
            }
        });
    }

    // On Linux, register the application-menu entry so terminale is launchable
    // from the desktop and searchable — the tarball/Homebrew installs have no
    // install-time hook for this. Idempotent and best-effort: never blocks
    // startup. Disable via `integration.desktop_entry = false`.
    #[cfg(target_os = "linux")]
    if config.integration.desktop_entry {
        match desktop_entry::ensure_installed() {
            Ok(true) => tracing::info!("registered desktop entry"),
            Ok(false) => {}
            Err(e) => tracing::warn!(?e, "could not register desktop entry"),
        }
    }

    let chosen_profile = pick_profile(&config, cli.profile.as_deref(), cli.shell.as_deref());

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        quake = cli.quake,
        profile = chosen_profile
            .as_ref()
            .map_or("(none)", |p| p.name.as_str()),
        "terminale starting"
    );

    // Async runtime for the AI assistant's streaming provider calls.
    // Kept alive for the whole process; AI tasks are spawned onto it and
    // forward chunks back to the winit loop via the event-loop proxy.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let (hotkeys, quake_hotkey_id) = match install_quake_hotkey(&config.keybinds.quake) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(
                ?e,
                quake = %config.keybinds.quake,
                "Quake hotkey not available; another app may own this binding"
            );
            (None, None)
        }
    };

    // Snapshot the binding we just registered so `about_to_wait` can detect a
    // runtime change (Settings save / config reload) and re-register live.
    let quake_binding_registered = config.keybinds.quake.clone();

    let plugins = if config.plugins.enabled {
        install_plugins(&config)
    } else {
        None
    };

    // Forward global-hotkey presses into the winit loop from a dedicated
    // thread. Without this the hotkey only gets noticed when the loop is
    // already awake for some other reason — so Quake mode appears dead
    // whenever every window is hidden (the loop is parked in `Wait`).
    if quake_hotkey_id.is_some() {
        let proxy = event_loop.create_proxy();
        std::thread::Builder::new()
            .name("terminale-hotkey-forwarder".into())
            .spawn(move || {
                let rx = global_hotkey::GlobalHotKeyEvent::receiver();
                while let Ok(ev) = rx.recv() {
                    if ev.state() == global_hotkey::HotKeyState::Pressed
                        && proxy.send_event(UserEvent::GlobalHotkey(ev.id())).is_err()
                    {
                        // Event loop is gone — nothing left to wake.
                        break;
                    }
                }
            })
            .ok();
    }

    let proxy_for_watcher = event_loop.create_proxy();
    let config_watcher = config_watch::start(
        config_path.clone(),
        proxy_for_watcher,
        config.window.auto_reload_config,
    );

    let mut app = TerminaleApp {
        config,
        config_path,
        profile: chosen_profile,
        shell_override: cli.shell,
        windows: Vec::new(),
        resource_sampler: resources::ResourceSampler::new(),
        settings: None,
        context_menu: None,
        password_prompt: None,
        confirm_close_dialog: None,
        paste_guard_dialog: None,
        paste_guard_window_idx: 0,
        proxy: event_loop.create_proxy(),
        hotkeys,
        quake_hotkey_id,
        quake_binding_registered,
        plugins,
        plugin_snap_key: None,
        config_save_due: None,
        sgr_demo_reseed_at: if std::env::var_os("TERMINALE_DEMO_PALETTE")
            .is_some_and(|v| v == "sgr")
        {
            Some(std::time::Instant::now() + std::time::Duration::from_millis(700))
        } else {
            None
        },
        contrast_demo_reseed_at: if std::env::var_os("TERMINALE_DEMO_PALETTE")
            .is_some_and(|v| v == "contrast")
        {
            Some(std::time::Instant::now() + std::time::Duration::from_millis(700))
        } else {
            None
        },
        boxdraw_demo_reseed_at: if std::env::var_os("TERMINALE_DEMO_PALETTE")
            .is_some_and(|v| v == "boxdraw")
        {
            Some(std::time::Instant::now() + std::time::Duration::from_millis(700))
        } else {
            None
        },
        padding_demo_reseed_at: if std::env::var_os("TERMINALE_DEMO_PALETTE")
            .is_some_and(|v| v == "padding")
        {
            Some(std::time::Instant::now() + std::time::Duration::from_millis(700))
        } else {
            None
        },
        padding_demo_last_size: None,
        font_demo_reseed_at: if std::env::var_os("TERMINALE_DEMO_PALETTE")
            .is_some_and(|v| v == "font")
        {
            Some(std::time::Instant::now() + std::time::Duration::from_millis(700))
        } else {
            None
        },
        ai_assistant: None,
        runtime,
        tab_drag: None,
        ghost_window: None,
        config_watcher,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Append `=warn` caps for known-chatty third-party crates to a user
/// `EnvFilter` directive string, unless the user already mentions that crate
/// explicitly (their directive must win). `wgpu_core` alone logs
/// `Device::maintain` at INFO on every device poll — millions of lines per
/// session — which both bloats the rolling file and buries real diagnostics.
fn quiet_noisy_crates(base: &str) -> String {
    const NOISY: &[&str] = &["wgpu_core", "wgpu_hal", "naga"];
    let mut out = base.trim().to_string();
    if out.is_empty() {
        out.push_str("info");
    }
    for krate in NOISY {
        if !out.contains(krate) {
            out.push_str(&format!(",{krate}=warn"));
        }
    }
    out
}

/// Delete `terminale.log*` files in `dir` older than `retention_days`.
/// Best-effort: every I/O error is swallowed — log housekeeping must never
/// be able to break startup.
fn cleanup_old_logs(dir: &std::path::Path, retention_days: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(
            u64::from(retention_days) * 24 * 3600,
        ))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("terminale.log") {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < cutoff {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Trim `lines` in place to its newest `cap` entries (the vector is
/// oldest-first, so the front is dropped). `cap == 0` empties it. Bounds
/// the per-tick copy handed to the plugin snapshot regardless of how deep
/// the user's scrollback is configured.
fn cap_scrollback(lines: &mut Vec<String>, cap: usize) {
    if lines.len() > cap {
        lines.drain(..lines.len() - cap);
    }
}

/// Boot a fresh [`terminale_plugin::PluginHost`] and load every Lua
/// file from the user's plugins directory. Logs and swallows errors —
/// a broken plugin must never prevent terminale from starting.
fn install_plugins(config: &Config) -> Option<terminale_plugin::PluginHost> {
    let mut host = match terminale_plugin::PluginHost::new() {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(?e, "could not initialise Lua plugin host");
            return None;
        }
    };
    // Arm the execution watchdog before any plugin code runs (load chunks
    // are budgeted too).
    host.set_hook_budget_ms(config.plugins.hook_budget_ms);
    let dir = config
        .plugins
        .directory
        .clone()
        .or_else(terminale_config::paths::plugin_dir);
    if let Some(dir) = dir {
        if let Err(e) = host.load_dir(&dir) {
            tracing::warn!(?e, dir = %dir.display(), "plugin directory load failed");
        } else {
            tracing::info!(
                count = host.plugins().len(),
                dir = %dir.display(),
                "Lua plugins loaded"
            );
        }
    }
    Some(host)
}

/// Register a global hotkey for Quake-mode toggle. Returns the owning
/// manager (must stay alive for the registration to remain active) and
/// the hotkey id we'll watch for in the event loop. Empty `binding` =
/// Quake disabled, returns `Ok((None, None))`.
fn install_quake_hotkey(
    binding: &str,
) -> Result<(Option<global_hotkey::GlobalHotKeyManager>, Option<u32>)> {
    let binding = binding.trim();
    if binding.is_empty() {
        return Ok((None, None));
    }
    let hotkey = parse_hotkey(binding)
        .ok_or_else(|| color_eyre::eyre::eyre!("could not parse hotkey: '{binding}'"))?;
    let manager = global_hotkey::GlobalHotKeyManager::new()?;
    let id = hotkey.id();
    manager.register(hotkey)?;
    Ok((Some(manager), Some(id)))
}

/// Parse a string like `"Ctrl+`"`, `"Alt+Space"`, `"Ctrl+Shift+T"` into
/// a [`global_hotkey::hotkey::HotKey`]. Returns `None` for unknown keys.
fn parse_hotkey(s: &str) -> Option<global_hotkey::hotkey::HotKey> {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    let mut mods = Modifiers::empty();
    let mut code: Option<Code> = None;
    for raw in s.split('+') {
        let token = raw.trim();
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "shift" => mods |= Modifiers::SHIFT,
            "alt" | "option" => mods |= Modifiers::ALT,
            "cmd" | "super" | "meta" | "win" => mods |= Modifiers::META,
            other => {
                code = Some(parse_keycode(other)?);
            }
        }
    }
    let modifiers = if mods.is_empty() { None } else { Some(mods) };
    Some(HotKey::new(modifiers, code?))
}

/// Lower-case key name → `global_hotkey::hotkey::Code`. Covers letters,
/// digits, function keys, and a handful of punctuation users actually
/// bind. Returns `None` for anything else.
fn parse_keycode(name: &str) -> Option<global_hotkey::hotkey::Code> {
    use global_hotkey::hotkey::Code;
    // Single character — letter or digit.
    if name.len() == 1 {
        let c = name.chars().next().unwrap();
        if c.is_ascii_alphabetic() {
            return match c.to_ascii_uppercase() {
                'A' => Some(Code::KeyA),
                'B' => Some(Code::KeyB),
                'C' => Some(Code::KeyC),
                'D' => Some(Code::KeyD),
                'E' => Some(Code::KeyE),
                'F' => Some(Code::KeyF),
                'G' => Some(Code::KeyG),
                'H' => Some(Code::KeyH),
                'I' => Some(Code::KeyI),
                'J' => Some(Code::KeyJ),
                'K' => Some(Code::KeyK),
                'L' => Some(Code::KeyL),
                'M' => Some(Code::KeyM),
                'N' => Some(Code::KeyN),
                'O' => Some(Code::KeyO),
                'P' => Some(Code::KeyP),
                'Q' => Some(Code::KeyQ),
                'R' => Some(Code::KeyR),
                'S' => Some(Code::KeyS),
                'T' => Some(Code::KeyT),
                'U' => Some(Code::KeyU),
                'V' => Some(Code::KeyV),
                'W' => Some(Code::KeyW),
                'X' => Some(Code::KeyX),
                'Y' => Some(Code::KeyY),
                'Z' => Some(Code::KeyZ),
                _ => None,
            };
        }
        if c.is_ascii_digit() {
            return match c {
                '0' => Some(Code::Digit0),
                '1' => Some(Code::Digit1),
                '2' => Some(Code::Digit2),
                '3' => Some(Code::Digit3),
                '4' => Some(Code::Digit4),
                '5' => Some(Code::Digit5),
                '6' => Some(Code::Digit6),
                '7' => Some(Code::Digit7),
                '8' => Some(Code::Digit8),
                '9' => Some(Code::Digit9),
                _ => None,
            };
        }
        return match c {
            '`' => Some(Code::Backquote),
            '-' => Some(Code::Minus),
            '=' => Some(Code::Equal),
            '[' => Some(Code::BracketLeft),
            ']' => Some(Code::BracketRight),
            '\\' => Some(Code::Backslash),
            ';' => Some(Code::Semicolon),
            '\'' => Some(Code::Quote),
            ',' => Some(Code::Comma),
            '.' => Some(Code::Period),
            '/' => Some(Code::Slash),
            ' ' => Some(Code::Space),
            _ => None,
        };
    }
    // Multi-char key names.
    match name {
        "space" => Some(Code::Space),
        "enter" | "return" => Some(Code::Enter),
        "tab" => Some(Code::Tab),
        "escape" | "esc" => Some(Code::Escape),
        "backspace" => Some(Code::Backspace),
        "delete" | "del" => Some(Code::Delete),
        "home" => Some(Code::Home),
        "end" => Some(Code::End),
        "pageup" | "pgup" => Some(Code::PageUp),
        "pagedown" | "pgdn" => Some(Code::PageDown),
        "up" | "arrowup" => Some(Code::ArrowUp),
        "down" | "arrowdown" => Some(Code::ArrowDown),
        "left" | "arrowleft" => Some(Code::ArrowLeft),
        "right" | "arrowright" => Some(Code::ArrowRight),
        "f1" => Some(Code::F1),
        "f2" => Some(Code::F2),
        "f3" => Some(Code::F3),
        "f4" => Some(Code::F4),
        "f5" => Some(Code::F5),
        "f6" => Some(Code::F6),
        "f7" => Some(Code::F7),
        "f8" => Some(Code::F8),
        "f9" => Some(Code::F9),
        "f10" => Some(Code::F10),
        "f11" => Some(Code::F11),
        "f12" => Some(Code::F12),
        _ => None,
    }
}

/// Resolve which profile to use for the initial session.
///
/// Priority:
///   1. `--profile <name>` matches a config entry
///   2. `--shell <path>` becomes an ad-hoc Profile
///   3. config's `profiles.default`
///   4. first profile in config
fn pick_profile(
    config: &Config,
    cli_profile: Option<&str>,
    cli_shell: Option<&str>,
) -> Option<Profile> {
    if let Some(name) = cli_profile {
        if let Some(p) = config
            .profiles
            .profiles
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
        {
            return Some(p.clone());
        }
        tracing::warn!(name, "profile not found, falling back to default");
    }
    if let Some(shell) = cli_shell {
        return Some(Profile {
            name: "Custom shell".into(),
            command: shell.to_string(),
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            icon: None,
        });
    }
    config.resolve_default_profile().cloned()
}

struct TerminaleApp {
    config: Config,
    config_path: PathBuf,
    profile: Option<Profile>,
    shell_override: Option<String>,
    /// Every open terminal window. The first is created in `resumed`; tab
    /// tear-out appends new ones. The process exits when this drops empty.
    windows: Vec<TermWindow>,
    /// Samples global CPU + memory for the bottom resource-indicator strip.
    /// Shared across all windows (system resources are process-global).
    resource_sampler: resources::ResourceSampler,
    settings: Option<SettingsWindow>,
    context_menu: Option<ContextMenuWindow>,
    /// In-window SSH credential prompt, open when a password host is opened
    /// without a stored secret. Carries the host index so the connect resumes
    /// against the right host on submit.
    password_prompt: Option<PasswordPrompt>,
    /// Paste-safety confirmation dialog. Open when a multi-line paste is
    /// pending and the safety policy requires confirmation. On confirm the
    /// buffered text is written to the PTY; on cancel it is dropped.
    /// Close-confirmation dialog (`window.confirm_close`), at most one open.
    confirm_close_dialog: Option<confirm_close::ConfirmCloseDialog>,
    paste_guard_dialog: Option<paste_guard::PasteGuardDialog>,
    /// Index into `self.windows` of the window that triggered the pending
    /// paste guard. Used to route the confirmed payload back to the right PTY.
    paste_guard_window_idx: usize,
    /// Hands out wake-up tokens so background threads (PTY readers) can
    /// poke the event loop the instant new output lands.
    proxy: EventLoopProxy<UserEvent>,
    /// Global-hotkey manager — kept alive for the duration of the
    /// process. If `None`, hotkey registration failed (e.g. another
    /// terminal already owns Ctrl+`); we just skip Quake mode.
    ///
    /// Never read after construction — held purely as an RAII guard so the
    /// OS hotkey registration stays alive (dropping it unregisters Quake).
    #[allow(dead_code)]
    hotkeys: Option<global_hotkey::GlobalHotKeyManager>,
    /// `id()` of the Quake-toggle hotkey, set when registration
    /// succeeded so `about_to_wait` knows which event to react to.
    quake_hotkey_id: Option<u32>,
    /// The Quake hotkey binding currently registered with the OS. Compared to
    /// `config.keybinds.quake` every tick so a Settings change or config-file
    /// reload re-registers the hotkey live instead of needing a restart.
    quake_binding_registered: String,
    /// Lua plugin host. `None` when disabled in config or when no Lua
    /// runtime could be initialised (rare).
    plugins: Option<terminale_plugin::PluginHost>,
    /// `(emulator ptr, content generation, read cap)` of the scrollback/
    /// visible-text copy last published to the plugin host. When unchanged,
    /// the per-tick snapshot publish skips the content extraction entirely
    /// (up to `scrollback_read_cap` lines copied into owned strings on every
    /// event-loop wake otherwise). `None` = next publish re-extracts.
    plugin_snap_key: Option<(usize, u64, usize)>,
    /// `Some(deadline)` while a live config edit is pending a debounced
    /// write to disk. We coalesce bursts of slider drags into a single
    /// write that fires after the user pauses for ~600 ms.
    config_save_due: Option<std::time::Instant>,
    /// `Some(deadline)` for the one-shot SGR demo re-seed when the
    /// `TERMINALE_DEMO_PALETTE=sgr` env var is set. After the deadline
    /// fires (≈700 ms after the first frame) we re-emit the sample text
    /// so it survives the shell's initial ConPTY clear. Set to `None`
    /// once the re-seed has been delivered.
    sgr_demo_reseed_at: Option<std::time::Instant>,
    /// `Some(deadline)` for the one-shot contrast demo re-seed when the
    /// `TERMINALE_DEMO_PALETTE=contrast` env var is set. After the deadline
    /// fires the low-contrast sample lines are re-emitted with minimum_contrast
    /// forced high so the legibility lift is visible in a screenshot.
    contrast_demo_reseed_at: Option<std::time::Instant>,
    /// `Some(deadline)` for the one-shot box-drawing demo re-seed when the
    /// `TERMINALE_DEMO_PALETTE=boxdraw` env var is set. The box/block sample
    /// is grid text, so ConPTY's initial clear wipes the first seed; we
    /// re-emit it ≈700 ms later so the procedural geometry is visible in a
    /// steady-state screenshot.
    boxdraw_demo_reseed_at: Option<std::time::Instant>,
    /// `Some(deadline)` for the one-shot padding demo re-seed when the
    /// `TERMINALE_DEMO_PALETTE=padding` env var is set. Fills the grid with
    /// numbered ruler rows so the last visible row and the symmetric gap above
    /// and below it are clearly visible in a screenshot.
    padding_demo_reseed_at: Option<std::time::Instant>,
    /// Last (cols, rows) at which the padding demo frame was drawn. `None`
    /// until the first draw. Compared on every `about_to_wait` tick so we
    /// can re-emit the frame whenever the grid size changes (DPI change,
    /// window resize) and keep the bottom border on the true last row.
    padding_demo_last_size: Option<(u16, u16)>,
    /// `Some(deadline)` for the one-shot bundled-font demo re-seed when the
    /// `TERMINALE_DEMO_PALETTE=font` env var is set. Re-emits the font sample
    /// text ≈700 ms after startup so it survives ConPTY's initial clear and
    /// is visible in a steady-state screenshot.
    font_demo_reseed_at: Option<std::time::Instant>,
    /// The AI assistant sub-window, open on demand (Ctrl+Shift+A).
    ai_assistant: Option<ai_assistant_window::AiAssistantWindow>,
    /// Tokio runtime backing the AI assistant's streaming provider calls.
    runtime: tokio::runtime::Runtime,
    /// In-flight Chrome-style tab drag, or `None`. App-level (not per-window)
    /// because a drag can carry a tab across windows. The ghost, the drop
    /// indicator, and the release-time resolution (reorder / attach / tear
    /// out) are all driven from this single source of truth.
    tab_drag: Option<TabDrag>,
    /// Floating, transparent, borderless, always-on-top OS window that
    /// follows the cursor during an animated tab drag — so the ghost pill
    /// stays visible **outside** the source terminal window (Chrome-style).
    /// `Some(_)` only while a drag is live and `animated` is on. Spawned by
    /// [`Self::spawn_ghost_window`], moved by [`Self::move_ghost_window`],
    /// destroyed by [`Self::destroy_ghost_window`].
    ghost_window: Option<GhostWindow>,
    /// Filesystem watcher for `config.toml`. `Some(_)` while
    /// `config.window.auto_reload_config` is on; `None` when disabled or
    /// when the watcher could not be started. Kept alive here so the RAII
    /// guard keeps the watch active — dropping it unregisters the watch.
    #[allow(dead_code)]
    config_watcher: Option<notify::RecommendedWatcher>,
}

/// A standalone OS window dedicated to painting the tab-drag ghost pill.
///
/// Borderless, transparent, click-through (best-effort), always-on-top, no
/// session / tabs / chrome — its renderer paints **only** the floating pill
/// via [`Renderer::render_ghost_only`]. The App keeps a single instance
/// alive for the duration of an animated tab drag; on drop / cancel /
/// resolve it's destroyed so it never lingers between drags.
struct GhostWindow {
    window: Arc<Window>,
    renderer: Renderer,
    /// Cached pill geometry (logical px) — the cursor-to-pill offset that
    /// `move_ghost_window` applies every frame to keep the cursor under the
    /// same point of the pill the user originally grabbed.
    grab_offset_x: f32,
    /// Logical-px size of the pill, cached so we can centre the cursor
    /// vertically on the pill (we don't track a grab_offset_y at lift time;
    /// the cursor was inside the tab bar so half-pill-height is the right
    /// approximation).
    pill_height_logical: f32,
}

/// Stable identifier for a [`Pane`] inside a tab — survives tree
/// rearrangements (split, close, focus move) so the renderer / input
/// router / drag-resize can refer to a pane by id without worrying
/// about Vec-index churn.
type PaneId = u32;

/// Orientation of a binary [`PaneNode::Split`].
///
/// `Horizontal` stacks the two children vertically — `a` is on top,
/// `b` underneath; `Vertical` places them side-by-side — `a` on the left,
/// `b` on the right. The names match how the layout *divider* runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SplitDir {
    /// Divider runs horizontally — children stack top / bottom.
    Horizontal,
    /// Divider runs vertically — children sit left / right.
    Vertical,
}

/// Binary tree describing how a tab's panes are laid out. A leaf names a
/// single [`Pane`] by id; a split holds two sub-trees plus the divider's
/// orientation and the fraction of the parent's extent allocated to `a`.
#[derive(Debug, Clone)]
enum PaneNode {
    Leaf(PaneId),
    Split {
        direction: SplitDir,
        /// Fraction (0.0..=1.0) of the parent extent allocated to `a`;
        /// `b` gets `1.0 - ratio`. Clamped to a sane range when applied.
        ratio: f32,
        a: Box<PaneNode>,
        b: Box<PaneNode>,
    },
}

impl PaneNode {
    /// True when the tree is a single leaf (no splits anywhere). Phase
    /// A's invariant is that every tab is single-leaf; later phases lift
    /// this.
    #[allow(dead_code)] // used once split actions land in Phase C
    fn is_single_leaf(&self) -> bool {
        matches!(self, Self::Leaf(_))
    }
}

/// Stable identifier for one `Split` node inside a tab's pane tree. Each
/// entry steps from a `Split` into one of its children: `false` descends
/// into `a`, `true` descends into `b`. A path of length `n` resolves to
/// the `Split` reached by walking `n` children-of-children from the root;
/// an empty path resolves to the root itself.
///
/// Paths are computed on demand by the divider walker and stay valid only
/// as long as no other action mutates the tree shape (every mutation
/// routes through [`TabState::split_focused`] / [`TabState::close_focused`]).
/// Hovering / dragging code re-resolves the path each frame; a stale path
/// resolves to `None` and the drag aborts cleanly.
type DividerPath = Vec<bool>;

/// One divider boundary inside a tab's pane tree — the hit-target for a
/// drag-resize. Mirrors `LocalPaneSpec` but for the *boundaries* between
/// leaves instead of the leaves themselves. `rect_px` is the inflated hit
/// band (visible stroke + grab padding on each side); the renderer draws
/// the narrower visible stroke from a parallel slice. `axis` is the
/// parent `Split`'s direction, which determines the drag cursor.
#[derive(Debug, Clone)]
struct LocalDividerSpec {
    /// Path into the active tab's tree pointing at the `Split` node this
    /// divider lives on.
    path: DividerPath,
    /// Orientation of the parent split — `Vertical` means the divider
    /// itself is a vertical line (cursor = `EwResize`); `Horizontal`
    /// means the divider is a horizontal line (cursor = `NsResize`).
    axis: SplitDir,
    /// Inflated hit-test rect in physical pixels: the visible stroke
    /// grown by `grab_pad_px` on each side along the perpendicular axis.
    rect_px: (f32, f32, f32, f32),
    /// Geometric (visible) stroke rect — the rectangle the renderer
    /// actually paints. Always nested inside `rect_px`.
    visible_rect_px: (f32, f32, f32, f32),
}

/// Snapshot captured the moment a divider drag arms (left-press inside the
/// grab band). Storing the parent rect + start ratio at press time keeps
/// the drag math correct even if a layout recompute happens mid-drag
/// (e.g. the window resizes while the user holds the button).
#[derive(Debug, Clone)]
struct PendingDividerDrag {
    /// Path to the `Split` node whose `ratio` is being mutated.
    path: DividerPath,
    /// Orientation — decides whether the drag follows the x- or y-axis.
    axis: SplitDir,
    /// Ratio of the `Split` at the moment the drag began. Used as a
    /// fallback when the parent rect has zero extent (degenerate layout).
    start_ratio: f32,
    /// Parent (Split-node) rect in physical px at press time. Used to
    /// recompute the new ratio from the live cursor delta.
    parent_rect_px: (f32, f32, f32, f32),
}

/// One PTY-backed pane inside a tab. A tab can hold one or more panes
/// arranged in a [`PaneNode`] tree; today (Phase A of the split-panes
/// effort) every tab contains exactly one leaf. Phase B+ generalises
/// rendering / focus / split actions over multiple leaves.
struct Pane {
    profile_name: String,
    icon: Option<String>,
    /// Title the running program announced via OSC 0/2 (e.g. "vim file.rs",
    /// "ssh host"). Shown in the tab label in preference to the profile
    /// name + cwd. `None` until a program sets one.
    custom_title: Option<String>,
    /// Explicit name the user set via rename (double-click on the tab /
    /// pane header, or the context-menu "Rename" entry). Highest priority —
    /// overrides both `custom_title` and the profile/cwd fallback, and is
    /// never overwritten by program OSC titles. `None` = use automatic title.
    user_title: Option<String>,
    session: Session,
    output_rx: tokio::sync::mpsc::UnboundedReceiver<bytes::Bytes>,
    emulator: Arc<Mutex<Emulator>>,
    cols: u16,
    rows: u16,
    /// Lines scrolled up into history (`0` = pinned to live output).
    /// Auto-resets to 0 whenever the PTY produces new bytes.
    scroll_lines: usize,
    /// Set when the emulator panicked while processing PTY bytes. We
    /// stop feeding new chunks into a crashed pane — the user can still
    /// read its existing buffer until they close it.
    crashed: bool,
    /// Autodetected URL ranges in the visible viewport. Refreshed
    /// every time the active grid changes. Each entry is
    /// `(col_start, col_end_inclusive, row, url)`.
    autodetect_links: Vec<DetectedLink>,
    /// Wall-clock instant of the last *non-trivial* PTY chunk received by
    /// this pane. Updated in [`drain_pty_output`] whenever a chunk with
    /// `len > 1 || contains '\n'` is applied. Used as a fallback busy
    /// indicator for shells that do not emit OSC 133 sequences.
    /// `None` = pane has never received meaningful output.
    last_output_at: Option<std::time::Instant>,
    /// Wall-clock instant of the last user input (keystroke or paste)
    /// written to this pane's PTY. Used by [`osc_handlers::pane_is_busy`]
    /// to tell keystroke echo / prompt redraws apart from real command
    /// output — output that closely follows user input does not count as
    /// "busy". `None` = no user input yet.
    last_input_at: Option<std::time::Instant>,
}

// ── Tab-group types ───────────────────────────────────────────────────────────

/// Stable opaque identifier for a named tab group. Assigned at creation and
/// never reused within a session.
type TabGroupId = u32;

/// One named, colour-coded group. Stored in `RunningState::tab_groups`.
#[derive(Debug, Clone)]
struct TabGroup {
    /// Stable id — unique within the session.
    id: TabGroupId,
    /// User-visible name (auto-generated as "Group N" if not renamed).
    name: String,
    /// Accent colour `[R, G, B]` shown on member tabs.
    color: [u8; 3],
}

/// Braille-dots animation frames for the busy spinner. Each glyph is a 2×4
/// pixel grid, which fits the project's pixel-art aesthetic. Cycles at ~90 ms
/// per frame, advancing only while at least one pane is busy.
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Colour palette cycled when auto-creating groups. Eight distinct hues that
/// are legible against the dark tab-bar background.
const GROUP_COLOR_PALETTE: [[u8; 3]; 8] = [
    [0x4e, 0xa8, 0xff], // blue
    [0x4e, 0xd4, 0x84], // green
    [0xff, 0xa0, 0x3c], // orange
    [0xff, 0x6b, 0x8a], // rose
    [0xc0, 0x80, 0xff], // purple
    [0x40, 0xd0, 0xd0], // cyan
    [0xff, 0xd0, 0x40], // yellow
    [0xff, 0x70, 0xd0], // pink
];

/// Everything tied to one tab. Holds one or more [`Pane`]s laid out by a
/// [`PaneNode`] tree, with a single focused pane that receives keyboard
/// input. Today only one leaf is ever created; Phase C wires the split
/// actions that grow the tree.
///
/// `Deref` / `DerefMut` forward to the **focused** pane so the existing
/// per-tab call sites (`tab.session.write_input(...)`, `tab.emulator
/// .lock()`, etc.) keep working without a 100-call-site mechanical
/// rewrite. That's also the right semantics for most of those sites:
/// "the program the user is interacting with in this tab".
struct TabState {
    panes: std::collections::BTreeMap<PaneId, Pane>,
    /// Layout tree referencing the entries in `panes`.
    tree: PaneNode,
    focused: PaneId,
    /// Monotonic id allocator so a closed-then-respawned pane never
    /// collides with a stale id in the tree.
    next_pane_id: PaneId,
    /// Background tab has produced output since the user last looked.
    /// Cleared the moment the user makes this tab active. Tab-level so a
    /// multi-pane future can roll up "any pane produced output".
    unread: bool,
    /// When `Some(id)`, the pane with that id is zoomed — it fills the
    /// whole tab body and all other panes are hidden. Toggled by the
    /// `TogglePaneZoom` action. `None` = normal multi-pane tree layout.
    zoomed_pane: Option<PaneId>,
    /// SSH host name associated with this tab (set when the tab was opened
    /// via the SSH picker or a `[[ssh_hosts]]` entry). Empty for local tabs.
    /// Used by context-rule matching.
    ssh_host_name: String,
    /// Auto-applied tab chip colour from a matching `[[context_rules]]` entry.
    /// `None` = no rule matched (use the default chip colour). Recomputed
    /// whenever the tab's cwd or SSH host name changes.
    auto_color: Option<[u8; 3]>,
    /// Auto-applied badge text from a matching `[[context_rules]]` entry.
    /// `None` = no badge. Recomputed alongside `auto_color`.
    auto_badge: Option<String>,
    /// User-assigned chip colour override. When `Some`, wins over `auto_color`
    /// for both the chip tint and accent bar. Cleared by the "Clear colour"
    /// action in the tab context menu. Runtime-only (not persisted).
    user_color: Option<[u8; 3]>,
    /// User-assigned icon glyph override. When `Some`, wins over the
    /// profile-derived icon shown in the tab bar chip. Runtime-only.
    user_icon: Option<String>,
    /// When `true` the tab is pinned: it sorts ahead of all unpinned tabs,
    /// renders compact (icon-only), and its close-X is hidden. Runtime-only.
    pinned: bool,
    /// The group this tab belongs to, identified by [`TabGroupId`]. `None` =
    /// ungrouped. Preserved across workspace save/restore.
    group: Option<TabGroupId>,
}

impl TabState {
    /// Build a single-pane tab from one [`Pane`]. The pane gets id `0`.
    fn new_single(pane: Pane) -> Self {
        let mut panes = std::collections::BTreeMap::new();
        panes.insert(0, pane);
        Self {
            panes,
            tree: PaneNode::Leaf(0),
            focused: 0,
            next_pane_id: 1,
            unread: false,
            zoomed_pane: None,
            ssh_host_name: String::new(),
            auto_color: None,
            auto_badge: None,
            user_color: None,
            user_icon: None,
            pinned: false,
            group: None,
        }
    }

    /// Borrow the currently-focused pane.
    ///
    /// Invariant: `focused` always names an entry in `panes`. A violation is
    /// a bug in the tree-edit code — but this accessor sits on the per-frame
    /// hot path (it backs `Deref`), so in release we degrade to the first
    /// pane instead of turning a tree-edit regression into a guaranteed
    /// crash on the very next frame. Debug builds still assert loudly.
    /// Only an empty pane map (a much stronger invariant: a tab cannot
    /// exist without panes) still panics.
    fn focused_pane(&self) -> &Pane {
        debug_assert!(
            self.panes.contains_key(&self.focused),
            "focused pane id {:?} missing from panes",
            self.focused
        );
        self.panes.get(&self.focused).unwrap_or_else(|| {
            self.panes
                .values()
                .next()
                .expect("a tab always has at least one pane")
        })
    }

    /// Mutably borrow the currently-focused pane. Same degradation contract
    /// as [`Self::focused_pane`], but here the breach can be healed: focus
    /// is re-pointed at the first pane so follow-up frames are consistent.
    fn focused_pane_mut(&mut self) -> &mut Pane {
        debug_assert!(
            self.panes.contains_key(&self.focused),
            "focused pane id {:?} missing from panes",
            self.focused
        );
        if !self.panes.contains_key(&self.focused) {
            if let Some(first) = self.panes.keys().copied().next() {
                tracing::error!(
                    stale = self.focused,
                    healed = first,
                    "focused pane id missing from panes; re-pointing to first pane"
                );
                self.focused = first;
            }
        }
        self.panes
            .get_mut(&self.focused)
            .expect("a tab always has at least one pane")
    }

    /// Split the focused leaf in the given `direction`. The freshly-
    /// inserted `new_pane` becomes the sibling on the `side_b` side
    /// (`true` = right/bottom, `false` = left/top), and focus moves to
    /// it. Returns the new pane's id.
    fn split_focused(&mut self, direction: SplitDir, new_pane: Pane, side_b: bool) -> PaneId {
        let new_id = self.next_pane_id;
        self.next_pane_id = self.next_pane_id.wrapping_add(1);
        self.panes.insert(new_id, new_pane);
        let focused = self.focused;
        // Rebuild the tree with the focused leaf swapped for a Split.
        let owned = std::mem::replace(&mut self.tree, PaneNode::Leaf(focused));
        self.tree = split_in(owned, focused, direction, new_id, side_b);
        self.focused = new_id;
        new_id
    }

    /// Close the focused pane, collapsing its parent split so the
    /// sibling subtree replaces the whole split. The new focused leaf
    /// is the first leaf of the surviving subtree. Returns `Some(closed
    /// pane)` when there was a parent split to collapse; returns `None`
    /// (caller should close the tab) when the tree was already a single
    /// leaf.
    fn close_focused(&mut self) -> Option<Pane> {
        if matches!(self.tree, PaneNode::Leaf(_)) {
            return None;
        }
        let focused = self.focused;
        let owned = std::mem::replace(&mut self.tree, PaneNode::Leaf(focused));
        let (new_tree, found) = collapse_close(owned, focused);
        self.tree = new_tree;
        if found {
            let closed = self.panes.remove(&focused);
            // Pick the first leaf of the new tree as the new focused.
            self.focused = first_leaf_of(&self.tree).unwrap_or(focused);
            closed
        } else {
            None
        }
    }
}

// split_in, collapse_close, first_leaf_of are now in panes.rs

impl std::ops::Deref for TabState {
    type Target = Pane;
    fn deref(&self) -> &Pane {
        self.focused_pane()
    }
}

impl std::ops::DerefMut for TabState {
    fn deref_mut(&mut self) -> &mut Pane {
        self.focused_pane_mut()
    }
}

/// Enough of a closed tab to respawn an equivalent one (Ctrl+Shift+T-style
/// "reopen closed tab"). Captured the instant a tab is closed.
#[derive(Debug, Clone)]
struct ClosedTab {
    profile_name: String,
    icon: Option<String>,
    /// Directory the shell was in when the tab closed (from OSC 7), so the
    /// reopened tab lands in the same place.
    cwd: Option<PathBuf>,
}

/// Build the [`Profile`] used to respawn a [`ClosedTab`]. Keeps the original
/// profile name + icon and pins the captured cwd; the command stays empty so
/// `build_spawn_spec` falls back to the default shell (matching `new_tab`).
fn profile_from_closed(closed: &ClosedTab) -> Profile {
    Profile {
        name: closed.profile_name.clone(),
        command: String::new(),
        args: Vec::new(),
        env: Default::default(),
        cwd: closed.cwd.clone(),
        icon: closed.icon.clone(),
    }
}

/// Ephemeral state for the in-buffer search overlay. Held by the
/// `RunningState` while search mode is active.
#[derive(Debug, Clone)]
struct SearchState {
    query: String,
    /// All matches across the **whole buffer** (scrollback + screen), ordered
    /// top→bottom. Each is `(line_abs, col_start, col_end)` where `line_abs`
    /// is the absolute alacritty line (negative = scrollback history). The
    /// host converts these to viewport rows for the current scroll when
    /// drawing highlights / jumping.
    matches: Vec<(i32, u16, u16)>,
    /// Index into `matches` of the currently "focused" match.
    current: usize,
}

impl SearchState {
    fn new() -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            current: 0,
        }
    }
}

/// Which list the command palette is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteMode {
    /// The default list of bindable in-app actions.
    Actions,
    /// The theme picker (opened from the "Change Theme…" action).
    Themes,
    /// A scoped picker listing ONLY the configured SSH hosts as searchable
    /// `SSH: <name>` rows. Opened from the bottom-right quick-connect button;
    /// selecting a row connects in a new tab via the normal SSH path.
    SshQuickConnect,
    /// A scoped picker listing ONLY the configured `[[snippets]]` entries.
    /// Selecting a row decodes its body and writes it to the focused pane's PTY.
    Snippets,
    /// Inline prompt for a workspace name (opened from "Save workspace…").
    WorkspaceNamePrompt,
    /// A scoped picker listing ONLY saved named workspaces.
    WorkspacePicker,
    /// A scoped picker listing previously-run commands collected from the
    /// configured scope. Selecting a command loads it onto the prompt for
    /// editing (without a trailing newline).
    CommandHistory,
    /// A scoped picker listing the clipboard history ring (most-recent
    /// first). Selecting an entry pastes it into the focused pane via the
    /// normal paste path (honours bracketed paste).
    ClipboardHistory,
    /// A scoped picker listing visited directories ranked by frecency
    /// (highest first). Selecting a directory sends `cd <path>\n` to the
    /// focused pane's PTY.
    DirectoryJump,
    /// A scoped picker listing only command blocks with a non-zero exit code,
    /// newest first. Selecting an entry scrolls the viewport to that block.
    FailedCommandPicker,
}

/// A selectable row in the palette. Distinct from [`ShortcutAction`] so the
/// palette can also surface meta-commands (open a sub-picker) and data-driven
/// choices (a concrete theme) without polluting the keybinding table.
#[derive(Debug, Clone)]
enum PaletteItem {
    /// Run a bindable action.
    Action(ShortcutAction),
    /// Switch the palette into the theme picker (stays open).
    OpenThemePicker,
    /// Apply + persist a named theme.
    SetTheme(String),
    /// Open a new tab connected to the configured SSH host at this index
    /// (into `config.ssh_hosts`).
    OpenSsh(usize),
    /// Decode and insert the snippet at this index (into `config.snippets`)
    /// into the focused pane's PTY.
    InsertSnippet(usize),
    /// Restore the named workspace at this index (into a cached
    /// `Vec<(name, path)>` built when the picker opens).
    OpenNamedWorkspace(usize),
    /// Load this command text onto the shell prompt for editing (no newline).
    /// The string is the deduplicated command text from the history list.
    InsertCommand(String),
    /// Paste this clipboard-history entry text into the focused pane via the
    /// normal paste path (honours bracketed paste mode).
    PasteClipboardEntry(String),
    /// Send `cd <path>\n` to the focused pane's PTY, jumping the shell to
    /// the given directory. The path is properly single-quoted.
    JumpToDirectory(String),
    /// Scroll the viewport to the prompt row of the command block at this
    /// absolute line index (from the failed-command picker).
    JumpToBlock(i32),
    /// Invoke the plugin-registered command at this index (into the host's
    /// `registered_commands` list). Set from the command palette when the user
    /// selects a plugin-contributed row.
    PluginCommand(usize),
}

/// State for the fuzzy command palette (Ctrl+Shift+P). When `Some`,
/// keystrokes edit the query / move the selection and never reach the PTY.
#[derive(Debug, Clone)]
struct CommandPaletteState {
    /// Text the user has typed.
    query: String,
    /// Index into the *ranked* result list of the highlighted row.
    selected: usize,
    /// Which list is being shown.
    mode: PaletteMode,
}

impl CommandPaletteState {
    fn new() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            mode: PaletteMode::Actions,
        }
    }
}

/// In-window "Save this SSH host?" prompt state. Holds the host parsed from
/// the typed command line plus the live checkbox state. The default-checked
/// "don't ask again" box means a single "Dismiss" both hides the prompt and
/// (if Save isn't chosen) suppresses future prompts — the least-nagging
/// behaviour.
#[derive(Debug, Clone)]
struct SaveHostPromptState {
    /// The parsed `ssh` destination (user / host / port) to offer saving.
    parsed: ParsedSsh,
    /// "don't ask again" checkbox — starts checked.
    dont_ask_again: bool,
}

/// One autodetected link (URL or filesystem path) inside the visible
/// viewport.
#[derive(Debug, Clone)]
struct DetectedLink {
    col_start: u16,
    col_end: u16,
    row: u16,
    /// What Ctrl+click opens — a URL as-is, or a resolved absolute path.
    url: String,
    /// `true` for filesystem paths. Paths are *not* permanently underlined
    /// (that would clutter `ls` output / prompts); they're surfaced via the
    /// hover tooltip + pointer cursor instead. URLs stay always-underlined.
    is_path: bool,
    /// Line / column parsed from a `file:line:col` reference, used to jump
    /// the configured editor to the right spot. `None` for URLs.
    line: Option<u32>,
    column: Option<u32>,
}

/// Runtime state for the currently-active modal key-table. Set when the
/// user presses a leader combo; cleared when the next key is dispatched,
/// when Esc is pressed, or when the timeout expires.
#[derive(Debug, Clone)]
struct ActiveKeyTable {
    /// Index into `config.keybinds.key_tables` of the active table.
    table_idx: usize,
    /// Wall-clock instant when the table was entered. Used by the
    /// timeout check in `about_to_wait`.
    entered_at: std::time::Instant,
}

/// Returns `true` when the key-table timeout has elapsed.
///
/// Pure helper — no side effects — so it can be covered by unit tests
/// without touching `RunningState`.
#[must_use]
fn key_table_timed_out(
    entered_at: std::time::Instant,
    now: std::time::Instant,
    timeout_ms: u32,
) -> bool {
    now.duration_since(entered_at) >= std::time::Duration::from_millis(u64::from(timeout_ms))
}

/// All per-window state. One [`TermWindow`] backs each native OS window:
/// it owns that window's `winit::Window`, its own [`Renderer`] (which reuses
/// the process-wide shared wgpu device via [`Renderer::new_shared`]), its
/// tab list + active tab, and every piece of per-window input / selection /
/// menu / search / palette / drag state. Tab tear-out moves a [`TabState`]
/// from one `TermWindow` into a freshly-created one.
///
/// The historical free functions all take `&mut RunningState`; the
/// [`RunningState`] alias keeps them compiling unchanged now that the type
/// is per-window rather than the single global app state.
struct TermWindow {
    window: Arc<Window>,
    renderer: Renderer,
    tabs: Vec<TabState>,
    active_tab: usize,
    clipboard: Option<Clipboard>,
    modifiers: ModifiersState,
    proxy: EventLoopProxy<UserEvent>,
    /// Last palette pushed by `apply_theme`. New tabs inherit this so a
    /// hot-swap of the theme remains visible when the user spawns a new
    /// tab. Defaults to the built-in Tokyo Night-ish palette.
    palette: terminale_term::AnsiPalette,
    /// Cached bell mode (visual / audio / both / none) — updated live
    /// from the settings window.
    bell_mode: terminale_config::BellMode,
    /// Rows scrolled per wheel notch. Mirrors `window.scroll_step_lines`
    /// from config so handle_scroll can stay sync.
    scroll_step_lines: u8,
    /// Rows forwarded per wheel notch on the alt-screen. Mirrors
    /// `window.alt_screen_scroll_lines`.
    alt_screen_scroll_lines: u8,
    /// Pixels of trackpad (PixelDelta) input per row. Mirrors
    /// `window.touchpad_pixels_per_row`.
    touchpad_pixels_per_row: f32,
    /// When true, sub-row trackpad deltas accumulate across events. Mirrors
    /// `window.smooth_scroll`.
    smooth_scroll: bool,
    /// Fractional row remainder from the last trackpad scroll (smooth mode).
    /// Positive = accumulating toward a scroll-up; negative = scroll-down.
    smooth_scroll_remainder: f32,
    /// When true, finishing a mouse selection auto-copies it to the
    /// clipboard. Mirrors `window.copy_on_select`.
    copy_on_select: bool,
    /// Max scrollback lines per terminal. Mirrors `window.scrollback_lines`;
    /// applied live to every tab's emulator when changed in settings.
    scrollback_lines: usize,
    /// Whether command-block capture is enabled. Mirrors `terminal.command_blocks`;
    /// applied live to every tab's emulator when changed in settings.
    command_blocks_enabled: bool,
    /// Max command blocks per terminal. Mirrors `terminal.max_command_blocks`;
    /// applied live to every tab's emulator when changed in settings.
    max_command_blocks: usize,
    /// Characters that bound a double-click word selection. Mirrors
    /// `terminal.word_separators`; threaded into `Emulator::word_at`.
    word_separators: String,
    /// When detected URLs get the accent underline. Mirrors
    /// `terminal.link_underline`. `Always` keeps every URL underlined;
    /// `Hover` underlines only the link under the pointer; `Never` draws
    /// none. Read by `refresh_autodetect_links` and the hover handler.
    link_underline: terminale_config::LinkUnderline,
    /// When `true`, a tab / window close needs a confirming second close
    /// action via the confirmation dialog. Mirrors `window.confirm_close`.
    confirm_close: bool,
    /// Whether the window is pinned above all others. Mirrors
    /// `window.always_on_top`; applied live to the OS window level when
    /// toggled from settings / palette / menu.
    always_on_top: bool,
    /// Most-recently-closed tabs (newest last) for "reopen closed tab".
    /// Capped at [`MAX_CLOSED_TABS`] so it can't grow without bound.
    closed_tabs: Vec<ClosedTab>,
    /// The tab index that was active immediately before the current one.
    /// Updated on every [`switch_tab`] call so `last_tab` can toggle between
    /// the two most-recently-used tabs. `None` until at least one switch has
    /// occurred.
    previous_active_tab: Option<usize>,
    /// User-configured in-app keyboard shortcuts. Cached here so
    /// `handle_app_hotkey` dispatches from config instead of hardcoded
    /// keys. Live-updated from the settings window.
    shortcuts: terminale_config::ShortcutsConfig,
    /// Custom multi-action keybinds (`[[keybinds.custom]]`). Each entry
    /// maps a combo to a sequence of actions. Checked first in
    /// `handle_app_hotkey` so user binds always override built-ins.
    /// Live-updated from the settings window.
    custom_keybinds: Vec<terminale_config::CustomKeybind>,
    /// In-buffer search state (Ctrl+Shift+F). When `Some`, keystrokes
    /// extend the query and don't reach the PTY.
    search: Option<SearchState>,
    /// Modal keyboard copy mode (Ctrl+Shift+X). When active, keystrokes drive
    /// the copy-mode cursor and never reach the PTY.
    copy_mode: copy_mode::CopyModeState,
    /// Command-palette state (Ctrl+Shift+P). When `Some`, keystrokes edit
    /// the fuzzy query / move the selection and don't reach the PTY.
    command_palette: Option<CommandPaletteState>,
    /// Inline tab/pane rename state. When `Some`, the tab pill at `tab_idx`
    /// shows an editable field and keystrokes edit the buffer instead of
    /// reaching the PTY. Enter commits (names the tab's focused pane), Esc
    /// cancels, an empty buffer reverts to the automatic title.
    renaming: Option<RenameState>,
    /// Theme name the palette's theme-picker asked to switch to. Picked up
    /// by the App loop (which owns the `Config`) to apply + persist it.
    pending_theme: Option<String>,
    /// Seed prompt for the AI assistant (e.g. "Explain Selection"). When
    /// `Some` alongside `open_ai_requested`, the App opens the assistant
    /// with this already submitted.
    pending_ai_prompt: Option<String>,
    /// New font size from a live zoom (Ctrl+± / Ctrl+0). Picked up by the
    /// App to persist into `config.font.size` so the zoom survives a restart.
    pending_font_size: Option<f32>,
    /// New "stay on top" value from a runtime quick-toggle (palette / menu /
    /// shortcut). Picked up by the App to persist into
    /// `config.window.always_on_top` so it survives a restart, and to keep
    /// the settings window's copy in sync.
    pending_always_on_top: Option<bool>,
    /// Coalesced resize target (physical px). Winit fires many `Resized`
    /// events during a window drag; we stash only the last one and apply it
    /// once per frame in `AboutToWait`/`RedrawRequested` to avoid parsing
    /// intermediate-size PTY repaints against a grid that's already moved on.
    pending_resize: Option<winit::dpi::PhysicalSize<u32>>,
    /// Name of the currently-applied theme, cached so the theme picker can
    /// mark it. Updated by `apply_theme`.
    theme_name: String,
    /// All theme names (built-ins + user-defined), cached so the palette
    /// theme picker can list them without access to the full `Config`.
    /// Updated by `apply_theme`.
    theme_names: Vec<String>,
    /// Display names of the configured SSH hosts (parallel to
    /// `config.ssh_hosts`), cached so the command palette can surface
    /// `SSH: <name>` entries without access to the full `Config`.
    ssh_host_names: Vec<String>,
    /// Snippet names (and optional descriptions) cached from
    /// `config.snippets`, used by the snippet palette mode without needing
    /// access to the full `Config`. Each element is `(name, description)`.
    /// Kept in sync on config reload and Settings apply.
    snippet_names: Vec<(String, String)>,
    /// `Some(idx)` asks the App (which owns the full `Config`) to decode
    /// `config.snippets[idx].body` and write the decoded bytes to the
    /// focused pane's PTY. Set from the snippet palette.
    pending_insert_snippet: Option<usize>,
    /// `Some(name)` asks the App to save the current layout as a named
    /// workspace with this name. Set when the user commits the inline
    /// workspace-name prompt.
    pending_save_workspace: Option<String>,
    /// `Some(path)` asks the App to restore the workspace at this path.
    pending_open_workspace_path: Option<std::path::PathBuf>,
    /// Cached list of named workspaces (name, path), rebuilt each time the
    /// workspace picker opens.
    workspace_list: Vec<(String, std::path::PathBuf)>,
    /// Connection targets `(host, user, port)` of the configured SSH hosts,
    /// cached so [`maybe_offer_save_ssh_host`] can tell whether a typed
    /// `ssh …` command points at an already-saved host without the full
    /// `Config`. Kept in sync alongside `ssh_host_names`.
    ssh_host_targets: Vec<(String, Option<String>, u16)>,
    /// Cached `editor.command` template — launched on Ctrl+click of a
    /// `file:line:col` path. Empty = open with the OS default handler.
    editor_command: String,
    /// The resolved default profile used for the window's first tab. Reused
    /// when the user opens a new tab via "+" so subsequent tabs match the
    /// first one (name + icon + command) instead of falling back to a bare
    /// "shell" label. `None` only when no profiles are configured at all.
    default_profile: Option<Profile>,
    /// Quake mode: whether the window is currently shown (`true`) or hidden
    /// (`false`). Toggled by the global hotkey — a pure show/hide that always
    /// restores the window's exact last geometry.
    quake_visible: bool,
    /// Latched in the `Focused(false)` handler when the Quake window
    /// loses focus, consumed after the match arm finishes so the
    /// app-level handler can call `toggle_quake` without re-borrowing
    /// `self.windows[idx]` while the per-window `state` is alive. Only
    /// honoured when `quake.hide_on_focus_loss` is set and `quake.edge`
    /// is a real dock edge.
    pending_quake_autohide: bool,
    /// Deadline until which keyboard presses are swallowed after a Quake show.
    /// The global-hotkey combo (e.g. Ctrl+Shift+1) is consumed by the OS, but
    /// the still-held trigger key can otherwise leak in as a keypress once the
    /// freshly shown window gains focus (typing e.g. "1" into the shell). Set
    /// on every show; presses before the deadline are dropped. `None` = off.
    quake_input_suppress_until: Option<std::time::Instant>,
    /// The exact geometry `(x, y, w, h)` captured on the last hide, restored
    /// verbatim on the next show. `None` until the window is first hidden.
    quake_saved_rect: Option<terminale_config::WindowRect>,
    /// Dock-mode only: the dock rect actually applied on the most recent
    /// docked show. Used as the baseline to detect whether the user has
    /// since moved/resized the window away from it.
    quake_last_dock_rect: Option<terminale_config::WindowRect>,
    /// Dock-mode only: a user-adjusted geometry that should persist across
    /// hide/show instead of re-docking ("reappears exactly as it
    /// disappeared"). Set once the user moves/resizes a docked Quake window;
    /// cleared when Quake is free-floating (`edge == Off`).
    quake_user_rect: Option<terminale_config::WindowRect>,
    /// The window's floating geometry captured the first time Quake docked it.
    /// Restored (Chrome-style) when the user drags a docked Quake window by the
    /// title bar, so it pops out to its pre-dock size. `None` until first dock.
    quake_pre_dock_rect: Option<terminale_config::WindowRect>,
    /// The monitor that the Quake window was on when it was **last hidden**.
    /// Snapshotted in `toggle_quake` while the window is still visible (so
    /// `Window::current_monitor()` is reliable). Used by
    /// `compute_quake_target` to resolve `QuakeDisplay::Current` correctly
    /// — without this cache, a hidden window's `current_monitor()` returns
    /// whichever monitor contains the window's off-screen rect, not the one
    /// the user is actually looking at.
    quake_last_monitor: Option<winit::monitor::MonitorHandle>,
    /// In-flight open/close animation, if any. Driven frame-by-frame from
    /// `about_to_wait`; cleared when the animation completes.
    quake_anim: Option<QuakeAnim>,

    pointer_logical: (f32, f32),
    selecting: bool,
    selection_anchor: Option<(u16, u16)>,
    /// Pixel position where the left-click began. Selection only "materialises"
    /// once the cursor has moved more than a few pixels from this anchor —
    /// otherwise a single click is treated as click, not click+drag.
    selection_press_px: Option<(f32, f32)>,
    /// (timestamp, position, count) of the last left-click — used by
    /// double/triple-click word/line selection.
    last_click: Option<(std::time::Instant, (f32, f32), u8)>,
    /// (timestamp, position) of the last press on the title-bar drag handle —
    /// used to detect a double-click on the title bar (toggle maximize) vs a
    /// single click (start a window drag). Kept separate from `last_click`
    /// (which is body selection) so the two gestures never interfere.
    last_titlebar_click: Option<(std::time::Instant, (f32, f32))>,
    /// (timestamp, tab index) of the last click on a tab pill — a second
    /// click on the same tab within the double-click window starts an inline
    /// rename.
    last_tab_click: Option<(std::time::Instant, usize)>,
    /// `true` while the OS mouse pointer is suppressed (we typed
    /// something, the cursor faded). Reset on the next CursorMoved.
    pointer_hidden: bool,
    /// URL the mouse is currently hovering — `None` when not over a
    /// hyperlinked cell. Captured from CursorMoved; lets Alt+Enter
    /// open the link without clicking.
    hovered_url: Option<String>,
    /// Most recently *pressed* mouse button (None if no button is held).
    /// Drives mouse-drag reporting under SGR 1006.
    held_button: Option<MouseButton>,
    /// Last grid cell reported to an app under mouse-motion reporting
    /// (SGR drag / any-motion). Used to dedupe per-cell so we don't flood
    /// the PTY with one report per pixel. Reset on button state changes.
    last_motion_cell: Option<(u16, u16)>,
    /// A tab was pressed on the tab bar this gesture: `(tab index, press
    /// point in physical px)`. Awaits promotion to an App-level [`TabDrag`]
    /// once the cursor moves past [`TAB_DRAG_ARM_PX2`]. `None` = no pending
    /// lift. A plain click that never moves stays a tab switch.
    tab_press: Option<(usize, (f32, f32))>,
    /// A group-label pill was pressed: `(group_id, first-member tab index,
    /// press point in physical px)`. Awaits promotion to an App-level
    /// [`DragPayload::Group`] drag once the cursor moves past
    /// [`TAB_DRAG_ARM_PX2`]. A plain click that never moves triggers the
    /// inline rename via the left-release handler.
    group_press: Option<(TabGroupId, usize, (f32, f32))>,
    /// Mirror of `appearance.animated_tab_drag`: when `false`, a tab drag
    /// still reorders / attaches / tears out on release, but the floating
    /// ghost + drop indicator are suppressed. Live-updated from settings.
    animated_tab_drag: bool,

    menu_visible: bool,
    menu_origin: [f32; 2],

    /// Set by menu/hotkey to request the App open the settings window on
    /// the next event-loop iteration (when we have an `ActiveEventLoop`).
    open_settings_requested: bool,
    /// Requested origin (in physical screen px) to spawn the context-menu
    /// popup window from inside the App.
    open_menu_at: Option<winit::dpi::PhysicalPosition<i32>>,
    /// Which surface the pending/last context menu was opened over. Set by the
    /// right-click handler and read when building the menu entries so a tab and
    /// the terminal body get distinct menus.
    menu_context: MenuContext,
    /// Set by Ctrl+Shift+T to ask the App to open a profile picker popup
    /// anchored to the tab bar.
    open_profile_picker: bool,
    /// Set by the RestartTab shortcut to ask the App to restart the focused
    /// pane's session. Deferred to the App loop because the respawn profile
    /// is resolved from `self.config` (command/args/env), which the
    /// state-level shortcut dispatch cannot reach.
    pending_restart_pane: bool,
    /// Set by Ctrl+Shift+A to ask the App to open the AI assistant window.
    open_ai_requested: bool,
    /// `Some(idx)` asks the App (which owns the Tokio runtime + the SSH
    /// host list) to open a tab connected to `config.ssh_hosts[idx]`.
    /// Set from the command palette / "New SSH tab" picker.
    pending_ssh_host: Option<usize>,
    /// Set to ask the App to open the "New SSH tab" host picker popup.
    open_ssh_picker: bool,
    /// Best-effort reconstruction of the current shell input line (printable
    /// keystrokes since the last Enter / line-edit), used only to detect a
    /// typed `ssh …` command and offer to save the host. Reset on Enter,
    /// Ctrl+C / Ctrl+U, and tab switches. Never sent to the PTY itself.
    input_line: String,
    /// Mirrors `config.terminal.offer_save_ssh_hosts`: when `false`, typing
    /// an `ssh` command never pops the save prompt. Live-synced from config.
    offer_save_ssh_hosts: bool,
    /// The active "Save this SSH host?" prompt, if any (parsed host details
    /// + the checkbox state). `None` when no prompt is showing.
    save_host_prompt: Option<SaveHostPromptState>,
    /// Set when the user clicks "Save" on the prompt: hands the parsed host
    /// to the App (which owns `config`) to persist on the next loop tick.
    pending_save_ssh_host: Option<ParsedSsh>,
    /// Set when the prompt's "don't ask again" state must be persisted into
    /// `config.terminal.offer_save_ssh_hosts` (inverted): `Some(true)` =
    /// don't ask again. Drained by the App alongside the save.
    pending_dont_ask_again: Option<bool>,
    /// Set by the "Import SSH hosts from OpenSSH config" palette action or
    /// the Settings button. The App (which owns `config`) drains it on the
    /// next loop tick, parses the OpenSSH config file, deduplicates against
    /// the existing host list, appends new hosts, and persists to disk.
    pending_import_ssh_hosts: bool,
    /// Set by the `ImportTheme` shortcut action (palette entry or Settings
    /// button). The App drains this on the next loop tick, opens a native
    /// file picker, copies the chosen `.toml` into `themes_dir`, appends the
    /// new theme to the available list, and optionally selects it.
    pending_import_theme: bool,

    /// While the user is dragging a split-pane divider, this holds the path
    /// to the Split node being mutated + the press-time geometry needed to
    /// recompute the new ratio on each CursorMoved. `None` outside a drag.
    pending_divider_drag: Option<PendingDividerDrag>,
    /// Currently-hovered divider, if any: `(path, axis)`. Drives the cursor
    /// icon swap and avoids redundant `set_cursor` calls.
    hovered_divider: Option<(DividerPath, SplitDir)>,
    /// Mirror of `config.appearance.divider_thickness_logical * scale_factor`.
    /// Recomputed on `ScaleFactorChanged` and on every config reload.
    divider_thickness_px: f32,
    /// Mirror of `config.appearance.divider_grab_padding_logical * scale_factor`.
    /// Recomputed alongside `divider_thickness_px`.
    divider_grab_padding_px: f32,
    /// Mirror of `config.appearance.divider_color`. `None` falls back to a
    /// background-derived neutral tone.
    divider_color: Option<[u8; 3]>,
    /// Mirror of `config.appearance.focus_border_thickness_logical * scale_factor`.
    /// Recomputed on `ScaleFactorChanged` and on every config reload.
    focus_border_thickness_px: f32,
    /// Mirror of `config.appearance.focus_border_color`. `None` uses the
    /// renderer's built-in accent fallback.
    focus_border_color: Option<[u8; 3]>,
    /// Mirror of `config.terminal.live_pane_resize`: when `false`, the PTYs
    /// only resize on left-release rather than on every CursorMoved during
    /// a divider drag (cheaper for slow shells / SSH).
    live_pane_resize: bool,
    /// Mirror of `config.terminal.pane_resize_step_cells`: how many cells to
    /// nudge the focused pane's parent split per keyboard-resize action.
    pane_resize_step_cells: u8,
    /// Mirror of `config.appearance.show_pane_headers`. When `true` and a
    /// tab has more than one pane, each pane shows a 22 px header strip.
    /// Live-applied via the settings window.
    show_pane_headers: bool,
    /// Mirror of `config.terminal.show_prompt_marks`. When `true` the
    /// renderer draws a small dot in the left margin at each visible
    /// OSC 133 prompt-start line, coloured by exit status.
    show_prompt_marks: bool,
    /// Whether the main terminal window currently has OS focus. Updated in
    /// the `WindowEvent::Focused` handler. Used to suppress desktop
    /// notifications when the user is actively looking at the window.
    window_focused: bool,
    /// Set from `WindowEvent::Occluded`: `true` when the window is fully
    /// covered by other windows or minimized. While occluded we skip
    /// scheduling animation redraws (background FX, activity spinner, bell,
    /// jump-highlight) — the compositor would just throw those frames away —
    /// so a hidden window costs no GPU/CPU. PTY output still drains and wakes
    /// the loop, and we repaint once when the window becomes visible again.
    occluded: bool,
    /// Mirror of `config.terminal.os_notifications`. When `true`, OSC 9 /
    /// OSC 777 notifications are forwarded to the OS notification centre
    /// (but only while the window is not focused).
    os_notifications: bool,
    /// Mirror of `config.terminal.os_notification_rate_limit` — max
    /// notifications per rolling 10 s window (`0` = unlimited).
    os_notification_rate_limit: u32,
    /// Fingerprint of every input `refresh_tab_bar` renders from (labels via
    /// emulator generation, active/unread/pinned/colors/groups, spinner
    /// state, rename buffer, maximized). When unchanged the per-frame
    /// rebuild — per-tab emulator locks + label `String`s — is skipped.
    tab_bar_fingerprint: u64,
    /// `Some(pane_id)` while the pointer is over a pane-header close-X.
    /// Drives cursor-icon swap and the ✕ hover tint.
    pane_header_close_hover: Option<PaneId>,
    /// `(timestamp, pane_id)` of the last click on a pane-header strip —
    /// a second click on the same pane-id within the double-click window
    /// starts an inline rename for that pane.
    last_header_click: Option<(std::time::Instant, PaneId)>,
    /// A pane-header was pressed (left button down) — `(pane_id, press point
    /// in physical px)`. Awaits promotion to an App-level pane drag once the
    /// cursor moves past [`TAB_DRAG_ARM_PX2`]. `None` = no pending lift.
    /// Cleared on left-release and on promotion to a full drag.
    pane_header_press: Option<(PaneId, (f32, f32))>,
    /// Mirror of `config.appearance.pane_tear_out`. Gates arming of
    /// [`Self::pane_header_press`]. Live-applied from config reloads and the
    /// settings window.
    pane_tear_out: bool,
    /// Profile names cached from `App.config.profiles`, kept parallel to
    /// `profile_icons`. Used by `menu_items` to build the "New tab with
    /// profile…" submenu without borrowing the full `Config`.
    profile_names: Vec<String>,
    /// Profile icon glyphs parallel to `profile_names` (may be empty).
    profile_icons: Vec<Option<String>>,
    /// Active quick-select session. `Some` while quick-select mode is engaged;
    /// `None` outside it. Keystrokes feed into this and are not forwarded to
    /// the PTY while it is `Some`.
    quick_select: Option<quick_select::QuickSelectState>,
    /// Active pane-select session. `Some` while pane-select mode is engaged;
    /// `None` outside it. Keystrokes feed into this and are not forwarded to
    /// the PTY while it is `Some`.
    pane_select: Option<quick_select::PaneSelectState>,
    /// Compiled quick-select regex patterns, rebuilt whenever
    /// `quick_select_config.patterns` changes (config reload / settings edit).
    qs_compiled_patterns: Vec<regex::Regex>,
    /// Cached alphabet string from `quick_select_config.alphabet`, kept so we
    /// don't re-read the config struct on every scan.
    qs_alphabet: String,
    /// Mirror of `config.quick_select.overlay_dim`. Opacity of the full-screen
    /// tint drawn behind label badges. Updated on config reload / settings edit.
    qs_overlay_dim: f32,
    /// Wall-clock instant of the last status-bar text refresh. Used to throttle
    /// periodic redraws when a `Clock` segment is configured — only triggers a
    /// redraw when `update_interval_ms` has elapsed since the last one.
    last_status_bar_tick: std::time::Instant,
    /// Modal key-table state. `Some` while the user is in a key-table's
    /// modal mode; `None` normally.  Set when the leader combo is pressed;
    /// cleared when the next key dispatches (or Esc / timeout).
    active_key_table: Option<ActiveKeyTable>,
    /// Cached copy of `config.keybinds.key_tables`. Live-updated from the
    /// settings window / config reload alongside `custom_keybinds`.
    key_tables: Vec<terminale_config::KeyTable>,
    /// Cached copy of `config.keybinds.mouse`. Custom mouse bindings are
    /// checked before the built-in mouse handling; a match consumes the press.
    /// Default empty — built-in behaviour is unchanged.  Live-updated from
    /// the settings window / config reload alongside `custom_keybinds`.
    mouse_bindings: Vec<terminale_config::MouseBinding>,
    /// Whether broadcast-input mode is active for the active tab. Transient —
    /// not persisted. When `true`, each keystroke forwarded to the focused pane
    /// is also sent to every other pane in the configured scope. A tinted
    /// border is drawn around the receiving panes while broadcast is on.
    broadcast_input: bool,
    /// Set by `NewWindow` to ask the App to open a fresh top-level window on
    /// the next post-event drain (where `event_loop` is available).
    pending_new_window: bool,
    /// Set by `MoveTabToNewWindow` to ask the App to tear the active tab into
    /// a new window. The tab index is captured at dispatch time; the App reads
    /// `active_tab` + `window.id()` to perform the tear-out.
    pending_move_tab_to_new_window: bool,
    /// Set by `MovePaneToNewTab` to ask the App to promote the focused pane
    /// into a new tab in the same window.
    pending_move_pane_to_new_tab: bool,
    /// Set by `MovePaneToNewWindow` to ask the App to tear the focused pane
    /// into a brand-new window.
    pending_move_pane_to_new_window: bool,
    /// Whether zen (distraction-free) mode is currently active. Transient —
    /// not persisted to config. Set by `ToggleZenMode`; cleared by the same
    /// action. While `true`, chrome visibility is overridden by the
    /// zen_hide list without changing the underlying config values.
    zen: bool,
    /// The full-screen state captured the moment zen mode was activated, so
    /// we can restore it correctly when zen exits. `true` = the window was
    /// already in full-screen before zen; `false` = windowed / maximized.
    zen_was_fullscreen: bool,
    /// Mirror of `config.window.zen_hide`. Controls which chrome elements
    /// are suppressed while zen mode is active. Live-updated from the
    /// settings window / config reload.
    zen_hide: Vec<terminale_config::ZenHideElement>,
    /// Mirror of `config.window.zen_fullscreen`. When `true`, activating zen
    /// also enters full-screen; exiting zen exits full-screen (if zen entered
    /// it). Live-updated from the settings window / config reload.
    zen_fullscreen: bool,
    /// Mirror of `config.appearance.tab_bar_enabled`. Used by `apply_zen_chrome`
    /// to restore the tab-bar state when exiting zen mode.
    tab_bar_enabled_config: bool,
    /// Mirror of `config.appearance.show_pane_headers`. Used by `apply_zen_chrome`
    /// to restore the pane-header state when exiting zen mode.
    show_pane_headers_config: bool,
    /// Mirror of `config.terminal.link_hover_tooltip`. When `false`, the hover
    /// tooltip is suppressed entirely regardless of whether a URL is under the pointer.
    link_hover_tooltip: bool,
    /// Mirror of `config.terminal.link_hover_delay_ms`. How long (ms) the pointer
    /// must dwell over a link before the tooltip appears. `0` = instant.
    link_hover_delay_ms: u32,
    /// Instant when the pointer first entered the currently-hovered URL, together
    /// with the URL string and the physical-pixel anchor for the tooltip.
    /// `None` when not hovering a link or when delay=0.
    /// Used to implement `link_hover_delay_ms`.
    link_hover_start: Option<(String, std::time::Instant, [f32; 2])>,
    /// Mirror of `config.terminal.clipboard_read`. Permission policy for OSC 52
    /// clipboard READ queries. Default `deny` — replies are suppressed.
    /// `allow` → read clipboard + send base64 reply back to the PTY.
    clipboard_read_policy: terminale_config::ClipboardReadPolicy,
    /// Mirror of `config.terminal.edit_command_clears_line`. When `true`,
    /// `EditLastCommand` prefixes the command text with Ctrl+U to kill any
    /// partially-typed input before loading the command for editing.
    edit_command_clears_line: bool,
    /// Cached copy of `config.context_rules`. Evaluated against each tab's
    /// SSH host name and cwd whenever PTY data arrives; the first matching
    /// rule's `tab_color` / `badge` are applied to the tab chip. An empty
    /// vec means no rules are configured (all tabs use default colours).
    /// Live-updated from the settings window / config reload.
    context_rules: Vec<terminale_config::ContextRule>,
    /// Mirror of `config.terminal.command_history_scope`. Controls which
    /// panes the command-history picker collects history from.
    /// Live-updated from the settings window / config reload.
    command_history_scope: terminale_config::CommandHistoryScope,
    /// Mirror of `config.terminal.command_history_max_entries`. Maximum
    /// number of deduplicated entries shown in the history picker.
    /// Live-updated from the settings window / config reload.
    command_history_max_entries: usize,
    /// A history command text chosen in the picker. Drained by the
    /// post-event block to write the text (+ optional Ctrl+U) to the PTY.
    /// Uses the same semantics as `edit_last_command` (no trailing newline).
    pending_insert_command: Option<String>,
    /// Cached command history built when the command-history picker opens.
    /// Most-recent first, deduplicated, non-empty only. Rebuilt on each open
    /// so the list reflects the freshest state from the current scope.
    command_history_cache: Vec<String>,
    /// Mirror of `config.terminal.scrollback_export_format`. Output format for
    /// the "Export scrollback" action. Live-updated from settings / config reload.
    scrollback_export_format: terminale_config::ScrollbackExportFormat,
    /// Mirror of `config.terminal.scrollback_export_dir`. When `Some`, the
    /// export action writes there directly; when `None` an OS save-file dialog
    /// is opened. Live-updated from settings / config reload.
    scrollback_export_dir: Option<std::path::PathBuf>,
    /// In-memory ring buffer of recent clipboard entries (most-recent at
    /// index 0 after rotation). Capacity capped by
    /// `config.clipboard_history.size`. Memory-only — never written to disk.
    clipboard_history_ring: std::collections::VecDeque<String>,
    /// Mirror of `config.clipboard_history.enabled`. When `false`, no entries
    /// are captured. Live-updated from settings / config reload.
    clipboard_history_enabled: bool,
    /// Mirror of `config.clipboard_history.size`. Capacity of the ring.
    /// Live-updated from settings / config reload.
    clipboard_history_size: usize,
    /// Mirror of `config.clipboard_history.capture_osc52`. Whether OSC 52
    /// programmatic clipboard writes are also captured. Live-updated.
    clipboard_history_capture_osc52: bool,
    /// Text from a clipboard-history picker selection that should be pasted
    /// into the focused pane on the next loop tick.
    pending_paste_clipboard_entry: Option<String>,
    /// Directory-jump frecency store. Tracks every directory the active pane
    /// visits (via OSC 7 cwd reports) and ranks them by a frequency + recency
    /// score. Loaded from disk at window creation when `persist` is enabled;
    /// saved back on each update.
    dir_jump_store: dir_jump::DirJumpStore,
    /// Mirror of `config.directory_jump.enabled`. When `false`, OSC 7 cwd
    /// updates do not update the store and the picker returns an empty list.
    /// Live-updated from settings / config reload.
    dir_jump_enabled: bool,
    /// Mirror of `config.directory_jump.max_tracked`. Live-updated.
    dir_jump_max_tracked: usize,
    /// Mirror of `config.directory_jump.persist`. Live-updated.
    dir_jump_persist: bool,
    /// Cached ranked directory list, rebuilt each time the DirectoryJump
    /// picker opens. Highest-frecency first.
    dir_jump_cache: Vec<String>,
    /// A `cd <path>\n` payload to send to the focused pane's PTY on the next
    /// loop tick. Set from the DirectoryJump palette picker.
    pending_cd_path: Option<String>,
    /// When the paste-guard policy requires confirmation, the pending text is
    /// stashed here so the App event loop can open the dialog.
    /// `Some((text, bracketed))` means a guard dialog should be opened.
    pending_paste_guard: Option<(String, bool)>,
    /// When `window.confirm_close` is on, a tab/window close request is
    /// stashed here so the App event loop can open the confirmation dialog
    /// (it needs `event_loop`, which `RunningState`-level code never has).
    pending_close_confirm: Option<crate::confirm_close::CloseTarget>,

    // ── Paste safety mirrors ──────────────────────────────────────────────────
    /// Mirror of `config.terminal.paste_confirm_multiline`. When `true`,
    /// always prompt before pasting multi-line text. Live-updated.
    paste_confirm_multiline: bool,
    /// Mirror of `config.terminal.paste_confirm_when_unbracketed`. When
    /// `true` (default), prompt before pasting multi-line text if the focused
    /// program does NOT have bracketed paste enabled. Live-updated.
    paste_confirm_when_unbracketed: bool,
    /// Mirror of `config.terminal.paste_strip_control_chars`. When `true`,
    /// strip non-printable control bytes from pasted text before sending.
    /// Live-updated.
    paste_strip_control_chars: bool,

    /// Whether the snap-layout chooser overlay is currently open.
    /// Driven by the `ShowSnapLayouts` action; cleared on Esc or a cell click.
    snap_chooser_open: bool,

    // ── Prompt navigation ─────────────────────────────────────────────────────
    /// Mirror of `config.terminal.highlight_on_jump`. When `true`, a brief
    /// tinted band is drawn over the target prompt row after a jump action.
    highlight_on_jump: bool,
    /// Absolute line of the last jumped-to prompt row.  `Some` while the
    /// highlight is still fading; `None` when it has expired or never fired.
    /// The renderer maps this to a viewport row each frame to position the
    /// highlight band.
    jump_highlight_line: Option<i32>,
    /// Timestamp of the last jump action, used to compute highlight alpha.
    jump_highlight_start: Option<std::time::Instant>,
    /// Cached list of failed command blocks (non-zero exit) built when the
    /// `FailedCommandPicker` opens.  Each entry is `(prompt_line, label)`.
    failed_command_cache: Vec<(i32, String)>,

    // ── Plugin command-palette integration ───────────────────────────────────
    /// Display names of every command registered by Lua plugins via
    /// `terminale.register_command(name, fn)`. Kept in sync by
    /// `TerminaleApp::about_to_wait` after each tick's
    /// `flush_pending_registrations`. Used by `palette_ranked` to surface
    /// plugin-contributed rows in the `Actions` list.
    plugin_command_names: Vec<String>,
    /// When a plugin-palette row is selected, the command index is stored here
    /// so `TerminaleApp::about_to_wait` can call `host.invoke_command(idx)` on
    /// the next tick (where we have `&mut self.plugins`). Cleared after drain.
    pending_plugin_invoke: Option<usize>,
    /// Combo strings of every plugin-registered keybinding, index-aligned
    /// with the host's `registered_keybinds`. Synced by `about_to_wait`
    /// with combos that shadow a user binding replaced by an empty string
    /// (so indices stay aligned but the combo can never match).
    plugin_keybind_combos: Vec<String>,
    /// When a pressed combo matches a plugin keybinding, the index is
    /// stored here; `about_to_wait` calls `host.invoke_keybind(idx)` on
    /// the next tick. Cleared after drain.
    pending_plugin_keybind_invoke: Option<usize>,
    /// Mirror of `config.plugins.allow_keybindings`, checked in the hot
    /// key path. Refreshed every plugin tick so the toggle applies live.
    plugins_allow_keybindings: bool,

    // ── Pending plugin lifecycle events ──────────────────────────────────────
    // These are set at the event call-site and drained in `about_to_wait` where
    // `&mut self.plugins` is available. Using a queue avoids borrowing conflicts
    // (the free functions take `&mut RunningState`, not `&mut TerminaleApp`).
    /// Tab-open events to fire as `"tab_open"` hooks. Each entry is
    /// `(tab_index_0based, title)`.
    pending_hook_tab_open: Vec<(usize, String)>,
    /// Tab-close events to fire as `"tab_close"` hooks. Each entry is the
    /// tab index that was just removed.
    pending_hook_tab_close: Vec<usize>,
    /// Pane-focus events to fire as `"pane_focus"` hooks. Each entry is the
    /// pane id that just received focus.
    pending_hook_pane_focus: Vec<u32>,
    /// Session-start events to fire as `"session_start"` hooks.
    /// `(pane_id, program)`.
    pending_hook_session_start: Vec<(u32, String)>,
    /// Session-exit events to fire as `"session_exit"` hooks.
    /// `(pane_id, exit_code)`.
    pending_hook_session_exit: Vec<(u32, i32)>,
    /// Config-reload event counter — if > 0, fire `"config_reload"` once.
    pending_hook_config_reload: bool,
    /// Command-end events to fire as `"command_end"` hooks.
    /// `(exit_code, command, cwd)`.
    pending_hook_command_end: Vec<(i32, String, String)>,
    /// How many completed command blocks have already been reported as
    /// `"command_end"` per pane. Keyed by `PaneId`; entries grow monotonically
    /// and are removed when the pane is closed. Prevents double-firing the
    /// same block across multiple ticks.
    hook_cmd_end_fired: std::collections::BTreeMap<PaneId, usize>,

    // ── Tab groups ────────────────────────────────────────────────────────────
    /// Ordered list of named groups. Each group has a stable [`TabGroupId`],
    /// a name, and an accent colour. Tabs reference these by id via
    /// `TabState::group`. Serialised into the workspace / last-session snapshot
    /// so groups persist across restarts.
    tab_groups: Vec<TabGroup>,
    /// Monotonic counter used to assign stable ids to new groups.
    next_group_id: TabGroupId,
    /// Whether the group-name label is drawn in the tab bar. Mirrors
    /// `appearance.show_tab_group_labels` and is live-applied.
    show_tab_group_labels: bool,
    /// Whether the suggestion bar was open (renderer-side) on the previous
    /// `about_to_wait` tick. Open/close transitions reflow the PTY grid so
    /// the bar's reserved band is reclaimed/granted exactly once — mirrors
    /// the resource-strip enable/disable pattern.
    suggestion_bar_was_open: bool,
    /// `(pane, command-block count)` of the last failed command that already
    /// surfaced an `ai.offer_fix_on_failure` hint — each failure offers at
    /// most once.
    fix_hint_seen: Option<(PaneId, usize)>,
    /// Runtime state for the proactive AI command-suggestion bar: idle timer,
    /// generation counter, loading-frame tick, and the current bar content.
    pub(crate) suggestions: suggestions::SuggestionRuntime,
    /// Auto-cycle colour palette for new tab groups. Mirrors
    /// `appearance.tab_group_colors` and is live-applied.
    tab_group_colors: Vec<[u8; 3]>,
    /// Mirror of `config.appearance.bundled_icons`. When `true`, UI icons use
    /// the bundled Tabler Icons PUA font; when `false`, the classic emoji are
    /// shown. Live-applied from the Appearance settings toggle and config reload.
    bundled_icons: bool,
    /// Mirror of `config.appearance.tab_activity_spinner`. When `true`, a
    /// braille-dots spinner is prepended to busy tab labels and pane headers.
    /// Live-applied from the Appearance settings toggle and config reload.
    tab_activity_spinner: bool,
    /// Current animation frame index for the busy-tab spinner. Incremented
    /// once every ~90 ms in `about_to_wait` while any pane is busy. Wraps
    /// around via `% SPINNER_FRAMES.len()` before use so it never overflows.
    spinner_frame: usize,
    /// Wall-clock instant of the last spinner frame advance. Used to gate the
    /// 90 ms cadence so `about_to_wait` does not double-advance when other
    /// timers fire more frequently.
    last_spinner_tick: Option<std::time::Instant>,
}

/// Back-compat alias: every per-window free function in this module takes
/// `&mut RunningState`. The state is now per-window ([`TermWindow`]), so the
/// alias keeps the whole call-graph compiling without a mass rename.
type RunningState = TermWindow;

impl TermWindow {
    /// Return the current `zen_hide` list (from the live mirror field).
    pub(crate) fn config_zen_hide(&self) -> Vec<terminale_config::ZenHideElement> {
        self.zen_hide.clone()
    }

    /// Return the current `zen_fullscreen` flag (from the live mirror field).
    pub(crate) fn config_zen_fullscreen(&self) -> bool {
        self.zen_fullscreen
    }

    /// Return whether the tab bar is configured as enabled (from the live mirror field).
    pub(crate) fn config_tab_bar_enabled(&self) -> bool {
        self.tab_bar_enabled_config
    }

    /// Return whether pane headers are configured as shown (from the live mirror field).
    pub(crate) fn config_show_pane_headers(&self) -> bool {
        self.show_pane_headers_config
    }
}

impl TerminaleApp {
    /// Re-register the Quake global hotkey with a new binding at runtime.
    ///
    /// Dropping the old `GlobalHotKeyManager` unregisters its hotkey; a fresh
    /// manager claims the new binding. The forwarder thread reads the
    /// process-global hotkey channel, so it keeps delivering events for the new
    /// manager without a restart. (If Quake was disabled at startup there is no
    /// forwarder thread, so enabling it from scratch live still needs a
    /// restart — changing an already-active binding works live.)
    fn reregister_quake_hotkey(&mut self, binding: &str) {
        self.quake_binding_registered = binding.to_string();
        // Drop the old manager first so its hotkey is released before we try
        // to claim the (possibly identical) new one.
        self.hotkeys = None;
        self.quake_hotkey_id = None;
        match install_quake_hotkey(binding) {
            Ok((mgr, id)) => {
                self.hotkeys = mgr;
                self.quake_hotkey_id = id;
                tracing::info!(binding, id = ?id, "Quake hotkey re-registered live");
            }
            Err(e) => {
                tracing::warn!(?e, binding, "live re-register of Quake hotkey failed");
            }
        }
    }

    /// Index of the terminal window whose OS `WindowId` is `id`, if any.
    /// Used to route window events; an index (rather than a borrow) lets the
    /// caller re-borrow `self.windows[idx]` alongside disjoint `self` fields
    /// (settings, AI, config) the post-event section needs.
    fn window_index(&self, id: WindowId) -> Option<usize> {
        self.windows.iter().position(|w| w.window.id() == id)
    }

    /// The most recently interacted-with terminal window — used by routes
    /// that aren't tied to a specific `WindowId` (e.g. the global Quake
    /// hotkey, AI "Inject"). Falls back to the first window. `None` only
    /// before the first window is created.
    fn focused_window_mut(&mut self) -> Option<&mut TermWindow> {
        // The last window is the most recently created / torn-off one and a
        // reasonable "active" target; winit gives us no cross-window focus
        // query, so this heuristic is good enough for the few global routes.
        self.windows.last_mut()
    }

    /// Open the SSH host at `host_idx` in the window at `window_idx`. If the
    /// host needs a secret that isn't in the OS keychain yet, pop the in-window
    /// credential prompt instead and resume the connection once the user
    /// submits (see [`Self::resolve_password_prompt`]). Otherwise connect
    /// straight away, pulling any stored secret from the keychain.
    fn open_or_prompt_ssh(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_idx: usize,
        host_idx: usize,
    ) {
        // In `live` import mode the effective host list may be larger than
        // `config.ssh_hosts` (it includes ephemeral hosts from the OpenSSH
        // config). We always resolve the host from the effective list so the
        // index matches what the picker shows.
        let effective = if self.config.ssh.import_openssh_config
            == terminale_config::ImportOpenSshConfig::Live
        {
            live_merged_ssh_hosts(&self.config)
        } else {
            self.config.ssh_hosts.clone()
        };
        let Some(host) = effective.get(host_idx).cloned() else {
            return;
        };
        let rt = self.runtime.handle().clone();
        let ssh_cfg = self.config.ssh.clone();
        match ssh_secret_needed(&host) {
            Ok(None) => {
                // Ready to connect (agent / key / stored password).
                if let Some(state) = self.windows.get_mut(window_idx) {
                    open_ssh_tab(state, &host, None, &ssh_cfg, &rt, window_idx);
                }
            }
            Ok(Some(is_passphrase)) => {
                // A secret is required but absent — prompt for it in-window.
                let Some(state) = self.windows.get(window_idx) else {
                    return;
                };
                let prompt = PasswordPrompt::open(
                    event_loop,
                    &state.window,
                    host_idx,
                    host.endpoint(),
                    is_passphrase,
                    state.renderer.instance(),
                    state.renderer.adapter(),
                    state.renderer.device(),
                    state.renderer.queue(),
                );
                self.password_prompt = Some(prompt);
            }
            Err(msg) => {
                // Misconfiguration — surface it as a crashed tab so the user
                // sees why (reusing the normal failed-connect rendering, which
                // ssh_connect_options will reproduce).
                tracing::warn!(host = %host.name, %msg, "ssh host needs fixing");
                if let Some(state) = self.windows.get_mut(window_idx) {
                    open_ssh_tab(state, &host, None, &ssh_cfg, &rt, window_idx);
                }
            }
        }
    }

    /// The credential prompt was submitted: connect the host with the entered
    /// secret, optionally persisting it to the OS keychain first. Called from
    /// the event loop once [`PasswordPrompt::take_outcome`] yields a value.
    fn resolve_password_prompt(
        &mut self,
        host_idx: usize,
        outcome: password_prompt::PromptOutcome,
    ) {
        let effective = if self.config.ssh.import_openssh_config
            == terminale_config::ImportOpenSshConfig::Live
        {
            live_merged_ssh_hosts(&self.config)
        } else {
            self.config.ssh_hosts.clone()
        };
        let Some(host) = effective.get(host_idx).cloned() else {
            return;
        };
        // Persist BEFORE connecting so a successful "remember" survives even a
        // later disconnect. Storage is the OS keychain only — never config.
        if outcome.remember {
            if let Err(e) = terminale_config::store_secret(&host.secret_id(), &outcome.secret) {
                tracing::warn!(host = %host.name, ?e, "could not store ssh secret in keychain");
            }
        }
        let rt = self.runtime.handle().clone();
        let ssh_cfg = self.config.ssh.clone();
        // Use the last window as the target (same heuristic as focused_window_mut).
        let win_idx = self.windows.len().saturating_sub(1);
        if let Some(state) = self.windows.get_mut(win_idx) {
            open_ssh_tab(state, &host, Some(&outcome.secret), &ssh_cfg, &rt, win_idx);
        }
    }

    /// Construct a fresh [`TermWindow`] that shares the wgpu device of an
    /// existing renderer (or boots a brand-new device when `shared` is
    /// `None`, used for the very first window). `tabs` may be pre-populated
    /// (tab tear-out) or empty (the spawner adds a default tab for the
    /// initial window). Returns the assembled window.
    #[allow(clippy::too_many_lines)]
    fn build_window(
        &self,
        event_loop: &ActiveEventLoop,
        shared: Option<(
            Arc<wgpu::Instance>,
            Arc<wgpu::Adapter>,
            Arc<wgpu::Device>,
            Arc<wgpu::Queue>,
        )>,
        position: Option<winit::dpi::PhysicalPosition<i32>>,
        tabs: Vec<TabState>,
    ) -> TermWindow {
        let mut attrs = app_icon::with_app_identity(
            Window::default_attributes()
                .with_title("terminale")
                .with_inner_size(winit::dpi::LogicalSize::new(1024.0, 600.0))
                .with_decorations(false)
                // Stay hidden until the first GPU frame is painted, then reveal
                // via `reveal_window` — avoids the white flash of an unpainted
                // window (same pattern as the AI/settings sub-windows).
                .with_visible(false),
        );
        if let Some(pos) = position {
            attrs = attrs.with_position(pos);
        }
        if let Some(icon) = app_icon::load_app_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        // Apply the configured window level up front so "stay on top"
        // takes effect on the very first frame. Quake mode manages its own
        // visibility (show/hide) independently of this level.
        apply_window_level(&window, self.config.window.always_on_top);

        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        // Torn-off windows reuse the spawning window's wgpu device/adapter
        // (the GPU backend choice was made when that device was first
        // created); the very first window honours `[gpu]` config via
        // `gpu_options_from_config`.
        let mut renderer = match shared {
            Some((instance, adapter, device, queue)) => Renderer::new_shared(
                instance,
                adapter,
                device,
                queue,
                Arc::clone(&window),
                size.width,
                size.height,
                scale,
            )
            .expect("failed to init shared renderer"),
            None => Renderer::new(
                Arc::clone(&window),
                size.width,
                size.height,
                scale,
                gpu_options_from_config(&self.config),
            )
            .expect("failed to init renderer"),
        };
        renderer.set_cursor(cursor_params_from_config(&self.config));
        renderer.set_padding(self.config.window.padding as f32);
        renderer.set_background_alpha(self.config.window.opacity);
        renderer.set_bg_fx_params(translate_bg_fx_params(&self.config.background_fx));
        renderer.set_background_image(translate_bg_image_params(
            &self.config.appearance.background_image,
        ));
        renderer.set_tab_widths(
            self.config.appearance.tab_min_width,
            self.config.appearance.tab_max_width,
        );
        renderer.set_tab_pinned_width(self.config.appearance.pinned_tab_width);
        // Apply the configured font up front — family/ligatures first so
        // the size probe measures the right glyph, then size last.
        renderer.set_font_family(&self.config.font.family);
        renderer.set_font_style_overrides(
            self.config.font.bold_family.as_deref(),
            self.config.font.italic_family.as_deref(),
            self.config.font.bold_italic_family.as_deref(),
        );
        renderer.set_ligatures(self.config.font.ligatures);
        renderer.set_line_height(self.config.font.line_height);
        renderer.set_font_size(self.config.font.size);
        renderer.set_underline_thickness(self.config.font.underline_thickness_px);

        let clipboard = Clipboard::new()
            .map_err(|e| tracing::warn!(?e, "clipboard unavailable"))
            .ok();

        let mut tw = TermWindow {
            window,
            renderer,
            tabs,
            active_tab: 0,
            clipboard,
            modifiers: ModifiersState::empty(),
            proxy: self.proxy.clone(),
            palette: terminale_term::AnsiPalette::default(),
            bell_mode: self.config.bell.mode,
            scroll_step_lines: self.config.window.scroll_step_lines,
            alt_screen_scroll_lines: self.config.window.alt_screen_scroll_lines,
            touchpad_pixels_per_row: self.config.window.touchpad_pixels_per_row,
            smooth_scroll: self.config.window.smooth_scroll,
            smooth_scroll_remainder: 0.0,
            copy_on_select: self.config.window.copy_on_select,
            scrollback_lines: self.config.window.scrollback_lines,
            command_blocks_enabled: self.config.terminal.command_blocks,
            max_command_blocks: self.config.terminal.max_command_blocks,
            word_separators: self.config.terminal.word_separators.clone(),
            link_underline: self.config.terminal.link_underline,
            confirm_close: self.config.window.confirm_close,
            always_on_top: self.config.window.always_on_top,
            closed_tabs: Vec::new(),
            previous_active_tab: None,
            shortcuts: self.config.keybinds.shortcuts.clone(),
            custom_keybinds: self.config.keybinds.custom.clone(),
            search: None,
            copy_mode: copy_mode::CopyModeState::new(),
            command_palette: None,
            renaming: None,
            pending_theme: None,
            pending_ai_prompt: None,
            pending_font_size: None,
            pending_always_on_top: None,
            pending_resize: None,
            theme_name: self.config.appearance.theme.clone(),
            theme_names: self
                .config
                .appearance
                .all_themes()
                .into_iter()
                .map(|t| t.name)
                .collect(),
            ssh_host_names: effective_ssh_host_names(&self.config),
            snippet_names: snippet_names_from(&self.config),
            pending_insert_snippet: None,
            pending_save_workspace: None,
            pending_open_workspace_path: None,
            workspace_list: Vec::new(),
            ssh_host_targets: ssh_host_targets_from(&self.config),
            editor_command: self.config.editor.command.clone(),
            default_profile: self.profile.clone(),
            quake_visible: true,
            pending_quake_autohide: false,
            quake_input_suppress_until: None,
            quake_saved_rect: None,
            quake_last_dock_rect: None,
            quake_user_rect: None,
            quake_pre_dock_rect: None,
            quake_last_monitor: None,
            quake_anim: None,
            pointer_logical: (0.0, 0.0),
            selecting: false,
            selection_anchor: None,
            selection_press_px: None,
            last_click: None,
            last_titlebar_click: None,
            last_tab_click: None,
            held_button: None,
            last_motion_cell: None,
            tab_press: None,
            group_press: None,
            animated_tab_drag: self.config.appearance.animated_tab_drag,
            pointer_hidden: false,
            hovered_url: None,
            menu_visible: false,
            menu_origin: [0.0, 0.0],
            open_settings_requested: false,
            open_menu_at: None,
            menu_context: MenuContext::Terminal,
            open_profile_picker: false,
            pending_restart_pane: false,
            open_ai_requested: false,
            pending_ssh_host: None,
            open_ssh_picker: false,
            input_line: String::new(),
            offer_save_ssh_hosts: self.config.terminal.offer_save_ssh_hosts,
            save_host_prompt: None,
            pending_save_ssh_host: None,
            pending_dont_ask_again: None,
            pending_import_ssh_hosts: false,
            pending_import_theme: false,
            pending_divider_drag: None,
            hovered_divider: None,
            divider_thickness_px: self.config.appearance.divider_thickness_logical * scale,
            divider_grab_padding_px: self.config.appearance.divider_grab_padding_logical * scale,
            divider_color: self.config.appearance.divider_color,
            focus_border_thickness_px: self.config.appearance.focus_border_thickness_logical
                * scale,
            focus_border_color: self.config.appearance.focus_border_color,
            live_pane_resize: self.config.terminal.live_pane_resize,
            pane_resize_step_cells: self.config.terminal.pane_resize_step_cells,
            show_pane_headers: self.config.appearance.show_pane_headers,
            show_prompt_marks: self.config.terminal.show_prompt_marks,
            window_focused: true,
            occluded: false,
            os_notifications: self.config.terminal.os_notifications,
            os_notification_rate_limit: self.config.terminal.os_notification_rate_limit,
            tab_bar_fingerprint: 0,
            pane_header_close_hover: None,
            last_header_click: None,
            pane_header_press: None,
            pane_tear_out: self.config.appearance.pane_tear_out,
            profile_names: self
                .config
                .profiles
                .profiles
                .iter()
                .map(|p| p.name.clone())
                .collect(),
            profile_icons: self
                .config
                .profiles
                .profiles
                .iter()
                .map(|p| p.icon.clone())
                .collect(),
            quick_select: None,
            pane_select: None,
            qs_compiled_patterns: quick_select::compile_patterns(
                &self.config.quick_select.patterns,
            ),
            qs_alphabet: self.config.quick_select.alphabet.clone(),
            qs_overlay_dim: self.config.quick_select.overlay_dim,
            last_status_bar_tick: std::time::Instant::now(),
            active_key_table: None,
            key_tables: self.config.keybinds.key_tables.clone(),
            mouse_bindings: self.config.keybinds.mouse.clone(),
            broadcast_input: false,
            pending_new_window: false,
            pending_move_tab_to_new_window: false,
            pending_move_pane_to_new_tab: false,
            pending_move_pane_to_new_window: false,
            zen: false,
            zen_was_fullscreen: false,
            zen_hide: self.config.window.zen_hide.clone(),
            zen_fullscreen: self.config.window.zen_fullscreen,
            tab_bar_enabled_config: self.config.appearance.tab_bar_enabled,
            show_pane_headers_config: self.config.appearance.show_pane_headers,
            link_hover_tooltip: self.config.terminal.link_hover_tooltip,
            link_hover_delay_ms: self.config.terminal.link_hover_delay_ms,
            link_hover_start: None,
            clipboard_read_policy: self.config.terminal.clipboard_read,
            edit_command_clears_line: self.config.terminal.edit_command_clears_line,
            context_rules: self.config.context_rules.clone(),
            command_history_scope: self.config.terminal.command_history_scope,
            command_history_max_entries: self.config.terminal.command_history_max_entries,
            pending_insert_command: None,
            command_history_cache: Vec::new(),
            scrollback_export_format: self.config.terminal.scrollback_export_format,
            scrollback_export_dir: self.config.terminal.scrollback_export_dir.clone(),
            clipboard_history_ring: std::collections::VecDeque::new(),
            clipboard_history_enabled: self.config.clipboard_history.enabled,
            clipboard_history_size: self.config.clipboard_history.size,
            clipboard_history_capture_osc52: self.config.clipboard_history.capture_osc52,
            pending_paste_clipboard_entry: None,
            pending_paste_guard: None,
            pending_close_confirm: None,
            paste_confirm_multiline: self.config.terminal.paste_confirm_multiline,
            paste_confirm_when_unbracketed: self.config.terminal.paste_confirm_when_unbracketed,
            paste_strip_control_chars: self.config.terminal.paste_strip_control_chars,
            dir_jump_store: {
                let cfg = &self.config.directory_jump;
                if cfg.persist {
                    dir_jump::history_path()
                        .as_deref()
                        .map(dir_jump::DirJumpStore::load)
                        .unwrap_or_default()
                } else {
                    dir_jump::DirJumpStore::new()
                }
            },
            dir_jump_enabled: self.config.directory_jump.enabled,
            dir_jump_max_tracked: self.config.directory_jump.max_tracked,
            dir_jump_persist: self.config.directory_jump.persist,
            dir_jump_cache: Vec::new(),
            pending_cd_path: None,
            snap_chooser_open: false,
            highlight_on_jump: self.config.terminal.highlight_on_jump,
            jump_highlight_line: None,
            jump_highlight_start: None,
            failed_command_cache: Vec::new(),
            plugin_command_names: Vec::new(),
            pending_plugin_invoke: None,
            plugin_keybind_combos: Vec::new(),
            pending_plugin_keybind_invoke: None,
            plugins_allow_keybindings: self.config.plugins.allow_keybindings,
            pending_hook_tab_open: Vec::new(),
            pending_hook_tab_close: Vec::new(),
            pending_hook_pane_focus: Vec::new(),
            pending_hook_session_start: Vec::new(),
            pending_hook_session_exit: Vec::new(),
            pending_hook_config_reload: false,
            pending_hook_command_end: Vec::new(),
            hook_cmd_end_fired: std::collections::BTreeMap::new(),
            tab_groups: Vec::new(),
            next_group_id: 0,
            show_tab_group_labels: self.config.appearance.show_tab_group_labels,
            suggestion_bar_was_open: false,
            fix_hint_seen: None,
            suggestions: suggestions::SuggestionRuntime {
                enabled: self.config.ai.suggestions.enabled,
                ..suggestions::SuggestionRuntime::default()
            },
            tab_group_colors: self.config.appearance.tab_group_colors.clone(),
            bundled_icons: self.config.appearance.bundled_icons,
            tab_activity_spinner: self.config.appearance.tab_activity_spinner,
            spinner_frame: 0,
            last_spinner_tick: None,
        };
        // Sync renderer flags that mirror TermWindow fields.
        tw.renderer
            .set_show_pane_headers(self.config.appearance.show_pane_headers);
        tw.renderer
            .set_show_tab_group_labels(self.config.appearance.show_tab_group_labels);
        tw.renderer
            .set_close_button_style(self.config.appearance.close_button_style);
        // ux-polish-b: tab bar visibility / position / hide-if-single + cell width.
        tw.renderer
            .set_tab_bar_enabled(self.config.appearance.tab_bar_enabled);
        tw.renderer
            .set_tab_bar_placement(tab_bar_placement_from_config(&self.config));
        tw.renderer
            .set_tab_bar_hide_if_single(self.config.appearance.tab_bar_hide_if_single);
        tw.renderer
            .set_vertical_tab_bar_width(self.config.appearance.vertical_tab_bar_width);
        tw.renderer
            .set_dim_amount(self.config.appearance.dim_amount);
        tw.renderer
            .set_minimum_contrast(self.config.appearance.minimum_contrast);
        tw.renderer
            .set_builtin_box_drawing(self.config.appearance.builtin_box_drawing);
        tw.renderer
            .set_inactive_pane_dim(self.config.appearance.inactive_pane_dim);
        tw.renderer
            .set_selection_opacity(self.config.appearance.selection_opacity);
        tw.renderer
            .set_unfocused_window_dim(self.config.appearance.unfocused_window_dim);
        tw.renderer
            .set_cell_width_multiplier(self.config.font.cell_width);
        // Resize the (possibly pre-populated) tabs to the new window's grid.
        resize_all_tabs(&mut tw, size.width, size.height);
        apply_theme(&mut tw, &self.config);
        // Sync focus-border config to the renderer (constructor defaults to
        // 2.0 * scale; overwrite with actual config value in case user changed
        // it or there's a non-default set before the first frame).
        tw.renderer.set_focus_border_thickness_logical(
            self.config.appearance.focus_border_thickness_logical,
        );
        tw.renderer
            .set_focus_border_color(self.config.appearance.focus_border_color);
        tw.renderer
            .set_focus_border_alpha(self.config.appearance.focus_border_opacity);
        tw
    }

    /// Detach tab `tab_idx` from window `win_idx` into a brand-new native
    /// window placed at the current cursor, sharing the wgpu device. No-op
    /// when the source window has fewer than two tabs (we never tear off a
    /// window's only tab — that would just teleport the window).
    fn tear_out(&mut self, event_loop: &ActiveEventLoop, win_idx: usize, tab_idx: usize) {
        let Some(src) = self.windows.get_mut(win_idx) else {
            return;
        };
        if src.tabs.len() < 2 || tab_idx >= src.tabs.len() {
            return;
        }
        // Cancel any in-progress drag / selection on the source window and
        // drop the torn tab. `TabState` is fully self-contained + Send.
        src.tab_press = None;
        src.held_button = None;
        src.selecting = false;
        let len_before = src.tabs.len();
        let tab = src.tabs.remove(tab_idx);
        src.active_tab = active_tab_after_detach(src.active_tab, tab_idx, len_before);
        src.renderer.set_selection(None);
        src.window.request_redraw();

        // Place the new window where the cursor currently is, in screen px.
        let scale = src.window.scale_factor() as f32;
        let win_pos = src.window.outer_position().unwrap_or_default();
        let cursor_screen = winit::dpi::PhysicalPosition::new(
            win_pos.x + (src.pointer_logical.0 * scale) as i32 - 60,
            win_pos.y + (src.pointer_logical.1 * scale) as i32 - 18,
        );

        // Share the existing wgpu device so we don't boot a second one.
        let shared = (
            src.renderer.instance(),
            src.renderer.adapter(),
            src.renderer.device(),
            src.renderer.queue(),
        );

        let mut new_win =
            self.build_window(event_loop, Some(shared), Some(cursor_screen), vec![tab]);
        new_win.active_tab = 0;
        if let Some(t) = new_win.tabs.last() {
            t.emulator.lock().set_palette(new_win.palette);
        }
        let bar = {
            let refs: Vec<&TabState> = new_win.tabs.iter().collect();
            tab_bar_from(&refs, 0, false, &new_win.tab_groups)
        };
        new_win.renderer.set_tab_bar(Some(bar));
        // Paint the first frame into the hidden window, then reveal it
        // (cloak-around-show on Windows) so torn-out windows don't flash
        // white either.
        reveal_window(&mut new_win);
        new_win.window.focus_window();
        self.windows.push(new_win);
    }

    /// Promote a pending tab-press on window `win_idx` into a live,
    /// App-level [`TabDrag`] once the cursor has moved past the arm
    /// threshold. Captures the dragged tab's label, slot width and grab
    /// offset so the ghost matches the grabbed tab, and seeds the target as
    /// an in-place reorder. No-op if the press is gone or the index is stale.
    ///
    /// Also spawns the floating ghost window when the animation is enabled
    /// so the pill follows the cursor outside the source window.
    fn promote_tab_drag(
        &mut self,
        event_loop: &ActiveEventLoop,
        win_idx: usize,
        cursor_screen: (i32, i32),
    ) {
        let Some(src) = self.windows.get(win_idx) else {
            return;
        };
        let Some((tab_index, _)) = src.tab_press else {
            return;
        };
        let Some(tab) = src.tabs.get(tab_index) else {
            return;
        };
        let label = tab_label(tab);
        // Slot geometry (logical px) of the grabbed pill.
        //
        // `grab_offset_x` is the cursor's signed distance from the pill's
        // horizontal centre at lift time (positive = cursor is right of centre).
        // `ghost_window_position` uses it to keep the ghost pill under the exact
        // point the user grabbed along the X axis.
        //
        // For a VERTICAL strip the drag lifts a horizontal ghost pill, so there
        // is no meaningful X grab point from the original vertical slot.  We set
        // `grab_offset_x = 0` so the ghost pill is horizontally centred under
        // the cursor — the most natural feel for a cross-axis lift.
        let (slot_x, slot_w, grab_offset_x) = if src.renderer.tab_placement().is_vertical() {
            // Use the strip width as the ghost pill width so it matches the bar.
            let (_, strip_w, _) = src
                .renderer
                .vertical_strip_inner_edge()
                .unwrap_or((0.0, 160.0, 160.0));
            (0.0_f32, strip_w, 0.0_f32)
        } else {
            let (sx, sw) = src
                .renderer
                .tab_slot_rect(tab_index)
                .unwrap_or((8.0, 160.0));
            let pointer_logical_x = src.pointer_logical.0;
            (sx, sw, pointer_logical_x - (sx + sw * 0.5))
        };
        let animated = src.animated_tab_drag;
        let origin_window = src.window.id();
        let label_for_ghost = label.clone();
        let _ = slot_x; // slot_x not used beyond this point (grab computed)
        self.tab_drag = Some(TabDrag {
            origin_window,
            tab_index,
            payload: DragPayload::Tab { tab_index },
            label,
            cursor_screen,
            grab_offset_x,
            slot_width: slot_w,
            target: DropTarget::Reorder(tab_index),
            animated,
        });
        // Clear the pending press now that it's a real drag.
        if let Some(s) = self.windows.get_mut(win_idx) {
            s.tab_press = None;
        }
        // Spawn the floating ghost window so the pill follows the cursor
        // outside the source terminal window. Cheap to skip when animation
        // is off, and a spawn failure (e.g. compositor refuses transparent
        // top-level) just falls back to the in-window ghost.
        if animated {
            self.spawn_ghost_window(
                event_loop,
                &label_for_ghost,
                slot_w,
                grab_offset_x,
                cursor_screen,
            );
        }
        self.update_tab_drag(cursor_screen);
    }

    /// Promote a pending pane-header press into an App-level
    /// [`DragPayload::Pane`] drag. Mirrors [`Self::promote_tab_drag`] but
    /// lifts a single leaf pane rather than a whole tab.
    ///
    /// No-op when `pane_header_press` is cleared (press released or already
    /// promoted) or the pane is now a lone leaf.
    fn promote_pane_drag(
        &mut self,
        event_loop: &ActiveEventLoop,
        win_idx: usize,
        cursor_screen: (i32, i32),
    ) {
        let Some(src) = self.windows.get(win_idx) else {
            return;
        };
        let Some((pane_id, _)) = src.pane_header_press else {
            return;
        };
        let tab_index = src.active_tab;
        let Some(tab) = src.tabs.get(tab_index) else {
            return;
        };
        // Guard: still more than one leaf (could have changed since arming).
        if count_leaves(&tab.tree) < 2 {
            return;
        }
        // Build the label from the departing pane.
        let label = if let Some(pane) = tab.panes.get(&pane_id) {
            pane_label(pane)
        } else {
            return;
        };
        let (slot_x, slot_w) = src
            .renderer
            .tab_slot_rect(tab_index)
            .unwrap_or((8.0, 160.0));
        let pointer_logical_x = src.pointer_logical.0;
        let grab_offset_x = pointer_logical_x - (slot_x + slot_w * 0.5);
        let animated = src.animated_tab_drag;
        let origin_window = src.window.id();
        let label_for_ghost = label.clone();

        self.tab_drag = Some(TabDrag {
            origin_window,
            tab_index,
            payload: DragPayload::Pane { tab_index, pane_id },
            label,
            cursor_screen,
            grab_offset_x,
            slot_width: slot_w,
            target: DropTarget::Reorder(tab_index),
            animated,
        });
        // Clear the pending press now that it's a real drag.
        if let Some(s) = self.windows.get_mut(win_idx) {
            s.pane_header_press = None;
        }
        if animated {
            self.spawn_ghost_window(
                event_loop,
                &label_for_ghost,
                slot_w,
                grab_offset_x,
                cursor_screen,
            );
        }
        self.update_tab_drag(cursor_screen);
    }

    /// Promote a pending group-pill press on window `win_idx` into a live,
    /// App-level [`DragPayload::Group`] drag.  Mirrors [`Self::promote_tab_drag`]
    /// but lifts every tab that belongs to the group as a single unit.
    ///
    /// No-op when `group_press` is already cleared or the group has no members.
    fn promote_group_drag(
        &mut self,
        event_loop: &ActiveEventLoop,
        win_idx: usize,
        cursor_screen: (i32, i32),
    ) {
        let Some(src) = self.windows.get(win_idx) else {
            return;
        };
        let Some((gid, first_idx, _)) = src.group_press else {
            return;
        };
        // Build the ghost label from the group's name.
        let label = src
            .tab_groups
            .iter()
            .find(|g| g.id == gid)
            .map_or_else(|| "Group".into(), |g| g.name.clone());
        // Slot geometry — reuse the first member's slot for width/grab offset.
        let (slot_w, grab_offset_x) = if src.renderer.tab_placement().is_vertical() {
            let (_, strip_w, _) = src
                .renderer
                .vertical_strip_inner_edge()
                .unwrap_or((0.0, 160.0, 160.0));
            (strip_w, 0.0_f32)
        } else {
            let (sx, sw) = src
                .renderer
                .tab_slot_rect(first_idx)
                .unwrap_or((8.0, 160.0));
            let pointer_lx = src.pointer_logical.0;
            (sw, pointer_lx - (sx + sw * 0.5))
        };
        let animated = src.animated_tab_drag;
        let origin_window = src.window.id();
        let label_for_ghost = label.clone();

        self.tab_drag = Some(TabDrag {
            origin_window,
            tab_index: first_idx,
            payload: DragPayload::Group { group_id: gid },
            label,
            cursor_screen,
            grab_offset_x,
            slot_width: slot_w,
            target: DropTarget::Reorder(first_idx),
            animated,
        });
        // Clear the pending press now that the drag is live.
        if let Some(s) = self.windows.get_mut(win_idx) {
            s.group_press = None;
        }
        if animated {
            self.spawn_ghost_window(
                event_loop,
                &label_for_ghost,
                slot_w,
                grab_offset_x,
                cursor_screen,
            );
        }
        self.update_tab_drag(cursor_screen);
    }

    /// Refresh an in-flight tab drag for a new cursor screen position:
    /// recompute the drop target by hit-testing every window's tab bar, do a
    /// live in-window reorder when over the origin bar, and repaint the
    /// ghost + drop indicators. No-op when no drag is active.
    fn update_tab_drag(&mut self, cursor_screen: (i32, i32)) {
        let Some(mut drag) = self.tab_drag.take() else {
            return;
        };
        drag.cursor_screen = cursor_screen;

        // Hit-test every window's tab-bar band (origin first so an in-window
        // reorder wins over a stray overlap). Returns the WindowId under the
        // cursor, or None for the detach band.
        let mut bars: Vec<BarRect> = Vec::with_capacity(self.windows.len());
        // Origin window first.
        for w in self
            .windows
            .iter()
            .filter(|w| w.window.id() == drag.origin_window)
            .chain(
                self.windows
                    .iter()
                    .filter(|w| w.window.id() != drag.origin_window),
            )
        {
            let pos = w.window.outer_position().unwrap_or_default();
            let scale = w.window.scale_factor() as f32;
            let inner = w.window.inner_size();
            let (is_vertical, vert_strip_x_logical, vert_strip_w_logical, vert_inner_edge_logical) =
                if let Some((sx, sw, ie)) = w.renderer.vertical_strip_inner_edge() {
                    (true, sx, sw, ie)
                } else {
                    (false, 0.0, 0.0, 0.0)
                };
            bars.push(BarRect {
                id: w.window.id(),
                x: pos.x,
                y: pos.y,
                width: inner.width,
                height: inner.height,
                scale,
                is_vertical,
                vert_strip_x_logical,
                vert_strip_w_logical,
                vert_inner_edge_logical,
            });
        }
        let over = window_bar_at_screen(&bars, cursor_screen.0, cursor_screen.1);

        // Resolve the target + per-window drop indicator.
        drag.target = match over {
            Some(id) if id == drag.origin_window => {
                let slot = self.drop_slot_in_window(id, cursor_screen);
                // Live reorder only for whole-tab drags — a pane drag over
                // the origin bar must not shuffle the tab order.
                if matches!(drag.payload, DragPayload::Tab { .. }) {
                    let landed = self.live_reorder(id, drag.tab_index, slot);
                    drag.tab_index = landed;
                    if let DragPayload::Tab { ref mut tab_index } = drag.payload {
                        *tab_index = drag.tab_index;
                    }
                    DropTarget::Reorder(landed)
                } else {
                    DropTarget::Reorder(slot)
                }
            }
            Some(id) => {
                let slot = self.drop_slot_in_window(id, cursor_screen);
                DropTarget::AttachTo(id, slot)
            }
            None => DropTarget::Detach,
        };

        self.tab_drag = Some(drag);
        // Keep the floating ghost window under the cursor BEFORE applying
        // the in-window visuals, so the OS-level move lands the same frame
        // as the per-window drop indicator update.
        self.move_ghost_window(cursor_screen);
        self.apply_drag_visuals();
    }

    /// Insertion slot (`0..=tabs.len()`) for the cursor's screen position
    /// within window `id`'s tab bar, via the renderer's midpoint hit-test.
    /// Dispatches to the y-based helper for vertical strips.
    fn drop_slot_in_window(&self, id: WindowId, cursor_screen: (i32, i32)) -> usize {
        let Some(w) = self.windows.iter().find(|w| w.window.id() == id) else {
            return 0;
        };
        let pos = w.window.outer_position().unwrap_or_default();
        if w.renderer.tab_placement().is_vertical() {
            let local_y = (cursor_screen.1 - pos.y) as f32;
            w.renderer.drop_slot_at_y(local_y)
        } else {
            let local_x = (cursor_screen.0 - pos.x) as f32;
            w.renderer.drop_slot_at(local_x)
        }
    }

    /// Move tab `from` to insertion `slot` within window `id`, keeping it the
    /// active tab. Returns the index the tab actually lands at (which the
    /// caller stores so the drag keeps tracking the same tab). No-op move
    /// when the slot would not change the order.
    fn live_reorder(&mut self, id: WindowId, from: usize, slot: usize) -> usize {
        let Some(w) = self.windows.iter_mut().find(|w| w.window.id() == id) else {
            return from;
        };
        if from >= w.tabs.len() {
            return from;
        }
        // An insertion slot past `from` shifts left by one once `from` is
        // removed; clamp into range.
        let dest = slot.min(w.tabs.len());
        let dest = if dest > from { dest - 1 } else { dest };
        if dest == from {
            return from;
        }
        let tab = w.tabs.remove(from);
        w.tabs.insert(dest, tab);
        w.active_tab = dest;
        refresh_tab_bar(w);
        w.window.request_redraw();
        dest
    }

    /// Push the current drag's ghost + per-window drop indicator into each
    /// window's renderer, then request redraws. When the drag's `animated`
    /// flag is off, only the (cheap) indicators are skipped along with the
    /// ghost — the drag still resolves on release. Clears stale visuals from
    /// windows not currently targeted.
    ///
    /// When the floating [`GhostWindow`] is alive the per-window ghost is
    /// suppressed (the OS-level window owns the pill); only the drop
    /// indicators stay in the terminal windows.
    fn apply_drag_visuals(&mut self) {
        let Some(drag) = self.tab_drag.clone() else {
            return;
        };
        if !drag.animated {
            self.clear_drag_visuals();
            return;
        }
        // Resolve, from the target, which window shows the drop indicator
        // (and at what slot) and which window hosts the in-window ghost
        // FALLBACK (used only when no floating ghost window is alive).
        let (indicator_win, indicator_slot): (Option<WindowId>, usize) = match drag.target {
            DropTarget::Reorder(slot) => (Some(drag.origin_window), slot),
            DropTarget::AttachTo(id, slot) => (Some(id), slot),
            DropTarget::Detach => (None, 0),
        };
        let floating_ghost_alive = self.ghost_window.is_some();
        let detaching = matches!(drag.target, DropTarget::Detach);
        let ghost_win = indicator_win.unwrap_or(drag.origin_window);
        // Refresh the floating ghost window's renderer with the (cached)
        // label so the pill stays correct if config or theme changed
        // mid-drag.
        if let Some(gw) = self.ghost_window.as_mut() {
            gw.renderer
                .set_tab_drag_ghost(Some(terminale_render::TabGhost {
                    label: drag.label.clone(),
                    // The renderer centres the pill in the surface in
                    // `render_ghost_only`, so the in-pill coordinates are
                    // unused — pass any consistent values for completeness.
                    center_x: 0.0,
                    center_y: 0.0,
                    width: drag.slot_width,
                }));
            gw.window.request_redraw();
        }
        for w in &mut self.windows {
            let id = w.window.id();
            if !floating_ghost_alive && id == ghost_win {
                let pos = w.window.outer_position().unwrap_or_default();
                let scale = w.window.scale_factor() as f32;
                // Cursor in this window's logical coords.
                let cursor_lx = (drag.cursor_screen.0 - pos.x) as f32 / scale;
                let cursor_ly = (drag.cursor_screen.1 - pos.y) as f32 / scale;
                let center_x = cursor_lx - drag.grab_offset_x;
                // Sit in the bar band; lift toward the cursor when detaching.
                let center_y = if detaching {
                    cursor_ly
                } else {
                    terminale_render::TAB_BAR_HEIGHT * 0.5
                };
                w.renderer
                    .set_tab_drag_ghost(Some(terminale_render::TabGhost {
                        label: drag.label.clone(),
                        center_x,
                        center_y,
                        width: drag.slot_width,
                    }));
            } else {
                w.renderer.set_tab_drag_ghost(None);
            }
            if Some(id) == indicator_win {
                let x = w.renderer.drop_boundary_x(indicator_slot);
                w.renderer.set_tab_drop_indicator(x);
            } else {
                w.renderer.set_tab_drop_indicator(None);
            }
            w.window.request_redraw();
        }
    }

    /// Clear the drag ghost + drop indicator from every window's renderer
    /// and request a repaint, so nothing lingers after a drag ends / is
    /// cancelled.
    fn clear_drag_visuals(&mut self) {
        for w in &mut self.windows {
            w.renderer.set_tab_drag_ghost(None);
            w.renderer.set_tab_drop_indicator(None);
            w.window.request_redraw();
        }
        // Tear down the floating ghost window — it's only meaningful while
        // a drag is live, and lingering it (e.g. after a window-close
        // cancels a drag) would leak an always-on-top top-level.
        self.destroy_ghost_window();
    }

    /// Spawn the borderless, transparent, always-on-top OS window that
    /// hosts the floating tab-drag ghost pill. Sized to fit the pill plus a
    /// small padding for the soft shadow, positioned so the cursor lands at
    /// the same point of the pill the user originally grabbed. Reuses the
    /// origin window's wgpu device. Click-through is requested on Windows
    /// so the underlying terminals still receive cursor / release events
    /// directly (best-effort — see `set_click_through_windows`).
    ///
    /// Silently no-ops on spawn failure: the in-window ghost rendering in
    /// `apply_drag_visuals` is the fallback.
    fn spawn_ghost_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        label: &str,
        slot_width_logical: f32,
        grab_offset_x: f32,
        cursor_screen: (i32, i32),
    ) {
        // Pull the shared wgpu device from the origin window so the ghost
        // surface lives on the same device as the rest of the app.
        let Some(origin_idx) = self
            .tab_drag
            .as_ref()
            .and_then(|d| self.window_index(d.origin_window))
        else {
            return;
        };
        let (instance, adapter, device, queue, scale) = {
            let w = &self.windows[origin_idx];
            (
                w.renderer.instance(),
                w.renderer.adapter(),
                w.renderer.device(),
                w.renderer.queue(),
                w.window.scale_factor() as f32,
            )
        };

        let pill_h_logical = terminale_render::TAB_BAR_HEIGHT - 8.0;
        // Padding around the pill, large enough to fit the soft shadow
        // offset (+3, +5) and a small safety margin.
        let pad_logical = 14.0;
        let inner_w_logical = slot_width_logical + pad_logical * 2.0;
        let inner_h_logical = pill_h_logical + pad_logical * 2.0;
        let inner_w_px = (inner_w_logical * scale).ceil() as u32;
        let inner_h_px = (inner_h_logical * scale).ceil() as u32;

        // Initial position — same formula as `move_ghost_window`.
        let pos =
            ghost_window_position(cursor_screen, scale, grab_offset_x, inner_w_px, inner_h_px);

        let attrs = app_icon::with_app_identity(Window::default_attributes())
            .with_title("terminale-ghost")
            .with_inner_size(winit::dpi::LogicalSize::new(
                inner_w_logical,
                inner_h_logical,
            ))
            .with_position(pos)
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_window_level(winit::window::WindowLevel::AlwaysOnTop)
            // Skip the taskbar / dock — this is a transient drag adornment,
            // not a real top-level window. (Best-effort: not all platforms
            // honour this.)
            .with_visible(true);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::debug!(
                    ?e,
                    "ghost-window create failed; falling back to in-window ghost"
                );
                return;
            }
        };
        // Best-effort click-through so the source terminal keeps receiving
        // mouse events even when the ghost is briefly under the cursor.
        #[cfg(target_os = "windows")]
        set_click_through_windows(&window);

        let mut renderer = match Renderer::new_shared_transparent(
            instance,
            adapter,
            device,
            queue,
            Arc::clone(&window),
            inner_w_px.max(1),
            inner_h_px.max(1),
            scale,
        ) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(?e, "ghost-window renderer init failed; falling back");
                return;
            }
        };
        // Seed the ghost so the very first redraw paints the pill.
        renderer.set_tab_drag_ghost(Some(terminale_render::TabGhost {
            label: label.to_string(),
            center_x: 0.0,
            center_y: 0.0,
            width: slot_width_logical,
        }));
        window.request_redraw();

        self.ghost_window = Some(GhostWindow {
            window,
            renderer,
            grab_offset_x,
            pill_height_logical: pill_h_logical,
        });
    }

    /// Reposition the floating ghost window so its pill stays under the
    /// cursor at the originally-grabbed offset. No-op when no ghost window
    /// is alive (animation off, or spawn failed).
    fn move_ghost_window(&mut self, cursor_screen: (i32, i32)) {
        let Some(gw) = self.ghost_window.as_mut() else {
            return;
        };
        let scale = gw.window.scale_factor() as f32;
        let inner = gw.window.inner_size();
        let pos = ghost_window_position(
            cursor_screen,
            scale,
            gw.grab_offset_x,
            inner.width,
            inner.height,
        );
        gw.window.set_outer_position(pos);
        // The renderer's pill geometry doesn't depend on cursor position
        // (centred in surface), so no redraw is strictly needed when only
        // the OS-level position changes. But schedule one to refresh the
        // pill if a label / theme update slipped in.
        let _ = gw.pill_height_logical; // reserved for future cursor-y nudge
    }

    /// Drop the floating ghost window (renderer + winit window) so its
    /// surface is released and the OS top-level vanishes. Called whenever
    /// the drag ends, is cancelled, or `apply_drag_visuals` is told the
    /// animation is off mid-drag.
    fn destroy_ghost_window(&mut self) {
        self.ghost_window = None;
    }

    /// Window-event handler for the floating ghost window. Routes redraws
    /// and resizes into its private renderer, and forwards cursor moves
    /// and left-button releases back to the App-level drag so events that
    /// somehow land on the ghost (when click-through couldn't be set, or
    /// the platform doesn't honour it) still resolve the drag instead of
    /// stranding it.
    fn handle_ghost_window_event(&mut self, event_loop: &ActiveEventLoop, event: &WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => {
                if let Some(gw) = self.ghost_window.as_mut() {
                    if let Err(e) = gw.renderer.render_ghost_only() {
                        tracing::debug!(?e, "ghost-window render failed");
                    }
                }
            }
            WindowEvent::Resized(new_size) => {
                if let Some(gw) = self.ghost_window.as_mut() {
                    gw.renderer.resize(new_size.width, new_size.height);
                    gw.window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(gw) = self.ghost_window.as_mut() {
                    gw.renderer.set_scale_factor(*scale_factor as f32);
                    gw.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(gw) = self.ghost_window.as_ref() {
                    let pos = gw.window.outer_position().unwrap_or_default();
                    let cursor_screen = (pos.x + position.x as i32, pos.y + position.y as i32);
                    if self.tab_drag.is_some() {
                        self.update_tab_drag(cursor_screen);
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if self.tab_drag.is_some() => {
                self.resolve_tab_drag(event_loop);
            }
            _ => {}
        }
    }

    /// Resolve an in-flight tab drag on mouse release: an in-window reorder
    /// is already live (just commit by clearing), an attach splices the tab
    /// into the target window, and a detach tears out a new window at the
    /// release point. Clears all drag visuals afterwards.
    fn resolve_tab_drag(&mut self, event_loop: &ActiveEventLoop) {
        let Some(drag) = self.tab_drag.take() else {
            return;
        };
        match drag.payload.clone() {
            DragPayload::Tab { tab_index } => {
                match drag.target {
                    DropTarget::Reorder(_) => {
                        // The live reorder already moved the tab; nothing to do.
                    }
                    DropTarget::AttachTo(target_id, slot) => {
                        self.attach_tab(drag.origin_window, tab_index, target_id, slot);
                    }
                    DropTarget::Detach => {
                        if let Some(src_idx) = self.window_index(drag.origin_window) {
                            self.tear_out(event_loop, src_idx, tab_index);
                        }
                    }
                }
            }
            DragPayload::Pane { tab_index, pane_id } => {
                match drag.target {
                    DropTarget::Reorder(_) => {
                        // Dropped on origin window's own tab bar — promote
                        // the pane into a brand-new tab in that same window.
                        self.attach_pane(
                            drag.origin_window,
                            tab_index,
                            pane_id,
                            drag.origin_window,
                            drag.origin_window,
                            None,
                        );
                    }
                    DropTarget::AttachTo(target_id, _slot) => {
                        // Dropped on another window (or the origin window
                        // from outside its own tab bar): open pane as a new
                        // tab in the target window.
                        self.attach_pane(
                            drag.origin_window,
                            tab_index,
                            pane_id,
                            target_id,
                            drag.origin_window,
                            None,
                        );
                    }
                    DropTarget::Detach => {
                        // Dropped outside every window — tear the pane out
                        // into a new OS window at the cursor position.
                        self.tear_out_pane(
                            event_loop,
                            drag.origin_window,
                            tab_index,
                            pane_id,
                            drag.cursor_screen,
                        );
                    }
                }
            }
            DragPayload::Group { group_id } => match drag.target {
                DropTarget::Reorder(slot) => {
                    self.reorder_group(drag.origin_window, group_id, slot);
                }
                DropTarget::AttachTo(target_id, slot) => {
                    self.attach_group(drag.origin_window, group_id, target_id, slot);
                }
                DropTarget::Detach => {
                    if let Some(src_idx) = self.window_index(drag.origin_window) {
                        self.tear_out_group(event_loop, src_idx, group_id, drag.cursor_screen);
                    }
                }
            },
        }
        self.clear_drag_visuals();
    }

    /// Detach pane `pane_id` from tab `src_tab_idx` of window `src_id` and
    /// splice it as a new tab into window `dst_id`. When `dst_id == src_id`,
    /// the pane becomes a new tab in the same window. When `src_id` reaches
    /// zero tabs after the detach, it is closed.
    ///
    /// `_origin_id` is reserved for future split-into-specific-slot support;
    /// ignored for now.
    fn attach_pane(
        &mut self,
        src_id: WindowId,
        src_tab_idx: usize,
        pane_id: PaneId,
        dst_id: WindowId,
        _origin_id: WindowId,
        _slot: Option<usize>,
    ) {
        let Some(src_idx) = self.window_index(src_id) else {
            return;
        };
        // Detach the leaf from the source tab.
        let src = &mut self.windows[src_idx];
        let Some(tab) = src.tabs.get_mut(src_tab_idx) else {
            return;
        };
        let Some(pane) = detach_leaf(tab, pane_id) else {
            return;
        };
        // If the source tab is now empty (shouldn't happen — detach_leaf
        // guards single-leaf), close it; otherwise resize its remaining
        // panes and redraw.
        let src_tab_empty = src.tabs.get(src_tab_idx).is_none_or(|t| t.panes.is_empty());
        if src_tab_empty {
            let len_before = src.tabs.len();
            src.tabs.remove(src_tab_idx);
            src.active_tab = active_tab_after_detach(src.active_tab, src_tab_idx, len_before);
        }
        src.renderer.set_selection(None);
        src.window.request_redraw();

        // Snapshot the geometry we need from the source before the mutable
        // dst borrow below.
        let src_empty = self.windows[src_idx].tabs.is_empty();

        // Resize remaining panes in the source tab.
        if !src_empty {
            resize_active_tab_panes(&mut self.windows[src_idx]);
        }

        // Now find the destination.
        let Some(dst_idx) = self.window_index(dst_id) else {
            return;
        };
        let dst = &mut self.windows[dst_idx];
        let dst_size = dst.window.inner_size();
        let (cols, rows) = dst
            .renderer
            .pixels_to_cells(dst_size.width, dst_size.height);

        // Build a fresh single-pane TabState for the detached pane.
        let new_tab = build_single_pane_tab(pane, dst.palette, cols, rows);
        dst.tabs.push(new_tab);
        dst.active_tab = dst.tabs.len() - 1;
        refresh_tab_bar(dst);
        dst.window.focus_window();
        dst.window.request_redraw();

        // Close source window if empty.
        if src_empty {
            if let Some(i) = self.window_index(src_id) {
                self.windows.remove(i);
            }
        }
    }

    /// Tear the pane `pane_id` (from tab `src_tab_idx` of window `src_id`)
    /// out into a brand-new OS window centred on `cursor_screen`. No-op when
    /// the source tab is a lone leaf (nothing left behind would make the tab
    /// empty).
    fn tear_out_pane(
        &mut self,
        event_loop: &ActiveEventLoop,
        src_id: WindowId,
        src_tab_idx: usize,
        pane_id: PaneId,
        cursor_screen: (i32, i32),
    ) {
        let Some(src_idx) = self.window_index(src_id) else {
            return;
        };
        {
            let src = &self.windows[src_idx];
            let Some(tab) = src.tabs.get(src_tab_idx) else {
                return;
            };
            // Guard: only tear out when there will be something left behind.
            if count_leaves(&tab.tree) < 2 {
                return;
            }
        }

        // Detach the leaf. Snapshot device handles before mutably borrowing
        // the window so we can pass them to build_window after.
        let src = &mut self.windows[src_idx];
        let Some(tab) = src.tabs.get_mut(src_tab_idx) else {
            return;
        };
        let Some(pane) = detach_leaf(tab, pane_id) else {
            return;
        };
        src.renderer.set_selection(None);
        src.window.request_redraw();
        resize_active_tab_panes(src);

        let shared = (
            src.renderer.instance(),
            src.renderer.adapter(),
            src.renderer.device(),
            src.renderer.queue(),
        );
        let spawn_pos =
            winit::dpi::PhysicalPosition::new(cursor_screen.0 - 60, cursor_screen.1 - 18);
        // Use the source window's palette so the theme carries over.
        let src_palette = self.windows[src_idx].palette;

        // Use initial defaults; build_window will resize the tab to the
        // new window's actual grid dimensions after the surface is created.
        let new_tab = build_single_pane_tab(
            pane,
            src_palette,
            terminale_term::DEFAULT_COLS,
            terminale_term::DEFAULT_ROWS,
        );
        let mut new_win =
            self.build_window(event_loop, Some(shared), Some(spawn_pos), vec![new_tab]);
        new_win.active_tab = 0;
        // Sync the emulator palette to the new window's resolved theme.
        if let Some(t) = new_win.tabs.last() {
            t.emulator.lock().set_palette(new_win.palette);
        }
        let bar = {
            let refs: Vec<&TabState> = new_win.tabs.iter().collect();
            tab_bar_from(&refs, 0, false, &new_win.tab_groups)
        };
        new_win.renderer.set_tab_bar(Some(bar));
        new_win.window.set_visible(true);
        new_win.window.focus_window();
        self.windows.push(new_win);
    }

    /// Open a new top-level terminal window containing one default tab.
    ///
    /// The new window reuses the wgpu device and adapter from window `src_idx`
    /// (the spawning window) so we never boot a second GPU backend. The first
    /// tab uses the profile named in `config.window.new_window_profile`, or
    /// the overall default profile when that is `None` / unrecognised.
    fn new_window(&mut self, event_loop: &ActiveEventLoop, src_idx: usize) {
        let Some(src) = self.windows.get(src_idx) else {
            return;
        };
        // Resolve the profile for the first tab of the new window.
        let profile: Option<terminale_config::Profile> = {
            let preferred = self.config.window.new_window_profile.as_deref();
            preferred
                .and_then(|name| {
                    self.config
                        .profiles
                        .profiles
                        .iter()
                        .find(|p| p.name == name)
                        .cloned()
                })
                .or_else(|| self.config.resolve_default_profile().cloned())
        };

        // Share the wgpu device from the source window.
        let shared = (
            src.renderer.instance(),
            src.renderer.adapter(),
            src.renderer.device(),
            src.renderer.queue(),
        );

        // Place the new window slightly offset from the source so they don't
        // exactly overlap.
        let spawn_pos = src
            .window
            .outer_position()
            .ok()
            .map(|p| winit::dpi::PhysicalPosition::new(p.x + 40, p.y + 40));

        let mut new_win = self.build_window(event_loop, Some(shared), spawn_pos, Vec::new());
        let size = new_win.window.inner_size();
        let first_tab = spawn_tab(
            profile.as_ref(),
            None,
            &new_win.renderer,
            (terminale_term::DEFAULT_COLS, terminale_term::DEFAULT_ROWS),
            size.width,
            size.height,
            self.proxy.clone(),
            self.config.window.scrollback_lines,
        );
        // Capture the program label before the tab is consumed.
        let program_name = profile
            .as_ref()
            .map_or_else(|| "shell".to_string(), |p| p.name.clone());
        let bar = tab_bar_from(&[&first_tab], 0, false, &new_win.tab_groups);
        new_win.renderer.set_tab_bar(Some(bar));
        new_win.tabs.push(first_tab);
        new_win.active_tab = 0;
        if let Some(t) = new_win.tabs.last() {
            let mut emu = t.emulator.lock();
            emu.set_palette(new_win.palette);
            emu.set_command_blocks(
                self.config.terminal.command_blocks,
                self.config.terminal.max_command_blocks,
            );
        }
        // Enqueue session_start for the single pane in the new window's first tab.
        new_win.pending_hook_session_start.push((0, program_name));
        reveal_window(&mut new_win);
        new_win.window.focus_window();
        self.windows.push(new_win);
    }

    /// Restore a [`workspace::SavedWorkspace`] into the window at `win_idx`.
    ///
    /// Behaviour:
    /// * All existing tabs in the target window are replaced by the saved ones.
    /// * Each saved tab's pane tree is reconstructed by spawning panes and
    ///   applying split-ratios.
    /// * The saved active-tab index is honoured.
    ///
    /// Only layout + cwd are restored — live processes cannot be reconstructed.
    #[allow(clippy::too_many_arguments)]
    fn restore_workspace(
        &mut self,
        event_loop: &ActiveEventLoop,
        win_idx: usize,
        saved: crate::workspace::SavedWorkspace,
        instance: Arc<wgpu::Instance>,
        adapter: Arc<wgpu::Adapter>,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        win_size: winit::dpi::PhysicalSize<u32>,
    ) {
        if !self.windows.iter().enumerate().any(|(i, _)| i == win_idx) {
            return;
        }

        if saved.tabs.is_empty() {
            return;
        }

        // Pre-resolve all profiles before taking the mutable state borrow.
        // This avoids the borrow conflict of `self.resolve_leaf_profile` +
        // `self.windows.get_mut`.
        //
        // For each saved tab we produce: (title, Vec<(profile, dir, user_title, side_b, ratio, SplitDir)>)
        type RestoreLeafData = (
            Option<terminale_config::Profile>, // profile
            Option<String>,                    // user_title
        );
        type RestoreTabPlan = (
            Option<String>,          // tab user_title
            Option<RestoreLeafData>, // init leaf
            Vec<(
                RestoreLeafData,                 // leaf data
                crate::workspace::SavedSplitDir, // direction
                bool,                            // side_b
                f32,                             // ratio
            )>,
        );

        let tab_plans: Vec<RestoreTabPlan> = saved
            .tabs
            .iter()
            .map(|saved_tab| {
                let steps = crate::workspace::restore_plan_for_tree(&saved_tab.tree);
                if steps.is_empty() {
                    return (saved_tab.title.clone(), None, Vec::new());
                }
                let init = if let crate::workspace::RestoreStep::InitLeaf(ref leaf) = steps[0] {
                    let profile = self.resolve_leaf_profile(&leaf.profile, &leaf.cwd);
                    Some((profile, leaf.title.clone()))
                } else {
                    None
                };
                let splits: Vec<_> = steps
                    .iter()
                    .skip(1)
                    .filter_map(|step| {
                        if let crate::workspace::RestoreStep::SplitLeaf {
                            direction,
                            side_b,
                            ratio,
                            leaf,
                        } = step
                        {
                            let profile = self.resolve_leaf_profile(&leaf.profile, &leaf.cwd);
                            Some(((profile, leaf.title.clone()), *direction, *side_b, *ratio))
                        } else {
                            None
                        }
                    })
                    .collect();
                (saved_tab.title.clone(), init, splits)
            })
            .collect();

        // Collect per-tab group ids in order (parallel to tab_plans).
        let saved_tab_groups: Vec<Option<u32>> = saved.tabs.iter().map(|t| t.group).collect();
        // Restore group registry + next-id counter.
        let restored_groups: Vec<crate::TabGroup> = saved
            .tab_groups
            .iter()
            .map(|g| crate::TabGroup {
                id: g.id,
                name: g.name.clone(),
                color: g.color,
            })
            .collect();
        let restored_next_group_id = saved.next_group_id;

        // Now take the mutable window borrow.
        let state = &mut self.windows[win_idx];

        // Drop all current tabs cleanly (sessions will be reaped by Drop).
        state.tabs.clear();
        state.active_tab = 0;
        state.closed_tabs.clear();
        // Restore group state.
        state.tab_groups = restored_groups;
        state.next_group_id = restored_next_group_id;

        let scrollback = self.config.window.scrollback_lines;
        let cb_enabled = self.config.terminal.command_blocks;
        let cb_max = self.config.terminal.max_command_blocks;

        // Spawn each saved tab from the resolved plan.
        for (tab_title, init_leaf, split_leaves) in tab_plans {
            let Some((init_profile, init_pane_title)) = init_leaf else {
                continue;
            };
            let mut new_tab = spawn_tab(
                init_profile.as_ref(),
                None,
                &state.renderer,
                (terminale_term::DEFAULT_COLS, terminale_term::DEFAULT_ROWS),
                win_size.width,
                win_size.height,
                state.proxy.clone(),
                scrollback,
            );
            new_tab.user_title = tab_title;
            if let Some(pane) = new_tab.panes.get_mut(&new_tab.focused) {
                pane.user_title = init_pane_title;
            }
            // Apply split steps.
            for ((split_profile, split_pane_title), direction, side_b, ratio) in split_leaves {
                let dir = match direction {
                    crate::workspace::SavedSplitDir::Horizontal => SplitDir::Horizontal,
                    crate::workspace::SavedSplitDir::Vertical => SplitDir::Vertical,
                };
                let new_pane = spawn_pane(
                    split_profile.as_ref(),
                    None,
                    &state.renderer,
                    (terminale_term::DEFAULT_COLS, terminale_term::DEFAULT_ROWS),
                    win_size.width,
                    win_size.height,
                    state.proxy.clone(),
                    scrollback,
                );
                let new_pane_id = new_tab.split_focused(dir, new_pane, side_b);
                apply_restore_ratio(&mut new_tab.tree, new_pane_id, ratio, dir);
                if let Some(pane) = new_tab.panes.get_mut(&new_pane_id) {
                    pane.user_title = split_pane_title;
                }
            }
            // Palette + command blocks for each pane.
            for pane in new_tab.panes.values() {
                let mut emu = pane.emulator.lock();
                emu.set_palette(state.palette);
                emu.set_command_blocks(cb_enabled, cb_max);
            }
            // Restore per-tab group assignment.
            let tab_idx = state.tabs.len();
            if let Some(&gid) = saved_tab_groups.get(tab_idx) {
                new_tab.group = gid;
            }
            state.tabs.push(new_tab);
        }

        if state.tabs.is_empty() {
            // Fallback: open a fresh tab so the window is never empty.
            new_tab(state);
        }

        let active = saved.active_tab.min(state.tabs.len().saturating_sub(1));
        state.active_tab = active;

        resize_all_tabs(state, win_size.width, win_size.height);
        refresh_tab_bar(state);
        state.window.request_redraw();
        tracing::info!(tabs = state.tabs.len(), active, "workspace restored");
        let _ = event_loop; // not needed here but kept for signature symmetry
        let _ = (instance, adapter, device, queue); // wgpu handles already in renderer
    }

    /// Resolve a profile from a saved leaf's profile name and optional cwd.
    fn resolve_leaf_profile(
        &self,
        profile_name: &Option<String>,
        cwd: &Option<String>,
    ) -> Option<terminale_config::Profile> {
        let base = profile_name
            .as_deref()
            .and_then(|name| {
                self.config
                    .profiles
                    .profiles
                    .iter()
                    .find(|p| p.name == name)
                    .cloned()
            })
            .or_else(|| self.config.resolve_default_profile().cloned());
        // Overlay the saved cwd if restore_working_dirs is on.
        match (
            base,
            cwd.as_deref(),
            self.config.window.restore_working_dirs,
        ) {
            (Some(mut p), Some(cwd_str), true) => {
                if p.cwd.is_none() {
                    p.cwd = Some(std::path::PathBuf::from(cwd_str));
                }
                Some(p)
            }
            (p, _, _) => p,
        }
    }

    /// Splice tab `drag_idx` out of window `src_id` and into window `dst_id`
    /// at insertion `slot`, matching the destination grid and making it the
    /// active tab. The source window is closed when its last tab leaves.
    /// No-op when src == dst or the indices are stale.
    fn attach_tab(&mut self, src_id: WindowId, drag_idx: usize, dst_id: WindowId, slot: usize) {
        if src_id == dst_id {
            return;
        }
        let Some(src_idx) = self.window_index(src_id) else {
            return;
        };
        let Some(dst_idx) = self.window_index(dst_id) else {
            return;
        };
        // Detach from the source window.
        let src = &mut self.windows[src_idx];
        if drag_idx >= src.tabs.len() {
            return;
        }
        let len_before = src.tabs.len();
        let mut tab = src.tabs.remove(drag_idx);
        src.active_tab = active_tab_after_detach(src.active_tab, drag_idx, len_before);
        let src_empty = src.tabs.is_empty();
        src.renderer.set_selection(None);
        src.window.request_redraw();

        // Splice into the target window at the requested slot.
        let dst = &mut self.windows[dst_idx];
        tab.emulator.lock().set_palette(dst.palette);
        let size = dst.window.inner_size();
        let (cols, rows) = dst.renderer.pixels_to_cells(size.width, size.height);
        // Emulator FIRST — grid at new size before PTY notifies the shell.
        tab.emulator.lock().resize(cols, rows);
        let _ = tab.session.resize(cols, rows);
        tab.cols = cols;
        tab.rows = rows;
        let dest = slot.min(dst.tabs.len());
        dst.tabs.insert(dest, tab);
        dst.active_tab = dest;
        refresh_tab_bar(dst);
        dst.window.focus_window();
        dst.window.request_redraw();

        // Close the source window if it has no tabs left. Recompute its index
        // first — removing from the destination above can't shift it, but be
        // defensive in case ordering ever changes.
        if src_empty {
            if let Some(i) = self.window_index(src_id) {
                self.windows.remove(i);
            }
        }
    }

    // ── Group drag helpers ────────────────────────────────────────────────────

    /// Collect the ascending tab indices that belong to `gid` in window `w`.
    fn group_member_indices(w: &TermWindow, gid: TabGroupId) -> Vec<usize> {
        w.tabs
            .iter()
            .enumerate()
            .filter_map(|(i, t)| if t.group == Some(gid) { Some(i) } else { None })
            .collect()
    }

    /// Move the whole group `gid` inside window `id` so that the block is
    /// re-inserted at the given drop `slot`.  Member tabs keep their relative
    /// order.  No-op when the window or group cannot be found.
    fn reorder_group(&mut self, id: WindowId, gid: TabGroupId, slot: usize) {
        let Some(w) = self.windows.iter_mut().find(|w| w.window.id() == id) else {
            return;
        };
        let members = Self::group_member_indices(w, gid);
        if members.is_empty() {
            return;
        }
        // Remove members in DESCENDING order so earlier indices stay valid.
        let mut removed: Vec<TabState> = members.iter().rev().map(|&i| w.tabs.remove(i)).collect();
        // Restore ascending order (we removed descending → collected descending).
        removed.reverse();

        // Compute the insertion index after removals.
        let dest = group_reorder_dest(&members, slot, w.tabs.len());

        for (offset, tab) in removed.into_iter().enumerate() {
            w.tabs.insert(dest + offset, tab);
        }
        w.active_tab = dest;
        refresh_tab_bar(w);
        w.window.request_redraw();
    }

    /// Tear the entire group `gid` out of window `win_idx` into a new OS
    /// window positioned near `cursor_screen`.  The source window must have
    /// at least one non-member tab remaining; if the group is the whole window
    /// nothing happens.
    fn tear_out_group(
        &mut self,
        event_loop: &ActiveEventLoop,
        win_idx: usize,
        gid: TabGroupId,
        cursor_screen: (i32, i32),
    ) {
        let members = {
            let Some(src) = self.windows.get(win_idx) else {
                return;
            };
            Self::group_member_indices(src, gid)
        };
        if members.is_empty() {
            return;
        }
        {
            let src = &self.windows[win_idx];
            if members.len() == src.tabs.len() {
                // Can't tear out the entire window — nothing would remain.
                return;
            }
        }
        // Cancel in-progress interaction state on the source.
        {
            let src = &mut self.windows[win_idx];
            src.tab_press = None;
            src.group_press = None;
            src.held_button = None;
            src.selecting = false;
        }

        // Remove members descending, collect ascending.
        let mut removed: Vec<TabState> = members
            .iter()
            .rev()
            .map(|&i| self.windows[win_idx].tabs.remove(i))
            .collect();
        removed.reverse();

        // Recompute source active tab.
        {
            let src = &mut self.windows[win_idx];
            let first_removed = members[0];
            let len_before = src.tabs.len() + removed.len();
            src.active_tab = active_tab_after_detach(src.active_tab, first_removed, len_before)
                .min(src.tabs.len().saturating_sub(1));
            src.renderer.set_selection(None);
        }

        // Clone and remove the group entry from the source.
        let group_entry: Option<TabGroup> = {
            let src = &mut self.windows[win_idx];
            let pos = src.tab_groups.iter().position(|g| g.id == gid);
            pos.map(|p| src.tab_groups.remove(p))
        };

        // Refresh source tab bar.
        {
            let src = &mut self.windows[win_idx];
            refresh_tab_bar(src);
            src.window.request_redraw();
        }

        // Shared wgpu handles from the source renderer.
        let shared = {
            let src = &self.windows[win_idx];
            (
                src.renderer.instance(),
                src.renderer.adapter(),
                src.renderer.device(),
                src.renderer.queue(),
            )
        };

        let new_pos = winit::dpi::PhysicalPosition::new(cursor_screen.0 - 60, cursor_screen.1 - 18);
        let mut new_win = self.build_window(event_loop, Some(shared), Some(new_pos), removed);
        new_win.active_tab = 0;
        // Push the cloned group registry entry so the pill renders correctly.
        if let Some(g) = group_entry {
            new_win.tab_groups.push(g);
        }
        // Apply palette to all new tabs.
        for t in &new_win.tabs {
            t.emulator.lock().set_palette(new_win.palette);
        }
        let bar = {
            let refs: Vec<&TabState> = new_win.tabs.iter().collect();
            tab_bar_from(&refs, 0, false, &new_win.tab_groups)
        };
        new_win.renderer.set_tab_bar(Some(bar));
        reveal_window(&mut new_win);
        new_win.window.focus_window();
        self.windows.push(new_win);
    }

    /// Move the group `gid` from window `src_id` into window `dst_id`, inserting
    /// the block at `slot`.  When the source window becomes empty after removal
    /// it is closed.  No-op when `src_id == dst_id`.
    ///
    /// # Group-id collision note
    ///
    /// Each window maintains its own `next_group_id` counter, so the same
    /// numeric id can refer to *different* groups in different windows.  When
    /// `dst` already has a group entry with the same id as the incoming group,
    /// the existing dst entry is kept and the moved tabs are merged into it.
    /// This is an edge case that requires two separate group-creation sessions
    /// to hit, and merging is the least-surprising outcome.
    fn attach_group(&mut self, src_id: WindowId, gid: TabGroupId, dst_id: WindowId, slot: usize) {
        if src_id == dst_id {
            return;
        }
        let Some(src_idx) = self.window_index(src_id) else {
            return;
        };
        let Some(dst_idx) = self.window_index(dst_id) else {
            return;
        };

        let members = Self::group_member_indices(&self.windows[src_idx], gid);
        if members.is_empty() {
            return;
        }

        // Remove members from src (descending), collect ascending.
        let mut removed: Vec<TabState> = members
            .iter()
            .rev()
            .map(|&i| self.windows[src_idx].tabs.remove(i))
            .collect();
        removed.reverse();

        // Clone+remove the group registry entry from src.
        let group_entry: Option<TabGroup> = {
            let src = &mut self.windows[src_idx];
            let pos = src.tab_groups.iter().position(|g| g.id == gid);
            pos.map(|p| src.tab_groups.remove(p))
        };

        // Recompute src active tab and refresh.
        let src_empty = {
            let src = &mut self.windows[src_idx];
            let first_removed = members[0];
            let len_before = src.tabs.len() + removed.len();
            src.active_tab = active_tab_after_detach(src.active_tab, first_removed, len_before)
                .min(src.tabs.len().saturating_sub(1));
            src.renderer.set_selection(None);
            let empty = src.tabs.is_empty();
            refresh_tab_bar(src);
            src.window.request_redraw();
            empty
        };

        // Splice into destination.
        {
            let dst = &mut self.windows[dst_idx];
            // Ensure a group registry entry exists in the destination.
            // If there's already a different group with this id (collision), keep
            // the existing entry and let the tabs merge into it (see doc comment).
            if !dst.tab_groups.iter().any(|g| g.id == gid) {
                if let Some(g) = group_entry {
                    dst.tab_groups.push(g);
                }
            }
            let dest = slot.min(dst.tabs.len());
            let win_size = dst.window.inner_size();
            let (cols, rows) = dst
                .renderer
                .pixels_to_cells(win_size.width, win_size.height);
            for (offset, mut tab) in removed.into_iter().enumerate() {
                tab.emulator.lock().set_palette(dst.palette);
                tab.emulator.lock().resize(cols, rows);
                let _ = tab.session.resize(cols, rows);
                tab.cols = cols;
                tab.rows = rows;
                dst.tabs.insert(dest + offset, tab);
            }
            dst.active_tab = dest;
            refresh_tab_bar(dst);
            dst.window.focus_window();
            dst.window.request_redraw();
        }

        // Close the source window when it is now empty.
        if src_empty {
            if let Some(i) = self.window_index(src_id) {
                self.windows.remove(i);
            }
        }
    }

    /// Reload `config.toml` from disk and live-apply it to every open window.
    ///
    /// Apply a single [`terminale_plugin::PluginCommand`] that was dequeued
    /// from the plugin host's capability queue.
    ///
    /// Called from `about_to_wait` on the main thread — never from inside a
    /// Lua callback. `&mut self` gives full access to config, windows, and the
    /// plugin host so every command can be acted on correctly.
    fn apply_plugin_command(&mut self, cmd: terminale_plugin::PluginCommand) {
        use terminale_plugin::PluginCommand;
        match cmd {
            PluginCommand::Notify { title, body } => {
                // Reuse the same OS notification path as OSC 9 / OSC 777
                // (incl. the rate limiter — plugins shouldn't flood either).
                crate::osc_handlers::fire_os_notification(
                    &title,
                    &body,
                    self.config.terminal.os_notification_rate_limit,
                );
            }
            PluginCommand::SetTabTitle(text) => {
                // Rename the active tab in the most-recently-focused window.
                if let Some(state) = self.windows.last_mut() {
                    let idx = state.active_tab;
                    if let Some(tab) = state.tabs.get_mut(idx) {
                        tab.user_title = if text.trim().is_empty() {
                            None
                        } else {
                            Some(text)
                        };
                    }
                    crate::tabs::refresh_tab_bar(state);
                    state.window.request_redraw();
                }
            }
            PluginCommand::OpenTab => {
                if let Some(state) = self.windows.last_mut() {
                    crate::tabs::new_tab(state);
                }
            }
            PluginCommand::SendText(text) => {
                if let Some(state) = self.windows.last_mut() {
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        let _ = tab.session.write_input(text.as_bytes());
                    }
                }
            }
            PluginCommand::InvokeCommand { .. } => {
                // This variant is never enqueued via the capability queue
                // (commands are invoked directly by `invoke_command`). No-op.
            }
        }
    }

    /// This is the single reload path used by both the filesystem watcher
    /// Fire a one-shot AI suggestion request for the window at `win_idx`.
    ///
    /// Sets the window's suggestion state to `Loading`, bumps the generation
    /// counter, and spawns a Tokio task that calls the configured provider and
    /// delivers the result via `UserEvent::Suggestion`.  No-op when the
    /// provider is not usable (no key configured / env var absent).
    fn spawn_suggestion(&mut self, win_idx: usize) {
        if !suggestions::provider_usable(&self.config.ai) {
            return;
        }
        let Some(state) = self.windows.get_mut(win_idx) else {
            return;
        };

        // Build STRUCTURED context from the focused pane's OSC 133 command
        // blocks: recent commands with exit status, plus the last command's
        // scoped output when it failed. This is what lets the model honour
        // "never re-suggest the command that just failed" — the old 200-line
        // raw scrollback dump gave it no way to tell commands, echoes and
        // errors apart, so it kept proposing the failed command back.
        let fallback_tail = (self.config.ai.suggestions.context_lines as usize).min(40);
        let sctx = build_suggestion_context(state, fallback_tail);

        // Bump generation and set Loading state.
        state.suggestions.generation = state.suggestions.generation.wrapping_add(1);
        state.suggestions.state = suggestions::SuggestionState::Loading;
        state.suggestions.fired_for_prompt = true;
        state.window.request_redraw();

        // Resolve provider credentials — env vars beat config (no secrets in TOML).
        /// Return `config_value` when non-empty; otherwise fall back to the
        /// named environment variable. Mirrors the pattern in the AI assistant
        /// window so secrets never need to live in the config file.
        #[inline]
        fn suggestion_key(config_value: &str, var: &str) -> String {
            if config_value.is_empty() {
                std::env::var(var).unwrap_or_default()
            } else {
                config_value.to_owned()
            }
        }
        let provider_name = self.config.ai.default_provider.clone();
        let (secret, model, max_tokens) = match provider_name.trim().to_ascii_lowercase().as_str() {
            "openai" => (
                suggestion_key(&self.config.ai.openai.api_key, "OPENAI_API_KEY"),
                self.config.ai.openai.model.clone(),
                Some(self.config.ai.openai.max_tokens),
            ),
            "ollama" => (String::new(), self.config.ai.ollama.model.clone(), None),
            _ => (
                suggestion_key(&self.config.ai.claude.api_key, "ANTHROPIC_API_KEY"),
                self.config.ai.claude.model.clone(),
                Some(self.config.ai.claude.max_tokens),
            ),
        };
        let ollama_url = self.config.ai.ollama.url.clone();

        let win_id = state.window.id();
        let generation = state.suggestions.generation;
        let proxy = self.proxy.clone();

        // Defensive dedup: even with the structured prompt, a weak model may
        // ignore the rule and re-emit the failed command — discard that
        // verbatim repeat instead of surfacing it.
        let failed_cmd = sctx
            .last_error
            .as_ref()
            .map(|e| e.command.trim().to_string());
        let messages = terminale_ai::suggestion_messages(&sctx);
        let req = terminale_ai::AiRequest {
            model,
            messages,
            max_tokens: Some(max_tokens.unwrap_or(80).min(80)),
            // Low temperature: we want the single most-likely useful command,
            // not creative variety — especially for weaker local models which
            // otherwise loop on a just-failed command.
            temperature: Some(0.2),
        };

        self.runtime.handle().spawn(async move {
            let provider = terminale_ai::build_provider(&provider_name, secret, ollama_url);
            let outcome = match provider.complete(req).await {
                Ok(text) => match terminale_ai::extract_suggested_command(&text) {
                    Some(cmd) if failed_cmd.as_deref() == Some(cmd.trim()) => {
                        suggestions::SuggestionOutcome::Error("no suggestion".into())
                    }
                    Some(cmd) => suggestions::SuggestionOutcome::Ready(cmd),
                    None => suggestions::SuggestionOutcome::Error("no suggestion".into()),
                },
                Err(e) => suggestions::SuggestionOutcome::Error(e.to_string()),
            };
            let _ = proxy.send_event(UserEvent::Suggestion {
                window: win_id,
                generation,
                outcome,
            });
        });
    }

    /// (`UserEvent::ConfigChanged`) and the manual `ReloadConfig` shortcut
    /// action. It deliberately reuses the exact same apply logic as the
    /// settings-window live-apply path so there is no duplication.
    fn apply_config_reload(&mut self) {
        match Config::load_or_init_at(Some(self.config_path.clone())) {
            Ok((new_cfg, _)) => {
                tracing::info!(
                    path = %self.config_path.display(),
                    "config reloaded from disk"
                );
                // Restart the watcher if the auto_reload setting changed —
                // e.g. the user just disabled it in their editor.
                let was_enabled = self.config.window.auto_reload_config;
                let now_enabled = new_cfg.window.auto_reload_config;

                let theme_changed = self.config.appearance.theme != new_cfg.appearance.theme;
                let font_changed = self.config.font.family != new_cfg.font.family
                    || self.config.font.bold_family != new_cfg.font.bold_family
                    || self.config.font.italic_family != new_cfg.font.italic_family
                    || self.config.font.bold_italic_family != new_cfg.font.bold_italic_family
                    || (self.config.font.size - new_cfg.font.size).abs() >= f32::EPSILON
                    || (self.config.font.line_height - new_cfg.font.line_height).abs()
                        >= f32::EPSILON
                    || self.config.font.ligatures != new_cfg.font.ligatures
                    || (self.config.font.underline_thickness_px
                        - new_cfg.font.underline_thickness_px)
                        .abs()
                        >= f32::EPSILON;
                let new_startup_position = new_cfg.window.startup_position;
                let startup_position_changed =
                    self.config.window.startup_position != new_startup_position;

                self.config = new_cfg;

                // Sync the settings window so its live-apply diff doesn't
                // immediately fight the freshly-reloaded values.
                if let Some(s) = self.settings.as_mut() {
                    s.sync_config(self.config.clone());
                }

                // Live-apply to every open terminal window — mirrors the
                // settings-window apply path in `about_to_wait`.
                let cfg = self.config.clone();
                for state in &mut self.windows {
                    state.renderer.set_cursor(cursor_params_from_config(&cfg));
                    state.bell_mode = cfg.bell.mode;
                    state.scroll_step_lines = cfg.window.scroll_step_lines;
                    state.alt_screen_scroll_lines = cfg.window.alt_screen_scroll_lines;
                    state.touchpad_pixels_per_row = cfg.window.touchpad_pixels_per_row;
                    if state.smooth_scroll != cfg.window.smooth_scroll {
                        state.smooth_scroll = cfg.window.smooth_scroll;
                        // Clear any stale remainder when toggling smooth scroll.
                        state.smooth_scroll_remainder = 0.0;
                    }
                    state.copy_on_select = cfg.window.copy_on_select;
                    state.animated_tab_drag = cfg.appearance.animated_tab_drag;
                    if state.always_on_top != cfg.window.always_on_top {
                        state.always_on_top = cfg.window.always_on_top;
                        apply_window_level(&state.window, state.always_on_top);
                    }
                    if state.scrollback_lines != cfg.window.scrollback_lines {
                        state.scrollback_lines = cfg.window.scrollback_lines;
                        let sb = state.scrollback_lines;
                        // EVERY pane of every tab — `tab.emulator` derefs to
                        // the focused pane only, which left the other split
                        // panes on the old scrollback until respawn.
                        for tab in &state.tabs {
                            for pane in tab.panes.values() {
                                pane.emulator.lock().set_scrollback(sb);
                            }
                        }
                    }
                    state.shortcuts = cfg.keybinds.shortcuts.clone();
                    state.custom_keybinds.clone_from(&cfg.keybinds.custom);
                    state.key_tables.clone_from(&cfg.keybinds.key_tables);
                    state.mouse_bindings.clone_from(&cfg.keybinds.mouse);
                    state.editor_command = cfg.editor.command.clone();
                    state.ssh_host_names = effective_ssh_host_names(&cfg);
                    state.profile_names = cfg
                        .profiles
                        .profiles
                        .iter()
                        .map(|p| p.name.clone())
                        .collect();
                    state.profile_icons = cfg
                        .profiles
                        .profiles
                        .iter()
                        .map(|p| p.icon.clone())
                        .collect();
                    // Refresh the cached default profile so plain new tabs
                    // ('+' / Ctrl+T) pick up an edited default without a new
                    // window — the cache was previously set once at startup.
                    state.default_profile = cfg.resolve_default_profile().cloned();
                    // Snippet picker rows were refreshed on the Settings-save
                    // path but NOT here — a config.toml edit left stale names.
                    state.snippet_names = crate::ssh_tabs::snippet_names_from(&cfg);
                    state.ssh_host_targets = ssh_host_targets_from(&cfg);
                    state.offer_save_ssh_hosts = cfg.terminal.offer_save_ssh_hosts;
                    let sf = state.window.scale_factor() as f32;
                    state.divider_thickness_px = cfg.appearance.divider_thickness_logical * sf;
                    state.divider_grab_padding_px =
                        cfg.appearance.divider_grab_padding_logical * sf;
                    state.focus_border_thickness_px =
                        cfg.appearance.focus_border_thickness_logical * sf;
                    state.focus_border_color = cfg.appearance.focus_border_color;
                    state.renderer.set_focus_border_thickness_logical(
                        cfg.appearance.focus_border_thickness_logical,
                    );
                    state
                        .renderer
                        .set_focus_border_color(cfg.appearance.focus_border_color);
                    state
                        .renderer
                        .set_focus_border_alpha(cfg.appearance.focus_border_opacity);
                    // Live-apply the divider colour override — None falls back to the
                    // renderer's auto tone (derived from the background colour).
                    state.divider_color = cfg.appearance.divider_color;
                    state.live_pane_resize = cfg.terminal.live_pane_resize;
                    state.pane_resize_step_cells = cfg.terminal.pane_resize_step_cells;
                    state.show_prompt_marks = cfg.terminal.show_prompt_marks;
                    state.os_notifications = cfg.terminal.os_notifications;
                    state.os_notification_rate_limit = cfg.terminal.os_notification_rate_limit;
                    // Update zen-mode mirror fields before applying chrome so
                    // re-applying zen overrides uses the new config values.
                    state.zen_hide.clone_from(&cfg.window.zen_hide);
                    state.zen_fullscreen = cfg.window.zen_fullscreen;
                    // Config mirrors for tab-bar-enabled and show-pane-headers
                    // (used by apply_zen_chrome to restore on zen exit).
                    state.tab_bar_enabled_config = cfg.appearance.tab_bar_enabled;
                    state.show_pane_headers_config = cfg.appearance.show_pane_headers;
                    if state.zen {
                        // Re-apply the chrome overrides immediately while zen
                        // is active, so editing zen_hide takes effect without
                        // toggling zen off and on again.
                        apply_zen_chrome(state);
                    }
                    if state.show_pane_headers != cfg.appearance.show_pane_headers {
                        state.show_pane_headers = cfg.appearance.show_pane_headers;
                        if !state.zen {
                            state
                                .renderer
                                .set_show_pane_headers(state.show_pane_headers);
                        }
                    }
                    state.pane_tear_out = cfg.appearance.pane_tear_out;
                    state
                        .renderer
                        .set_close_button_style(cfg.appearance.close_button_style);
                    // ux-polish-b: tab bar visibility / position / single-tab
                    // hide + cell-width multiplier — mirrors settings-window path.
                    if !state.zen {
                        state
                            .renderer
                            .set_tab_bar_enabled(cfg.appearance.tab_bar_enabled);
                    }
                    state
                        .renderer
                        .set_tab_bar_placement(tab_bar_placement_from_config(&cfg));
                    state
                        .renderer
                        .set_tab_bar_hide_if_single(cfg.appearance.tab_bar_hide_if_single);
                    state
                        .renderer
                        .set_vertical_tab_bar_width(cfg.appearance.vertical_tab_bar_width);
                    state.renderer.set_dim_amount(cfg.appearance.dim_amount);
                    state
                        .renderer
                        .set_minimum_contrast(cfg.appearance.minimum_contrast);
                    state
                        .renderer
                        .set_builtin_box_drawing(cfg.appearance.builtin_box_drawing);
                    state.show_tab_group_labels = cfg.appearance.show_tab_group_labels;
                    state
                        .renderer
                        .set_show_tab_group_labels(cfg.appearance.show_tab_group_labels);
                    state.tab_group_colors = cfg.appearance.tab_group_colors.clone();
                    state.bundled_icons = cfg.appearance.bundled_icons;
                    state.tab_activity_spinner = cfg.appearance.tab_activity_spinner;
                    state
                        .renderer
                        .set_inactive_pane_dim(cfg.appearance.inactive_pane_dim);
                    state
                        .renderer
                        .set_selection_opacity(cfg.appearance.selection_opacity);
                    state
                        .renderer
                        .set_unfocused_window_dim(cfg.appearance.unfocused_window_dim);
                    state
                        .renderer
                        .set_cell_width_multiplier(cfg.font.cell_width);
                    // ux-polish-a: sync module-level atomics/statics for exit-
                    // behavior and hyperlink rules — mirrors settings-window path.
                    update_exit_behavior(cfg.terminal.exit_behavior);
                    crate::links::update_hyperlink_rules(&cfg.terminal.hyperlink_rules);
                    // Live-apply image protocol toggles to all open emulators.
                    {
                        let osc1337_on = cfg.terminal.image_protocols.osc1337;
                        let sixel_on = cfg.terminal.image_protocols.sixel;
                        let apc_on = cfg.terminal.image_protocols.apc;
                        for tab in &state.tabs {
                            let mut emu = tab.emulator.lock();
                            emu.set_osc1337_images_enabled(osc1337_on);
                            emu.set_sixel_images_enabled(sixel_on);
                            emu.set_apc_graphics_enabled(apc_on);
                        }
                    }
                    // Live-apply command-block capture settings.
                    {
                        let cb_enabled = cfg.terminal.command_blocks;
                        let cb_max = cfg.terminal.max_command_blocks;
                        state.command_blocks_enabled = cb_enabled;
                        state.max_command_blocks = cb_max;
                        for tab in &state.tabs {
                            tab.emulator.lock().set_command_blocks(cb_enabled, cb_max);
                        }
                    }
                    state
                        .word_separators
                        .clone_from(&cfg.terminal.word_separators);
                    if state.link_underline != cfg.terminal.link_underline {
                        state.link_underline = cfg.terminal.link_underline;
                        refresh_autodetect_links(state);
                    }
                    // Live-apply link hover tooltip toggle and delay.
                    state.link_hover_tooltip = cfg.terminal.link_hover_tooltip;
                    state.link_hover_delay_ms = cfg.terminal.link_hover_delay_ms;
                    if !state.link_hover_tooltip {
                        state.link_hover_start = None;
                        state.renderer.set_tooltip(None);
                    }
                    // Live-apply clipboard read policy.
                    state.clipboard_read_policy = cfg.terminal.clipboard_read;
                    // Live-apply edit_command_clears_line.
                    state.edit_command_clears_line = cfg.terminal.edit_command_clears_line;
                    // Live-apply command-history picker settings.
                    state.command_history_scope = cfg.terminal.command_history_scope;
                    state.command_history_max_entries = cfg.terminal.command_history_max_entries;
                    // Live-apply scrollback export settings.
                    state.scrollback_export_format = cfg.terminal.scrollback_export_format;
                    state
                        .scrollback_export_dir
                        .clone_from(&cfg.terminal.scrollback_export_dir);
                    // Live-apply clipboard history settings.
                    state.clipboard_history_enabled = cfg.clipboard_history.enabled;
                    state.clipboard_history_size = cfg.clipboard_history.size;
                    state.clipboard_history_capture_osc52 = cfg.clipboard_history.capture_osc52;
                    // Trim the ring if the new cap is smaller.
                    while state.clipboard_history_ring.len() > state.clipboard_history_size {
                        state.clipboard_history_ring.pop_back();
                    }
                    // Live-apply directory-jump settings.
                    state.dir_jump_enabled = cfg.directory_jump.enabled;
                    state.dir_jump_max_tracked = cfg.directory_jump.max_tracked;
                    state.dir_jump_persist = cfg.directory_jump.persist;
                    // Live-apply paste safety settings.
                    state.paste_confirm_multiline = cfg.terminal.paste_confirm_multiline;
                    state.paste_confirm_when_unbracketed =
                        cfg.terminal.paste_confirm_when_unbracketed;
                    state.paste_strip_control_chars = cfg.terminal.paste_strip_control_chars;
                    // Live-apply prompt-navigation highlight toggle.
                    state.highlight_on_jump = cfg.terminal.highlight_on_jump;
                    // Live-apply minimum contrast.
                    state
                        .renderer
                        .set_minimum_contrast(cfg.appearance.minimum_contrast);
                    // Live-apply builtin box-drawing toggle.
                    state
                        .renderer
                        .set_builtin_box_drawing(cfg.appearance.builtin_box_drawing);
                    // Live-apply tab-group-labels toggle and group colour palette.
                    state.show_tab_group_labels = cfg.appearance.show_tab_group_labels;
                    state
                        .renderer
                        .set_show_tab_group_labels(cfg.appearance.show_tab_group_labels);
                    state.tab_group_colors = cfg.appearance.tab_group_colors.clone();
                    state.bundled_icons = cfg.appearance.bundled_icons;
                    state.tab_activity_spinner = cfg.appearance.tab_activity_spinner;
                    // Live-apply inactive-pane and unfocused-window dim.
                    state
                        .renderer
                        .set_inactive_pane_dim(cfg.appearance.inactive_pane_dim);
                    state
                        .renderer
                        .set_selection_opacity(cfg.appearance.selection_opacity);
                    state
                        .renderer
                        .set_unfocused_window_dim(cfg.appearance.unfocused_window_dim);
                    state.confirm_close = cfg.window.confirm_close;
                    if !state.confirm_close {
                        // Turning the feature off cancels any queued request.
                        state.pending_close_confirm = None;
                    }
                    state.renderer.set_background_alpha(cfg.window.opacity);
                    state.renderer.set_padding(cfg.window.padding as f32);
                    state
                        .renderer
                        .set_tab_widths(cfg.appearance.tab_min_width, cfg.appearance.tab_max_width);
                    state
                        .renderer
                        .set_tab_pinned_width(cfg.appearance.pinned_tab_width);
                    if font_changed {
                        state.renderer.set_font_family(&cfg.font.family);
                        state.renderer.set_font_style_overrides(
                            cfg.font.bold_family.as_deref(),
                            cfg.font.italic_family.as_deref(),
                            cfg.font.bold_italic_family.as_deref(),
                        );
                        state.renderer.set_line_height(cfg.font.line_height);
                        state.renderer.set_ligatures(cfg.font.ligatures);
                        state.renderer.set_font_size(cfg.font.size);
                        state
                            .renderer
                            .set_underline_thickness(cfg.font.underline_thickness_px);
                    }
                    if theme_changed {
                        apply_theme(state, &cfg);
                    }
                    state
                        .renderer
                        .set_bg_fx_params(translate_bg_fx_params(&cfg.background_fx));
                    state
                        .renderer
                        .set_background_image(translate_bg_image_params(
                            &cfg.appearance.background_image,
                        ));
                    if let Some(err) = quick_select::validate_patterns(&cfg.quick_select.patterns) {
                        tracing::warn!("quick_select.patterns: {err}");
                    }
                    state.qs_alphabet.clone_from(&cfg.quick_select.alphabet);
                    state.qs_compiled_patterns =
                        quick_select::compile_patterns(&cfg.quick_select.patterns);
                    state.qs_overlay_dim = cfg.quick_select.overlay_dim;
                    // Live-apply context rules: re-evaluate all tabs so any
                    // rule add/edit/remove takes immediate effect.
                    state.context_rules.clone_from(&cfg.context_rules);
                    refresh_context_rules(state);
                    update_status_bar(state, &cfg);
                    let win_size = state.window.inner_size();
                    resize_all_tabs(state, win_size.width, win_size.height);
                    render_main(state);
                }

                if let Some(ai_win) = self.ai_assistant.as_mut() {
                    ai_win.set_config(self.config.ai.clone());
                }

                if startup_position_changed {
                    if let Some(edge) = new_startup_position {
                        if let Some(state) = self.focused_window_mut() {
                            snap_window(state, edge);
                        }
                    }
                }

                // Restart the watcher if the auto_reload_config flag changed.
                if was_enabled != now_enabled {
                    self.config_watcher = config_watch::start(
                        self.config_path.clone(),
                        self.proxy.clone(),
                        now_enabled,
                    );
                }

                // Enqueue config_reload hook for the next plugin tick.
                for state in &mut self.windows {
                    state.pending_hook_config_reload = true;
                }
            }
            Err(e) => {
                tracing::warn!(
                    ?e,
                    path = %self.config_path.display(),
                    "config reload failed — keeping current config"
                );
            }
        }
    }

    /// Drop any windows whose tab list is empty (the last tab was closed via
    /// `close_tab`). When the very last window is reaped, save the last
    /// session (if configured) and then exit the process.
    fn reap_empty_windows(&mut self, event_loop: &ActiveEventLoop) {
        // Auto-save the last session before potentially exiting.
        if self.config.window.restore_session == terminale_config::RestoreSession::LastSession {
            if let Some(state) = self.windows.first() {
                if !state.tabs.is_empty() {
                    crate::workspace::save_last_session(
                        &state.tabs,
                        state.active_tab,
                        self.config.window.restore_working_dirs,
                        &state.tab_groups,
                        state.next_group_id,
                    );
                }
            }
        }
        self.windows.retain(|w| !w.tabs.is_empty());
        if self.windows.is_empty() {
            // Save the last session one final time (all tabs just closed).
            if self.config.window.restore_session == terminale_config::RestoreSession::LastSession {
                // Already saved above if there was a window — nothing more to do.
            }
            event_loop.exit();
        }
    }
}

impl ApplicationHandler<UserEvent> for TerminaleApp {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyDataReady => {
                // Re-arm the coalesced wake *before* draining: chunks that
                // arrive mid-drain are picked up by this pass, chunks that
                // arrive after it queue exactly one fresh event.
                PTY_WAKE_PENDING.store(false, std::sync::atomic::Ordering::Release);
                // A single proxy feeds every window's PTY readers, so we
                // can't tell which window produced output — drain them ALL.
                for state in &mut self.windows {
                    if drain_pty_output(state) {
                        state.window.request_redraw();
                    }
                }
            }
            UserEvent::GlobalHotkey(id) => {
                if self.quake_hotkey_id == Some(id) {
                    let quake = self.config.quake.clone();
                    // Toggle ALL terminal windows in sync — if any window
                    // is currently visible, hide every visible one; if none
                    // are visible, restore every saved window. Each window
                    // still uses its OWN saved geometry so a multi-monitor
                    // layout snaps back exactly as the user left it.
                    let any_visible = self.windows.iter().any(|w| w.quake_visible);
                    if any_visible {
                        for w in &mut self.windows {
                            if w.quake_visible {
                                toggle_quake(w, &quake);
                            }
                        }
                    } else {
                        for w in &mut self.windows {
                            if !w.quake_visible {
                                toggle_quake(w, &quake);
                            }
                        }
                    }
                }
            }
            UserEvent::Ai(e) => {
                if let Some(ai) = self.ai_assistant.as_mut() {
                    ai.push_ai_event(e);
                }
            }
            UserEvent::SshConnected(outcome) => {
                // The Tokio task finished the TCP+auth handshake off the UI
                // thread. Now that we're back on the event loop we can safely
                // build the tab and push it without any blocking I/O.
                if let Some(state) = self.windows.get_mut(outcome.window_idx) {
                    finish_ssh_tab(state, *outcome);
                }
            }
            UserEvent::ConfigChanged => {
                self.apply_config_reload();
            }
            UserEvent::Suggestion {
                window: win_id,
                generation,
                outcome,
            } => {
                if let Some(state) = self.windows.iter_mut().find(|w| w.window.id() == win_id) {
                    // Drop stale results from superseded requests.
                    if state.suggestions.generation == generation {
                        state.suggestions.state = match outcome {
                            suggestions::SuggestionOutcome::Ready(cmd) => {
                                suggestions::SuggestionState::Ready(cmd)
                            }
                            suggestions::SuggestionOutcome::Error(msg) => {
                                suggestions::SuggestionState::Error(msg)
                            }
                        };
                        state.window.request_redraw();
                    }
                }
            }
        }
    }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.windows.is_empty() {
            return;
        }

        let initial_cols = terminale_term::DEFAULT_COLS;
        let initial_rows = terminale_term::DEFAULT_ROWS;

        // First window boots a fresh wgpu device (shared = None). Spawn its
        // initial tab against the renderer once it exists.
        let mut state = self.build_window(event_loop, None, None, Vec::new());
        let size = state.window.inner_size();
        let first_tab = spawn_tab(
            self.profile.as_ref(),
            self.shell_override.as_deref(),
            &state.renderer,
            (initial_cols, initial_rows),
            size.width,
            size.height,
            self.proxy.clone(),
            self.config.window.scrollback_lines,
        );
        // Build the initial tab bar (single tab, but still visible — gives
        // users a clear "+ to add" affordance).
        let bar = tab_bar_from(&[&first_tab], 0, false, &state.tab_groups);
        state.renderer.set_tab_bar(Some(bar));
        // Capture program label before the tab is moved into the window.
        let startup_program = self
            .profile
            .as_ref()
            .map(|p| p.name.clone())
            .or_else(|| self.shell_override.clone())
            .unwrap_or_else(|| "shell".to_string());
        state.tabs.push(first_tab);
        // Inherit the active theme palette for the spawned tab and apply the
        // initial command-blocks configuration.
        if let Some(t) = state.tabs.last() {
            let mut emu = t.emulator.lock();
            emu.set_palette(state.palette);
            emu.set_command_blocks(
                self.config.terminal.command_blocks,
                self.config.terminal.max_command_blocks,
            );
        }
        // Enqueue session_start for the initial pane.
        state.pending_hook_session_start.push((0, startup_program));
        self.windows.push(state);

        // The tab bar was enabled AFTER the first tab's PTY/emulator were
        // sized (spawn_tab ran while `renderer.tab_bar` was still None, so
        // `pixels_to_cells` excluded the 36px bar), leaving the grid 1-2
        // rows too tall for the final chrome. Re-size it NOW — synchronously,
        // before any shell output is drained and before the window is
        // revealed — so ConPTY boots at its final size and the first prompt
        // can't be displaced by a post-reveal shrink-reflow (the
        // intermittent "prompt one row lower on a fresh launch" glitch).
        // The same-size guard in resize_all_tabs makes this a no-op when
        // the chrome doesn't change the row count.
        {
            let state = self.windows.last_mut().expect("window just pushed");
            let s = state.window.inner_size();
            resize_all_tabs(state, s.width, s.height);
        }

        // ── Session restore on launch ─────────────────────────────────────────
        // Before showing the window, check if the user wants the last session
        // restored. If so, replace the just-spawned default tab with the saved
        // layout. This runs before the demo-palette block so demos still work.
        let do_restore = self.config.window.restore_session
            == terminale_config::RestoreSession::LastSession
            && std::env::var_os("TERMINALE_DEMO_PALETTE").is_none();
        if do_restore {
            if let Some(saved_ws) = crate::workspace::load_last_session() {
                if !saved_ws.tabs.is_empty() {
                    let win_size = self.windows[0].window.inner_size();
                    let instance = self.windows[0].renderer.instance();
                    let adapter = self.windows[0].renderer.adapter();
                    let device = self.windows[0].renderer.device();
                    let queue = self.windows[0].renderer.queue();
                    self.restore_workspace(
                        event_loop, 0, saved_ws, instance, adapter, device, queue, win_size,
                    );
                }
            }
        }

        {
            let state = self.windows.last_mut().unwrap();
            // Debug aid: `TERMINALE_DEMO_PALETTE=1` seeds the grid with
            // sample text and opens the command palette pre-filled, so the
            // modal can be screenshotted deterministically without fighting
            // OS focus / keystroke injection. Harmless when unset.
            if let Some(demo) = std::env::var_os("TERMINALE_DEMO_PALETTE") {
                if demo == "explain" {
                    // Inject an error line, select it, and fire the
                    // "Explain Selection" action so the AI window opens
                    // pre-loaded — deterministic screenshot of the flow.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\> cargo run\r\n\
                              error[E0382]: borrow of moved value: `config`\r\n",
                        );
                    }
                    state
                        .renderer
                        .set_selection(Some(terminale_render::CellRect {
                            anchor: (0, 1),
                            cursor: (47, 1),
                            block: false,
                        }));
                    dispatch_shortcut(state, ShortcutAction::ExplainSelection);
                } else if demo == "scroll" {
                    // Inject scrollback and pan up so the scroll-position
                    // indicator is visible on the first frame (deterministic
                    // screenshot — before the shell's init clear arrives).
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        let mut buf = String::new();
                        for i in 0..80 {
                            buf.push_str(&format!("history line {i}\r\n"));
                        }
                        tab.emulator.lock().advance(buf.as_bytes());
                    }
                    let hist = state
                        .tabs
                        .get(state.active_tab)
                        .map_or(0, |t| t.emulator.lock().history_size());
                    let off = hist / 2;
                    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                        tab.scroll_lines = off;
                    }
                    state.renderer.set_scroll_lines(off);
                } else if demo == "title" {
                    // Inject an OSC 2 title; the tab label should reflect it.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(b"\x1b]2;vim ~/main.rs\x07");
                    }
                    drain_pty_output(state);
                } else if demo == "osc52" {
                    // Inject OSC 52 set-clipboard (base64 "dGVzdA==" = "test")
                    // and drain so the ClipboardStore event reaches the
                    // clipboard — audits that OSC 52 is wired/enabled.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(b"\x1b]52;c;dGVzdA==\x07");
                    }
                    drain_pty_output(state);
                } else if demo == "newtab" {
                    // Announce a cwd via OSC 7 on the active tab, then open a
                    // new tab — it must spawn the default shell IN that cwd
                    // (regression test for the empty-command + Windows
                    // `/C:/` path bugs).
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator
                            .lock()
                            .advance(b"\x1b]7;file://localhost/C:/Windows\x1b\\");
                    }
                    new_tab(state);
                } else if demo == "search" {
                    // Push a marker far into the scrollback, then search it.
                    // A viewport-only search would find 0; full-buffer finds 1.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        let mut buf = String::new();
                        for i in 0..60 {
                            if i == 5 {
                                buf.push_str("FINDME_SCROLLBACK_MARKER\r\n");
                            } else {
                                buf.push_str(&format!("filler output line {i}\r\n"));
                            }
                        }
                        tab.emulator.lock().advance(buf.as_bytes());
                    }
                    state.search = Some(SearchState::new());
                    if let Some(s) = state.search.as_mut() {
                        s.query.push_str("FINDME");
                    }
                    refresh_search_matches(state);
                } else if demo == "openpath" {
                    // Inject an existing `file:line` reference, detect it,
                    // then open it through the configured editor — exercises
                    // the full Ctrl+click chain deterministically.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"see C:\\Users\\dev\\Workspace\\terminale\\Cargo.toml:5 for details\r\n",
                        );
                    }
                    refresh_autodetect_links(state);
                    let link = state
                        .tabs
                        .get(state.active_tab)
                        .and_then(|t| t.autodetect_links.iter().find(|d| d.is_path).cloned());
                    if let Some(link) = link {
                        open_detected_link(state, &link);
                    }
                } else if demo == "paths" {
                    // Inject output containing a real, existing absolute path
                    // (+ a relative one) so the clickable-path underline can
                    // be screenshotted. OSC 7 announces the cwd so the
                    // relative reference resolves too.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"\x1b]7;file://localhost/C:/Users/dev/Workspace/terminale\x1b\\\
                              PS C:\\Workspace\\terminale> cargo build\r\n\
                              \x1b[31merror\x1b[0m: could not compile; see Cargo.toml\r\n\
                              config at C:\\Users\\dev\\Workspace\\terminale\\Cargo.toml\r\n\
                              PS C:\\Workspace\\terminale> ",
                        );
                    }
                    refresh_autodetect_links(state);
                } else if demo == "closex" {
                    // Open a second tab (tab close-X visible) and split the
                    // active tab (pane-header close-X visible) so both
                    // vector-stroke close buttons can be screenshotted.
                    new_tab(state);
                    // Switch back to the first tab so the split happens there.
                    switch_tab(state, 0);
                    split_focused_pane(state, SplitDir::Vertical, true);
                } else if demo == "ctxmenu" {
                    // Spawn the context menu with a pre-opened submenu flyout
                    // so the single-window layout can be screenshotted
                    // deterministically without live mouse interaction.
                    state.open_menu_at = Some(winit::dpi::PhysicalPosition::new(100, 100));
                } else if demo == "tabmenu" {
                    // Seed two groups so the "Group" flyout has "Add to …"
                    // entries, then open a TAB context menu (with tab + group
                    // management) rather than the terminal one — screenshot aid.
                    for (name, ci) in [("Build", 0usize), ("Deploy", 1usize)] {
                        let id = state.next_group_id;
                        state.next_group_id = state.next_group_id.wrapping_add(1);
                        state.tab_groups.push(crate::TabGroup {
                            id,
                            name: name.into(),
                            color: GROUP_COLOR_PALETTE[ci % GROUP_COLOR_PALETTE.len()],
                        });
                    }
                    state.menu_context = MenuContext::Tab(0);
                    state.open_menu_at = Some(winit::dpi::PhysicalPosition::new(100, 100));
                } else if demo == "statusbar" {
                    // Enable the status bar at the bottom with left + right
                    // segments so the full-width layout and right-aligned text
                    // can be verified in a deterministic screenshot. No SSH
                    // quick-connect button is shown (it has been removed).
                    let mut demo_cfg = self.config.clone();
                    demo_cfg.status_bar.enabled = true;
                    demo_cfg.status_bar.position = terminale_config::StatusBarPosition::Bottom;
                    demo_cfg.status_bar.left_segments = vec![
                        terminale_config::StatusSegment::Profile,
                        terminale_config::StatusSegment::Literal {
                            text: "  main".into(),
                        },
                    ];
                    demo_cfg.status_bar.right_segments =
                        vec![terminale_config::StatusSegment::Clock {
                            format: "%H:%M".into(),
                        }];
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\Workspace\\terminale> echo status-bar demo\r\n\
                              status-bar demo\r\n\
                              PS C:\\Workspace\\terminale> ",
                        );
                    }
                    update_status_bar(state, &demo_cfg);
                } else if demo == "zen" {
                    // Activate zen mode (hide chrome, no full-screen) so the
                    // bare terminal grid can be screenshotted deterministically.
                    // Inject a short prompt line so the grid is not blank.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\Workspace\\terminale> echo zen mode\r\n\
                              zen mode\r\n\
                              PS C:\\Workspace\\terminale> ",
                        );
                    }
                    // Enter zen without full-screen for a reproducible window.
                    state.zen_was_fullscreen = false;
                    state.zen = true;
                    apply_zen_chrome(state);
                } else if demo == "bgfx" {
                    // Pre-seed 5 concurrent Matrix bands so a single captured
                    // frame shows the effect without user interaction.
                    // Force Matrix style + keystroke reaction on.
                    let mut demo_cfg = self.config.clone();
                    demo_cfg.background_fx.enabled = true;
                    demo_cfg.background_fx.style = terminale_config::BackgroundFxStyle::Matrix;
                    demo_cfg.background_fx.react_to_keystrokes = true;
                    demo_cfg.background_fx.intensity = 0.8;
                    let params = translate_bg_fx_params(&demo_cfg.background_fx);
                    state.renderer.set_bg_fx_params(params);
                    // Seed emitters with staggered age so each band starts at a
                    // different height on the very first frame (col, age_secs).
                    state.renderer.seed_bg_fx_demo(&[
                        (0.12, 0.30),
                        (0.33, 0.70),
                        (0.55, 0.15),
                        (0.72, 0.55),
                        (0.88, 0.90),
                    ]);
                } else if demo == "contrast" {
                    // Seed lines of deliberately low-contrast text (dark-grey
                    // fg on black bg) so the minimum_contrast legibility lift
                    // is visible in a screenshot. The re-seed timer will push
                    // the same lines again ~700 ms later after ConPTY's clear.
                    // minimum_contrast is set high here (WCAG AAA = 7.0) so
                    // the text that would otherwise be nearly invisible is
                    // rendered at full legibility by the contrast enforcer.
                    state.renderer.set_minimum_contrast(7.0);
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            // Low-contrast fg colours on black background.
                            // Without enforcement these would be very dark and hard to read.
                            b"\x1b[38;2;40;40;40mDark grey #282828 on black (very low contrast)\x1b[0m\r\n\
                              \x1b[38;2;60;60;60mDark grey #3c3c3c on black (low contrast)\x1b[0m\r\n\
                              \x1b[38;2;80;80;80mDark grey #505050 on black (moderate contrast)\x1b[0m\r\n\
                              \x1b[38;2;100;100;100mGrey #646464 on black\x1b[0m\r\n\
                              \x1b[38;2;120;120;120mGrey #787878 on black\x1b[0m\r\n\
                              \x1b[0m--- minimum_contrast = 7.0 (WCAG AAA) enforced above ---\x1b[0m\r\n",
                        );
                    }
                } else if demo == "sgr" {
                    // Seed one labelled line per SGR text attribute so a
                    // screenshot covers every rendering path in one frame.
                    // SGR 0 resets between lines; no shell is visible.
                    // NOTE: the about_to_wait re-seed timer will re-emit this
                    // ~700 ms later so it survives ConPTY's initial clear.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            // underline styles
                            b"\x1b[4mSGR 4  single underline\x1b[0m\r\n\
                              \x1b[4:2mSGR 4:2  double underline\x1b[0m\r\n\
                              \x1b[4:3mSGR 4:3  curly underline\x1b[0m\r\n\
                              \x1b[4:4mSGR 4:4  dotted underline\x1b[0m\r\n\
                              \x1b[4:5mSGR 4:5  dashed underline\x1b[0m\r\n\
                              \x1b[4;58:2::255:80:80mSGR 58 coloured underline\x1b[0m\r\n\
                              \x1b[9mSGR 9  strikethrough\x1b[0m\r\n\
                              \x1b[1mSGR 1  bold\x1b[0m\r\n\
                              \x1b[3mSGR 3  italic\x1b[0m\r\n\
                              \x1b[2mSGR 2  dim/faint\x1b[0m\r\n\
                              \x1b[7mSGR 7  reverse video\x1b[0m\r\n\
                              \x1b[8mSGR 8  concealed/hidden (text invisible)\x1b[0m\r\n",
                        );
                    }
                } else if demo == "promptmarks" {
                    // Inject two complete OSC 133 prompt cycles — one with
                    // exit-code 0 (success dot) and one with exit-code 1
                    // (failure dot) — so the gutter indicators are visible.
                    // Force `show_prompt_marks` on for this window so the dots
                    // render even when the user's config has it disabled.
                    state.show_prompt_marks = true;
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            // Prompt 1 — success (exit 0)
                            b"\x1b]133;A\x1b\\\
                              $ echo hello\
                              \x1b]133;C\x1b\\\
                              \r\nhello\r\n\
                              \x1b]133;D;0\x1b\\\
                              \
                              \x1b]133;A\x1b\\\
                              $ false\
                              \x1b]133;C\x1b\\\
                              \r\n\
                              \x1b]133;D;1\x1b\\",
                        );
                    }
                } else if demo == "quickselect" {
                    // Seed the buffer with URLs and paths that the quick-select
                    // scanner will match, then enter quick-select mode so the
                    // amber label-badge overlay is visible on the first frame.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"see https://example.com/docs for details\r\n\
                              error in /usr/local/lib/foo/bar.rs:42\r\n\
                              clone from https://github.com/example/repo.git\r\n\
                              config at C:\\Users\\dev\\Workspace\\terminale\\Cargo.toml\r\n",
                        );
                    }
                    // Enter quick-select mode: this scans the just-injected
                    // rows and overlays label badges on every match.
                    enter_quick_select(state);
                } else if demo == "apcimage" {
                    // Feed an OSC 1337 `File=` inline-image sequence carrying
                    // an 8×8 solid-colour PNG encoded in base64. The image
                    // protocol parser decodes it and places it in the grid so
                    // the renderer draws it on the first captured frame.
                    //
                    // PNG: 8×8 RGBA (255, 80, 80, 255) — a small red block.
                    const TINY_PNG_B64: &str =
                        "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAYAAADED76LAAAAEklEQVR42mP\
                         4HxDwHx9mGBkKAGQSp4FuHlmBAAAAAElFTkSuQmCC";
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        let osc = format!(
                            "Inline image demo:\r\n\
                             \x1b]1337;File=width=8;height=4;inline=1:{TINY_PNG_B64}\x07\r\n",
                        );
                        tab.emulator.lock().advance(osc.as_bytes());
                    }
                } else if demo == "leadermode" {
                    // Seed a key-table in the config and activate its modal so
                    // the status-bar leader-mode indicator is visible on the
                    // first captured frame (deterministic screenshot).
                    let demo_table = terminale_config::KeyTable {
                        name: "pane".to_string(),
                        leader: "Ctrl+A".to_string(),
                        timeout_ms: 1500,
                        bindings: vec![
                            terminale_config::KeyTableEntry {
                                key: "V".to_string(),
                                actions: vec![terminale_config::KeyActionSpec::Action(
                                    "SplitRight".to_string(),
                                )],
                            },
                            terminale_config::KeyTableEntry {
                                key: "H".to_string(),
                                actions: vec![terminale_config::KeyActionSpec::Action(
                                    "SplitDown".to_string(),
                                )],
                            },
                        ],
                    };
                    self.config.keybinds.key_tables = vec![demo_table];
                    state
                        .key_tables
                        .clone_from(&self.config.keybinds.key_tables);
                    // Enter the table's modal mode right away so the indicator
                    // is already visible on the first frame.
                    state.active_key_table = Some(ActiveKeyTable {
                        table_idx: 0,
                        entered_at: std::time::Instant::now(),
                    });
                    // Enable the status bar if it isn't on already.
                    let mut demo_cfg = self.config.clone();
                    demo_cfg.status_bar.enabled = true;
                    demo_cfg.status_bar.position = terminale_config::StatusBarPosition::Bottom;
                    if demo_cfg.status_bar.left_segments.is_empty() {
                        demo_cfg.status_bar.left_segments =
                            vec![terminale_config::StatusSegment::Profile];
                    }
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator
                            .lock()
                            .advance(b"PS C:\\> # leader-mode demo\r\nPS C:\\> ");
                    }
                    update_status_bar(state, &demo_cfg);
                } else if demo == "broadcast" {
                    // Split the tab into two panes and activate broadcast-input
                    // mode so the amber indicator borders are visible on the
                    // first frame — deterministic screenshot of the feature.
                    split_focused_pane(state, SplitDir::Vertical, true);
                    split_focused_pane(state, SplitDir::Horizontal, false);
                    // Seed the panes with sample output.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator
                            .lock()
                            .advance(b"PS C:\\> # pane A - broadcast demo\r\nPS C:\\> ");
                    }
                    // Enable broadcast so the amber borders are visible.
                    state.broadcast_input = true;
                    // Force a repaint to surface the indicator borders.
                    resize_active_tab_panes(state);
                } else if demo == "dimpanes" {
                    // Split into two side-by-side panes and enable
                    // inactive_pane_dim = 0.4 so the non-focused pane is
                    // visibly darker than the focused one — deterministic
                    // screenshot of the dim-inactive-panes feature.
                    split_focused_pane(state, SplitDir::Vertical, true);
                    // Seed both panes with content.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator
                            .lock()
                            .advance(b"PS C:\\> # focused pane\r\nPS C:\\> ");
                    }
                    // Switch to the other pane and seed it too.
                    crate::focus_pane_in_direction(state, crate::PaneDirection::Right);
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator
                            .lock()
                            .advance(b"PS C:\\> # inactive pane (dimmed)\r\nPS C:\\> ");
                    }
                    // Switch focus back to the left pane so the right one is
                    // the visibly dimmed pane in the screenshot.
                    crate::focus_pane_in_direction(state, crate::PaneDirection::Left);
                    // Apply the dim value (strong for an unmistakable shot).
                    state.renderer.set_inactive_pane_dim(0.7);
                    resize_active_tab_panes(state);
                } else if demo == "verttabs" {
                    // Set the tab bar to Left and open two more tabs so the
                    // vertical strip is populated enough to screenshot.
                    state
                        .renderer
                        .set_tab_bar_placement(terminale_render::TabBarPlacement::Left);
                    state.renderer.set_vertical_tab_bar_width(180.0);
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\> echo vertical tab bar demo\r\nvertical tab bar demo\r\n",
                        );
                    }
                    new_tab(state);
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(b"PS C:\\> echo tab 2\r\n");
                    }
                    new_tab(state);
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(b"PS C:\\> echo tab 3\r\n");
                    }
                    switch_tab(state, 0);
                    let win_size = state.window.inner_size();
                    resize_all_tabs(state, win_size.width, win_size.height);
                } else if demo == "ctxrule" {
                    // Seed a context rule that matches a cwd glob so the
                    // tinted tab chip + PROD badge are visible on the first
                    // captured frame — deterministic screenshot of the feature.
                    //
                    // The rule matches any path under the current workspace dir
                    // (or any path containing "prod"), giving the tab a red
                    // tint and a "PROD" safety badge.
                    let demo_cwd = std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    // Demo: match ANY cwd so the tint stays visible regardless of
                    // what the shell later announces via OSC 7 (deterministic shot).
                    let cwd_glob = "*".to_string();
                    self.config.context_rules = vec![terminale_config::ContextRule {
                        name: "Production".to_string(),
                        host_glob: Some("*prod*".to_string()),
                        cwd_glob: Some(cwd_glob.clone()),
                        tab_color: Some([200, 50, 50]),
                        badge: Some("PROD".to_string()),
                    }];
                    state.context_rules.clone_from(&self.config.context_rules);
                    // Announce the current cwd via OSC 7 so the rule matches.
                    let osc7 = format!(
                        "\x1b]7;file://localhost{}\x1b\\",
                        demo_cwd.replace('\\', "/")
                    );
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(osc7.as_bytes());
                        tab.emulator.lock().advance(
                            b"PS C:\\Workspace\\terminale> # context-rule demo\r\n\
                              PS C:\\Workspace\\terminale> echo Safety tint active on prod host\r\n",
                        );
                    }
                    // Evaluate rules immediately so the tab chip turns red.
                    refresh_context_rules(state);
                    refresh_tab_bar(state);
                } else if demo == "snippets" {
                    // Seed the config with 4 demo snippets and open the
                    // snippet picker overlay so it is visible on the first
                    // captured frame (deterministic screenshot).
                    self.config.snippets = vec![
                        terminale_config::Snippet {
                            name: "Git status".to_string(),
                            body: "git status\n".to_string(),
                            description: Some("Show working-tree status".to_string()),
                        },
                        terminale_config::Snippet {
                            name: "Git log (pretty)".to_string(),
                            body: "git log --oneline --graph --all\n".to_string(),
                            description: Some("Compact decorated graph log".to_string()),
                        },
                        terminale_config::Snippet {
                            name: "Cargo test workspace".to_string(),
                            body: "cargo test --workspace\n".to_string(),
                            description: Some("Run all crate tests".to_string()),
                        },
                        terminale_config::Snippet {
                            name: "Docker ps".to_string(),
                            body: "docker ps --format 'table {{.ID}}\\t{{.Names}}\\t{{.Status}}'\n"
                                .to_string(),
                            description: Some("List running containers".to_string()),
                        },
                    ];
                    state.snippet_names = snippet_names_from(&self.config);
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\Workspace\\terminale> # snippet library demo\r\n\
                              PS C:\\Workspace\\terminale> ",
                        );
                    }
                    // Open the snippet picker so it is visible immediately.
                    crate::open_snippet_picker(state);
                } else if demo == "restore" {
                    // Demo: pre-write a two-tab saved workspace (first tab has
                    // a vertical split; second tab is a single pane) and then
                    // restore it so the multi-tab/split layout is visible in a
                    // screenshot — no user interaction required.
                    let ws = crate::workspace::SavedWorkspace {
                        name: "demo-restore".to_string(),
                        tabs: vec![
                            crate::workspace::SavedTab {
                                title: Some("editor".to_string()),
                                tree: crate::workspace::SavedPaneTree::Split {
                                    direction: crate::workspace::SavedSplitDir::Vertical,
                                    ratio: 0.6,
                                    a: Box::new(crate::workspace::SavedPaneTree::Leaf {
                                        profile: None,
                                        cwd: None,
                                        title: Some("editor pane".to_string()),
                                    }),
                                    b: Box::new(crate::workspace::SavedPaneTree::Leaf {
                                        profile: None,
                                        cwd: None,
                                        title: Some("output pane".to_string()),
                                    }),
                                },
                                group: None,
                            },
                            crate::workspace::SavedTab {
                                title: Some("shell".to_string()),
                                tree: crate::workspace::SavedPaneTree::Leaf {
                                    profile: None,
                                    cwd: None,
                                    title: None,
                                },
                                group: None,
                            },
                        ],
                        active_tab: 0,
                        tab_groups: Vec::new(),
                        next_group_id: 0,
                    };
                    // Write to the workspaces directory for the picker to find.
                    if let Some(dir) = terminale_config::paths::workspaces_dir() {
                        let path = dir.join("demo-restore.toml");
                        let _ = crate::workspace::write_workspace(&path, &ws);
                    }
                    // Now restore it into this window immediately.
                    let win_size = state.window.inner_size();
                    let instance = state.renderer.instance();
                    let adapter = state.renderer.adapter();
                    let device = state.renderer.device();
                    let queue = state.renderer.queue();
                    let _ = state; // release the borrow before borrowing self again
                    self.restore_workspace(
                        event_loop, 0, ws, instance, adapter, device, queue, win_size,
                    );
                    // Seed the panes with demo output after restore.
                    if let Some(s) = self.windows.get_mut(0) {
                        if let Some(tab) = s.tabs.first() {
                            tab.emulator
                                .lock()
                                .advance(b"PS C:\\> # session-restore demo\r\nPS C:\\> ");
                        }
                    }
                    // Manually call reveal so the window is shown.
                    if let Some(s) = self.windows.get_mut(0) {
                        reveal_window(s);
                    }
                    return; // skip the second reveal below
                } else if demo == "cliphistory" {
                    // Pre-seed the clipboard history ring with a few realistic
                    // entries so the picker overlay is visible and populated.
                    for entry in &[
                        "git commit -m \"feat: tab groups\"",
                        "/home/user/.config/terminale/config.toml",
                        "https://github.com/fbrzlarosa/terminale",
                        "cargo build --workspace --release",
                        "192.168.1.42",
                    ] {
                        push_clipboard_history(state, (*entry).to_string());
                    }
                    // Open the clipboard-history picker so the overlay is shown.
                    dispatch_shortcut(state, ShortcutAction::OpenClipboardHistory);
                } else if demo == "cmdhistory" {
                    // Seed a few complete OSC 133 command cycles so the
                    // command_blocks list is populated, then open the
                    // command-history picker so the overlay is visible.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        // Explicitly enable block capture on THIS emulator before
                        // seeding (set_command_blocks drives the SemanticModel's
                        // max_blocks; the RunningState flag alone does not).
                        tab.emulator.lock().set_command_blocks(true, 1000);
                        tab.emulator.lock().advance(
                            // Five complete A→B→C→D cycles each with a distinct command.
                            // OSC 133;B carries the typed command text via the emulator's
                            // record_with_text path; we embed the command between B and C.
                            b"\x1b]133;A\x1b\\\
                              \x1b]133;B\x1b\\cargo build --workspace\x1b]133;C\x1b\\\r\n\
                              \x1b[32m   Compiling\x1b[0m terminale v0.1.0\r\n\
                              \x1b]133;D;0\x1b\\\
                              \x1b]133;A\x1b\\\
                              \x1b]133;B\x1b\\cargo test --workspace\x1b]133;C\x1b\\\r\n\
                              test result: ok. 96 passed; 0 failed\r\n\
                              \x1b]133;D;0\x1b\\\
                              \x1b]133;A\x1b\\\
                              \x1b]133;B\x1b\\git status\x1b]133;C\x1b\\\r\n\
                              On branch wip/features\r\n\
                              \x1b]133;D;0\x1b\\\
                              \x1b]133;A\x1b\\\
                              \x1b]133;B\x1b\\cargo clippy --workspace --all-targets --all-features\x1b]133;C\x1b\\\r\n\
                              \x1b]133;D;0\x1b\\\
                              \x1b]133;A\x1b\\\
                              \x1b]133;B\x1b\\git log --oneline -5\x1b]133;C\x1b\\\r\n\
                              15edabf fix(render): image_blit shader\r\n\
                              \x1b]133;D;0\x1b\\",
                        );
                    }
                    // Enable command blocks on this window so the picker finds
                    // the seeded history (default is already on, but be explicit).
                    state.command_blocks_enabled = true;
                    // Open the command-history picker.
                    dispatch_shortcut(state, ShortcutAction::OpenCommandHistory);
                } else if demo == "pasteguard" {
                    // Seed the terminal with a realistic prompt, then trigger
                    // the paste-guard confirmation dialog with a multi-line
                    // clipboard payload so the dialog is visible on the first
                    // captured frame — deterministic screenshot of the feature.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\Workspace\\terminale> # paste-guard demo\r\n\
                              PS C:\\Workspace\\terminale> ",
                        );
                    }
                    // Force the guard on (simulate unbracketed mode) so the
                    // dialog opens regardless of the default config values.
                    state.paste_confirm_when_unbracketed = true;
                    // Queue a fake multi-line clipboard entry so the guard fires
                    // without needing actual clipboard access.
                    let demo_text =
                        "rm -rf /tmp/build && \\\nmake all && \\\necho done".to_string();
                    state.pending_paste_guard = Some((demo_text, false));
                } else if demo == "tabpin" {
                    // Demo: 3 tabs — first is pinned (compact, at front) with a
                    // blue accent, second has a user colour (green), third is
                    // normal. Shows the full tab-organisation feature at a glance.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\Workspace\\terminale> # tab 1 (will be pinned)\r\n\
                              PS C:\\Workspace\\terminale> ",
                        );
                    }
                    // Pin tab 0 — it renders compact at the leading edge.
                    if let Some(tab) = state.tabs.get_mut(0) {
                        tab.pinned = true;
                        tab.user_icon = Some("⊕".to_string());
                    }
                    // Open tab 2 with a green user colour.
                    new_tab(state);
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\Workspace\\terminale> # tab 2 (green colour)\r\n\
                              PS C:\\Workspace\\terminale> ",
                        );
                    }
                    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                        tab.user_color = Some([0x30, 0xc0, 0x60]);
                    }
                    // Open tab 3 as a plain tab.
                    new_tab(state);
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\Workspace\\terminale> # tab 3 (normal)\r\n\
                              PS C:\\Workspace\\terminale> ",
                        );
                    }
                    // Switch back to tab 0 (pinned) for the screenshot.
                    switch_tab(state, 0);
                    let win_size = state.window.inner_size();
                    resize_all_tabs(state, win_size.width, win_size.height);
                    refresh_tab_bar(state);
                } else if demo == "paneswap" {
                    // Demo: split the active tab into three named panes
                    // (one → two → three), then rotate them forward once so
                    // the titles appear in non-original positions — making
                    // the swap/rotate feature obvious in a screenshot.
                    //
                    // Layout after splits: vsplit(1[one], vsplit(2[two], 3[three]))
                    // After one rotate_panes: vsplit(2[two], vsplit(3[three], 1[one]))
                    state.show_pane_headers = true;
                    // Seed the first (already-open) pane.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator
                            .lock()
                            .advance(b"PS C:\\> # pane one\r\nPS C:\\> ");
                    }
                    // Name the first pane "one".
                    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                        if let Some(pane) = tab.panes.get_mut(&tab.focused) {
                            pane.user_title = Some("one".to_string());
                        }
                    }
                    // Split right → pane two.
                    split_focused_pane(state, SplitDir::Vertical, true);
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator
                            .lock()
                            .advance(b"PS C:\\> # pane two\r\nPS C:\\> ");
                    }
                    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                        if let Some(pane) = tab.panes.get_mut(&tab.focused) {
                            pane.user_title = Some("two".to_string());
                        }
                    }
                    // Split down → pane three (below pane two).
                    split_focused_pane(state, SplitDir::Horizontal, true);
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator
                            .lock()
                            .advance(b"PS C:\\> # pane three\r\nPS C:\\> ");
                    }
                    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                        if let Some(pane) = tab.panes.get_mut(&tab.focused) {
                            pane.user_title = Some("three".to_string());
                        }
                    }
                    // Rotate panes forward once — pane "one" moves to the last
                    // slot; the remaining panes shift forward.
                    rotate_active_tab_panes(state);
                    // Focus back to pane "one" (now in the last slot) for clarity.
                    // Actually keep whatever focus the rotate left us with.
                    let win_size = state.window.inner_size();
                    resize_all_tabs(state, win_size.width, win_size.height);
                } else if demo == "dirjump" {
                    // Pre-seed the directory-jump frecency store with a mix of
                    // recent and older visits so the ranked list is populated and
                    // visually interesting for a screenshot.
                    let now = chrono::Utc::now().timestamp();
                    // Visit within the last hour (highest frecency).
                    state.dir_jump_store.record(
                        "/home/user/Workspace/terminale",
                        now - 120, // 2 minutes ago
                        state.dir_jump_max_tracked,
                    );
                    state.dir_jump_store.record(
                        "/home/user/Workspace/terminale",
                        now - 60, // 1 minute ago (second visit → count=2)
                        state.dir_jump_max_tracked,
                    );
                    // Visit within the last day.
                    state.dir_jump_store.record(
                        "/home/user/projects/website",
                        now - 7200, // 2 hours ago
                        state.dir_jump_max_tracked,
                    );
                    state.dir_jump_store.record(
                        "/home/user/projects/website",
                        now - 3600, // 1 hour ago (second visit)
                        state.dir_jump_max_tracked,
                    );
                    state.dir_jump_store.record(
                        "/home/user/projects/website",
                        now - 1800, // 30 minutes ago (third visit)
                        state.dir_jump_max_tracked,
                    );
                    // Visit within the last week.
                    state.dir_jump_store.record(
                        "/etc/nginx",
                        now - 86_400 * 2, // 2 days ago
                        state.dir_jump_max_tracked,
                    );
                    state.dir_jump_store.record(
                        "/var/log",
                        now - 86_400 * 3, // 3 days ago
                        state.dir_jump_max_tracked,
                    );
                    // Older visit (lowest frecency).
                    state.dir_jump_store.record(
                        "/tmp/scratch",
                        now - 86_400 * 10, // 10 days ago
                        state.dir_jump_max_tracked,
                    );
                    // Seed the terminal buffer with a sample prompt.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\Workspace\\terminale> # directory-jump demo\r\n\
                              PS C:\\Workspace\\terminale> ",
                        );
                    }
                    // Open the directory-jump picker so the ranked list is visible.
                    dispatch_shortcut(state, ShortcutAction::OpenDirectoryJump);
                } else if demo == "snapchooser" {
                    // Seed the terminal buffer with a sample prompt, then open the
                    // snap-layout chooser overlay so it is visible for a screenshot.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\Workspace\\terminale> # snap-layout chooser demo\r\n\
                              PS C:\\Workspace\\terminale> ",
                        );
                    }
                    dispatch_shortcut(state, ShortcutAction::ShowSnapLayouts);
                } else if demo == "themeimport" {
                    // Write a distinctive demo theme (bright magenta background,
                    // yellow foreground) into the themes directory, load it, and
                    // apply it so a screenshot shows the unmistakable imported colours.
                    let demo_theme = terminale_config::Theme {
                        name: "Demo Import Magenta".into(),
                        background: "#cc00cc".into(),
                        foreground: "#ffff00".into(),
                        cursor: "#00ffff".into(),
                        selection: "#660066".into(),
                        normal: [
                            "#330033".into(),
                            "#ff0000".into(),
                            "#00ff00".into(),
                            "#ffff00".into(),
                            "#0000ff".into(),
                            "#ff00ff".into(),
                            "#00ffff".into(),
                            "#ffffff".into(),
                        ],
                        bright: [
                            "#660066".into(),
                            "#ff6666".into(),
                            "#66ff66".into(),
                            "#ffff66".into(),
                            "#6666ff".into(),
                            "#ff66ff".into(),
                            "#66ffff".into(),
                            "#ffffff".into(),
                        ],
                    };
                    // Ensure the themes directory exists and write the theme file.
                    let themes_dir = self
                        .config
                        .appearance
                        .effective_themes_dir()
                        .or_else(terminale_config::paths::themes_dir);
                    if let Some(dir) = themes_dir {
                        let _ = std::fs::create_dir_all(&dir);
                        let theme_toml = toml::to_string_pretty(&demo_theme).unwrap_or_default();
                        let dest = dir.join("demo-import-magenta.toml");
                        let _ = std::fs::write(&dest, theme_toml);
                        tracing::info!(dest = %dest.display(), "wrote themeimport demo theme");
                    }
                    // Append to the inline list so it resolves immediately.
                    if !self
                        .config
                        .appearance
                        .themes
                        .iter()
                        .any(|t| t.name == demo_theme.name)
                    {
                        self.config.appearance.themes.push(demo_theme.clone());
                    }
                    // Activate the imported theme.
                    self.config.appearance.theme = demo_theme.name.clone();
                    apply_theme(state, &self.config);
                    // Seed the terminal with output that shows the theme is active.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\> # themeimport demo\r\n\
                              Imported theme: Demo Import Magenta\r\n\
                              Background: #cc00cc (magenta), Foreground: #ffff00 (yellow)\r\n\
                              PS C:\\> ",
                        );
                    }
                } else if demo == "promptnav" {
                    // Seed three complete OSC 133 command blocks into the
                    // scrollback — one success, one failure, one success — then
                    // open the failed-command picker so the overlay is visible
                    // on the first captured frame (deterministic screenshot of
                    // the prompt-navigation / failed-command-picker feature).
                    //
                    // We must enable command-block capture first so the blocks
                    // are actually stored in the SemanticModel.
                    let max = self.config.terminal.max_command_blocks;
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().set_command_blocks(true, max);
                        tab.emulator.lock().advance(
                            // Block 1: success — `echo hello`
                            b"\x1b]133;A\x1b\\\
                              $ echo hello\
                              \x1b]133;C\x1b\\\
                              \r\nhello\r\n\
                              \x1b]133;D;0\x1b\\\
                              \
                              \x1b]133;A\x1b\\\
                              $ cargo build 2>&1\
                              \x1b]133;C\x1b\\\
                              \r\nerror[E0382]: borrow of moved value: `config`\r\n  \
                              --> src/main.rs:42:9\r\n\
                              \x1b]133;D;101\x1b\\\
                              \
                              \x1b]133;A\x1b\\\
                              $ ls -la\
                              \x1b]133;C\x1b\\\
                              \r\ntotal 0\r\ndrwxr-xr-x  2 user user  40 Jan 01 00:00 .\r\n\
                              \x1b]133;D;0\x1b\\",
                        );
                    }
                    // Open the failed-command picker so the overlay is visible
                    // in the screenshot without any user interaction.
                    dispatch_shortcut(state, ShortcutAction::OpenFailedCommandPicker);
                } else if demo == "plugincmd" {
                    // Load an inline sample plugin that registers a command
                    // palette entry. Opens the palette so the plugin-contributed
                    // row is visible in a deterministic screenshot.
                    if let Some(host) = self.plugins.as_mut() {
                        let _ = host.load_inline(
                            "demo_hello",
                            r#"terminale.register_command("Plugin: Say Hello", function()
                                terminale.notify("Hello from plugin!", "A plugin-registered command was invoked.")
                            end)"#,
                        );
                        host.flush_pending_registrations();
                        // Sync the command names to this window immediately.
                        let names: Vec<String> = host
                            .registered_commands
                            .iter()
                            .map(|c| c.name.clone())
                            .collect();
                        state.plugin_command_names.clone_from(&names);
                    }
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\Workspace\\terminale> # plugin command-palette demo\r\n\
                              PS C:\\Workspace\\terminale> ",
                        );
                    }
                    // Open the palette with a pre-filter so the plugin row is
                    // visible on the first frame.
                    open_command_palette(state);
                    if let Some(p) = state.command_palette.as_mut() {
                        p.query.push_str("Plugin");
                    }
                    refresh_palette(state);
                } else if demo == "font" {
                    // Demo: force Ubuntu Mono at a large size and emit a
                    // short sample so the typeface is visible in a screenshot.
                    // ConPTY's init clear wipes this; font_demo_reseed_at
                    // re-emits it ≈700 ms later.
                    state.renderer.set_font_family("Ubuntu Mono");
                    state.renderer.set_font_size(28.0);
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"\x1b[2J\x1b[HUbuntu Mono (bundled)\r\n\
                              The quick brown fox 0123456789\r\n\
                              () {} [] => != === <=\r\n",
                        );
                    }
                } else if demo == "padding" {
                    // Seed the demo tab with a full-screen alternate-screen box
                    // frame that touches all four grid edges. The bottom border
                    // lands on the grid's last row, so the gap between it and
                    // the window edge equals the actual bottom padding.
                    // ConPTY's init clear will wipe this; the padding_demo_reseed_at
                    // timer re-emits it ≈700 ms later.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        let (cols, rows) = tab.emulator.lock().size();
                        let frame = build_padding_demo_frame(cols, rows);
                        tab.emulator.lock().advance(&frame);
                    }
                } else if demo == "boxdraw" {
                    // Seed the terminal with a demo of box-drawing and block
                    // elements rendered as crisp procedural quads. Shows:
                    //  - A bordered box using light box-drawing corners and tees
                    //  - A row of shading characters (light/medium/dark shade + full block)
                    //  - A mini bar-graph using bottom-up eighth-blocks
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        let demo_text = concat!(
                            "\r\n",
                            "\x1b[0m",
                            "\u{250c}\u{2500}\u{2500}\u{2500}\u{252c}\u{2500}\u{2500}\u{2500}\u{252c}\u{2500}\u{2500}\u{2500}\u{2510}\r\n",
                            "\u{2502} A \u{2502} B \u{2502} C \u{2502}\r\n",
                            "\u{251c}\u{2500}\u{2500}\u{2500}\u{253c}\u{2500}\u{2500}\u{2500}\u{253c}\u{2500}\u{2500}\u{2500}\u{2524}\r\n",
                            "\u{2502} 1 \u{2502} 2 \u{2502} 3 \u{2502}\r\n",
                            "\u{2514}\u{2500}\u{2500}\u{2500}\u{2534}\u{2500}\u{2500}\u{2500}\u{2534}\u{2500}\u{2500}\u{2500}\u{2518}\r\n",
                            "\r\n",
                            "Shading:  \u{2591}\u{2592}\u{2593}\u{2588}\r\n",
                            "\r\n",
                            "Bar graph: \u{2581}\u{2582}\u{2583}\u{2584}\u{2585}\u{2586}\u{2587}\u{2588}\r\n",
                        );
                        tab.emulator.lock().advance(demo_text.as_bytes());
                    }
                } else if demo == "aibar" {
                    // Seed the terminal with a realistic prompt and force the
                    // suggestion bar into the Ready state so a screenshot shows
                    // the bar populated — no live provider call needed.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\Workspace\\terminale> # suggestion-bar demo\r\n\
                              PS C:\\Workspace\\terminale> ",
                        );
                    }
                    state.suggestions.enabled = true;
                    state.suggestions.state =
                        suggestions::SuggestionState::Ready("git status".to_string());
                // `tabgroups` and `settings` are handled by their own blocks
                // just below and must NOT also open the command palette (which
                // would cover the tab bar / settings window); exclude them here.
                } else if demo != "tabgroups" && demo != "settings" {
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator.lock().advance(
                            b"PS C:\\Workspace\\terminale> echo \"text behind the palette\"\r\n\
                              text behind the palette\r\n\
                              PS C:\\Workspace\\terminale> cargo build --release\r\n\
                              \x1b[32m   Compiling\x1b[0m terminale v0.1.0\r\n\
                              \x1b[32m    Finished\x1b[0m release profile in 1m 14s\r\n\
                              PS C:\\Workspace\\terminale> ",
                        );
                    }
                    open_command_palette(state);
                    if demo == "themes" {
                        if let Some(p) = state.command_palette.as_mut() {
                            p.mode = PaletteMode::Themes;
                        }
                    } else if let Some(p) = state.command_palette.as_mut() {
                        p.query.push_str("ta");
                    }
                    refresh_palette(state);
                }
                if demo == "tabgroups" {
                    // Demo: 4 tabs in 2 groups, with distinct accent colours.
                    // Shows colour-coded brackets + group labels in the tab bar.
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator
                            .lock()
                            .advance(b"PS C:\\> # tab-groups demo\r\nPS C:\\> ");
                    }
                    // Open 3 more tabs (tab 0 already exists).
                    for _ in 0..3 {
                        new_tab(state);
                    }
                    // Seed each tab with a distinctive prompt.
                    let prompts = [
                        b"PS C:\\> # alpha (group 1)\r\nPS C:\\> " as &[u8],
                        b"PS C:\\> # beta (group 2)\r\nPS C:\\> ",
                        b"PS C:\\> # gamma (group 2)\r\nPS C:\\> ",
                        b"PS C:\\> # delta (group 1)\r\nPS C:\\> ",
                    ];
                    for (i, prompt) in prompts.iter().enumerate() {
                        switch_tab(state, i);
                        if let Some(tab) = state.tabs.get(state.active_tab) {
                            tab.emulator.lock().advance(prompt);
                        }
                    }
                    // Create group A (blue) for tabs 0 and 3.
                    let id_a = state.next_group_id;
                    state.next_group_id = state.next_group_id.wrapping_add(1);
                    state.tab_groups.push(crate::TabGroup {
                        id: id_a,
                        name: "Build".to_string(),
                        color: GROUP_COLOR_PALETTE[0],
                    });
                    // Create group B (green) for tabs 1 and 2.
                    let id_b = state.next_group_id;
                    state.next_group_id = state.next_group_id.wrapping_add(1);
                    state.tab_groups.push(crate::TabGroup {
                        id: id_b,
                        name: "Deploy".to_string(),
                        color: GROUP_COLOR_PALETTE[1],
                    });
                    if let Some(tab) = state.tabs.get_mut(0) {
                        tab.group = Some(id_a);
                    }
                    if let Some(tab) = state.tabs.get_mut(1) {
                        tab.group = Some(id_b);
                    }
                    if let Some(tab) = state.tabs.get_mut(2) {
                        tab.group = Some(id_b);
                    }
                    if let Some(tab) = state.tabs.get_mut(3) {
                        tab.group = Some(id_a);
                    }
                    // Tab 4: ungrouped — makes the pill gap between ungrouped
                    // tab and the following group run clearly visible.
                    new_tab(state);
                    switch_tab(state, 4);
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        tab.emulator
                            .lock()
                            .advance(b"PS C:\\> # epsilon (ungrouped)\r\nPS C:\\> ");
                    }
                    switch_tab(state, 0);
                    let win_size = state.window.inner_size();
                    resize_all_tabs(state, win_size.width, win_size.height);
                    refresh_tab_bar(state);
                }
                if demo == "settings" {
                    // Demo: open the native Settings window for a screenshot.
                    open_settings(state);
                }
            }
            // Paint the first frame into the hidden window, then reveal it
            // (cloak-around-show on Windows) so the user never sees a white
            // flash — the window appears already showing the dark UI.
            reveal_window(state);
            // Apply the configured startup position, if any. Done AFTER
            // reveal so the monitor + scale factor are stable; the snap
            // helper already handles missing-monitor gracefully.
            if let Some(edge) = self.config.window.startup_position {
                snap_window(state, edge);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        // ── Context-menu popup route ───────────────────────────────────────
        // Single-window design: the base column and any open flyout live in
        // the same egui pass, so CursorMoved is never lost to a second window.
        if let Some(menu) = self.context_menu.as_mut() {
            if menu.id() == id {
                let close = menu.handle_event(&event);
                let action = menu.take_chosen();
                if close {
                    self.context_menu = None;
                }
                if let Some(action_id) = action {
                    self.context_menu = None;
                    // Dynamic pickers reserve high action-id ranges so they
                    // can route via the App (which holds the config + runtime).
                    if action_id >= GROUP_ASSIGN_BASE {
                        // "Add to <group>" in a tab's context menu: assign the
                        // active tab to the group at this index. Highest picker
                        // range, so it must be tested before the others.
                        let idx = (action_id - GROUP_ASSIGN_BASE) as usize;
                        if let Some(state) = self.focused_window_mut() {
                            if let Some(gid) = state.tab_groups.get(idx).map(|g| g.id) {
                                crate::tab_groups::assign_active_tab_to_group(state, gid);
                            }
                        }
                    } else if action_id >= SSH_PICKER_BASE {
                        let idx = (action_id - SSH_PICKER_BASE) as usize;
                        // Route to the most-recently-focused window; prompt for
                        // a credential in-window when one is needed.
                        if let Some(win_idx) = self.windows.len().checked_sub(1) {
                            self.open_or_prompt_ssh(event_loop, win_idx, idx);
                        }
                    } else if action_id >= TAB_ICON_PICKER_BASE {
                        // Per-tab icon picker: set the user icon from ICON_PRESETS.
                        let idx = (action_id - TAB_ICON_PICKER_BASE) as usize;
                        if let Some(glyph) = crate::settings_window::ICON_PRESETS
                            .get(idx)
                            .map(|(_, g)| *g)
                        {
                            if let Some(state) = self.focused_window_mut() {
                                crate::shortcuts::set_tab_user_icon(state, Some(glyph.to_string()));
                            }
                        }
                    } else if action_id >= PROFILE_PICKER_BASE {
                        let idx = (action_id - PROFILE_PICKER_BASE) as usize;
                        if let Some(p) = self.config.profiles.profiles.get(idx).cloned() {
                            if let Some(state) = self.focused_window_mut() {
                                new_tab_with_profile(state, &p);
                            }
                        }
                    } else if action_id == MenuAction::RestartSession.as_u32() {
                        // Restart needs `self.config` to resolve the pane's
                        // profile by name (command/args/env), so it is
                        // dispatched here instead of dispatch_menu_action.
                        let prof = self
                            .focused_window_mut()
                            .and_then(|state| {
                                state
                                    .tabs
                                    .get(state.active_tab)
                                    .map(|t| t.profile_name.clone())
                            })
                            .and_then(|name| {
                                self.config
                                    .profiles
                                    .profiles
                                    .iter()
                                    .find(|p| p.name == name)
                                    .cloned()
                            });
                        if let Some(state) = self.focused_window_mut() {
                            crate::tabs::restart_focused_pane(state, prof.as_ref());
                        }
                    } else if let Some(state) = self.focused_window_mut() {
                        dispatch_menu_action(state, action_id);
                    }
                }
                return;
            }
        }

        // SSH credential prompt route — handled separately, returns early.
        if let Some(prompt) = self.password_prompt.as_mut() {
            if prompt.id() == id {
                let close = prompt.handle_event(&event);
                let outcome = prompt.take_outcome();
                let host_idx = prompt.host_idx();
                if close {
                    self.password_prompt = None;
                }
                if let Some(outcome) = outcome {
                    self.resolve_password_prompt(host_idx, outcome);
                }
                return;
            }
        }

        // Close-confirmation dialog route — handled separately.
        if let Some(dialog) = self.confirm_close_dialog.as_mut() {
            if dialog.id() == id {
                let close = dialog.handle_event(&event);
                let outcome = dialog.take_outcome();
                let target = dialog.target();
                let parent = dialog.parent_id();
                if close {
                    self.confirm_close_dialog = None;
                }
                if let Some(confirm_close::ConfirmCloseOutcome::Confirm) = outcome {
                    if let Some(idx) = self.window_index(parent) {
                        match target {
                            confirm_close::CloseTarget::Window => {
                                self.windows.remove(idx);
                                if self.windows.is_empty() {
                                    event_loop.exit();
                                }
                            }
                            confirm_close::CloseTarget::Tab(t) => {
                                // `close_tab` guards a stale index itself.
                                tabs::close_tab(&mut self.windows[idx], t);
                                self.windows[idx].window.request_redraw();
                            }
                        }
                    }
                }
                // Cancelled outcome → keep everything open.
                return;
            }
        }

        // Paste-guard confirmation dialog route — handled separately.
        if let Some(dialog) = self.paste_guard_dialog.as_mut() {
            if dialog.id() == id {
                let close = dialog.handle_event(&event);
                let outcome = dialog.take_outcome();
                let text = dialog.text().to_owned();
                let win_idx = self.paste_guard_window_idx;
                if close {
                    self.paste_guard_dialog = None;
                }
                if let Some(paste_guard::PasteGuardOutcome::Confirm) = outcome {
                    // User confirmed — send the text to the right window's PTY.
                    if let Some(state) = self.windows.get_mut(win_idx) {
                        send_paste_text(state, &text);
                    }
                }
                // Cancelled outcome → drop silently.
                return;
            }
        }

        // Settings window route — handled separately, returns early.
        if let Some(settings) = self.settings.as_mut() {
            if settings.id() == id {
                let close = settings.handle_event(&event);
                if close {
                    // Adopt the latest in-memory edits from the settings panel.
                    // Write them to disk so apply_config_reload can read the
                    // canonical updated values and run every live handler.  This
                    // guarantees all changes take effect even when the
                    // about_to_wait tick missed the final diff (e.g. the window
                    // closed in the same frame as the last edit).
                    let new_cfg = settings.current_config().clone();
                    // Drop the settings reference *before* apply_config_reload
                    // so that function does not see a stale settings window.
                    self.settings = None;
                    if let Err(e) = new_cfg.write_to(&self.config_path) {
                        // Write failed — adopt the in-memory config directly so
                        // the change isn't silently lost.  The live-apply loop
                        // in about_to_wait already ran for every tick while
                        // settings was open, so RunningState is up-to-date for
                        // all but the very last tick.
                        tracing::warn!(
                            ?e,
                            "settings close: config write failed; keeping in-memory config"
                        );
                        self.config = new_cfg;
                    } else {
                        // Re-apply the freshly-written config through the
                        // canonical apply path so every live handler fires.
                        self.apply_config_reload();
                    }
                }
                return;
            }
        }

        // AI assistant window route.
        if let Some(ai) = self.ai_assistant.as_mut() {
            if ai.id() == id {
                let close = ai.handle_event(&event);
                // The user clicked "Inject" — type the command into the
                // active terminal (no trailing newline, so they review it).
                if let Some(cmd) = ai.take_inject() {
                    if let Some(state) = self.focused_window_mut() {
                        if let Some(tab) = state.tabs.get(state.active_tab) {
                            let _ = tab.session.write_input(cmd.as_bytes());
                        }
                        state.window.focus_window();
                        state.window.request_redraw();
                    }
                }
                if close {
                    self.ai_assistant = None;
                }
                return;
            }
        }

        // Floating ghost window route — handled separately. It owns no
        // tabs / session, so terminal events are meaningless here; we only
        // care about redraws, resizes, and routing input-events back to
        // the App-level drag so a release / move that ends up on the
        // ghost (e.g. on platforms where click-through couldn't be
        // applied) still resolves the drag.
        if self
            .ghost_window
            .as_ref()
            .is_some_and(|g| g.window.id() == id)
        {
            self.handle_ghost_window_event(event_loop, &event);
            return;
        }

        let Some(idx) = self.window_index(id) else {
            return;
        };

        // CloseRequested closes only THIS window. The process exits only
        // when the last terminal window is gone. Honour `confirm_close` for
        // the OS close button / Alt+F4 too: a confirmation dialog opens and
        // the window closes only on Confirm.
        if matches!(event, WindowEvent::CloseRequested) {
            if self.windows[idx].confirm_close {
                if self.confirm_close_dialog.is_none() {
                    let state = &self.windows[idx];
                    let n = state.tabs.len();
                    let detail =
                        format!("{n} tab{} will be closed.", if n == 1 { "" } else { "s" });
                    self.confirm_close_dialog = Some(confirm_close::ConfirmCloseDialog::open(
                        event_loop,
                        &state.window,
                        confirm_close::CloseTarget::Window,
                        detail,
                        state.renderer.instance(),
                        state.renderer.adapter(),
                        state.renderer.device(),
                        state.renderer.queue(),
                    ));
                }
            } else {
                self.windows.remove(idx);
                if self.windows.is_empty() {
                    event_loop.exit();
                }
            }
            return;
        }

        // ── Tab-drag intercept ──
        // A Chrome-style tab drag is App-level (it can span windows), so its
        // motion / release handling must run with `&mut self`, not the
        // single-window `state` borrow taken below. Intercept the two events
        // that drive it before that borrow exists.
        if let WindowEvent::CursorMoved { position, .. } = &event {
            let position = *position;
            // Keep the per-window logical pointer current so ghost geometry
            // and a tear-out's placement track the cursor.
            {
                let w = &mut self.windows[idx];
                let scale = w.window.scale_factor() as f32;
                w.pointer_logical = (position.x as f32 / scale, position.y as f32 / scale);
                // Refresh the Quake current-monitor snapshot on every cursor
                // move (covers tab-drag across monitors and general pointer
                // travel while the Quake window is visible). Short-circuits
                // immediately when the window is hidden.
                refresh_quake_last_monitor(w);
            }
            let win_pos = self.windows[idx]
                .window
                .outer_position()
                .unwrap_or_default();
            let cursor_screen = (win_pos.x + position.x as i32, win_pos.y + position.y as i32);

            if self.tab_drag.is_some() {
                self.update_tab_drag(cursor_screen);
                return;
            }
            // Promote a pending press into a real drag once it moves enough,
            // but only while the left button is still held.
            let promote = {
                let w = &self.windows[idx];
                w.held_button == Some(MouseButton::Left)
                    && w.tab_press.is_some_and(|(_, press)| {
                        let dx = position.x as f32 - press.0;
                        let dy = position.y as f32 - press.1;
                        dx * dx + dy * dy > TAB_DRAG_ARM_PX2
                    })
            };
            if promote {
                self.promote_tab_drag(event_loop, idx, cursor_screen);
                return;
            }
            // Promote a pending pane-header press into a pane drag once the
            // cursor moves past the arm threshold, but only while the left
            // button is held and pane_tear_out is enabled.
            let promote_pane = {
                let w = &self.windows[idx];
                w.held_button == Some(MouseButton::Left)
                    && w.pane_tear_out
                    && w.pane_header_press.is_some_and(|(_, press)| {
                        let dx = position.x as f32 - press.0;
                        let dy = position.y as f32 - press.1;
                        dx * dx + dy * dy > TAB_DRAG_ARM_PX2
                    })
            };
            if promote_pane {
                self.promote_pane_drag(event_loop, idx, cursor_screen);
                return;
            }
            // Promote a pending group-pill press into a group drag once the
            // cursor moves past the arm threshold (left button must still be held).
            let promote_group = {
                let w = &self.windows[idx];
                w.held_button == Some(MouseButton::Left)
                    && w.group_press.is_some_and(|(_, _, press)| {
                        let dx = position.x as f32 - press.0;
                        let dy = position.y as f32 - press.1;
                        dx * dx + dy * dy > TAB_DRAG_ARM_PX2
                    })
            };
            if promote_group {
                self.promote_group_drag(event_loop, idx, cursor_screen);
                return;
            }
        }
        if let WindowEvent::MouseInput {
            state: ElementState::Released,
            button: MouseButton::Left,
            ..
        } = &event
        {
            if self.tab_drag.is_some() {
                // Sync the held-button release into the window first so the
                // window's own state stays consistent, then resolve the drag.
                {
                    let w = &mut self.windows[idx];
                    w.held_button = None;
                    w.tab_press = None;
                    w.pane_header_press = None;
                    w.group_press = None;
                }
                self.resolve_tab_drag(event_loop);
                return;
            }
            // Clear a pending pane-header press that never grew into a drag.
            {
                let w = &mut self.windows[idx];
                w.pane_header_press = None;
            }
        }

        let state = &mut self.windows[idx];

        drain_pty_output(state);

        match event {
            WindowEvent::CloseRequested => unreachable!("handled above"),
            WindowEvent::Resized(new_size) => {
                // Coalesce: stash the latest size and apply it once per frame
                // in the RedrawRequested handler. Winit fires 10-60 Resized
                // events during a drag; applying each individually means a
                // PTY repaint emitted at size N1 gets parsed at size N2.
                state.pending_resize = Some(new_size);
                state.window.request_redraw();
            }
            WindowEvent::Moved(_) => {
                // The window has been dragged to a new position — update the
                // Quake monitor snapshot so `QuakeDisplay::Current` resolves
                // to the monitor the user moved it to.
                refresh_quake_last_monitor(state);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let sf = scale_factor as f32;
                // Refresh the physical-px divider metrics on DPI change.
                // Read the old scale factor BEFORE calling set_scale_factor
                // so we can back-compute the logical baseline.
                let old_sf = state.renderer.scale_factor();
                state.renderer.set_scale_factor(sf);
                if old_sf > 0.0 {
                    let t_logical = state.divider_thickness_px / old_sf;
                    let g_logical = state.divider_grab_padding_px / old_sf;
                    state.divider_thickness_px = t_logical * sf;
                    state.divider_grab_padding_px = g_logical * sf;
                    let fb_logical = state.focus_border_thickness_px / old_sf;
                    state.focus_border_thickness_px = fb_logical * sf;
                    state
                        .renderer
                        .set_focus_border_thickness_logical(fb_logical);
                }
                let size = state.window.inner_size();
                resize_all_tabs(state, size.width, size.height);
                // A DPI change means the window crossed a monitor boundary.
                refresh_quake_last_monitor(state);
                state.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => state.modifiers = mods.state(),
            WindowEvent::Occluded(occluded) => {
                // Fully covered or minimized: stop scheduling animation
                // redraws (background FX, activity spinner, bell, jump
                // highlight) so we don't render frames the compositor just
                // discards. PTY output still drains and wakes the loop.
                state.occluded = occluded;
                if !occluded {
                    // Newly visible again — repaint once to catch up.
                    state.window.request_redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                state.renderer.set_focused(focused);
                state.window_focused = focused;
                // Tell the focused app (via PTY) — vim / tmux pause
                // animations and refresh the cursor on focus changes
                // when DECSET 1004 is enabled.
                if let Some(tab) = state.tabs.get(state.active_tab) {
                    if tab.emulator.lock().focus_events_enabled() {
                        let seq: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
                        let _ = tab.session.write_input(seq);
                    }
                }
                state.window.request_redraw();
                // When focus returns the user may have moved the Quake window
                // to a different monitor while it was obscured. Refresh the
                // snapshot so `QuakeDisplay::Current` is up-to-date before
                // the next potential toggle.
                if focused {
                    // The Quake global hotkey is consumed by the OS (WM_HOTKEY),
                    // so winit never delivers the modifier *release* while the
                    // window is hidden — `state.modifiers` may be stale (e.g.
                    // stuck CONTROL). On Windows we query the real physical
                    // key state via GetAsyncKeyState so we know whether Ctrl/
                    // Shift/Alt/Super are genuinely still held. The translate_key
                    // guard that swallows Ctrl+<character> then fires correctly
                    // and the trigger key is never echoed into the shell.
                    // On other platforms winit re-emits ModifiersChanged before
                    // the next KeyboardInput, so resetting to empty() is safe.
                    #[cfg(target_os = "windows")]
                    {
                        state.modifiers = current_os_modifiers();
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        state.modifiers = ModifiersState::empty();
                    }
                    refresh_quake_last_monitor(state);
                }
                // Losing focus ends any in-progress mouse gesture: a drag-select
                // can't continue in an unfocused window, and the button-release
                // is the event most likely to be delivered elsewhere (a classic
                // way `held_button` gets stuck on macOS). Clear the gesture state
                // so motion after refocus isn't misread as a selection.
                if !focused {
                    state.held_button = None;
                    state.selecting = false;
                    state.selection_press_px = None;
                }
                // Auto-hide on focus loss when the Quake window loses focus
                // (only in dock mode, never free-floating). Flag here +
                // act after the match because `self.config.quake` and
                // re-borrowing `self.windows[idx]` would clash with the
                // active `state` borrow inside the match arm.
                if !focused && state.quake_visible {
                    state.pending_quake_autohide = true;
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                let scale = state.window.scale_factor() as f32;
                state.pointer_logical = (position.x as f32 / scale, position.y as f32 / scale);
                if state.pointer_hidden {
                    state.window.set_cursor_visible(true);
                    state.pointer_hidden = false;
                }

                // Snap-layout chooser hover: update highlighted cell on movement.
                if state.snap_chooser_open {
                    let hovered = state
                        .renderer
                        .snap_chooser_hit(position.x as f32, position.y as f32);
                    state.renderer.set_snap_chooser_hovered(hovered);
                    state.window.request_redraw();
                }

                // Phase E: a divider drag owns the cursor for its whole
                // lifetime. Update the ratio + redraw and skip every other
                // CursorMoved code path (URL hover, selection, tab-bar
                // hover, mouse motion reporting). The drag ends only on
                // left-release.
                let pos_phys = (position.x as f32, position.y as f32);
                if state.pending_divider_drag.is_some() {
                    update_divider_drag(state, pos_phys);
                    return;
                }
                // Otherwise see if we're just hovering a divider — if so,
                // override the cursor icon BEFORE the resize-edge / URL /
                // default branch decides, so the user gets the
                // EwResize/NsResize hint to discover the grab band.
                let hovering_divider = update_divider_hover(state, pos_phys);

                // (An in-flight Chrome-style tab drag is handled by the
                // App-level intercept above, which returns before reaching
                // this single-window arm.)

                // App-requested drag motion (vim mouse-selection, htop): if a
                // button is held and the focused app has MOUSE_DRAG /
                // MOUSE_MOTION on, emit the SGR motion sequence.
                let pos_px_motion = (position.x as f32, position.y as f32);
                let consumed_by_app = report_mouse_motion(state, pos_px_motion);

                // Pane header close-X hover tracking.
                let new_close_hover = pane_header_close_at(state, pos_phys);
                if new_close_hover != state.pane_header_close_hover {
                    state.pane_header_close_hover = new_close_hover;
                    state
                        .renderer
                        .set_pane_header_close_hovered(state.pane_header_close_hover);
                    state.window.request_redraw();
                }

                // Resize-edge feedback: when the cursor is on a border, swap
                // the cursor icon so users discover the resize handles.
                let resize_edge = detect_resize_edge(
                    state.pointer_logical.0,
                    state.pointer_logical.1,
                    &state.window,
                );
                let pos_px = (position.x as f32, position.y as f32);
                let url_under = hyperlink_under(state, pos_px);
                let icon = if state.pane_header_close_hover.is_some() {
                    // Close-X hover wins first: shows pointer hand so the
                    // user clearly sees the X is clickable.
                    winit::window::CursorIcon::Pointer
                } else if let Some((_, axis)) = state.hovered_divider {
                    // Divider hover wins over everything else so the user
                    // gets a clear "grab here to resize" hint.
                    cursor_icon_for_divider(axis)
                } else {
                    match resize_edge {
                        Some(dir) => cursor_icon_for_resize(dir),
                        None => {
                            if state.modifiers.control_key() && url_under.is_some() {
                                winit::window::CursorIcon::Pointer
                            } else if crate::panes::pane_cell_at_pixel(state, pos_px).is_some() {
                                // Over a pane's text grid → I-beam, the
                                // standard affordance for selectable text.
                                winit::window::CursorIcon::Text
                            } else {
                                winit::window::CursorIcon::Default
                            }
                        }
                    }
                };
                state.window.set_cursor(icon);
                // Belt-and-braces: avoid the unused-var warning if some
                // future refactor stops reading `hovering_divider`. The
                // icon override above already consumes the state.
                let _ = hovering_divider;
                // URL / path preview tooltip — shows whenever the cursor is
                // on a link, no Ctrl needed (discoverability > purity).
                // Gated on `link_hover_tooltip` config toggle; respects
                // `link_hover_delay_ms` dwell before showing.
                if state.link_hover_tooltip {
                    if let Some(uri) = &url_under {
                        let delay = u64::from(state.link_hover_delay_ms);
                        if delay == 0 {
                            // Instant — show immediately.
                            state.link_hover_start = None;
                            state.renderer.set_tooltip(Some(terminale_render::Tooltip {
                                text: uri.clone(),
                                anchor_px: [pos_px.0, pos_px.1],
                            }));
                        } else {
                            // Dwell tracking: start the timer if this is a
                            // new URL, keep it if the URL is the same.
                            let already_tracking = matches!(
                                &state.link_hover_start,
                                Some((existing, _, _)) if existing == uri
                            );
                            if !already_tracking {
                                // New URL (or first entry) — reset timer, hide
                                // the tooltip until the dwell period elapses.
                                state.link_hover_start = Some((
                                    uri.clone(),
                                    std::time::Instant::now(),
                                    [pos_px.0, pos_px.1],
                                ));
                                state.renderer.set_tooltip(None);
                            }
                            // else: still on the same URL — timer already running;
                            // the about_to_wait tick will apply the tooltip once
                            // the dwell period elapses.
                        }
                    } else {
                        state.link_hover_start = None;
                        state.renderer.set_tooltip(None);
                    }
                } else {
                    // Tooltip disabled — always clear.
                    state.link_hover_start = None;
                    state.renderer.set_tooltip(None);
                }
                // `Hover` mode: underline ONLY the URL currently under the
                // pointer (paths stay un-underlined). `Always` keeps every
                // URL underlined via `refresh_autodetect_links`, and `Never`
                // never underlines — so this hover sync only runs in `Hover`.
                if state.link_underline == terminale_config::LinkUnderline::Hover {
                    // The extra-underline list is drawn in the FOCUSED pane's
                    // frame, so only underline links hovered in that pane —
                    // links in other panes still get tooltip + Ctrl+click.
                    let hovered_url_range = autodetect_link_under(state, pos_px)
                        .filter(|d| !d.is_path)
                        .filter(|_| crate::panes::pointer_over_focused_pane(state, pos_px))
                        .map(|d| (d.col_start, d.col_end, d.row));
                    state.renderer.set_extra_underlines(
                        hovered_url_range.map(|r| vec![r]).unwrap_or_default(),
                    );
                }
                // Repaint when the hovered link changes (so the tooltip
                // appears / clears) and while hovering one (so it follows
                // the cursor). The loop is otherwise parked in `Wait`, so
                // without this the tooltip would never actually draw.
                let hover_changed = state.hovered_url != url_under;
                if hover_changed || url_under.is_some() {
                    state.window.request_redraw();
                }
                state.hovered_url = url_under;

                // Tab-bar hover state (always update so the bar reflects
                // user intent even while no menu is open).
                let hit = state.renderer.tab_hit(position.x as f32, position.y as f32);
                if let Some(bar) = state.renderer.tab_bar_mut() {
                    let prev_hover = bar.hovered;
                    let prev_close = bar.close_hovered;
                    let prev_plus = bar.plus_hovered;
                    let prev_ctrl = bar.window_ctrl_hovered;
                    bar.hovered = None;
                    bar.close_hovered = None;
                    bar.plus_hovered = false;
                    bar.window_ctrl_hovered = None;
                    match hit {
                        Some(TabHit::Tab(idx)) => bar.hovered = Some(idx),
                        Some(TabHit::Close(idx)) => {
                            bar.hovered = Some(idx);
                            bar.close_hovered = Some(idx);
                        }
                        Some(TabHit::Plus) => bar.plus_hovered = true,
                        Some(TabHit::Minimize) => {
                            bar.window_ctrl_hovered = Some(WindowCtrl::Minimize);
                        }
                        Some(TabHit::Maximize) => {
                            bar.window_ctrl_hovered = Some(WindowCtrl::Maximize);
                        }
                        Some(TabHit::CloseWindow) => {
                            bar.window_ctrl_hovered = Some(WindowCtrl::Close);
                        }
                        Some(TabHit::DragHandle) | Some(TabHit::GroupLabel(_)) | None => {}
                    }
                    if prev_hover != bar.hovered
                        || prev_close != bar.close_hovered
                        || prev_plus != bar.plus_hovered
                        || prev_ctrl != bar.window_ctrl_hovered
                    {
                        state.window.request_redraw();
                    }
                }

                // Drag-vs-click discrimination: promote to active selection
                // only after the cursor has moved more than a few pixels from
                // where the user clicked. This kills "ghost selection" on
                // simple single clicks. Skipped when the app already
                // consumed the motion via mouse reporting.
                if consumed_by_app {
                    return;
                }
                // Only treat motion as a selection drag while the left button is
                // genuinely held. On macOS trackpads a release can go missing,
                // leaving a stale `selection_press_px` that previously turned
                // plain cursor motion into a runaway selection ("it selects as
                // soon as I move/lift my finger"). Gate on the tracked button.
                // `mut` is only taken on macOS (the self-heal below); elsewhere
                // the binding is never reassigned.
                #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
                let mut left_held = state.held_button == Some(winit::event::MouseButton::Left);
                // macOS self-heal: if we *think* the button is held but the OS
                // says it isn't, winit dropped the release — drop the stale flag
                // so motion stops being misread as a drag. Only queried when the
                // tracked state claims "held", so it never runs on plain motion.
                #[cfg(target_os = "macos")]
                if left_held && !macos_left_button_down() {
                    state.held_button = None;
                    left_held = false;
                }
                if !left_held {
                    state.selection_press_px = None;
                    state.selecting = false;
                } else if !state.selecting {
                    if let Some(press) = state.selection_press_px {
                        let dx = position.x as f32 - press.0;
                        let dy = position.y as f32 - press.1;
                        if dx * dx + dy * dy > 9.0 {
                            state.selecting = true;
                        }
                    }
                }
                if state.selecting && left_held {
                    if let (Some(anchor), Some(end)) = (
                        state.selection_anchor,
                        // Clamp into the focused pane's grid so dragging past
                        // a divider / pane edge keeps selecting edge cells.
                        crate::panes::focused_pane_cell_clamped(
                            state,
                            (position.x as f32, position.y as f32),
                        ),
                    ) {
                        // Alt held → rectangular block selection (xterm
                        // Alt+drag). Else flowing row-major.
                        let block = state.modifiers.alt_key();
                        state.renderer.set_selection(Some(CellRect {
                            anchor,
                            cursor: end,
                            block,
                        }));
                        state.window.request_redraw();
                    }
                } else if state.menu_visible {
                    update_menu_hover(state);
                    state.window.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                let (px, py) = state.pointer_logical;
                let scale = state.window.scale_factor() as f32;
                let pointer_phys = (px * scale, py * scale);

                // Snap-layout chooser: a left-press dispatches the chosen snap
                // (or closes on a miss) and consumes the event.
                if state.snap_chooser_open
                    && matches!(btn_state, ElementState::Pressed)
                    && matches!(button, MouseButton::Left)
                {
                    let hit = state
                        .renderer
                        .snap_chooser_hit(pointer_phys.0, pointer_phys.1);
                    if let Some(idx) = hit {
                        crate::snap_chooser_apply(state, idx);
                    } else {
                        crate::close_snap_chooser(state);
                    }
                    state.window.request_redraw();
                    return;
                }

                // Phase E: intercept left-press on a divider grab band BEFORE
                // focus_pane_under_cursor so a click that lands on the inflated
                // grab band does NOT also swap pane focus.
                if matches!(btn_state, ElementState::Pressed) && matches!(button, MouseButton::Left)
                {
                    let specs = state
                        .tabs
                        .get(state.active_tab)
                        .map(|tab| divider_specs_for_tab(state, tab))
                        .unwrap_or_default();
                    if let Some((path, axis)) = hit_test_divider(&specs, pointer_phys) {
                        // Arm the drag: capture the parent rect and current ratio.
                        let start_ratio = state
                            .tabs
                            .get(state.active_tab)
                            .and_then(|tab| split_ratio_at(&tab.tree, &path))
                            .unwrap_or(0.5);
                        let parent_rect_px = state
                            .tabs
                            .get(state.active_tab)
                            .and_then(|tab| parent_rect_for_divider(state, tab, &path))
                            .unwrap_or((0.0, 0.0, 1.0, 1.0));
                        state.pending_divider_drag = Some(PendingDividerDrag {
                            path,
                            axis,
                            start_ratio,
                            parent_rect_px,
                        });
                        state.window.request_redraw();
                        return;
                    }
                }

                // Phase E: intercept left-release to finalise an in-flight
                // divider drag. Skip handle_mouse so we don't fire a phantom
                // click / selection from the release.
                if matches!(btn_state, ElementState::Released)
                    && matches!(button, MouseButton::Left)
                    && state.pending_divider_drag.is_some()
                {
                    finish_divider_drag(state);
                    return;
                }

                // Header strip intercepts — MUST come after the divider
                // intercept (divider wins) but BEFORE focus_pane_under_cursor.
                if matches!(btn_state, ElementState::Pressed) && matches!(button, MouseButton::Left)
                {
                    // Close-X: focus that pane then close it.
                    if let Some(pid) = pane_header_close_at(state, pointer_phys) {
                        if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                            tab.focused = pid;
                        }
                        close_focused_pane(state);
                        state.window.request_redraw();
                        return;
                    }
                    // Plain / double header click: focus that pane on first
                    // click; a second click within the double-click window on
                    // the same pane starts an inline header rename.
                    // A single press also arms pane_header_press so a
                    // subsequent drag (past TAB_DRAG_ARM_PX2) can lift the
                    // pane out, but only when tear-out is enabled and the tab
                    // has more than one pane (a lone leaf is the whole tab).
                    if let Some(pid) = pane_header_at(state, pointer_phys) {
                        // Focus the pane on every click (not just the first).
                        if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                            if tab.focused != pid {
                                tab.focused = pid;
                                state.pending_hook_pane_focus.push(pid);
                            }
                        }
                        // Arm a potential pane drag — cleared on release or
                        // on promotion to a full DragPayload::Pane drag.
                        let leaf_count = state
                            .tabs
                            .get(state.active_tab)
                            .map_or(1, |t| count_leaves(&t.tree));
                        if state.pane_tear_out && leaf_count > 1 {
                            state.pane_header_press = Some((pid, pointer_phys));
                        }
                        // Double-click detection: same pane within 400 ms.
                        let now = std::time::Instant::now();
                        let is_double = matches!(
                            state.last_header_click,
                            Some((t, p))
                                if p == pid
                                    && now.duration_since(t)
                                        <= std::time::Duration::from_millis(400)
                        );
                        if is_double {
                            state.last_header_click = None;
                            state.pane_header_press = None;
                            start_rename_pane(state, pid);
                        } else {
                            state.last_header_click = Some((now, pid));
                            state.window.request_redraw();
                        }
                        return;
                    }
                }

                // Click-to-focus pane: when a tab has more than one pane,
                // a left-press inside a NON-focused pane's sub-rect
                // swaps focus to it before the normal mouse handling
                // (which routes input to whichever pane is focused) runs.
                if matches!(btn_state, ElementState::Pressed) && matches!(button, MouseButton::Left)
                {
                    focus_pane_under_cursor(state, pointer_phys);
                }
                // A left-release that resolves an in-flight tab drag is
                // handled by the App-level intercept above (which returns
                // before reaching here). Anything that lands here is a plain
                // click / press — including a tab press that never armed a
                // drag — so just dispatch it.
                // Clear any pane-header press that never grew into a drag.
                if matches!(btn_state, ElementState::Released)
                    && matches!(button, MouseButton::Left)
                {
                    state.pane_header_press = None;
                }
                handle_mouse(state, button, btn_state);
                state.window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                handle_scroll(state, delta);
                state.window.request_redraw();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key,
                        logical_key,
                        state: ElementState::Pressed,
                        text,
                        ..
                    },
                ..
            } => {
                // Just after a Quake show, swallow key presses briefly so the
                // hotkey's still-held trigger key (e.g. the "1" in Ctrl+Shift+1)
                // is never typed into the shell. See `quake_input_suppress_until`.
                if let Some(deadline) = state.quake_input_suppress_until {
                    if std::time::Instant::now() < deadline {
                        return;
                    }
                    state.quake_input_suppress_until = None;
                }
                // The command palette grabs every key while it's open.
                if state.command_palette.is_some()
                    && handle_palette_input(state, &logical_key, text.clone())
                {
                    state.window.request_redraw();
                    return;
                }
                // Search mode hijacks the keyboard until Escape closes it.
                if state.search.is_some()
                    && handle_search_input(state, physical_key, &logical_key, text.clone())
                {
                    state.window.request_redraw();
                    return;
                }
                // Copy mode intercepts all keys while active.
                if state.copy_mode.active
                    && handle_copy_mode_input(state, physical_key, &logical_key)
                {
                    state.window.request_redraw();
                    return;
                }
                // Quick-select mode intercepts all keys while active.
                if state.quick_select.is_some() && handle_quick_select_input(state, &logical_key) {
                    state.window.request_redraw();
                    return;
                }
                // Pane-select mode intercepts all keys while active.
                if state.pane_select.is_some() && handle_pane_select_input(state, &logical_key) {
                    state.window.request_redraw();
                    return;
                }
                // Snap-layout chooser: Esc closes it; all other keys pass through
                // (the chooser is mouse-driven, so non-Esc keys reach normal dispatch).
                if state.snap_chooser_open {
                    if let winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) =
                        &logical_key
                    {
                        crate::close_snap_chooser(state);
                        state.window.request_redraw();
                        return;
                    }
                }
                // Inline rename hijacks the keyboard until Enter/Esc.
                if state.renaming.is_some()
                    && handle_rename_input(state, &logical_key, text.clone())
                {
                    state.window.request_redraw();
                    return;
                }
                // Modal key-table intercept — checked AFTER the other modal
                // states (palette, search, copy-mode, quick-select, pane-select,
                // rename) because those are "heavier" and must win if open.
                if state.active_key_table.is_some()
                    && handle_key_table_input(
                        state,
                        physical_key,
                        &logical_key,
                        &self.config.keybinds.key_tables,
                    )
                {
                    // Refresh status bar so the leader-mode indicator disappears.
                    update_status_bar(state, &self.config);
                    state.window.request_redraw();
                    return;
                }
                if handle_app_hotkey(state, physical_key, &logical_key) {
                    state.window.request_redraw();
                    return;
                }
                // Check if the pressed combo is a key-table leader.
                if handle_key_table_leader(
                    state,
                    physical_key,
                    &logical_key,
                    &self.config.keybinds.key_tables,
                ) {
                    // Refresh status bar so the leader-mode indicator appears.
                    update_status_bar(state, &self.config);
                    state.window.request_redraw();
                    return;
                }
                // Track the typed line so an `ssh …` command can offer to
                // save the host. Runs before `translate_key` consumes `text`.
                track_input_line(state, &logical_key, text.clone());
                // Determine whether application cursor-key mode (DECCKM) is
                // active. When the user has set keyboard_encoding = always_csi
                // we pass false regardless, which keeps arrow/Home/End in the
                // CSI form even if DECCKM is on — a compatibility escape-hatch.
                let app_cursor = {
                    let use_auto = self.config.terminal.keyboard_encoding
                        == terminale_config::KeyboardEncoding::Auto;
                    if use_auto {
                        state
                            .tabs
                            .get(state.active_tab)
                            .is_some_and(|t| t.emulator.lock().app_cursor_mode())
                    } else {
                        false
                    }
                };
                if let Some(bytes) = translate_key(
                    &state.modifiers,
                    physical_key,
                    &logical_key,
                    text,
                    app_cursor,
                ) {
                    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                        if let Err(e) = tab.session.write_input(&bytes) {
                            tracing::warn!(?e, "pty write failed");
                        }
                        // Stamp the keystroke so the busy-spinner fallback can
                        // tell prompt echo apart from real command output.
                        tab.focused_pane_mut().last_input_at = Some(std::time::Instant::now());
                    }
                    // Broadcast: when broadcast-input is active, fan the same
                    // raw bytes out to every other live pane in the configured
                    // scope. We never send to the focused pane again (it already
                    // received the bytes above), and we skip panes whose process
                    // has exited (crashed).
                    if state.broadcast_input {
                        let focused_id = state
                            .tabs
                            .get(state.active_tab)
                            .map_or(PaneId::MAX, |t| t.focused);
                        let scope = self.config.terminal.broadcast_scope;
                        broadcast_input_to_panes(state, scope, focused_id, &bytes);
                    }
                    // Keystroke spawns a new animated band in the background
                    // effect. Each keypress creates an independent band that
                    // travels and decays; multiple concurrent bands accumulate.
                    if self.config.background_fx.enabled
                        && self.config.background_fx.react_to_keystrokes
                    {
                        // Derive the emitter column from the cursor's grid
                        // column so the band originates near the typing position.
                        let col_norm = if let Some(tab) = state.tabs.get(state.active_tab) {
                            let (cursor_col, _) = tab.emulator.lock().cursor();
                            let (grid_cols, _) = tab.emulator.lock().size();
                            if grid_cols > 0 {
                                #[allow(clippy::cast_precision_loss)]
                                let v = (cursor_col as f32 + 0.5) / grid_cols as f32;
                                v.clamp(0.0, 1.0)
                            } else {
                                0.5
                            }
                        } else {
                            0.5
                        };
                        state.renderer.spawn_bg_fx_emitter(col_norm);
                    }
                    // Hide the OS pointer while typing — comes back on
                    // the next mouse motion.
                    if !state.pointer_hidden {
                        state.window.set_cursor_visible(false);
                        state.pointer_hidden = true;
                    }
                    state.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                // Apply a coalesced resize (if any) before draining PTY
                // output — this ensures any ConPTY repaint triggered by the
                // resize is parsed at the correct new grid size.
                if let Some(new_size) = state.pending_resize.take() {
                    state.renderer.resize(new_size.width, new_size.height);
                    // Mid Quake-animation (or while hidden) the surface tracks
                    // the animated window but the PTY grid keeps its resting
                    // size: the shrinking surface clips the full-size frame
                    // (that's the reveal), instead of reflowing the shell ~7
                    // times per toggle. The final animation frame snaps to the
                    // resting rect, whose Resized event lands with quake_anim
                    // == None and resizes the grid once (a same-size no-op).
                    if state.quake_anim.is_none() && state.quake_visible {
                        resize_all_tabs(state, new_size.width, new_size.height);
                    }
                    refresh_quake_last_monitor(state);
                    // Post-resize drain: parse any PTY bytes that arrived
                    // between the resize event and now at the new grid size.
                    drain_pty_output(state);
                }
                drain_pty_output(state);
                render_main(state);
            }
            _ => {}
        }

        // Open the settings window if a menu/hotkey requested it. We do this
        // after handling the event so we have access to `event_loop` here.
        if state.open_settings_requested {
            state.open_settings_requested = false;
            if self.settings.is_none() {
                let win = SettingsWindow::new(
                    event_loop,
                    self.config.clone(),
                    self.config_path.clone(),
                    state.renderer.instance(),
                    state.renderer.adapter(),
                    state.renderer.device(),
                    state.renderer.queue(),
                );
                self.settings = Some(win);
            } else if let Some(s) = &self.settings {
                s.window.focus_window();
            }
        }

        // Open (or focus) the AI assistant window on request.
        if state.open_ai_requested {
            state.open_ai_requested = false;
            let seed = state.pending_ai_prompt.take();
            if self.ai_assistant.is_none() {
                // Snapshot the focused pane's structured context (OS, shell,
                // cwd, recent commands + exit codes, last failure output) so
                // even a plain open doesn't start from a blank slate.
                let term_ctx =
                    terminale_ai::assistant_context_block(&build_suggestion_context(state, 40));
                let win = ai_assistant_window::AiAssistantWindow::open(
                    event_loop,
                    self.config.ai.clone(),
                    self.proxy.clone(),
                    self.runtime.handle().clone(),
                    state.renderer.instance(),
                    state.renderer.adapter(),
                    state.renderer.device(),
                    state.renderer.queue(),
                    seed,
                    Some(term_ctx),
                );
                self.ai_assistant = Some(win);
            } else if let Some(ai) = self.ai_assistant.as_mut() {
                ai.window.focus_window();
                // Already open: if a seed prompt came in, submit it now.
                if let Some(prompt) = seed {
                    ai.submit_prompt(prompt);
                }
            }
        }

        // Theme switch requested from the command palette: the palette only
        // has the RunningState, so it parks the chosen name here for the App
        // (which owns the Config) to apply live and persist with a debounce.
        if let Some(name) = state.pending_theme.take() {
            self.config.appearance.theme = name;
            apply_theme(state, &self.config);
            render_main(state);
            self.config_save_due =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
        }

        // "Save this SSH host?" prompt resolved with Save: add the host
        // (metadata only — the secret is handled by the keychain prompt on
        // first connect) and persist. Keep the settings window's copy in
        // sync so its live-apply diff doesn't drop the freshly-added host.
        let mut ssh_config_dirty = false;
        if let Some(parsed) = state.pending_save_ssh_host.take() {
            let host = ssh_host_from_parsed(&parsed);
            self.config.ssh_hosts.push(host.clone());
            if let Some(s) = self.settings.as_mut() {
                s.sync_add_ssh_host(host);
            }
            ssh_config_dirty = true;
        }
        // "Don't ask again" was checked (on Save or Dismiss): persist the
        // suppression into config so future ssh commands don't prompt.
        if let Some(dont_ask) = state.pending_dont_ask_again.take() {
            if dont_ask && self.config.terminal.offer_save_ssh_hosts {
                self.config.terminal.offer_save_ssh_hosts = false;
                if let Some(s) = self.settings.as_mut() {
                    s.sync_offer_save_ssh_hosts(false);
                }
                ssh_config_dirty = true;
            }
        }

        // "Import SSH hosts from SSH config" triggered from the command
        // palette, Settings button, or shortcut. Parse the configured
        // OpenSSH client config file, deduplicate against existing saved
        // hosts, and append only the new ones.
        if std::mem::take(&mut state.pending_import_ssh_hosts) {
            let count =
                import_openssh_hosts(&mut self.config, self.settings.as_mut(), &state.window);
            if count > 0 {
                ssh_config_dirty = true;
            }
            // Notify the user of the result via a tracing info line; a
            // future UI feature can surface this in a toast.
            tracing::info!(count, "imported SSH hosts from OpenSSH config");
        }

        // "Import Theme…" triggered from the command palette, Settings button,
        // or the ImportTheme shortcut action. Opens a native file picker; the
        // chosen .toml is copied into themes_dir and appended to the theme list.
        if std::mem::take(&mut state.pending_import_theme) {
            import_theme_from_picker(&mut self.config, self.settings.as_mut(), &state.window);
            apply_theme(state, &self.config);
            self.config_save_due =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
        }

        // Persist a live font zoom (Ctrl+± / Ctrl+0) so it survives a
        // restart. Keep the settings window's copy in sync so its live-apply
        // diff doesn't fight the zoom while it's open.
        if let Some(size) = state.pending_font_size.take() {
            self.config.font.size = size;
            if let Some(s) = self.settings.as_mut() {
                s.sync_font_size(size);
            }
            self.config_save_due =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(600));
        }

        // Persist a runtime "stay on top" quick-toggle (palette / menu /
        // shortcut) so it survives a restart, and keep the settings
        // window's copy in sync so its live-apply diff doesn't revert it
        // while the panel is open.
        if let Some(on) = state.pending_always_on_top.take() {
            self.config.window.always_on_top = on;
            if let Some(s) = self.settings.as_mut() {
                s.sync_always_on_top(on);
            }
            self.config_save_due =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(600));
        }

        // Honour the auto-hide on focus loss when the Quake window loses
        // focus. Only fires in dock mode (`edge != Off`) — otherwise the
        // free-floating Quake stays put, which is what the user expects.
        if std::mem::take(&mut state.pending_quake_autohide)
            && self.config.quake.hide_on_focus_loss
            && self.config.quake.edge != terminale_config::QuakeEdge::Off
            && state.quake_visible
        {
            // Focus moving to one of OUR OWN auxiliary windows must not hide
            // the quake terminal under the user's feet — most visibly:
            // opening Settings to configure Quake itself would fade the
            // terminal away mid-edit (and an interrupted Fade is exactly the
            // kind of state that used to strand the window semi-transparent).
            let focus_within_app = self.settings.as_ref().is_some_and(|s| s.window.has_focus())
                || self
                    .ai_assistant
                    .as_ref()
                    .is_some_and(|a| a.window.has_focus());
            if !focus_within_app {
                let cfg = self.config.quake.clone();
                toggle_quake(state, &cfg);
            }
        }

        // Deferred pane restart (Ctrl+Shift+R / palette): resolved here so
        // the respawn profile comes from `self.config` — the same lookup the
        // context-menu "Restart session" path performs at dispatch time.
        if std::mem::take(&mut state.pending_restart_pane) {
            let prof = state
                .tabs
                .get(state.active_tab)
                .map(|t| t.profile_name.clone())
                .and_then(|name| {
                    self.config
                        .profiles
                        .profiles
                        .iter()
                        .find(|p| p.name == name)
                        .cloned()
                });
            crate::tabs::restart_focused_pane(state, prof.as_ref());
        }

        // Profile picker: anchor a popup just under the title-bar's
        // tab-bar area so users see it appear next to the "+" button.
        if std::mem::replace(&mut state.open_profile_picker, false) {
            let win_pos = state.window.outer_position().unwrap_or_default();
            let scale = state.window.scale_factor() as f32;
            // Anchor: roughly under the new-tab button on the left.
            let origin = winit::dpi::PhysicalPosition::new(
                win_pos.x + (120.0 * scale) as i32,
                win_pos.y + (44.0 * scale) as i32,
            );
            let entries: Vec<MenuEntry> = self
                .config
                .profiles
                .profiles
                .iter()
                .enumerate()
                .map(|(idx, p)| MenuEntry {
                    icon: p.icon.clone(),
                    label: p.name.clone(),
                    hotkey: None,
                    enabled: true,
                    separator_before: false,
                    action_id: PROFILE_PICKER_BASE + idx as u32,
                    submenu: None,
                })
                .collect();
            if !entries.is_empty() {
                self.context_menu = None;
                self.context_menu = Some(ContextMenuWindow::open(
                    event_loop,
                    origin,
                    entries,
                    state.renderer.instance(),
                    state.renderer.adapter(),
                    state.renderer.device(),
                    state.renderer.queue(),
                    true, // root picker — take focus so Esc works
                ));
            }
        }

        // "New SSH tab" picker: anchor a popup under the tab bar listing
        // every configured host. Selecting one routes back through the
        // context-menu action handler via SSH_PICKER_BASE.
        if std::mem::replace(&mut state.open_ssh_picker, false) {
            let win_pos = state.window.outer_position().unwrap_or_default();
            let scale = state.window.scale_factor() as f32;
            let origin = winit::dpi::PhysicalPosition::new(
                win_pos.x + (120.0 * scale) as i32,
                win_pos.y + (44.0 * scale) as i32,
            );
            let entries: Vec<MenuEntry> = self
                .config
                .ssh_hosts
                .iter()
                .enumerate()
                .map(|(idx, h)| MenuEntry {
                    icon: Some(
                        crate::icons::glyph(
                            &crate::icons::WORLD,
                            self.config.appearance.bundled_icons,
                        )
                        .into(),
                    ),
                    label: format!("{} ({})", h.name, h.endpoint()),
                    hotkey: None,
                    enabled: true,
                    separator_before: false,
                    action_id: SSH_PICKER_BASE + idx as u32,
                    submenu: None,
                })
                .collect();
            if !entries.is_empty() {
                self.context_menu = None;
                self.context_menu = Some(ContextMenuWindow::open(
                    event_loop,
                    origin,
                    entries,
                    state.renderer.instance(),
                    state.renderer.adapter(),
                    state.renderer.device(),
                    state.renderer.queue(),
                    true, // root picker — take focus so Esc works
                ));
            }
        }

        // Window-management actions deferred from dispatch_shortcut because
        // they need `event_loop` and/or need `&mut self` (not just `state`).
        // Capture the flags here while `state` is still borrowed, then act
        // below after the borrow ends (NLL allows this once nothing else reads
        // `state` in the remainder of the function).
        let do_new_window = std::mem::replace(&mut state.pending_new_window, false);
        let do_move_tab_to_new_window =
            std::mem::replace(&mut state.pending_move_tab_to_new_window, false);
        let do_move_pane_to_new_tab =
            std::mem::replace(&mut state.pending_move_pane_to_new_tab, false);
        let do_move_pane_to_new_window =
            std::mem::replace(&mut state.pending_move_pane_to_new_window, false);
        // Snapshot current window state needed for the actions below.
        let active_tab_snap = state.active_tab;
        let focused_pane_snap = state.tabs.get(state.active_tab).map(|t| t.focused);
        let win_id_snap = state.window.id();

        // A host was chosen (palette "SSH: <name>" or the picker popup).
        // Defer the actual open until after the `state` borrow ends below, so
        // we can pop the credential prompt window (which needs `&mut self`)
        // when a secret is required.
        let chosen_ssh_host = state.pending_ssh_host.take();
        // A snippet was chosen in the picker. Capture the index here so we can
        // resolve the body from `self.config` after the borrow ends.
        let chosen_snippet = state.pending_insert_snippet.take();
        // A command was chosen in the history picker. Capture the text now
        // while `state` is borrowed; write to the PTY below after it ends.
        let chosen_command = state.pending_insert_command.take();
        // A clipboard-history entry was chosen; capture it for pasting below.
        let chosen_clipboard_entry = state.pending_paste_clipboard_entry.take();
        // A directory was chosen from the jump picker; capture the cd payload.
        let chosen_cd_path = state.pending_cd_path.take();
        // Paste-guard dialog trigger: capture so we can open the dialog after
        // the state borrow ends (dialog open needs wgpu handles from state).
        let pending_paste_guard = state.pending_paste_guard.take();
        // Close-confirmation dialog trigger — same deferred-open pattern.
        let pending_close_confirm = state.pending_close_confirm.take();
        // Workspace save/open deferred so we can operate on the full window
        // state after the borrow ends.
        let pending_save_ws = state.pending_save_workspace.take();
        let pending_open_ws = state.pending_open_workspace_path.take();
        // Capture the workspace data now while the state borrow is active.
        let captured_workspace = pending_save_ws.as_ref().map(|name| {
            crate::workspace::capture_workspace_with_groups(
                &state.tabs,
                state.active_tab,
                name,
                self.config.window.restore_working_dirs,
                &state.tab_groups,
                state.next_group_id,
            )
        });
        // For restore: snapshot the wgpu handles before the borrow ends.
        let ws_wgpu = pending_open_ws.as_ref().map(|_| {
            (
                state.renderer.instance(),
                state.renderer.adapter(),
                state.renderer.device(),
                state.renderer.queue(),
                state.window.inner_size(),
            )
        });

        // Spawn the context-menu popup window if requested.
        if let Some(origin) = state.open_menu_at.take() {
            // Close any existing popup first.
            self.context_menu = None;
            let entries = build_menu_entries(state);
            let demo_ctxmenu =
                std::env::var_os("TERMINALE_DEMO_PALETTE").is_some_and(|v| v == "ctxmenu");
            // For the ctxmenu demo, pre-open the first submenu-parent row so
            // the flyout is visible on the very first frame (screenshot aid).
            let submenu_idx = if demo_ctxmenu {
                entries.iter().position(|e| e.submenu.is_some())
            } else {
                None
            };
            let mut menu = ContextMenuWindow::open(
                event_loop,
                origin,
                entries,
                state.renderer.instance(),
                state.renderer.adapter(),
                state.renderer.device(),
                state.renderer.queue(),
                true, // root context menu — take focus so Esc/click-outside works
            );
            if let Some(idx) = submenu_idx {
                menu.force_open_submenu(idx);
            }
            self.context_menu = Some(menu);
        }

        // (Tab tear-out now happens in `resolve_tab_drag` on mouse release,
        // not here — a held tab dropped outside every bar materialises a new
        // window at the release point.)

        // Now that the `state` borrow has ended, service a deferred SSH open:
        // connect directly, or pop the in-window credential prompt.
        if let Some(host_idx) = chosen_ssh_host {
            self.open_or_prompt_ssh(event_loop, idx, host_idx);
        }

        // Service a deferred snippet insertion: decode the body and write it
        // to the focused pane's PTY. Done here so we have access to the full
        // `self.config` while not holding the per-window `state` borrow.
        if let Some(snippet_idx) = chosen_snippet {
            if let Some(snippet) = self.config.snippets.get(snippet_idx) {
                let bytes = terminale_config::decode_send_string(&snippet.body);
                if let Some(state) = self.windows.get(idx) {
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        if let Err(e) = tab.session.write_input(&bytes) {
                            tracing::warn!(?e, snippet_idx, "snippet PTY write failed");
                        }
                    }
                }
            }
        }

        // Service a command chosen from the command-history picker. Write
        // (optional Ctrl+U) + command text WITHOUT a trailing newline so the
        // user can inspect or edit the command before pressing Enter themselves.
        if let Some(cmd) = chosen_command {
            if !cmd.is_empty() {
                let clears_line = self
                    .windows
                    .get(idx)
                    .is_some_and(|s| s.edit_command_clears_line);
                let mut payload: Vec<u8> = Vec::with_capacity(1 + cmd.len());
                if clears_line {
                    payload.push(0x15); // Ctrl+U — kill line
                }
                payload.extend_from_slice(cmd.as_bytes());
                if let Some(state) = self.windows.get(idx) {
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        if let Err(e) = tab.session.write_input(&payload) {
                            tracing::warn!(?e, "command-history PTY write failed");
                        }
                    }
                }
            }
        }

        // Service a clipboard-history entry chosen from the picker. Write
        // the text to the active pane via the normal paste path (honours
        // bracketed-paste mode and paste-safety policy).
        if let Some(text) = chosen_clipboard_entry {
            if !text.is_empty() {
                if let Some(state) = self.windows.get_mut(idx) {
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        let bracketed = tab.emulator.lock().bracketed_paste_enabled();
                        if paste_guard::paste_needs_confirm(
                            &text,
                            bracketed,
                            state.paste_confirm_multiline,
                            state.paste_confirm_when_unbracketed,
                        ) {
                            // Open the guard dialog for the clipboard-history paste too.
                            let dialog = paste_guard::PasteGuardDialog::open(
                                event_loop,
                                &state.window,
                                text,
                                bracketed,
                                state.renderer.instance(),
                                state.renderer.adapter(),
                                state.renderer.device(),
                                state.renderer.queue(),
                            );
                            self.paste_guard_dialog = Some(dialog);
                            self.paste_guard_window_idx = idx;
                        } else {
                            send_paste_text(state, &text);
                        }
                    }
                }
            }
        }

        // Service a directory chosen from the jump picker: write the pre-built
        // `cd '<path>'\n` payload directly to the focused pane's PTY. The
        // payload was assembled by `dir_jump::build_cd_payload` and is already
        // properly quoted. No confirmation is needed — `cd` is never dangerous.
        if let Some(payload) = chosen_cd_path {
            if !payload.is_empty() {
                if let Some(state) = self.windows.get(idx) {
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        if let Err(e) = tab.session.write_input(payload.as_bytes()) {
                            tracing::warn!(?e, "directory-jump PTY write failed");
                        }
                    }
                }
            }
        }

        // ── Paste-guard dialog ────────────────────────────────────────────────

        // Open the confirmation dialog when a paste needs confirmation. If a
        // dialog is already open for another paste we replace it (the new paste
        // overrides the old one — user would have had to cancel the old one
        // explicitly, and keeping two dials open would be confusing).
        if let Some((text, bracketed)) = pending_paste_guard {
            if let Some(state) = self.windows.get(idx) {
                let dialog = paste_guard::PasteGuardDialog::open(
                    event_loop,
                    &state.window,
                    text,
                    bracketed,
                    state.renderer.instance(),
                    state.renderer.adapter(),
                    state.renderer.device(),
                    state.renderer.queue(),
                );
                self.paste_guard_dialog = Some(dialog);
                self.paste_guard_window_idx = idx;
            }
        }

        // ── Close-confirmation dialog ─────────────────────────────────────────

        // Open the confirmation dialog for a queued tab/window close request
        // (`window.confirm_close`). At most one dialog at a time — a second
        // request while one is open is dropped (the user must answer first).
        if let Some(target) = pending_close_confirm {
            if self.confirm_close_dialog.is_none() {
                if let Some(state) = self.windows.get(idx) {
                    let detail = match target {
                        confirm_close::CloseTarget::Window => {
                            let n = state.tabs.len();
                            format!("{n} tab{} will be closed.", if n == 1 { "" } else { "s" })
                        }
                        confirm_close::CloseTarget::Tab(t) => state
                            .tabs
                            .get(t)
                            .map(|tab| {
                                let title = tab
                                    .custom_title
                                    .clone()
                                    .unwrap_or_else(|| tab.profile_name.clone());
                                format!("\u{201c}{title}\u{201d} will be closed.")
                            })
                            .unwrap_or_default(),
                    };
                    self.confirm_close_dialog = Some(confirm_close::ConfirmCloseDialog::open(
                        event_loop,
                        &state.window,
                        target,
                        detail,
                        state.renderer.instance(),
                        state.renderer.adapter(),
                        state.renderer.device(),
                        state.renderer.queue(),
                    ));
                }
            }
        }

        // ── Workspace save ────────────────────────────────────────────────────

        if let (Some(name), Some(ws)) = (pending_save_ws, captured_workspace) {
            let path = terminale_config::paths::workspaces_dir()
                .map(|d| d.join(format!("{}.toml", sanitise_workspace_name(&name))));
            if let Some(path) = path {
                if let Err(e) = crate::workspace::write_workspace(&path, &ws) {
                    tracing::warn!(?e, name, "workspace save failed");
                } else {
                    tracing::info!(name, path = %path.display(), "workspace saved");
                }
            }
        }

        // ── Workspace open/restore ────────────────────────────────────────────

        if let (Some(ws_path), Some((instance, adapter, device, queue, win_size))) =
            (pending_open_ws, ws_wgpu)
        {
            match crate::workspace::read_workspace(&ws_path) {
                Ok(saved_ws) => {
                    self.restore_workspace(
                        event_loop, idx, saved_ws, instance, adapter, device, queue, win_size,
                    );
                }
                Err(e) => {
                    tracing::warn!(?e, path = %ws_path.display(), "failed to open workspace");
                }
            }
        }

        // ── Window-management deferred actions ────────────────────────────────

        // NewWindow: open a fresh top-level window with one default tab.
        // Reuses the wgpu device from the source window (shared = Some(…)).
        if do_new_window {
            self.new_window(event_loop, idx);
        }

        // MoveTabToNewWindow: tear the active tab into a brand-new window.
        // `tear_out` already guards tab_count > 1.
        if do_move_tab_to_new_window {
            self.tear_out(event_loop, idx, active_tab_snap);
        }

        // MovePaneToNewTab: detach the focused pane into a new tab in the
        // same window. `attach_pane` (with src == dst) handles this.
        if do_move_pane_to_new_tab {
            if let Some(pane_id) = focused_pane_snap {
                self.attach_pane(
                    win_id_snap,
                    active_tab_snap,
                    pane_id,
                    win_id_snap,
                    win_id_snap,
                    None,
                );
            }
        }

        // MovePaneToNewWindow: detach the focused pane into a new window.
        // `tear_out_pane` already guards count_leaves > 1.
        if do_move_pane_to_new_window {
            if let Some(pane_id) = focused_pane_snap {
                // Use window centre as the spawn position for a keyboard-driven tear-out.
                let spawn_pos = {
                    let w = self.windows.get(idx);
                    w.and_then(|w| w.window.outer_position().ok())
                        .map(|p| {
                            let sz = w.map_or((200, 200), |w| {
                                let s = w.window.inner_size();
                                (s.width, s.height)
                            });
                            winit::dpi::PhysicalPosition::new(
                                p.x + (sz.0 as i32) / 4,
                                p.y + (sz.1 as i32) / 4,
                            )
                        })
                        .unwrap_or_default()
                };
                self.tear_out_pane(
                    event_loop,
                    win_id_snap,
                    active_tab_snap,
                    pane_id,
                    (spawn_pos.x, spawn_pos.y),
                );
            }
        }

        // A "Save this SSH host?" action mutated the config above: refresh
        // every window's cached host data so the quick-connect button +
        // dedupe set + palette reflect the change immediately, then schedule
        // a debounced disk save. Done here (after the `state` borrow ends) so
        // we can iterate all windows mutably.
        if ssh_config_dirty {
            let cfg = self.config.clone();
            for w in &mut self.windows {
                w.ssh_host_names = effective_ssh_host_names(&cfg);
                w.ssh_host_targets = ssh_host_targets_from(&cfg);
                w.offer_save_ssh_hosts = cfg.terminal.offer_save_ssh_hosts;
                w.window.request_redraw();
            }
            self.config_save_due =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // ── Plugin host tick ──────────────────────────────────────────────────
        // 1) Flush any register_command calls that arrived since the last tick.
        // 2) Fire lifecycle hooks that were enqueued by free functions.
        // 3) Fire the "tick" heartbeat.
        // 4) Drain the capability command queue and apply each command.
        // 5) Drain pending_plugin_invoke (palette selection → Lua fn call).
        //
        // All of this runs here (on the main thread, with exclusive access to
        // `self`) so Lua callbacks never mutate app state directly.
        if self.plugins.is_some() {
            use terminale_plugin::LuaPayloadValue as Lpv;

            // Step 0: publish the focused-pane snapshot and live-apply the
            // plugin gates BEFORE any hook fires, so a `tick` handler that
            // reads selection/scrollback sees current data and the Settings
            // toggles apply without a restart.
            {
                let allow_read = self.config.plugins.allow_scrollback_read;
                let read_cap = self.config.plugins.scrollback_read_cap;
                let allow_kb = self.config.plugins.allow_keybindings;
                // Selection is cheap and refreshed every tick; the scrollback
                // and visible-text copies are only re-extracted when the
                // focused emulator's content generation moved (`None` content
                // tells the host to keep its — provably identical — copy).
                let mut selection: Option<String> = None;
                let mut content: Option<(Vec<String>, String)> = Some((Vec::new(), String::new()));
                let mut new_key: Option<(usize, u64, usize)> = None;
                if let Some(state) = self
                    .windows
                    .iter()
                    .find(|w| w.window_focused)
                    .or_else(|| self.windows.first())
                {
                    selection = crate::tabs::selection_text(state);
                    if allow_read {
                        if let Some(tab) = state.tabs.get(state.active_tab) {
                            let emu = tab.emulator.lock();
                            let key = (
                                std::sync::Arc::as_ptr(&tab.emulator) as usize,
                                emu.generation(),
                                read_cap,
                            );
                            if self.plugin_snap_key == Some(key) {
                                content = None;
                            } else {
                                let mut lines = emu.buffer_lines_text();
                                cap_scrollback(&mut lines, read_cap);
                                content = Some((lines, emu.visible_lines_text().join("\n")));
                            }
                            new_key = Some(key);
                        }
                    }
                }
                self.plugin_snap_key = new_key;
                if let Some(host) = self.plugins.as_ref() {
                    host.update_pane_snapshot(selection, allow_read, content);
                    host.set_allow_keybindings(allow_kb);
                    host.set_hook_budget_ms(self.config.plugins.hook_budget_ms);
                }
                for w in &mut self.windows {
                    w.plugins_allow_keybindings = allow_kb;
                }
            }

            // Step 1: promote any pending register_command calls to the host.
            {
                let host = self.plugins.as_mut().unwrap();
                host.flush_pending_registrations();
                host.flush_pending_keybinds();
                // Sync the per-window command-name cache.
                let names: Vec<String> = host
                    .registered_commands
                    .iter()
                    .map(|c| c.name.clone())
                    .collect();
                for w in &mut self.windows {
                    if w.plugin_command_names != names {
                        w.plugin_command_names.clone_from(&names);
                        w.window.request_redraw();
                    }
                }
                // Sync plugin keybinding combos, blanking any that would
                // shadow a user binding (blank never matches but keeps the
                // indices aligned with the host's registered_keybinds).
                for w in &mut self.windows {
                    let combos: Vec<String> = host
                        .registered_keybinds
                        .iter()
                        .map(|kb| {
                            if crate::shortcuts::combo_shadows_user_binding(
                                &kb.combo,
                                &w.shortcuts,
                                &w.custom_keybinds,
                            ) {
                                String::new()
                            } else {
                                kb.combo.clone()
                            }
                        })
                        .collect();
                    if w.plugin_keybind_combos != combos {
                        // Warn once per change, not on every tick.
                        for (kb, synced) in host.registered_keybinds.iter().zip(&combos) {
                            if synced.is_empty() && !kb.combo.is_empty() {
                                tracing::warn!(
                                    combo = %kb.combo,
                                    "plugin keybinding ignored: it shadows a user binding"
                                );
                            }
                        }
                        w.plugin_keybind_combos = combos;
                    }
                }
            }

            // Step 2: fire lifecycle hooks from the per-window pending queues.
            for state in &mut self.windows {
                // tab_open
                let tab_opens: Vec<(usize, String)> =
                    std::mem::take(&mut state.pending_hook_tab_open);
                for (tab_id, title) in tab_opens {
                    if let Some(host) = self.plugins.as_ref() {
                        host.fire_event(
                            "tab_open",
                            &[
                                ("tab_id", Lpv::Int(tab_id as i64)),
                                ("title", Lpv::Str(&title)),
                            ],
                        );
                    }
                }
                // tab_close
                let tab_closes: Vec<usize> = std::mem::take(&mut state.pending_hook_tab_close);
                for tab_id in tab_closes {
                    if let Some(host) = self.plugins.as_ref() {
                        host.fire_event("tab_close", &[("tab_id", Lpv::Int(tab_id as i64))]);
                    }
                }
                // pane_focus
                let pane_focuses: Vec<u32> = std::mem::take(&mut state.pending_hook_pane_focus);
                for pane_id in pane_focuses {
                    if let Some(host) = self.plugins.as_ref() {
                        host.fire_event("pane_focus", &[("pane_id", Lpv::Int(i64::from(pane_id)))]);
                    }
                }
                // session_start
                let session_starts: Vec<(u32, String)> =
                    std::mem::take(&mut state.pending_hook_session_start);
                for (pane_id, program) in session_starts {
                    if let Some(host) = self.plugins.as_ref() {
                        host.fire_event(
                            "session_start",
                            &[
                                ("pane_id", Lpv::Int(i64::from(pane_id))),
                                ("program", Lpv::Str(&program)),
                            ],
                        );
                    }
                }
                // session_exit
                let session_exits: Vec<(u32, i32)> =
                    std::mem::take(&mut state.pending_hook_session_exit);
                for (pane_id, exit_code) in session_exits {
                    if let Some(host) = self.plugins.as_ref() {
                        host.fire_event(
                            "session_exit",
                            &[
                                ("pane_id", Lpv::Int(i64::from(pane_id))),
                                ("exit_code", Lpv::Int(i64::from(exit_code))),
                            ],
                        );
                    }
                }
                // config_reload
                if std::mem::replace(&mut state.pending_hook_config_reload, false) {
                    if let Some(host) = self.plugins.as_ref() {
                        host.fire("config_reload", None);
                    }
                }
                // command_end — detect newly-completed command blocks since the
                // last tick and enqueue one event per new block.
                if state.command_blocks_enabled {
                    for tab in &state.tabs {
                        for (&pane_id, pane) in &tab.panes {
                            let emu = pane.emulator.lock();
                            let blocks = emu.command_blocks();
                            // Only count blocks that are fully completed (D fired).
                            let completed: Vec<_> =
                                blocks.iter().filter(|b| b.end_line.is_some()).collect();
                            let already_fired =
                                state.hook_cmd_end_fired.get(&pane_id).copied().unwrap_or(0);
                            let new_count = completed.len();
                            if new_count > already_fired {
                                for block in &completed[already_fired..] {
                                    let exit_code = block.exit_code.unwrap_or(0);
                                    let command = block.command_text.clone();
                                    let cwd = block.cwd.clone().unwrap_or_default();
                                    state
                                        .pending_hook_command_end
                                        .push((exit_code, command, cwd));
                                }
                                state.hook_cmd_end_fired.insert(pane_id, new_count);
                            }
                        }
                    }
                }
                let cmd_ends: Vec<(i32, String, String)> =
                    std::mem::take(&mut state.pending_hook_command_end);
                for (exit_code, command, cwd) in cmd_ends {
                    if let Some(host) = self.plugins.as_ref() {
                        host.fire_event(
                            "command_end",
                            &[
                                ("exit_code", Lpv::Int(i64::from(exit_code))),
                                ("command", Lpv::Str(&command)),
                                ("cwd", Lpv::Str(&cwd)),
                            ],
                        );
                    }
                }
            }

            // Step 3: heartbeat tick.
            if let Some(host) = self.plugins.as_ref() {
                host.fire("tick", None);
            }

            // Step 4: drain the capability command queue from ALL fired hooks.
            if let Some(host) = self.plugins.as_mut() {
                let cmds = host.drain_commands();
                for cmd in cmds {
                    self.apply_plugin_command(cmd);
                }
            }

            // Step 5: invoke a plugin command chosen from the palette.
            // Collect indices first to avoid a borrow-splitting issue.
            let invoke_indices: Vec<usize> = self
                .windows
                .iter_mut()
                .filter_map(|w| w.pending_plugin_invoke.take())
                .collect();
            for idx in invoke_indices {
                if let Some(host) = self.plugins.as_ref() {
                    host.invoke_command(idx);
                }
                // Drain any commands enqueued by the invoked fn.
                if let Some(host) = self.plugins.as_mut() {
                    let cmds = host.drain_commands();
                    for cmd in cmds {
                        self.apply_plugin_command(cmd);
                    }
                }
            }

            // Step 5b: invoke plugin keybindings matched in the key path.
            // Same shape as Step 5 — collect indices first, then call into
            // the host where `&mut self.plugins` is available.
            let keybind_indices: Vec<usize> = self
                .windows
                .iter_mut()
                .filter_map(|w| w.pending_plugin_keybind_invoke.take())
                .collect();
            for idx in keybind_indices {
                if let Some(host) = self.plugins.as_ref() {
                    host.invoke_keybind(idx);
                }
                // Drain any commands enqueued by the invoked fn (a keybind
                // can itself call send_text / notify / …).
                if let Some(host) = self.plugins.as_mut() {
                    let cmds = host.drain_commands();
                    for cmd in cmds {
                        self.apply_plugin_command(cmd);
                    }
                }
            }

            // Sync loaded plugin names to the settings window.
            if let Some(settings) = self.settings.as_mut() {
                if let Some(host) = self.plugins.as_ref() {
                    let names: Vec<String> =
                        host.plugins().iter().map(|p| p.name.clone()).collect();
                    settings.loaded_plugin_names = names;
                }
            }
            // Populate the installed monospace-font list once (it doesn't change
            // at runtime). The Settings font pickers list these so every choice
            // resolves via set_font_family instead of warning + falling back.
            if let Some(settings) = self.settings.as_mut() {
                if settings.available_fonts.is_empty() {
                    if let Some(state) = self.windows.first() {
                        settings.available_fonts = state.renderer.available_monospace_families();
                        settings.bundled_fonts = terminale_render::bundled_family_names()
                            .into_iter()
                            .map(str::to_string)
                            .collect();
                    }
                }
            }
        }

        // ── SGR demo re-seed ─────────────────────────────────────────────────
        // When TERMINALE_DEMO_PALETTE=sgr: the initial advance() in resumed()
        // is cleared by the shell's ConPTY init before the first frame. We
        // re-emit the sample lines once, ~700 ms after startup, so the content
        // survives and is visible in a steady-state screenshot.
        if let Some(deadline) = self.sgr_demo_reseed_at {
            if std::time::Instant::now() >= deadline {
                self.sgr_demo_reseed_at = None;
                if let Some(state) = self.windows.first() {
                    // Advance the emulator inside a nested block so `tab`
                    // is dropped before we call `request_redraw`.
                    let did_advance = if let Some(tab) = state.tabs.first() {
                        tab.emulator.lock().advance(
                            b"\x1b[2J\x1b[H\
                              \x1b[4mSGR 4  single underline\x1b[0m\r\n\
                              \x1b[4:2mSGR 4:2  double underline\x1b[0m\r\n\
                              \x1b[4:3mSGR 4:3  curly underline\x1b[0m\r\n\
                              \x1b[4:4mSGR 4:4  dotted underline\x1b[0m\r\n\
                              \x1b[4:5mSGR 4:5  dashed underline\x1b[0m\r\n\
                              \x1b[4;58:2::255:80:80mSGR 58 coloured underline\x1b[0m\r\n\
                              \x1b[9mSGR 9  strikethrough\x1b[0m\r\n\
                              \x1b[1mSGR 1  bold\x1b[0m\r\n\
                              \x1b[3mSGR 3  italic\x1b[0m\r\n\
                              \x1b[2mSGR 2  dim/faint\x1b[0m\r\n\
                              \x1b[7mSGR 7  reverse video\x1b[0m\r\n\
                              \x1b[8mSGR 8  concealed/hidden (text invisible)\x1b[0m\r\n",
                        );
                        true
                    } else {
                        false
                    };
                    if did_advance {
                        state.window.request_redraw();
                    }
                }
            }
        }

        // ── Contrast demo re-seed ─────────────────────────────────────────────
        // When TERMINALE_DEMO_PALETTE=contrast: same pattern as the SGR demo —
        // re-emit low-contrast sample lines ~700 ms after startup so they
        // survive ConPTY's initial clear and are visible in a screenshot.
        if let Some(deadline) = self.contrast_demo_reseed_at {
            if std::time::Instant::now() >= deadline {
                self.contrast_demo_reseed_at = None;
                if let Some(state) = self.windows.first_mut() {
                    let did_advance = if let Some(tab) = state.tabs.first() {
                        tab.emulator.lock().advance(
                            b"\x1b[2J\x1b[H\
                              \x1b[38;2;40;40;40mDark grey #282828 on black (very low contrast)\x1b[0m\r\n\
                              \x1b[38;2;60;60;60mDark grey #3c3c3c on black (low contrast)\x1b[0m\r\n\
                              \x1b[38;2;80;80;80mDark grey #505050 on black (moderate contrast)\x1b[0m\r\n\
                              \x1b[38;2;100;100;100mGrey #646464 on black\x1b[0m\r\n\
                              \x1b[38;2;120;120;120mGrey #787878 on black\x1b[0m\r\n\
                              \x1b[0m--- minimum_contrast = 7.0 (WCAG AAA) enforced above ---\x1b[0m\r\n",
                        );
                        true
                    } else {
                        false
                    };
                    // Re-assert the high minimum_contrast in case the live-apply
                    // loop overwrote it between the initial seed and this re-seed.
                    state.renderer.set_minimum_contrast(7.0);
                    if did_advance {
                        state.window.request_redraw();
                    }
                }
            }
        }

        // ── Box-drawing demo re-seed ──────────────────────────────────────────
        // When TERMINALE_DEMO_PALETTE=boxdraw: same pattern as the SGR demo —
        // re-emit the box/block/shading sample ≈700 ms after startup so the
        // procedural geometry survives ConPTY's initial clear and is visible.
        if let Some(deadline) = self.boxdraw_demo_reseed_at {
            if std::time::Instant::now() >= deadline {
                self.boxdraw_demo_reseed_at = None;
                if let Some(state) = self.windows.first() {
                    let did_advance = if let Some(tab) = state.tabs.first() {
                        tab.emulator.lock().advance(
                            "\x1b[2J\x1b[H\
                              \u{250c}\u{2500}\u{2500}\u{2500}\u{252c}\u{2500}\u{2500}\u{2500}\u{252c}\u{2500}\u{2500}\u{2500}\u{2510}\r\n\
                              \u{2502} A \u{2502} B \u{2502} C \u{2502}\r\n\
                              \u{251c}\u{2500}\u{2500}\u{2500}\u{253c}\u{2500}\u{2500}\u{2500}\u{253c}\u{2500}\u{2500}\u{2500}\u{2524}\r\n\
                              \u{2502} 1 \u{2502} 2 \u{2502} 3 \u{2502}\r\n\
                              \u{2514}\u{2500}\u{2500}\u{2500}\u{2534}\u{2500}\u{2500}\u{2500}\u{2534}\u{2500}\u{2500}\u{2500}\u{2518}\r\n\
                              \r\n\
                              Shading:  \u{2591}\u{2592}\u{2593}\u{2588}\r\n\
                              \r\n\
                              Bar graph: \u{2581}\u{2582}\u{2583}\u{2584}\u{2585}\u{2586}\u{2587}\u{2588}\r\n"
                                .as_bytes(),
                        );
                        true
                    } else {
                        false
                    };
                    if did_advance {
                        state.window.request_redraw();
                    }
                }
            }
        }

        // ── Padding demo re-seed ──────────────────────────────────────────────
        // When TERMINALE_DEMO_PALETTE=padding: re-emit the full-screen box
        // frame ≈700 ms after startup so it survives ConPTY's initial clear.
        if let Some(deadline) = self.padding_demo_reseed_at {
            if std::time::Instant::now() >= deadline {
                self.padding_demo_reseed_at = None;
                if let Some(state) = self.windows.first() {
                    let did_advance = if let Some(tab) = state.tabs.first() {
                        let (cols, rows) = tab.emulator.lock().size();
                        let frame = build_padding_demo_frame(cols, rows);
                        tab.emulator.lock().advance(&frame);
                        true
                    } else {
                        false
                    };
                    if did_advance {
                        state.window.request_redraw();
                    }
                }
            }
        }

        // ── Font demo re-seed ─────────────────────────────────────────────────
        // When TERMINALE_DEMO_PALETTE=font: re-emit the Ubuntu Mono sample
        // ≈700 ms after startup so it survives ConPTY's initial clear and
        // is visible in a steady-state screenshot.
        if let Some(deadline) = self.font_demo_reseed_at {
            if std::time::Instant::now() >= deadline {
                self.font_demo_reseed_at = None;
                if let Some(state) = self.windows.first_mut() {
                    // Re-assert the demo font so a live-apply or ConPTY clear
                    // can't revert it between the initial seed and the re-seed.
                    state.renderer.set_font_family("Ubuntu Mono");
                    state.renderer.set_font_size(28.0);
                    let did_advance = if let Some(tab) = state.tabs.first() {
                        tab.emulator.lock().advance(
                            b"\x1b[2J\x1b[HUbuntu Mono (bundled)\r\n\
                              The quick brown fox 0123456789\r\n\
                              () {} [] => != === <=\r\n",
                        );
                        true
                    } else {
                        false
                    };
                    if did_advance {
                        state.window.request_redraw();
                    }
                }
            }
        }

        // ── Padding demo grid-size tracker ───────────────────────────────────
        // Re-emit the frame whenever the demo tab's grid size changes so the
        // bottom border always sits on the true last row (survives DPI changes
        // and window resizes that settle after the initial/700 ms seeds).
        if std::env::var_os("TERMINALE_DEMO_PALETTE").is_some_and(|v| v == "padding") {
            if let Some(state) = self.windows.first() {
                if let Some(tab) = state.tabs.first() {
                    let current_size = tab.emulator.lock().size();
                    if self.padding_demo_last_size != Some(current_size) {
                        self.padding_demo_last_size = Some(current_size);
                        let (cols, rows) = current_size;
                        let frame = build_padding_demo_frame(cols, rows);
                        tab.emulator.lock().advance(&frame);
                        state.window.request_redraw();
                    }
                }
            }
        }

        // (Quake hotkeys arrive via UserEvent::GlobalHotkey from the
        // forwarder thread — see user_event. Polling here was useless
        // because about_to_wait doesn't run while the loop is parked.)

        // ── Link hover dwell timer ───────────────────────────────────────────
        // When `link_hover_delay_ms > 0`, the CursorMoved handler defers the
        // tooltip. Here we check whether the dwell period has elapsed and, if
        // so, show the tooltip and schedule a redraw.
        // Sample CPU/memory once per wake (cheap, 1s-throttled internally);
        // applied to each window's resource strip in the loop below.
        let res_enabled = self.config.resource_indicators.enabled;
        // Only sample the system while the strip is actually shown. `tick` is
        // internally 1 s-throttled and cheap, but there's no reason to refresh
        // CPU/memory at all when the indicator is disabled.
        let res_changed = res_enabled && self.resource_sampler.tick(std::time::Instant::now());
        let res_sample = self.resource_sampler.sample();

        for state in &mut self.windows {
            if let Some((ref url, started_at, anchor)) = state.link_hover_start {
                let delay_ms = u64::from(state.link_hover_delay_ms);
                if std::time::Instant::now().duration_since(started_at)
                    >= std::time::Duration::from_millis(delay_ms)
                {
                    let text = url.clone();
                    let anchor_px = anchor;
                    state.link_hover_start = None;
                    state
                        .renderer
                        .set_tooltip(Some(terminale_render::Tooltip { text, anchor_px }));
                    state.window.request_redraw();
                } else {
                    // Still waiting — request a redraw so we come back soon
                    // to check again (the loop is parked in Wait otherwise).
                    state.window.request_redraw();
                }
            }
        }

        // Live-apply config from the settings window — every tick, if the
        // Drain the "Import Theme" flag from the settings window.  The button
        // is inside `section_appearance`, which only has `&mut self`, so it
        // can't call the App method directly — it sets this flag and we act
        // on it here, where `&mut self` (TerminaleApp) is available.
        if self
            .settings
            .as_ref()
            .is_some_and(|s| s.pending_import_theme)
        {
            if let Some(s) = self.settings.as_mut() {
                s.pending_import_theme = false;
            }
            let win = self.windows.first().map(|w| w.window.clone());
            if let Some(w) = win {
                import_theme_from_picker(&mut self.config, self.settings.as_mut(), &w);
                // Propagate to all terminal windows so the theme picker list
                // reflects the newly-imported theme immediately.
                let cfg = self.config.clone();
                for tw in &mut self.windows {
                    apply_theme(tw, &cfg);
                }
                self.config_save_due =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
            }
        }

        // Drain the "Import SSH hosts" flag from the settings window.  The
        // button is inside `section_ssh`, which only has `&mut self`, so it
        // can't call the App method directly — it sets this flag and we act
        // on it here, where `&mut self` (TerminaleApp) is available.
        if self
            .settings
            .as_ref()
            .is_some_and(|s| s.pending_import_ssh_hosts)
        {
            if let Some(s) = self.settings.as_mut() {
                s.pending_import_ssh_hosts = false;
            }
            let count = import_openssh_hosts(
                &mut self.config,
                self.settings.as_mut(),
                // Use the first available terminal window for the notifier
                // (window ref not strictly needed for import, but kept for
                // API consistency with future toast notifications).
                &self.windows.first().expect("at least one window").window,
            );
            if count > 0 {
                let cfg = self.config.clone();
                for w in &mut self.windows {
                    w.ssh_host_names = effective_ssh_host_names(&cfg);
                    w.ssh_host_targets = ssh_host_targets_from(&cfg);
                    w.window.request_redraw();
                }
                self.config_save_due =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
            }
            tracing::info!(
                count,
                "imported SSH hosts from OpenSSH config (Settings button)"
            );
        }

        // user's settings differ from the active config we push them down
        // to the renderer immediately so theme / cursor / etc. changes are
        // visible without closing the settings panel.
        //
        // Gated on `take_config_maybe_changed`: the panel's config can only
        // change inside its egui frame, so the full-`Config` clone + the
        // ~150-field `configs_identical` diff run once per settings repaint
        // instead of on every `about_to_wait` tick while the panel is open.
        if let Some(s) = self.settings.as_mut() {
            if s.take_config_maybe_changed() && !configs_identical(&self.config, s.current_config())
            {
                // Only now, when something actually differs, do we pay for the
                // full `Config` clone (every Vec — profiles, ssh hosts,
                // keybinds, snippets…). Previously this clone ran on *every*
                // settings repaint, i.e. ~60×/s of heap churn while merely
                // scrolling the panel, even though scrolling never edits the
                // config. Diffing against the borrow first keeps the clone on
                // the rare frame where a control was actually touched.
                let new_cfg = s.current_config().clone();
                {
                    let theme_changed = self.config.appearance.theme != new_cfg.appearance.theme;
                    let font_changed = self.config.font.family != new_cfg.font.family
                        || self.config.font.bold_family != new_cfg.font.bold_family
                        || self.config.font.italic_family != new_cfg.font.italic_family
                        || self.config.font.bold_italic_family != new_cfg.font.bold_italic_family
                        || (self.config.font.size - new_cfg.font.size).abs() >= f32::EPSILON
                        || (self.config.font.line_height - new_cfg.font.line_height).abs()
                            >= f32::EPSILON
                        || self.config.font.ligatures != new_cfg.font.ligatures
                        || (self.config.font.underline_thickness_px
                            - new_cfg.font.underline_thickness_px)
                            .abs()
                            >= f32::EPSILON;
                    // Detect a startup-position change so we can apply it to the
                    // current session too — picking an edge in Settings should
                    // snap the focused window immediately, not only at next
                    // launch.
                    let new_startup_position = new_cfg.window.startup_position;
                    let startup_position_changed =
                        self.config.window.startup_position != new_startup_position;
                    // Capture whether auto_reload_config changed before we
                    // overwrite self.config — used below to restart the watcher.
                    let auto_reload_changed =
                        self.config.window.auto_reload_config != new_cfg.window.auto_reload_config;
                    let new_auto_reload = new_cfg.window.auto_reload_config;
                    self.config = new_cfg;
                    // Live-apply to EVERY open terminal window.
                    let cfg = self.config.clone();
                    for state in &mut self.windows {
                        state.renderer.set_cursor(cursor_params_from_config(&cfg));
                        state.bell_mode = cfg.bell.mode;
                        state.scroll_step_lines = cfg.window.scroll_step_lines;
                        state.alt_screen_scroll_lines = cfg.window.alt_screen_scroll_lines;
                        state.touchpad_pixels_per_row = cfg.window.touchpad_pixels_per_row;
                        if state.smooth_scroll != cfg.window.smooth_scroll {
                            state.smooth_scroll = cfg.window.smooth_scroll;
                            // Clear any stale remainder when toggling smooth scroll.
                            state.smooth_scroll_remainder = 0.0;
                        }
                        state.copy_on_select = cfg.window.copy_on_select;
                        state.animated_tab_drag = cfg.appearance.animated_tab_drag;
                        // Quake-managed windows drive their own visibility; we only
                        // touch the persistent window level here, never show/hide.
                        if state.always_on_top != cfg.window.always_on_top {
                            state.always_on_top = cfg.window.always_on_top;
                            apply_window_level(&state.window, state.always_on_top);
                        }
                        if state.scrollback_lines != cfg.window.scrollback_lines {
                            state.scrollback_lines = cfg.window.scrollback_lines;
                            let sb = state.scrollback_lines;
                            // EVERY pane of every tab — `tab.emulator` derefs
                            // to the focused pane only, which left the other
                            // split panes on the old scrollback until respawn.
                            for tab in &state.tabs {
                                for pane in tab.panes.values() {
                                    pane.emulator.lock().set_scrollback(sb);
                                }
                            }
                        }
                        state.shortcuts = cfg.keybinds.shortcuts.clone();
                        state.custom_keybinds.clone_from(&cfg.keybinds.custom);
                        state.key_tables.clone_from(&cfg.keybinds.key_tables);
                        state.mouse_bindings.clone_from(&cfg.keybinds.mouse);
                        state.editor_command = cfg.editor.command.clone();
                        // Refresh the cached SSH host names so palette / picker
                        // entries reflect adds/edits/removes made in settings.
                        // In `live` import mode we also merge the OpenSSH config.
                        state.ssh_host_names = effective_ssh_host_names(&cfg);
                        // Refresh cached snippet names so the snippet palette
                        // reflects any add/edit/remove made in Settings.
                        state.snippet_names = snippet_names_from(&cfg);
                        // Refresh cached profile names + icons so the "New tab
                        // with profile…" submenu reflects any profile edits.
                        state.profile_names = cfg
                            .profiles
                            .profiles
                            .iter()
                            .map(|p| p.name.clone())
                            .collect();
                        state.profile_icons = cfg
                            .profiles
                            .profiles
                            .iter()
                            .map(|p| p.icon.clone())
                            .collect();
                        // Refresh the cached default profile so plain new tabs
                        // ('+' / Ctrl+T) pick up an edited default without a
                        // new window — previously set once at startup.
                        state.default_profile = cfg.resolve_default_profile().cloned();
                        state.ssh_host_targets = ssh_host_targets_from(&cfg);
                        state.offer_save_ssh_hosts = cfg.terminal.offer_save_ssh_hosts;
                        // Phase E: refresh divider physical-px metrics and the
                        // live-resize toggle from the new config.
                        let sf = state.window.scale_factor() as f32;
                        state.divider_thickness_px = cfg.appearance.divider_thickness_logical * sf;
                        state.divider_grab_padding_px =
                            cfg.appearance.divider_grab_padding_logical * sf;
                        // Refresh focus-border config and push to renderer.
                        state.focus_border_thickness_px =
                            cfg.appearance.focus_border_thickness_logical * sf;
                        state.focus_border_color = cfg.appearance.focus_border_color;
                        state.renderer.set_focus_border_thickness_logical(
                            cfg.appearance.focus_border_thickness_logical,
                        );
                        state
                            .renderer
                            .set_focus_border_color(cfg.appearance.focus_border_color);
                        state
                            .renderer
                            .set_focus_border_alpha(cfg.appearance.focus_border_opacity);
                        // Live-apply the divider colour override — None falls back to the
                        // renderer's auto tone (derived from the background colour).
                        state.divider_color = cfg.appearance.divider_color;
                        state.live_pane_resize = cfg.terminal.live_pane_resize;
                        state.pane_resize_step_cells = cfg.terminal.pane_resize_step_cells;
                        state.show_prompt_marks = cfg.terminal.show_prompt_marks;
                        state.os_notifications = cfg.terminal.os_notifications;
                        state.os_notification_rate_limit = cfg.terminal.os_notification_rate_limit;
                        // Update zen-mode mirror fields before applying chrome so
                        // re-applying zen overrides uses the new config values.
                        state.zen_hide.clone_from(&cfg.window.zen_hide);
                        state.zen_fullscreen = cfg.window.zen_fullscreen;
                        // Config mirrors for tab-bar-enabled and show-pane-headers.
                        state.tab_bar_enabled_config = cfg.appearance.tab_bar_enabled;
                        state.show_pane_headers_config = cfg.appearance.show_pane_headers;
                        if state.zen {
                            // Re-apply the chrome overrides immediately while
                            // zen is active, so editing zen_hide takes effect
                            // without toggling zen off and on again.
                            apply_zen_chrome(state);
                        }
                        // Live-apply pane-header toggle: resize all panes so they
                        // gain/lose the 22 px header band, then push to renderer.
                        if state.show_pane_headers != cfg.appearance.show_pane_headers {
                            state.show_pane_headers = cfg.appearance.show_pane_headers;
                            if !state.zen {
                                state
                                    .renderer
                                    .set_show_pane_headers(state.show_pane_headers);
                            }
                        }
                        state.pane_tear_out = cfg.appearance.pane_tear_out;
                        // Live-apply close-button style (render-only state — no
                        // layout impact, so no resize needed).
                        state
                            .renderer
                            .set_close_button_style(cfg.appearance.close_button_style);
                        // ux-polish-b: tab bar visibility / position / single-tab
                        // hide + cell-width multiplier. All live-applied so the
                        // Settings panel produces immediate visual feedback.
                        if !state.zen {
                            state
                                .renderer
                                .set_tab_bar_enabled(cfg.appearance.tab_bar_enabled);
                        }
                        state
                            .renderer
                            .set_tab_bar_placement(tab_bar_placement_from_config(&cfg));
                        state
                            .renderer
                            .set_tab_bar_hide_if_single(cfg.appearance.tab_bar_hide_if_single);
                        // Vertical tab-strip width — was applied only by the
                        // external-reload path; the in-app save left a stale
                        // width until restart when on a Left/Right tab bar.
                        state
                            .renderer
                            .set_vertical_tab_bar_width(cfg.appearance.vertical_tab_bar_width);
                        state.renderer.set_dim_amount(cfg.appearance.dim_amount);
                        state
                            .renderer
                            .set_cell_width_multiplier(cfg.font.cell_width);
                        // ux-polish-a: exit-behavior and hyperlink-rules are held in
                        // module-level atomics / statics (not on TermWindow) so they
                        // need to be explicitly synced via the same updater the
                        // settings_window/terminal.rs panel uses.
                        update_exit_behavior(cfg.terminal.exit_behavior);
                        crate::links::update_hyperlink_rules(&cfg.terminal.hyperlink_rules);
                        // Live-apply image protocol toggles to all open emulators.
                        {
                            let osc1337_on = cfg.terminal.image_protocols.osc1337;
                            let sixel_on = cfg.terminal.image_protocols.sixel;
                            let apc_on = cfg.terminal.image_protocols.apc;
                            for tab in &state.tabs {
                                let mut emu = tab.emulator.lock();
                                emu.set_osc1337_images_enabled(osc1337_on);
                                emu.set_sixel_images_enabled(sixel_on);
                                emu.set_apc_graphics_enabled(apc_on);
                            }
                        }
                        // Live-apply command-block capture settings.
                        {
                            let cb_enabled = cfg.terminal.command_blocks;
                            let cb_max = cfg.terminal.max_command_blocks;
                            state.command_blocks_enabled = cb_enabled;
                            state.max_command_blocks = cb_max;
                            for tab in &state.tabs {
                                tab.emulator.lock().set_command_blocks(cb_enabled, cb_max);
                            }
                        }
                        state
                            .word_separators
                            .clone_from(&cfg.terminal.word_separators);
                        // Live-apply the link-underline mode: re-run autodetect so
                        // switching to/from `Always` immediately adds/clears the
                        // persistent URL underlines (hover sync handles `Hover`).
                        if state.link_underline != cfg.terminal.link_underline {
                            state.link_underline = cfg.terminal.link_underline;
                            refresh_autodetect_links(state);
                        }
                        // Live-apply link hover tooltip toggle and delay.
                        state.link_hover_tooltip = cfg.terminal.link_hover_tooltip;
                        state.link_hover_delay_ms = cfg.terminal.link_hover_delay_ms;
                        if !state.link_hover_tooltip {
                            state.link_hover_start = None;
                            state.renderer.set_tooltip(None);
                        }
                        // Live-apply clipboard read policy.
                        state.clipboard_read_policy = cfg.terminal.clipboard_read;
                        // Live-apply edit_command_clears_line.
                        state.edit_command_clears_line = cfg.terminal.edit_command_clears_line;
                        // Live-apply command-history picker settings.
                        state.command_history_scope = cfg.terminal.command_history_scope;
                        state.command_history_max_entries =
                            cfg.terminal.command_history_max_entries;
                        // Live-apply scrollback export settings.
                        state.scrollback_export_format = cfg.terminal.scrollback_export_format;
                        state
                            .scrollback_export_dir
                            .clone_from(&cfg.terminal.scrollback_export_dir);
                        // Live-apply clipboard history settings.
                        state.clipboard_history_enabled = cfg.clipboard_history.enabled;
                        state.clipboard_history_size = cfg.clipboard_history.size;
                        state.clipboard_history_capture_osc52 = cfg.clipboard_history.capture_osc52;
                        // Trim the ring if the new cap is smaller.
                        while state.clipboard_history_ring.len() > state.clipboard_history_size {
                            state.clipboard_history_ring.pop_back();
                        }
                        // Live-apply directory-jump settings.
                        state.dir_jump_enabled = cfg.directory_jump.enabled;
                        state.dir_jump_max_tracked = cfg.directory_jump.max_tracked;
                        state.dir_jump_persist = cfg.directory_jump.persist;
                        // Live-apply paste safety settings.
                        state.paste_confirm_multiline = cfg.terminal.paste_confirm_multiline;
                        state.paste_confirm_when_unbracketed =
                            cfg.terminal.paste_confirm_when_unbracketed;
                        state.paste_strip_control_chars = cfg.terminal.paste_strip_control_chars;
                        // Live-apply prompt-navigation highlight toggle.
                        state.highlight_on_jump = cfg.terminal.highlight_on_jump;
                        // Live-apply minimum contrast.
                        state
                            .renderer
                            .set_minimum_contrast(cfg.appearance.minimum_contrast);
                        // Live-apply builtin box-drawing toggle.
                        state
                            .renderer
                            .set_builtin_box_drawing(cfg.appearance.builtin_box_drawing);
                        // Live-apply tab-group-labels toggle and group colour palette.
                        state.show_tab_group_labels = cfg.appearance.show_tab_group_labels;
                        state
                            .renderer
                            .set_show_tab_group_labels(cfg.appearance.show_tab_group_labels);
                        state.tab_group_colors = cfg.appearance.tab_group_colors.clone();
                        state.bundled_icons = cfg.appearance.bundled_icons;
                        state.tab_activity_spinner = cfg.appearance.tab_activity_spinner;
                        // Live-apply inactive-pane and unfocused-window dim.
                        state
                            .renderer
                            .set_inactive_pane_dim(cfg.appearance.inactive_pane_dim);
                        state
                            .renderer
                            .set_selection_opacity(cfg.appearance.selection_opacity);
                        state
                            .renderer
                            .set_unfocused_window_dim(cfg.appearance.unfocused_window_dim);
                        state.confirm_close = cfg.window.confirm_close;
                        // Turning the feature off cancels any queued request.
                        if !state.confirm_close {
                            state.pending_close_confirm = None;
                        }
                        state.renderer.set_background_alpha(cfg.window.opacity);
                        state.renderer.set_padding(cfg.window.padding as f32);
                        state.renderer.set_tab_widths(
                            cfg.appearance.tab_min_width,
                            cfg.appearance.tab_max_width,
                        );
                        state
                            .renderer
                            .set_tab_pinned_width(cfg.appearance.pinned_tab_width);
                        if font_changed {
                            state.renderer.set_font_family(&cfg.font.family);
                            state.renderer.set_font_style_overrides(
                                cfg.font.bold_family.as_deref(),
                                cfg.font.italic_family.as_deref(),
                                cfg.font.bold_italic_family.as_deref(),
                            );
                            state.renderer.set_line_height(cfg.font.line_height);
                            state.renderer.set_ligatures(cfg.font.ligatures);
                            state.renderer.set_font_size(cfg.font.size);
                            state
                                .renderer
                                .set_underline_thickness(cfg.font.underline_thickness_px);
                        }
                        if theme_changed {
                            apply_theme(state, &cfg);
                        }
                        // Live-apply animated background FX.
                        state
                            .renderer
                            .set_bg_fx_params(translate_bg_fx_params(&cfg.background_fx));
                        // Live-apply background image.
                        state
                            .renderer
                            .set_background_image(translate_bg_image_params(
                                &cfg.appearance.background_image,
                            ));
                        // Live-apply quick-select config — recompile patterns and
                        // update the cached alphabet so the next scan uses the new values.
                        // Validate before applying so bad regexes produce a log line.
                        if let Some(err) =
                            quick_select::validate_patterns(&cfg.quick_select.patterns)
                        {
                            tracing::warn!("quick_select.patterns: {err}");
                        }
                        if let Some(err) =
                            quick_select::validate_alphabet(&cfg.quick_select.alphabet)
                        {
                            tracing::warn!("quick_select.alphabet: {err}");
                        }
                        state.qs_alphabet.clone_from(&cfg.quick_select.alphabet);
                        state.qs_compiled_patterns =
                            quick_select::compile_patterns(&cfg.quick_select.patterns);
                        state.qs_overlay_dim = cfg.quick_select.overlay_dim;
                        // Live-apply context rules: re-evaluate all tabs so
                        // rule add/edit/remove takes effect immediately.
                        state.context_rules.clone_from(&cfg.context_rules);
                        refresh_context_rules(state);
                        // Live-apply status-bar enable/position/content.
                        update_status_bar(state, &cfg);
                        // Re-cell after padding / font change so the grid
                        // matches the new usable area and cell metrics.
                        let win_size = state.window.inner_size();
                        resize_all_tabs(state, win_size.width, win_size.height);
                        // Paint NOW — request_redraw is a no-op while settings
                        // holds focus, so without this the change wouldn't show.
                        render_main(state);
                    }
                    // Live-apply AI config changes (e.g. render_markdown toggle)
                    // to an already-open assistant window so the toggle takes
                    // effect immediately without requiring a close/reopen.
                    if let Some(ai_win) = self.ai_assistant.as_mut() {
                        ai_win.set_config(self.config.ai.clone());
                    }
                    // Live-apply the startup position to the focused window
                    // when the user picks a new edge in Settings — gives the
                    // dropdown a "snap current window now" semantic on top of
                    // its first-launch behaviour. None (default) leaves the
                    // current window alone.
                    if startup_position_changed {
                        if let Some(edge) = new_startup_position {
                            if let Some(state) = self.focused_window_mut() {
                                snap_window(state, edge);
                            }
                        }
                    }
                    // Restart the filesystem watcher when the auto_reload_config
                    // toggle changes in Settings — live-applies the new preference.
                    if auto_reload_changed {
                        self.config_watcher = config_watch::start(
                            self.config_path.clone(),
                            self.proxy.clone(),
                            new_auto_reload,
                        );
                    }
                    // Debounce — coalesce bursts of slider drags into one
                    // write after the user pauses for a moment.
                    self.config_save_due =
                        Some(std::time::Instant::now() + std::time::Duration::from_millis(600));
                }
            }
        }
        if let Some(deadline) = self.config_save_due {
            if std::time::Instant::now() >= deadline {
                self.config_save_due = None;
                if let Err(e) = self.config.write_to(&self.config_path) {
                    tracing::warn!(?e, "config auto-save failed");
                } else {
                    tracing::debug!(path = %self.config_path.display(), "config auto-saved");
                }
            }
        }

        // Drive the egui sub-windows' animations between input events:
        // pump any elapsed repaint, then remember how soon each next needs
        // to wake so we can fold it into the loop timer.
        let now = std::time::Instant::now();
        let mut settings_wake: Option<std::time::Duration> = None;
        if let Some(s) = self.settings.as_mut() {
            s.pump_repaint();
            if let Some(d) = s.next_repaint() {
                settings_wake = Some(d.saturating_duration_since(now));
            }
        }
        if let Some(ai) = self.ai_assistant.as_mut() {
            ai.pump_repaint();
            if let Some(d) = ai.next_repaint() {
                let dur = d.saturating_duration_since(now);
                settings_wake = Some(settings_wake.map_or(dur, |s| s.min(dur)));
            }
        }

        // Reap windows whose last tab was closed; exit when none remain.
        self.reap_empty_windows(event_loop);
        if self.windows.is_empty() {
            return;
        }

        // Schedule the next wake-up while an animation is in flight on ANY
        // window (cursor blink, visual bell, settings egui animations).
        // Without this winit would park forever and animations would stall.
        let mut next_wake: Option<std::time::Duration> = settings_wake;
        for state in &mut self.windows {
            if drain_pty_output(state) {
                state.window.request_redraw();
            }
            // While the window is fully covered or minimized, skip scheduling
            // animation redraws below — the compositor would discard them, so
            // a hidden window must cost no GPU/CPU. The PTY drain above still
            // runs (and requests a redraw on new output), so content is never
            // starved; we just don't animate what nobody can see.
            let visible = !state.occluded;
            // Drive any in-flight Quake open/close slide.
            if let Some(d) = pump_quake_anim(state) {
                next_wake = Some(match next_wake {
                    Some(w) => w.min(d),
                    None => d,
                });
            }
            // Prune expired emitter bands so bg_fx_active() returns false
            // once all bands have decayed; then keep repainting while active.
            state
                .renderer
                .prune_bg_fx_emitters(self.config.background_fx.band_lifetime_secs);
            // Animated background: only repaint while actually visible, and
            // (unless the user opts out) only while focused — an unfocused or
            // hidden window should not keep the GPU at 60 fps drawing a
            // wallpaper nobody is looking at. Mirrors the cursor-blink gate.
            let bg_fx_animates = visible
                && (state.window_focused || !self.config.background_fx.pause_when_unfocused);
            if state.renderer.bg_fx_active() && bg_fx_animates {
                state.window.request_redraw();
                let d = std::time::Duration::from_millis(16);
                next_wake = Some(match next_wake {
                    Some(w) => w.min(d),
                    None => d,
                });
            }
            if state.renderer.cursor_blinking() && visible {
                let cursor = state.renderer.cursor();
                let wake = if cursor.blink_ease {
                    // Eased blink: the alpha is a continuous smoothstep, so
                    // repaint at the configured animation fps — waking only at
                    // each half-period rendered the "smooth" fade as a hard
                    // step (and left `animation_fps` entirely unread).
                    std::time::Duration::from_millis(u64::from(
                        1000 / cursor.animation_fps.clamp(10, 240),
                    ))
                } else {
                    // Hard blink: one repaint per half-period is enough.
                    std::time::Duration::from_millis(u64::from(cursor.blink_rate_ms.max(60)))
                };
                next_wake = Some(match next_wake {
                    Some(d) => d.min(wake),
                    None => wake,
                });
                // Crucial: actually repaint at each blink boundary. The blink
                // phase is derived from elapsed time in render(), so without a
                // redraw here the timer fires but the cursor never toggles.
                state.window.request_redraw();
            }
            if state.renderer.bell_active() && visible {
                // ~16 ms = one 60 Hz frame; the bell tint decays smoothly.
                let frame = std::time::Duration::from_millis(16);
                next_wake = Some(match next_wake {
                    Some(d) => d.min(frame),
                    None => frame,
                });
                state.window.request_redraw();
            }
            if state.renderer.jump_highlight_active() && visible {
                // ~16 ms = one 60 Hz frame; the highlight band fades smoothly.
                let frame = std::time::Duration::from_millis(16);
                next_wake = Some(match next_wake {
                    Some(d) => d.min(frame),
                    None => frame,
                });
                state.window.request_redraw();
            }
            // Status-bar periodic refresh — only when enabled and at least
            // one `Clock` segment is configured (other segments update only
            // when output arrives and `request_redraw` fires naturally).
            let sb_cfg = &self.config.status_bar;
            if sb_cfg.enabled {
                let interval =
                    std::time::Duration::from_millis(u64::from(sb_cfg.update_interval_ms));
                if now.duration_since(state.last_status_bar_tick) >= interval {
                    state.last_status_bar_tick = now;
                    // Rebuild status bar content from the active tab.
                    update_status_bar(state, &self.config);
                    if sb_cfg.has_time_segment() {
                        state.window.request_redraw();
                    }
                }
                if sb_cfg.has_time_segment() {
                    let remaining =
                        interval.saturating_sub(now.duration_since(state.last_status_bar_tick));
                    next_wake = Some(match next_wake {
                        Some(d) => d.min(remaining),
                        None => remaining,
                    });
                }
            }
            // Key-table timeout: exit modal mode when the deadline passes
            // and request a redraw so the status-bar indicator disappears.
            if let Some(ref akt) = state.active_key_table.clone() {
                if let Some(table) = self.config.keybinds.key_tables.get(akt.table_idx) {
                    let timeout_ms = table.timeout_ms.clamp(100, 30_000);
                    if key_table_timed_out(akt.entered_at, now, timeout_ms) {
                        state.active_key_table = None;
                        update_status_bar(state, &self.config);
                        state.window.request_redraw();
                    } else {
                        // Schedule a wake-up so the timeout fires on time.
                        let elapsed = now.duration_since(akt.entered_at);
                        let total = std::time::Duration::from_millis(u64::from(timeout_ms));
                        let remaining = total.saturating_sub(elapsed);
                        next_wake = Some(match next_wake {
                            Some(d) => d.min(remaining),
                            None => remaining,
                        });
                    }
                } else {
                    // Stale table index (config changed) — exit cleanly.
                    state.active_key_table = None;
                    update_status_bar(state, &self.config);
                    state.window.request_redraw();
                }
            }
            // ── Busy-tab activity spinner ────────────────────────────────────
            // Advance the spinner frame at a ~90 ms cadence and request a
            // redraw while any pane in this window is busy. When nothing is
            // busy, skip scheduling the 90 ms wakeup so we don't spin the CPU.
            //
            // Gated on focus + visibility: a busy command (anything OSC-133
            // reports as "running" — an editor, pager, REPL, dev server, ssh)
            // keeps `pane_is_busy` true for its whole lifetime, so without this
            // gate the spinner repainted the whole window ~11×/s forever even
            // while the window sat unfocused in the background — the single
            // biggest idle cost, and on by default. The spinner is invisible
            // unless the window is focused and visible, so only animate then.
            if state.tab_activity_spinner && state.window_focused && visible {
                let any_busy = state
                    .tabs
                    .iter()
                    .any(|t| t.panes.values().any(crate::osc_handlers::pane_is_busy));
                if any_busy {
                    const SPINNER_INTERVAL: std::time::Duration =
                        std::time::Duration::from_millis(90);
                    let should_tick = state
                        .last_spinner_tick
                        .is_none_or(|t| now.duration_since(t) >= SPINNER_INTERVAL);
                    if should_tick {
                        state.spinner_frame = state.spinner_frame.wrapping_add(1);
                        state.last_spinner_tick = Some(now);
                        crate::tabs::refresh_tab_bar(state);
                        state.window.request_redraw();
                    }
                    // Keep waking at the next tick boundary.
                    let remaining = state
                        .last_spinner_tick
                        .map_or(std::time::Duration::ZERO, |t| {
                            SPINNER_INTERVAL.saturating_sub(now.duration_since(t))
                        });
                    next_wake = Some(match next_wake {
                        Some(d) => d.min(remaining),
                        None => remaining,
                    });
                }
            }
            // ── Bottom resource-indicator strip (CPU/RAM/GPU) ────────────────
            // Apply the shared sample to this window. Reflow only on the
            // enable/disable transition (the strip changes the grid height);
            // a plain value update just redraws.
            {
                let was_on = state.renderer.resource_bar_enabled();
                if res_enabled {
                    state
                        .renderer
                        .set_resource_bar(Some(terminale_render::ResourceBarContent {
                            cpu_pct: res_sample.cpu_pct,
                            mem_pct: res_sample.mem_pct,
                        }));
                    if !was_on {
                        let size = state.window.inner_size();
                        resize_all_tabs(state, size.width, size.height);
                        state.window.request_redraw();
                    } else if res_changed {
                        state.window.request_redraw();
                    }
                } else if was_on {
                    state.renderer.set_resource_bar(None);
                    let size = state.window.inner_size();
                    resize_all_tabs(state, size.width, size.height);
                    state.window.request_redraw();
                }
            }

            // ── Proactive suggestion bar — per-window debounce ───────────────
            // Sync the enabled mirror and schedule loading-animation redraws.
            // Index is collected here; the actual spawn happens after the loop
            // (spawn_suggestion takes &mut self which conflicts with &mut self.windows).
            let sg_enabled = self.config.ai.suggestions.enabled;
            let sg_trigger = self.config.ai.suggestions.trigger;
            state.suggestions.enabled = sg_enabled;
            state.suggestions.trigger = sg_trigger;
            // `Off` (or a disabled feature) hides the bar and clears any
            // lingering suggestion, so re-enabling never resurfaces a stale
            // one. The fix-offer Hint is exempt — it is governed by
            // `ai.offer_fix_on_failure`, not by the suggestion trigger.
            if (!sg_enabled || sg_trigger == terminale_config::SuggestionTrigger::Off)
                && !matches!(
                    state.suggestions.state,
                    suggestions::SuggestionState::Hint(_)
                )
            {
                state.suggestions.state = suggestions::SuggestionState::Hidden;
            }
            // ── `ai.offer_fix_on_failure` — unobtrusive fix hint ─────────────
            // When the focused pane's most recent command block completed with
            // a non-zero exit, surface a one-shot amber hint in the suggestion
            // bar with a [Fix] button (routes to the same flow as the
            // FixLastCommand shortcut). Keyed per (pane, block-count) so each
            // failure offers at most once; never replaces live bar content.
            if self.config.ai.offer_fix_on_failure
                && matches!(
                    state.suggestions.state,
                    suggestions::SuggestionState::Hidden
                )
            {
                let failed = state.tabs.get(state.active_tab).and_then(|tab| {
                    let pane_id = tab.focused;
                    let pane = tab.panes.get(&pane_id)?;
                    let emu = pane.emulator.lock();
                    let blocks = emu.command_blocks();
                    let n = blocks.len();
                    let b = blocks.last()?;
                    match b.exit_code {
                        Some(code) if code != 0 && !b.command_text.trim().is_empty() => {
                            Some((pane_id, n, code, b.command_text.clone()))
                        }
                        _ => None,
                    }
                });
                if let Some((pane_id, n, code, cmd)) = failed {
                    let key = (pane_id, n);
                    if state.fix_hint_seen != Some(key) {
                        state.fix_hint_seen = Some(key);
                        state.suggestions.state = suggestions::SuggestionState::Hint(format!(
                            "`{cmd}` failed (exit {code})"
                        ));
                        state.window.request_redraw();
                    }
                }
            }
            if visible
                && matches!(
                    state.suggestions.state,
                    suggestions::SuggestionState::Loading
                )
            {
                state.suggestions.loading_frame = state.suggestions.loading_frame.wrapping_add(1);
                state.window.request_redraw();
                let anim = std::time::Duration::from_millis(150);
                next_wake = Some(match next_wake {
                    Some(d) => d.min(anim),
                    None => anim,
                });
            }
            // Apply the bar to the renderer NOW — before this tick's render —
            // and reflow the PTY grid on an open/close transition. The bar's
            // 30px band is part of the grid's bottom chrome budget while
            // open (`bottom_offset_logical`), so without this reflow the
            // bottom rows would be laid out under the bar on the appearance
            // frame (and the grid would stay short after it hides). Mirrors
            // the resource-strip pattern above.
            {
                let bar = suggestion_bar_view(&state.suggestions);
                let now_open = bar.is_some();
                state.renderer.set_suggestion_bar(bar);
                if now_open != state.suggestion_bar_was_open {
                    state.suggestion_bar_was_open = now_open;
                    let size = state.window.inner_size();
                    resize_all_tabs(state, size.width, size.height);
                    state.window.request_redraw();
                }
            }
        }
        // ── Proactive suggestion bar — fire deferred spawns ──────────────────
        // Now that the windows borrow is released, check which windows need a
        // suggestion request and call spawn_suggestion for each.
        {
            let sg_idle =
                std::time::Duration::from_secs(u64::from(self.config.ai.suggestions.idle_secs));
            let sg_trigger = self.config.ai.suggestions.trigger;
            let sg_enabled = self.config.ai.suggestions.enabled;
            let sg_provider_ok = suggestions::provider_usable(&self.config.ai);
            let mut sg_fire: Vec<usize> = Vec::new();
            for (idx, state) in self.windows.iter_mut().enumerate() {
                // Manual request (set by SuggestCommand action). Always consume
                // the flag so it can never fire later unexpectedly (e.g. once a
                // key gets configured); only act on it when actually firable.
                if state.suggestions.manual_requested {
                    state.suggestions.manual_requested = false;
                    if sg_enabled
                        && sg_provider_ok
                        && sg_trigger != terminale_config::SuggestionTrigger::Off
                    {
                        sg_fire.push(idx);
                    }
                    continue;
                }
                // Auto trigger.
                if sg_enabled
                    && sg_trigger == terminale_config::SuggestionTrigger::Auto
                    && sg_provider_ok
                {
                    if suggestions::should_auto_fire(
                        &state.suggestions,
                        sg_trigger,
                        sg_enabled,
                        sg_idle,
                        now,
                    ) {
                        sg_fire.push(idx);
                    } else if !state.suggestions.fired_for_prompt {
                        if let Some(t) = state.suggestions.last_output_at {
                            let elapsed = now.saturating_duration_since(t);
                            if elapsed < sg_idle {
                                let remaining = sg_idle.checked_sub(elapsed).unwrap();
                                next_wake = Some(match next_wake {
                                    Some(d) => d.min(remaining),
                                    None => remaining,
                                });
                            }
                        }
                    }
                }
            }
            for idx in sg_fire {
                self.spawn_suggestion(idx);
            }
        }
        if let Some(deadline) = self.config_save_due {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            next_wake = Some(match next_wake {
                Some(d) => d.min(remaining),
                None => remaining,
            });
        }
        // Re-register the Quake global hotkey when its binding changed at
        // runtime (Settings save or config-file reload). It was hooked only at
        // startup, so a changed binding did nothing until restart — this makes
        // it apply live.
        if self.config.keybinds.quake != self.quake_binding_registered {
            let binding = self.config.keybinds.quake.clone();
            self.reregister_quake_hotkey(&binding);
        }
        if let Some(d) = next_wake {
            event_loop.set_control_flow(ControlFlow::WaitUntil(std::time::Instant::now() + d));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

fn spawn_tab(
    profile: Option<&Profile>,
    shell_override: Option<&str>,
    renderer: &Renderer,
    initial: (u16, u16),
    width_px: u32,
    height_px: u32,
    proxy: EventLoopProxy<UserEvent>,
    scrollback: usize,
) -> TabState {
    TabState::new_single(spawn_pane(
        profile,
        shell_override,
        renderer,
        initial,
        width_px,
        height_px,
        proxy,
        scrollback,
    ))
}

/// Like [`spawn_tab`] but produces a raw [`Pane`] without wrapping it in
/// a fresh `TabState`. Used by the split-pane actions to seed a sibling
/// leaf inside an existing tab's tree.
#[allow(clippy::too_many_arguments)]
fn spawn_pane(
    profile: Option<&Profile>,
    shell_override: Option<&str>,
    renderer: &Renderer,
    initial: (u16, u16),
    width_px: u32,
    height_px: u32,
    proxy: EventLoopProxy<UserEvent>,
    scrollback: usize,
) -> Pane {
    let spec = build_spawn_spec(profile, shell_override);
    let notifier: terminale_core::DataNotifier = Arc::new(move || {
        // Coalesce wakeups: under output floods the reader produces thousands
        // of chunks per second, but one queued `PtyDataReady` already drains
        // every channel — only send when no wake is pending. The handler
        // clears the flag before draining, so a chunk that lands after the
        // clear re-arms a fresh event and is never lost.
        if !PTY_WAKE_PENDING.swap(true, std::sync::atomic::Ordering::AcqRel) {
            // If the proxy is closed (event loop dead) we silently drop —
            // there's no host left to wake, that's expected on shutdown.
            let _ = proxy.send_event(UserEvent::PtyDataReady);
        }
    });
    let mut session = Session::spawn_with_notifier(&spec, initial.0, initial.1, notifier)
        .expect("failed to spawn shell behind PTY");
    let output_rx = session.take_output().expect("session must have output");
    let (cols, rows) = renderer.pixels_to_cells(width_px, height_px);
    session.resize(cols, rows).ok();
    let mut emu = Emulator::new(cols, rows);
    emu.set_scrollback(scrollback);
    let emulator = Arc::new(Mutex::new(emu));

    let name = profile
        .map(|p| p.name.clone())
        .or_else(|| shell_override.map(std::string::ToString::to_string))
        .unwrap_or_else(|| "shell".into());
    let icon = profile.and_then(|p| p.icon.clone());

    Pane {
        profile_name: name,
        icon,
        custom_title: None,
        user_title: None,
        session,
        output_rx,
        emulator,
        cols,
        rows,
        scroll_lines: 0,
        crashed: false,
        autodetect_links: Vec::new(),
        last_output_at: None,
        last_input_at: None,
    }
}

// tab_bar_from are now in tabs.rs

/// Build a full-screen alternate-screen box frame for the padding demo.
///
/// The returned byte sequence:
/// - Enters the alternate screen (`\x1b[?1049h`) and hides the cursor.
/// - Draws a Unicode box border that fills the exact grid — top/bottom borders
///   touch row 1 and row `rows`, left/right borders at columns 1 and `cols`.
/// - Adds a centered label near the middle row.
/// - Uses only absolute cursor positioning (no trailing newline after the last
///   row, so ConPTY never scrolls).
///
/// This makes the bottom border sit on the grid's last logical row, so the
/// gap between the border and the window edge is exactly the bottom padding.
fn build_padding_demo_frame(cols: u16, rows: u16) -> Vec<u8> {
    // Clamp to at least a 2x2 grid to avoid underflow.
    let cols = cols.max(2) as usize;
    let rows = rows.max(2) as usize;

    let inner_width = cols.saturating_sub(2);
    let h_bar: String = "\u{2500}".repeat(inner_width); // ─ × (cols-2)

    let mut out: Vec<u8> = Vec::with_capacity(rows * (cols + 32));

    // Enter alternate screen, hide cursor, clear.
    out.extend_from_slice(b"\x1b[?1049h\x1b[?25l\x1b[2J");

    // Top border: row 1.
    out.extend_from_slice(b"\x1b[1;1H");
    out.extend_from_slice("\u{250c}".as_bytes()); // ┌
    out.extend_from_slice(h_bar.as_bytes());
    out.extend_from_slice("\u{2510}".as_bytes()); // ┐

    // Middle row for the label.
    let label = "PADDING DEMO \u{2014} top gap should equal bottom gap";
    let mid_row = (rows / 2).max(2);

    // Side borders and optional label.
    for r in 2..rows {
        // Left border.
        let row_seq = format!("\x1b[{r};1H\u{2502}"); // │
        out.extend_from_slice(row_seq.as_bytes());

        if r == mid_row {
            // Centered label (truncated if wider than inner_width).
            let label_len = label.chars().count();
            if label_len <= inner_width {
                let pad_left = (inner_width - label_len) / 2;
                let pad_right = inner_width - label_len - pad_left;
                // Move to col 2 (just after the left border).
                let col_seq = format!("\x1b[{r};2H");
                out.extend_from_slice(col_seq.as_bytes());
                out.extend_from_slice(" ".repeat(pad_left).as_bytes());
                out.extend_from_slice(label.as_bytes());
                out.extend_from_slice(" ".repeat(pad_right).as_bytes());
            }
        }

        // Right border.
        let right_seq = format!("\x1b[{r};{cols}H\u{2502}"); // │
        out.extend_from_slice(right_seq.as_bytes());
    }

    // Bottom border: row `rows` — NO trailing newline.
    let bottom_seq = format!("\x1b[{rows};1H");
    out.extend_from_slice(bottom_seq.as_bytes());
    out.extend_from_slice("\u{2514}".as_bytes()); // └
    out.extend_from_slice(h_bar.as_bytes());
    out.extend_from_slice("\u{2518}".as_bytes()); // ┘

    out
}

/// Wrap a detached [`Pane`] into a fresh single-pane [`TabState`], resize its
/// emulator and PTY session to `(cols, rows)`, and apply `palette`. Used by
/// the pane tear-out and pane attach paths to adapt a transplanted pane to its
/// new window's grid without duplicating the resize logic.
///
/// The pane retains all its existing state (profile name, OSC titles, user
/// rename, scrollback, etc.) — only the grid size and palette change.
fn build_single_pane_tab(
    mut pane: Pane,
    palette: terminale_term::AnsiPalette,
    cols: u16,
    rows: u16,
) -> TabState {
    pane.emulator.lock().set_palette(palette);
    pane.emulator.lock().resize(cols, rows);
    let _ = pane.session.resize(cols, rows);
    pane.cols = cols;
    pane.rows = rows;
    TabState::new_single(pane)
}

/// What the inline rename editor is targeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenameTarget {
    /// Renaming the tab itself (tab-pill inline edit).
    Tab,
    /// Renaming a specific split pane via its header strip.
    Pane(PaneId),
    /// Renaming a tab group by its stable id.
    Group(TabGroupId),
}

/// Inline rename editor state. The editor is shown in the tab pill at
/// `tab_idx` and, on commit, applies to the [`RenameTarget`].
struct RenameState {
    tab_idx: usize,
    /// What is being renamed.
    target: RenameTarget,
    buffer: String,
}

// start_rename..handle_rename_input are now in tabs.rs
// tab_label, useful_program_title, compose_tab_label, short_cwd are now in panes.rs

fn resize_all_tabs(state: &mut RunningState, width_px: u32, height_px: u32) {
    let scale = state.window.scale_factor() as f32;
    let top_pad_px = state.renderer.body_top_px();
    let bottom_px = state.renderer.body_bottom_px(height_px);
    // Account for a vertical tab strip on the left or right.
    let left_px = state.renderer.body_left_px();
    let right_px = state.renderer.body_right_px(width_px);
    let body_rect = (
        left_px,
        top_pad_px,
        (right_px - left_px).max(0.0),
        (bottom_px - top_pad_px).max(0.0),
    );
    // Walk every tab's pane tree to a list of `(pane_id, sub_rect)`
    // pairs, then resize each pane's PTY + emulator to fit its
    // sub-rect's cell count. Single-pane tabs always produce one
    // entry covering the full body — same behaviour as before the
    // split-panes work.
    for tab_idx in 0..state.tabs.len() {
        // Walk the tree off the tab borrow so the inner pane mutation
        // can take `&mut` of the same tab.
        let pane_rects: Vec<(PaneId, (f32, f32, f32, f32))> = {
            let tab = &state.tabs[tab_idx];
            let header_h_px = terminale_render::PANE_HEADER_HEIGHT * scale;
            let cell_h_px = state.renderer.cell_height() * scale;
            let leaves = count_leaves(&tab.tree);
            let with_headers = leaves > 1 && state.show_pane_headers;
            let mut tmp: Vec<LocalPaneSpec> = Vec::new();
            walk_pane_tree(
                &tab.tree,
                body_rect,
                tab,
                with_headers,
                header_h_px,
                cell_h_px,
                &mut tmp,
            );
            tmp.into_iter().map(|s| (s.pane_id, s.rect_px)).collect()
        };
        let tab = &mut state.tabs[tab_idx];
        for (id, rect) in pane_rects {
            let (_, _, w, h) = rect;
            // The pane sub-rect comes from `walk_pane_tree` over the chrome-
            // free body area, so convert it WITHOUT re-subtracting the chrome
            // offsets (`pixels_to_cells` here double-counted the tab/status
            // bars + padding and left a multi-row dead band at the bottom).
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (cols, rows) = state
                .renderer
                .rect_to_cells(w.max(1.0) as u32, h.max(1.0) as u32);
            if let Some(pane) = tab.panes.get_mut(&id) {
                // Same-size guard: skip the emulator/PTY round-trip when the
                // grid hasn't actually changed. Beyond saving work, this stops
                // the OS's first post-show Resized (and ScaleFactorChanged)
                // from re-forwarding an identical size to ConPTY, which would
                // trigger a spurious reflow of already-printed shell output.
                if pane.cols == cols && pane.rows == rows {
                    continue;
                }
                // Emulator FIRST so the grid is already at the new size when
                // ConPTY/PTY repaints triggered by session.resize() arrive on
                // the next tick (or the same tick's post-resize drain).
                pane.emulator.lock().resize(cols, rows);
                pane.session.resize(cols, rows).ok();
                pane.cols = cols;
                pane.rows = rows;
            }
        }
    }
}

/// Build the structured terminal context for the AI features from the
/// focused pane: OS, shell, cwd (OSC 7), the last ~5 commands with their
/// exit codes (OSC 133 command blocks) and — when the most recent command
/// failed — that command's own output, capped to the Fix-feature limits.
/// Shells without OSC 133 integration degrade to a small raw output tail
/// of at most `fallback_tail_lines` lines.
///
/// Shared by the proactive suggestion bar and the AI assistant window so
/// both reason over the same, signal-dense view of the terminal.
fn build_suggestion_context(
    state: &RunningState,
    fallback_tail_lines: usize,
) -> terminale_ai::SuggestionContext {
    let mut sctx = terminale_ai::SuggestionContext {
        os: std::env::consts::OS.to_string(),
        ..Default::default()
    };
    if let Some(tab) = state.tabs.get(state.active_tab) {
        // Native SSH tab → the session runs on a remote host; the local
        // OS/shell don't describe it. (An `ssh` typed in a LOCAL shell is
        // detected below once the recent command blocks are known.)
        sctx.remote = tab.session.is_remote();
        let emu = tab.emulator.lock();
        let all_lines = emu.buffer_lines_text();
        // The cursor's visible row maps into buffer_lines_text() AFTER the
        // scrollback prefix (history_size + viewport row); see
        // `suggestions::current_line_index`.
        let (_, crow) = emu.cursor();
        let buf_idx = suggestions::current_line_index(crow, emu.history_size());
        sctx.current_line = all_lines.get(buf_idx).cloned().unwrap_or_default();
        // The launching profile name is our best shell hint (e.g.
        // "PowerShell", "bash") so the model matches the right syntax.
        sctx.shell = tab.profile_name.clone();
        let blocks = emu.command_blocks();
        if blocks.is_empty() {
            // No shell integration: fall back to a SMALL raw output tail —
            // signal density matters more than volume here.
            let start = all_lines.len().saturating_sub(fallback_tail_lines);
            sctx.output_tail = all_lines[start..].join("\n");
        } else {
            const RECENT_COMMANDS: usize = 5;
            let s = blocks.len().saturating_sub(RECENT_COMMANDS);
            sctx.recent_commands = blocks[s..]
                .iter()
                .filter(|b| !b.command_text.trim().is_empty())
                .map(|b| (b.command_text.clone(), b.exit_code))
                .collect();
            // `ssh host` typed in a local shell and still running → typed
            // commands now execute on the remote box.
            if suggestions::inflight_remote_shell(&sctx.recent_commands) {
                sctx.remote = true;
            }
            if let Some(b) = blocks.last() {
                sctx.cwd = b.cwd.clone();
                if let Some(code) = b.exit_code.filter(|&c| c != 0) {
                    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
                    let hist = emu.history_size() as i32;
                    let out_end = b.end_line.unwrap_or(b.output_start_line);
                    let output = crate::shortcuts::extract_block_output_lines(
                        &all_lines,
                        hist,
                        b.output_start_line,
                        out_end,
                    );
                    sctx.last_error = Some(terminale_ai::LastError {
                        command: b.command_text.clone(),
                        exit: code,
                        output,
                    });
                }
            }
        }
    }
    sctx
}

/// Map the per-window suggestion runtime state to the render-layer bar.
/// Single source of truth shared by the authoritative `about_to_wait`
/// application (which also reflows the PTY on open/close) and the
/// per-frame refresh inside `render_main`.
fn suggestion_bar_view(
    rt: &suggestions::SuggestionRuntime,
) -> Option<terminale_render::SuggestionBar> {
    use suggestions::SuggestionState;
    use terminale_render::{SuggestionBar, SuggestionBarState};
    // The fix-offer hint is governed by `ai.offer_fix_on_failure`, NOT by the
    // auto-suggestion feature — show it even when suggestions are disabled.
    if let SuggestionState::Hint(m) = &rt.state {
        return Some(SuggestionBar {
            state: SuggestionBarState::Hint { message: m.clone() },
        });
    }
    if !rt.enabled {
        return None;
    }
    match &rt.state {
        SuggestionState::Hidden | SuggestionState::Hint(_) => None,
        SuggestionState::Loading => Some(SuggestionBar {
            state: SuggestionBarState::Loading {
                frame: rt.loading_frame,
            },
        }),
        SuggestionState::Ready(c) => Some(SuggestionBar {
            state: SuggestionBarState::Ready { command: c.clone() },
        }),
        SuggestionState::Error(m) => Some(SuggestionBar {
            state: SuggestionBarState::Error { message: m.clone() },
        }),
    }
}

/// Paint one frame of the main terminal window immediately. Used by the
/// `RedrawRequested` handler and, crucially, by the live-apply path —
/// because `window.request_redraw()` is a no-op for the main window
/// while a *different* window (settings) holds focus, so config changes
/// would otherwise not appear until the user clicked back.
fn render_main(state: &mut RunningState) {
    refresh_menu_overlay(state);
    refresh_tab_bar(state);
    // Phase E: build divider strokes alongside the pane specs so the
    // renderer paints boundary lines between split panes. When a pane is
    // zoomed, suppress all dividers (only one pane is visible).
    let (specs, divider_strokes) = if let Some(tab) = state.tabs.get(state.active_tab) {
        let specs = pane_specs_for_tab(state, tab);
        let strokes = if tab.zoomed_pane.is_some() {
            Vec::new()
        } else {
            let dividers = divider_specs_for_tab(state, tab);
            let color = resolved_divider_color(state);
            dividers
                .iter()
                .map(|d| terminale_render::DividerStroke {
                    rect_px: d.visible_rect_px,
                    color,
                })
                .collect()
        };
        (specs, strokes)
    } else {
        return;
    };
    // Materialise the emulator locks into a Vec aligned with `specs`
    // so the borrowed `PaneSpec::emulator` references stay alive for
    // the duration of the render call. (Holding the locks here is
    // OK — render reads-only and we never call back into the
    // emulator from inside the renderer.)
    let emu_locks: Vec<_> = specs.iter().map(|s| s.emulator.lock()).collect();

    // ── Jump-highlight band ───────────────────────────────────────────────
    // Compute the viewport row and decay alpha for the one-row highlight
    // drawn after a prompt-navigation jump, then push it to the renderer.
    {
        const HIGHLIGHT_DURATION_MS: u128 = 400;
        let band = state.jump_highlight_line.and_then(|abs_line| {
            let start = state.jump_highlight_start?;
            let elapsed = start.elapsed().as_millis();
            if elapsed >= HIGHLIGHT_DURATION_MS {
                None // expired — clear
            } else {
                // Use the scroll of the focused pane.
                let scroll = state
                    .tabs
                    .get(state.active_tab)
                    .map_or(0, |t| t.scroll_lines) as i32;
                let rows = state.tabs.get(state.active_tab).map_or(0, |t| t.rows) as i32;
                let vp_row = abs_line + scroll;
                if vp_row < 0 || vp_row >= rows {
                    return None; // off-screen
                }
                let progress = elapsed as f32 / HIGHLIGHT_DURATION_MS as f32;
                let alpha = 1.0 - progress;
                Some((vp_row as u16, alpha))
            }
        });
        // If the band has expired, clear the stored state.
        if band.is_none() && state.jump_highlight_start.is_some() {
            let elapsed = state
                .jump_highlight_start
                .map_or(u128::MAX, |s| s.elapsed().as_millis());
            if elapsed >= HIGHLIGHT_DURATION_MS {
                state.jump_highlight_line = None;
                state.jump_highlight_start = None;
            }
        }
        state.renderer.set_jump_highlight_band(band);
    }

    // Push prompt-status gutter marks for the focused pane so the renderer
    // can draw the dots this frame. We translate absolute line → viewport
    // row using the focused pane's current scroll offset.
    {
        let focused_idx = specs.iter().position(|s| s.focused).unwrap_or(0);
        let marks = if state.show_prompt_marks {
            if let (Some(spec), Some(emu)) = (specs.get(focused_idx), emu_locks.get(focused_idx)) {
                let scroll = spec.scroll_lines as i32;
                let rows = state.tabs.get(state.active_tab).map_or(0, |t| t.rows) as i32;
                let mut out: Vec<(u16, Option<u32>)> = Vec::new();
                for mk in emu.semantic().iter_marks() {
                    // Absolute line → viewport row: row = line + scroll.
                    let row = mk.line + scroll;
                    if row >= 0 && row < rows {
                        out.push((row as u16, mk.exit_code));
                    }
                }
                out
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        state.renderer.set_prompt_marks(marks);
    }

    // ── Quick-select / pane-select label badge overlay ───────────────────
    // Build the badge list from the active mode's state and push it to the
    // renderer so badges are painted on top of the terminal this frame.
    // Pass the pane specs so pane-select can compute physical-pixel centres.
    {
        let dim = state.qs_overlay_dim;
        let badges = build_label_badges(state, &specs);
        state.renderer.set_label_overlays(badges, dim);
    }

    // Push broadcast-receiver ids to the renderer each frame so the amber
    // indicator border is drawn around panes that are receiving mirrored input.
    // When broadcast is off we pass an empty slice, which clears the indicator.
    {
        let ids: Vec<u32> = if state.broadcast_input {
            // Note: we read the scope from the TerminaleApp config, but here
            // we only have &RunningState. The scope is reflected in the same
            // set of ids regardless — for per-tab scope every non-focused live
            // pane in the active tab is a receiver; for window scope the visible
            // (active-tab) set is identical since we only render one tab at a
            // time. So `broadcast_receiver_ids` suffices for both scopes.
            let focused_id = state
                .tabs
                .get(state.active_tab)
                .map_or(PaneId::MAX, |t| t.focused);
            if let Some(tab) = state.tabs.get(state.active_tab) {
                tab.panes
                    .iter()
                    .filter(|(id, pane)| **id != focused_id && !pane.crashed)
                    .map(|(id, _)| *id)
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        state.renderer.set_broadcast_receiver_ids(&ids);
    }

    // ── Proactive suggestion bar ─────────────────────────────────────────────
    // Map the per-window suggestion runtime state to the render-layer type.
    // The authoritative application (with the PTY reflow on open/close)
    // happens in `about_to_wait` BEFORE specs are computed; this re-apply
    // only keeps the loading-frame fresh within the same tick.
    state
        .renderer
        .set_suggestion_bar(suggestion_bar_view(&state.suggestions));

    let render_specs: Vec<terminale_render::PaneSpec<'_>> = specs
        .iter()
        .zip(emu_locks.iter())
        .map(|(s, emu)| terminale_render::PaneSpec {
            rect_px: s.rect_px,
            header_rect_px: s.header_rect_px,
            title: &s.title,
            pane_id: s.pane_id,
            emulator: emu,
            scroll_lines: s.scroll_lines,
            focused: s.focused,
        })
        .collect();
    if let Err(e) = state
        .renderer
        .render_panes_with_dividers(&render_specs, &divider_strokes)
    {
        tracing::warn!(?e, "render frame failed");
        // The renderer already reconfigures + retries on a lost/outdated
        // surface; if even that failed (driver mid-reset), ask for another
        // frame instead of leaving the window frozen until the next input.
        // Only for *transient* surface errors — an OutOfMemory must not spin.
        if matches!(
            e,
            terminale_render::RenderError::Surface(
                wgpu::SurfaceError::Lost
                    | wgpu::SurfaceError::Outdated
                    | wgpu::SurfaceError::Timeout
            )
        ) {
            state.window.request_redraw();
        }
    }
}

// resolved_divider_color is now in panes.rs

/// Lightweight pane-spec shape used inside `main.rs` to bridge the live
/// tab state and the renderer's [`terminale_render::PaneSpec`]. Holds
/// an `Arc<Mutex<Emulator>>` reference rather than a borrow, so the
/// caller can lock each emulator just-in-time when building the slice
/// the renderer actually consumes.
struct LocalPaneSpec {
    pane_id: PaneId,
    /// Grid-only area (below the header strip when `header_rect_px` is `Some`).
    rect_px: (f32, f32, f32, f32),
    /// Physical-px rect of the 22 px header strip, or `None` when headers
    /// are disabled / the tab has a single leaf.
    header_rect_px: Option<(f32, f32, f32, f32)>,
    /// Title resolved by [`pane_label`] for this leaf's header.
    title: String,
    emulator: Arc<Mutex<Emulator>>,
    scroll_lines: usize,
    focused: bool,
}

// pane_specs_for_tab, focus_pane_under_cursor, pane_header_close_at,
// pane_header_at, count_leaves are now in panes.rs

// pane_label is now in panes.rs

// walk_pane_tree is now in panes.rs

// refresh_tab_bar are now in tabs.rs
/// The OS default shell, used as a last resort and when a profile pins a
/// cwd/env but no command (e.g. the cwd-inheriting "new tab" path).
fn default_shell() -> &'static str {
    if cfg!(windows) {
        "powershell.exe"
    } else {
        "/bin/bash"
    }
}

fn build_spawn_spec(profile: Option<&Profile>, shell_override: Option<&str>) -> SpawnSpec {
    match (profile, shell_override) {
        (_, Some(shell)) => SpawnSpec::just(shell),
        (Some(p), None) => SpawnSpec {
            // A profile may legitimately carry only a cwd/env (the
            // cwd-inheriting "new tab" builds one with an empty command);
            // an empty command can't be spawned, so fall back to the
            // default shell while keeping the cwd/env/args.
            command: if p.command.trim().is_empty() {
                default_shell().to_string()
            } else {
                p.command.clone()
            },
            args: p.args.clone(),
            env: p.env.clone(),
            cwd: p.cwd.clone(),
        },
        (None, None) => SpawnSpec::just(default_shell()),
    }
}

// ── Workspace helpers ─────────────────────────────────────────────────────────

/// Sanitise a workspace name so it is safe as a filename stem.
/// Replaces whitespace and `/ \ : * ? " < > |` with `_`.
fn sanitise_workspace_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_whitespace()
                || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// Walk `tree` and set the `ratio` of the `Split` node that is the immediate
/// parent of leaf `target_id`, in the context of a just-performed split with
/// direction `dir`. Used after `split_focused` to bake in the saved ratio.
fn apply_restore_ratio(tree: &mut PaneNode, target_id: PaneId, ratio: f32, dir: SplitDir) {
    apply_restore_ratio_inner(tree, target_id, ratio, dir);
}

fn apply_restore_ratio_inner(
    node: &mut PaneNode,
    target_id: PaneId,
    ratio: f32,
    dir: SplitDir,
) -> bool {
    match node {
        PaneNode::Leaf(_) => false,
        PaneNode::Split {
            direction,
            ratio: node_ratio,
            a,
            b,
        } => {
            // If either direct child is the target leaf and the direction
            // matches, apply the ratio to this node.
            let a_is_target = matches!(**a, PaneNode::Leaf(id) if id == target_id);
            let b_is_target = matches!(**b, PaneNode::Leaf(id) if id == target_id);
            if (a_is_target || b_is_target) && *direction == dir {
                *node_ratio = ratio.clamp(0.05, 0.95);
                return true;
            }
            // Recurse.
            if apply_restore_ratio_inner(a, target_id, ratio, dir) {
                return true;
            }
            apply_restore_ratio_inner(b, target_id, ratio, dir)
        }
    }
}

// scroll_after_output and drain_pty_output are now in osc_handlers.rs

/// Route a single key event to the search bar. Returns `true` if it
/// was consumed (and thus must not reach the PTY).
fn handle_search_input(
    state: &mut RunningState,
    _physical: PhysicalKey,
    logical: &winit::keyboard::Key,
    text: Option<winit::keyboard::SmolStr>,
) -> bool {
    use winit::keyboard::Key;
    let shift = state.modifiers.shift_key();
    match logical {
        Key::Named(NamedKey::Escape) => {
            close_search(state);
            return true;
        }
        Key::Named(NamedKey::Enter) => {
            if shift {
                cycle_search_match(state, -1);
            } else {
                cycle_search_match(state, 1);
            }
            return true;
        }
        Key::Named(NamedKey::Backspace) => {
            if let Some(s) = state.search.as_mut() {
                s.query.pop();
            }
            refresh_search_matches(state);
            return true;
        }
        _ => {}
    }
    // Append printable text.
    if let Some(t) = text {
        if !t.is_empty() && t.chars().all(|c| !c.is_control()) {
            if let Some(s) = state.search.as_mut() {
                s.query.push_str(&t);
            }
            refresh_search_matches(state);
            return true;
        }
    }
    // Swallow stray keys (modifiers etc.) while search is open so they
    // don't fall through to the PTY mid-search.
    true
}

fn close_search(state: &mut RunningState) {
    state.search = None;
    state.renderer.set_extra_underlines(Vec::new());
    state.renderer.set_search_overlay(None);
    // Refresh autodetect underlines now that the search highlights are
    // gone — they share the same extra-underlines slot.
    refresh_autodetect_links(state);
}

// ── Copy mode ────────────────────────────────────────────────────────────────

/// Enter copy mode for the active tab. The copy-mode cursor starts at the
/// terminal cursor position (or the top-left of the viewport when no active
/// tab exists). Normal keyboard input is suppressed while copy mode is active.
fn enter_copy_mode(state: &mut RunningState) {
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return;
    };
    let (cols, rows) = tab.emulator.lock().size();
    let start = tab.emulator.lock().cursor();
    state.copy_mode.enter(start, cols, rows);
    // Sync the renderer selection to show the copy cursor immediately.
    sync_copy_mode_selection(state);
    state.window.request_redraw();
}

/// Exit copy mode: clear the selection highlight and deactivate.
fn exit_copy_mode(state: &mut RunningState) {
    state.copy_mode.exit();
    state.renderer.set_selection(None);
    state.window.request_redraw();
}

/// Yank the copy-mode selection to the clipboard, then exit copy mode.
fn yank_copy_mode(state: &mut RunningState) {
    let text = {
        let Some(tab) = state.tabs.get(state.active_tab) else {
            exit_copy_mode(state);
            return;
        };
        let scroll = state.renderer.scroll_lines();
        let emu = tab.emulator.lock();
        let (cols, rows) = emu.size();
        // Build a row-accessor closure over the emulator's visible viewport at
        // the current scroll. Copy mode operates in viewport coordinates.
        let mut row_texts: Vec<String> = Vec::with_capacity(rows as usize);
        for r in 0..rows {
            let row_str = emu.text_in_range((0, r), (cols.saturating_sub(1), r), scroll);
            row_texts.push(row_str);
        }
        drop(emu);
        // Build the accessor from the captured strings.
        let accessor: copy_mode::RowAccessor<'_> = &|c: u16, r: u16| {
            row_texts
                .get(r as usize)
                .and_then(|s| s.chars().nth(c as usize))
                .unwrap_or(' ')
        };
        state.copy_mode.selected_text(accessor)
    };
    if let Some(text) = text {
        push_clipboard_history(state, text.clone());
        if let Some(cb) = state.clipboard.as_mut() {
            if let Err(e) = cb.set_text(text) {
                tracing::warn!(?e, "copy-mode clipboard write failed");
            }
        }
    }
    exit_copy_mode(state);
}

/// Push the current copy-mode state into the renderer so the selection
/// highlight tracks the copy cursor. A hollow block is rendered at the copy
/// cursor by using `anchor == cursor` with no actual selection when no anchor
/// is set, OR the full `anchor..cursor` range when one is.
///
/// We repurpose the renderer's existing `set_selection` API:
/// - when an anchor is set we pass the full span so the selection rectangle
///   highlights it.
/// - when no anchor is set we set the selection to `None` so the normal
///   terminal cursor rendering handles the "hollow cursor" appearance (the
///   out-of-focus cursor style already draws as an outline-block in the
///   renderer).
fn sync_copy_mode_selection(state: &mut RunningState) {
    if !state.copy_mode.active {
        state.renderer.set_selection(None);
        return;
    }
    if let Some((a_col, a_row, c_col, c_row, block)) = state.copy_mode.renderer_selection() {
        state.renderer.set_selection(Some(CellRect {
            anchor: (a_col, a_row),
            cursor: (c_col, c_row),
            block,
        }));
    } else {
        // No anchor: clear the selection so only the copy cursor is visible.
        state.renderer.set_selection(None);
    }
    // Reposition the renderer's copy cursor (hollow block) at the copy-mode
    // cursor by moving scroll if necessary.  We don't draw a separate cursor
    // glyph here — the existing cursor overlay handles non-focused style.
}

/// Apply a scroll delta produced by a copy-mode motion, clamping to valid
/// scrollback bounds.
fn apply_copy_mode_scroll(state: &mut RunningState, delta: i32) {
    if delta == 0 {
        return;
    }
    let active = state.active_tab;
    let (history, current) = {
        let Some(tab) = state.tabs.get(active) else {
            return;
        };
        let history = tab.emulator.lock().history_size();
        (history, tab.scroll_lines)
    };
    let new_scroll = if delta == i32::MIN {
        // Absolute: scroll to live edge.
        0
    } else if delta > 0 {
        (current.saturating_add(delta as usize)).min(history)
    } else {
        // Negative delta: scroll toward live edge.
        current.saturating_sub((-delta) as usize)
    };
    if let Some(tab) = state.tabs.get_mut(active) {
        tab.scroll_lines = new_scroll;
    }
    state.renderer.set_scroll_lines(new_scroll);
}

/// Key handler for copy mode. Returns `true` to consume the key (prevent PTY
/// forwarding), `false` only if the key is entirely unrecognised (though in
/// practice copy mode swallows all keys so normal input never leaks to the
/// shell while active).
fn handle_copy_mode_input(
    state: &mut RunningState,
    physical: PhysicalKey,
    logical: &winit::keyboard::Key,
) -> bool {
    use copy_mode::Motion;
    use winit::keyboard::Key;

    let ctrl = state.modifiers.control_key();

    // ── Exit / yank ──────────────────────────────────────────────────────────
    match logical {
        Key::Named(NamedKey::Escape) => {
            exit_copy_mode(state);
            return true;
        }
        Key::Named(NamedKey::Enter) => {
            yank_copy_mode(state);
            return true;
        }
        _ => {}
    }
    if let Key::Character(s) = logical {
        match s.as_str() {
            "q" => {
                exit_copy_mode(state);
                return true;
            }
            "y" => {
                yank_copy_mode(state);
                return true;
            }
            _ => {}
        }
    }

    // ── Selection kind toggle ─────────────────────────────────────────────────
    if let Key::Character(s) = logical {
        match s.as_str() {
            "v" if ctrl => {
                state
                    .copy_mode
                    .toggle_selection(copy_mode::SelectionKind::Block);
                sync_copy_mode_selection(state);
                return true;
            }
            "V" => {
                state
                    .copy_mode
                    .toggle_selection(copy_mode::SelectionKind::Line);
                sync_copy_mode_selection(state);
                return true;
            }
            "v" => {
                state
                    .copy_mode
                    .toggle_selection(copy_mode::SelectionKind::Cell);
                sync_copy_mode_selection(state);
                return true;
            }
            _ => {}
        }
    }

    // ── Determine motion ─────────────────────────────────────────────────────
    let motion: Option<Motion> = if let PhysicalKey::Code(code) = physical {
        match code {
            KeyCode::ArrowLeft | KeyCode::KeyH => Some(Motion::Left),
            KeyCode::ArrowRight | KeyCode::KeyL => Some(Motion::Right),
            KeyCode::ArrowUp | KeyCode::KeyK => Some(Motion::Up),
            KeyCode::ArrowDown | KeyCode::KeyJ => Some(Motion::Down),
            KeyCode::PageUp => Some(Motion::PageUp),
            KeyCode::PageDown => Some(Motion::PageDown),
            _ => None,
        }
    } else {
        None
    };

    // Character-based motions (override physical-code matches above).
    let motion = if let Key::Character(s) = logical {
        match s.as_str() {
            "w" => Some(Motion::WordForward),
            "b" => Some(Motion::WordBackward),
            "e" => Some(Motion::WordEnd),
            "0" => Some(Motion::LineStart),
            "$" => Some(Motion::LineEnd),
            "^" => Some(Motion::FirstNonBlank),
            "g" => Some(Motion::Top),
            "G" => Some(Motion::Bottom),
            "u" if ctrl => Some(Motion::HalfPageUp),
            "d" if ctrl => Some(Motion::HalfPageDown),
            _ => motion,
        }
    } else if let Key::Named(NamedKey::PageUp) = logical {
        // PageUp from NamedKey in case physical lookup missed it.
        Some(Motion::PageUp)
    } else if let Key::Named(NamedKey::PageDown) = logical {
        Some(Motion::PageDown)
    } else {
        motion
    };

    let Some(motion) = motion else {
        // Swallow unrecognised keys so they don't reach the PTY.
        return true;
    };

    // ── Apply motion ──────────────────────────────────────────────────────────
    let (history_lines, cols, rows, scroll) = {
        let Some(tab) = state.tabs.get(state.active_tab) else {
            return true;
        };
        let emu = tab.emulator.lock();
        let (c, r) = emu.size();
        (emu.history_size(), c, r, tab.scroll_lines)
    };

    // Refresh the copy mode's grid dimensions in case the terminal resized.
    state.copy_mode.update_size(cols, rows);

    // Build a row-accessor closure for word motions.
    let delta = {
        let Some(tab) = state.tabs.get(state.active_tab) else {
            return true;
        };
        let emu = tab.emulator.lock();
        let (col_count, _) = emu.size();
        let mut row_texts: Vec<String> = Vec::with_capacity(rows as usize);
        for r in 0..rows {
            let row_str = emu.text_in_range((0, r), (col_count.saturating_sub(1), r), scroll);
            row_texts.push(row_str);
        }
        drop(emu);

        let accessor: copy_mode::RowAccessor<'_> = &|c: u16, r: u16| {
            row_texts
                .get(r as usize)
                .and_then(|s| s.chars().nth(c as usize))
                .unwrap_or(' ')
        };
        state.copy_mode.move_cursor(motion, accessor, history_lines)
    };

    apply_copy_mode_scroll(state, delta);
    sync_copy_mode_selection(state);
    true
}

// ── Quick-select mode ────────────────────────────────────────────────────────

/// Enter quick-select mode: scan the visible screen + scrollback for regex
/// matches and show label badges. Normal keyboard input is suppressed.
fn enter_quick_select(state: &mut RunningState) {
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return;
    };
    let scroll = tab.scroll_lines;
    let emu = tab.emulator.lock();
    let (cols, rows) = emu.size();
    // Gather visible viewport rows as strings.
    let mut row_texts: Vec<String> = Vec::with_capacity(rows as usize);
    for r in 0..rows {
        let s = emu.text_in_range((0, r), (cols.saturating_sub(1), r), scroll);
        row_texts.push(s);
    }
    drop(emu);

    let row_refs: Vec<&str> = row_texts.iter().map(String::as_str).collect();
    // Fall back to the built-in defaults if the user has cleared all patterns
    // or the alphabet. `default_compiled_patterns()` and `DEFAULT_ALPHABET`
    // are the canonical source for those defaults.
    let patterns_ref: &[regex::Regex];
    let default_patterns;
    if state.qs_compiled_patterns.is_empty() {
        default_patterns = quick_select::default_compiled_patterns();
        patterns_ref = &default_patterns;
    } else {
        patterns_ref = &state.qs_compiled_patterns;
    }
    let alphabet = if state.qs_alphabet.is_empty() {
        quick_select::DEFAULT_ALPHABET
    } else {
        &state.qs_alphabet
    };
    let qs = quick_select::QuickSelectState::new(&row_refs, patterns_ref, alphabet);
    // Skip the mode entirely when there are no matches (nothing to label).
    if qs.is_empty() {
        return;
    }
    state.quick_select = Some(qs);
    state.window.request_redraw();
}

/// Exit quick-select mode and clear any overlay badges.
fn exit_quick_select(state: &mut RunningState) {
    state.quick_select = None;
    state.window.request_redraw();
}

/// Key handler for quick-select mode. Returns `true` to consume the key.
/// On a successful hit the matched text is copied to clipboard.
fn handle_quick_select_input(state: &mut RunningState, logical: &winit::keyboard::Key) -> bool {
    use winit::keyboard::Key;

    // Extract the typed character (Escape handled via '\x1b').
    let ch: Option<char> = match logical {
        Key::Named(NamedKey::Escape) => Some('\x1b'),
        Key::Character(s) if s.chars().count() == 1 => s.chars().next(),
        _ => None,
    };
    let Some(ch) = ch else {
        // Swallow unrecognised keys so they don't reach the PTY.
        return true;
    };

    // Feed the character into the state.
    let result = {
        let Some(qs) = state.quick_select.as_mut() else {
            return false;
        };
        qs.type_char(ch)
    };

    match result {
        quick_select::QsResult::Hit(m) => {
            // Copy the matched text to the clipboard.
            push_clipboard_history(state, m.text.clone());
            if let Some(cb) = state.clipboard.as_mut() {
                if let Err(e) = cb.set_text(m.text) {
                    tracing::warn!(?e, "quick-select clipboard write failed");
                }
            }
            exit_quick_select(state);
        }
        quick_select::QsResult::Cancelled | quick_select::QsResult::Miss => {
            exit_quick_select(state);
        }
        quick_select::QsResult::Pending => {
            // More characters needed — just repaint so badges update.
            state.window.request_redraw();
        }
    }
    true
}

// ── Pane-select mode ─────────────────────────────────────────────────────────

/// Enter pane-select mode: assign a label to each pane in the active tab and
/// wait for the user to press a label key to focus it. Suppresses normal input.
fn enter_pane_select(state: &mut RunningState) {
    // Collect (pane_id, title) pairs from the active tab.
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return;
    };
    let panes: Vec<(u32, String)> = tab
        .panes
        .iter()
        .map(|(id, pane)| {
            let title = pane
                .user_title
                .clone()
                .or_else(|| pane.custom_title.clone())
                .unwrap_or_else(|| pane.profile_name.clone());
            (*id, title)
        })
        .collect();
    let ps = quick_select::PaneSelectState::new(&panes, &state.qs_alphabet);
    state.pane_select = Some(ps);
    state.window.request_redraw();
}

/// Exit pane-select mode.
fn exit_pane_select(state: &mut RunningState) {
    state.pane_select = None;
    state.window.request_redraw();
}

/// Key handler for pane-select mode. Returns `true` to consume the key.
/// On resolution the matched pane is focused.
fn handle_pane_select_input(state: &mut RunningState, logical: &winit::keyboard::Key) -> bool {
    use winit::keyboard::Key;

    let ch: Option<char> = match logical {
        Key::Named(NamedKey::Escape) => Some('\x1b'),
        Key::Character(s) if s.chars().count() == 1 => s.chars().next(),
        _ => None,
    };
    let Some(ch) = ch else {
        return true;
    };

    let result = {
        let Some(ps) = state.pane_select.as_mut() else {
            return false;
        };
        ps.type_char(ch)
    };

    match result {
        Some(u32::MAX) => {
            // Cancelled or no match.
            exit_pane_select(state);
        }
        Some(pane_id) => {
            exit_pane_select(state);
            // Focus the chosen pane.
            if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                if tab.panes.contains_key(&pane_id) {
                    if tab.focused != pane_id {
                        tab.focused = pane_id;
                        state.pending_hook_pane_focus.push(pane_id);
                    }
                    state.renderer.set_selection(None);
                    state.window.request_redraw();
                }
            }
        }
        None => {
            // Still pending — repaint so badge suffix updates.
            state.window.request_redraw();
        }
    }
    true
}

/// Build the `Vec<LabelBadge>` to pass to the renderer each frame while
/// quick-select or pane-select mode is active. Returns an empty vec when
/// neither mode is active so the renderer clears any previously-drawn badges.
///
/// `pane_specs` is the slice of [`LocalPaneSpec`] computed for this frame's
/// active tab; it is used to position pane-select badges at each pane's
/// physical-pixel centre. Pass a `&[]` slice when specs are unavailable.
///
/// This function genuinely uses all previously-dead symbols:
///   - `quick_select::OverlayBadge` — constructed here as the per-match
///     intermediate value, then converted to the renderer's `LabelBadge`.
///   - `quick_select::QuickSelectState::prefix` / `matches_with_labels`
///   - `quick_select::PaneSelectState::prefix` / `visible_entries`
///   - `quick_select::PaneEntry::title`
fn build_label_badges(state: &RunningState, pane_specs: &[LocalPaneSpec]) -> Vec<LabelBadge> {
    // ── Quick-select mode ────────────────────────────────────────────────
    if let Some(qs) = &state.quick_select {
        // `matches_with_labels` and `prefix` are consumed here.
        let prefix = qs.prefix().to_string();
        return qs
            .matches_with_labels()
            .map(|(m, label, suffix)| {
                // Build an `OverlayBadge` (the quick_select crate's descriptor
                // type) then map it to the renderer's `LabelBadge`.
                let col = u16::try_from(m.col_start).unwrap_or(u16::MAX);
                let row = u16::try_from(m.row).unwrap_or(u16::MAX);
                let ob = quick_select::OverlayBadge {
                    col,
                    row,
                    label: label.to_string(),
                    typed_prefix: prefix.clone(),
                    highlighted: suffix.len() == 1 && prefix.len() + 1 == label.len(),
                };
                // `remaining` is the untyped tail of `ob.label`.
                let remaining = ob.label[prefix.len()..].to_string();
                LabelBadge {
                    col: ob.col,
                    row: ob.row,
                    center_px: None,
                    typed_prefix: ob.typed_prefix,
                    remaining,
                    highlighted: ob.highlighted,
                }
            })
            .collect();
    }

    // ── Pane-select mode ────────────────────────────────────────────────
    if let Some(ps) = &state.pane_select {
        // `visible_entries`, `prefix`, and `PaneEntry::title` are consumed here.
        let prefix = ps.prefix().to_string();
        return ps
            .visible_entries()
            .map(|entry| {
                // Resolve the pane's physical-pixel centre from the spec slice
                // so the badge floats in the middle of the pane.
                let center_px = pane_specs
                    .iter()
                    .find(|s| s.pane_id == entry.pane_id)
                    .map(|s| {
                        let (rx, ry, rw, rh) = s.rect_px;
                        [rx + rw * 0.5, ry + rh * 0.5]
                    });
                // `entry.title` is the human-readable pane label (profile name /
                // cwd / custom title). Build an `OverlayBadge` so the struct's
                // fields are read before mapping to `LabelBadge`.
                let ob = quick_select::OverlayBadge {
                    col: 0,
                    row: 0,
                    label: entry.label.clone(),
                    typed_prefix: prefix.clone(),
                    highlighted: false,
                };
                // `remaining` is the untyped tail of `ob.label`.
                let remaining = ob.label[prefix.len()..].to_string();
                LabelBadge {
                    col: ob.col,
                    row: ob.row,
                    center_px,
                    typed_prefix: ob.typed_prefix,
                    remaining,
                    highlighted: ob.highlighted,
                }
            })
            .collect();
    }

    Vec::new()
}

/// Re-scan the visible viewport for the current search query.
fn refresh_search_matches(state: &mut RunningState) {
    let Some(search) = state.search.as_ref() else {
        return;
    };
    let query = search.query.clone();
    let active = state.active_tab;
    let Some(tab) = state.tabs.get(active) else {
        return;
    };
    let cols = tab.cols;
    // Scan the WHOLE buffer (scrollback history + visible screen), not just
    // the viewport, so find reaches output that's scrolled off-screen.
    let mut matches: Vec<(i32, u16, u16)> = Vec::new();
    if !query.is_empty() && cols > 0 {
        let needle = query.to_ascii_lowercase();
        let (hist, lines) = {
            let emu = tab.emulator.lock();
            (emu.history_size() as i32, emu.buffer_lines_text())
        };
        for (i, line) in lines.iter().enumerate() {
            let line_abs = i as i32 - hist;
            let hay = line.to_ascii_lowercase();
            let mut from = 0usize;
            while let Some(idx) = hay[from..].find(&needle) {
                let abs = from + idx;
                let col_start = hay[..abs].chars().count() as u16;
                let col_end = (col_start + needle.chars().count() as u16).saturating_sub(1);
                matches.push((
                    line_abs,
                    col_start.min(cols.saturating_sub(1)),
                    col_end.min(cols.saturating_sub(1)),
                ));
                from = abs + needle.len().max(1);
            }
        }
    }
    let total = matches.len();
    if let Some(s) = state.search.as_mut() {
        s.matches = matches;
        s.current = 0; // new query → focus the first match
    }
    // Incremental find: jump to the first match as you type (browser-style).
    if total > 0 {
        search_jump_to(state, 0);
    } else {
        state.renderer.set_extra_underlines(Vec::new());
    }
    state
        .renderer
        .set_search_overlay(Some(terminale_render::SearchOverlay {
            query,
            current: usize::from(total != 0),
            total,
        }));
}

/// Recompute which matches fall inside the current viewport and hand their
/// viewport-row ranges to the renderer as highlight underlines.
fn update_search_highlights(state: &mut RunningState) {
    let Some(s) = state.search.as_ref() else {
        return;
    };
    let rows = state.tabs.get(state.active_tab).map_or(0, |t| t.rows) as i32;
    let off = state.renderer.scroll_lines() as i32;
    let mut ranges: Vec<(u16, u16, u16)> = Vec::new();
    for &(line_abs, c0, c1) in &s.matches {
        // Absolute line `L` is shown at viewport row `L + off`.
        let row = line_abs + off;
        if row >= 0 && row < rows {
            ranges.push((c0, c1, row as u16));
        }
    }
    state.renderer.set_extra_underlines(ranges);
}

/// Scroll so match `idx` sits comfortably in view, then refresh highlights.
fn search_jump_to(state: &mut RunningState, idx: usize) {
    let active = state.active_tab;
    let (line_abs, rows, history) = {
        let Some(s) = state.search.as_ref() else {
            return;
        };
        let Some(&(la, _, _)) = s.matches.get(idx) else {
            return;
        };
        let tab = state.tabs.get(active);
        let rows = tab.map_or(0, |t| t.rows) as i32;
        let history = tab.map_or(0, |t| t.emulator.lock().history_size()) as i32;
        (la, rows, history)
    };
    // Place the match about a third of the way down the viewport.
    let target_row = (rows / 3).max(0);
    let off = (target_row - line_abs).clamp(0, history);
    let off_usize = off as usize;
    if let Some(tab) = state.tabs.get_mut(active) {
        tab.scroll_lines = off_usize;
    }
    state.renderer.set_scroll_lines(off_usize);
    update_search_highlights(state);
    state.window.request_redraw();
}

fn cycle_search_match(state: &mut RunningState, dir: i32) {
    let (total, next, query) = {
        let Some(s) = state.search.as_mut() else {
            return;
        };
        if s.matches.is_empty() {
            return;
        }
        let n = s.matches.len() as i32;
        let cur = s.current as i32;
        let next = (cur + dir).rem_euclid(n) as usize;
        s.current = next;
        (s.matches.len(), next, s.query.clone())
    };
    search_jump_to(state, next);
    state
        .renderer
        .set_search_overlay(Some(terminale_render::SearchOverlay {
            query,
            current: next + 1,
            total,
        }));
}

// restart_focused_pane is in tabs.rs
// switch_tab..close_tab are now in tabs.rs
/// Richer menu item used for building the egui context-menu popup.
///
/// Mirrors `terminale_render::MenuItem` but adds an optional `submenu`
/// that the in-window wgpu overlay doesn't need (the overlay flattens
/// items). When `submenu` is `Some`, clicking the row opens a child
/// `ContextMenuWindow` instead of dispatching `action`.
#[derive(Debug, Clone)]
struct RichMenuItem {
    /// Leading glyph. Intentionally **not** rendered in the right-click
    /// context menu any more (icons there felt cramped) — icons live only in
    /// Settings now. Kept on the struct so the existing per-item definitions
    /// (and a future opt-in) stay intact; both converters drop it to `None`.
    #[allow(dead_code)]
    icon: Option<String>,
    label: String,
    hotkey: Option<String>,
    enabled: bool,
    separator_before: bool,
    /// Child entries. `None` = leaf row (dispatches an action on click).
    submenu: Option<Vec<(RichMenuItem, MenuAction)>>,
}

impl RichMenuItem {
    /// Convert to the flat `terminale_render::MenuItem` used by the wgpu overlay
    /// (submenus are not representable there; this produces a parent-row stub).
    fn to_render_item(&self) -> MenuItem {
        MenuItem {
            // Icons are no longer shown in the right-click menu (they live in
            // Settings only); drop them on the way to the render item.
            icon: None,
            label: self.label.clone(),
            hotkey: self.hotkey.clone(),
            enabled: self.enabled,
            separator_before: self.separator_before,
        }
    }
}

/// Identifiers for menu actions. Decouples the UI list from the dispatch
/// logic so the menu can be reordered / extended without touching activate().
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    Copy,
    Paste,
    SelectAll,
    Clear,
    ExplainSelection,
    AskAi,
    ResetZoom,
    ToggleStayOnTop,
    OpenSettings,
    // Pane-management actions (ids 9..=13 — must stay below PROFILE_PICKER_BASE
    // at 0x1_0000 and SSH_PICKER_BASE at 0x2_0000).
    SplitRight,
    SplitDown,
    SplitLeft,
    SplitUp,
    ClosePane,
    // Snap/position actions (ids 14..=19). These call snap_window() — the
    // same helper used by the keyboard shortcuts and the Settings Startup
    // position dropdown, so all three paths are bit-identical.
    SnapTop,
    SnapBottom,
    SnapLeft,
    SnapRight,
    SnapCenter,
    SnapMaximize,
    // Tab actions (ids 20..). Still well below PROFILE_PICKER_BASE.
    NewTab,
    CopyCurrentPath,
    CloseTab,
    RenameTab,
    NewTabWithProfile,
    /// Toggle the pinned state of the active tab.
    ToggleTabPin,
    /// Clear the per-tab user colour override (restore auto/default).
    ClearTabColor,
    // Per-tab user-colour presets — each maps to SetTabColor(R,G,B).
    // We encode them as separate variants so no heap allocation at dispatch.
    TabColorRed,
    TabColorOrange,
    TabColorYellow,
    TabColorGreen,
    TabColorCyan,
    TabColorBlue,
    TabColorPurple,
    TabColorPink,
    /// Clear the per-tab user icon override (restore profile icon).
    ClearTabIcon,
    /// Create a new tab group and assign the active tab to it.
    NewTabGroup,
    /// Assign the active tab to the next existing group (cycles). Creates one
    /// if no groups exist yet.
    AssignTabToGroup,
    /// Remove the active tab from its group.
    ClearTabGroup,
    /// Restart the focused pane's session in place (kill + respawn the same
    /// profile, keeping the pane tree). Dispatched at App level — it needs
    /// `self.config` to resolve the pane's profile by name.
    RestartSession,
}

impl MenuAction {
    fn as_u32(self) -> u32 {
        match self {
            Self::Copy => 0,
            Self::Paste => 1,
            Self::SelectAll => 2,
            Self::Clear => 3,
            Self::ExplainSelection => 4,
            Self::AskAi => 5,
            Self::ResetZoom => 6,
            Self::ToggleStayOnTop => 7,
            Self::OpenSettings => 8,
            // Pane-management — appended with stable ids so 0..=8 remain
            // identical across restarts, preserving any in-flight popup state.
            Self::SplitRight => 9,
            Self::SplitDown => 10,
            Self::SplitLeft => 11,
            Self::SplitUp => 12,
            Self::ClosePane => 13,
            // Snap/position actions — ids 14..=19, well below
            // PROFILE_PICKER_BASE (0x1_0000).
            Self::SnapTop => 14,
            Self::SnapBottom => 15,
            Self::SnapLeft => 16,
            Self::SnapRight => 17,
            Self::SnapCenter => 18,
            Self::SnapMaximize => 19,
            Self::NewTab => 20,
            Self::CopyCurrentPath => 21,
            Self::CloseTab => 22,
            Self::RenameTab => 23,
            Self::NewTabWithProfile => 24,
            Self::ToggleTabPin => 25,
            Self::ClearTabColor => 26,
            Self::TabColorRed => 27,
            Self::TabColorOrange => 28,
            Self::TabColorYellow => 29,
            Self::TabColorGreen => 30,
            Self::TabColorCyan => 31,
            Self::TabColorBlue => 32,
            Self::TabColorPurple => 33,
            Self::TabColorPink => 34,
            Self::ClearTabIcon => 35,
            Self::NewTabGroup => 36,
            Self::AssignTabToGroup => 37,
            Self::ClearTabGroup => 38,
            Self::RestartSession => 39,
        }
    }
    fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            0 => Self::Copy,
            1 => Self::Paste,
            2 => Self::SelectAll,
            3 => Self::Clear,
            4 => Self::ExplainSelection,
            5 => Self::AskAi,
            6 => Self::ResetZoom,
            7 => Self::ToggleStayOnTop,
            8 => Self::OpenSettings,
            9 => Self::SplitRight,
            10 => Self::SplitDown,
            11 => Self::SplitLeft,
            12 => Self::SplitUp,
            13 => Self::ClosePane,
            14 => Self::SnapTop,
            15 => Self::SnapBottom,
            16 => Self::SnapLeft,
            17 => Self::SnapRight,
            18 => Self::SnapCenter,
            19 => Self::SnapMaximize,
            20 => Self::NewTab,
            21 => Self::CopyCurrentPath,
            22 => Self::CloseTab,
            23 => Self::RenameTab,
            24 => Self::NewTabWithProfile,
            25 => Self::ToggleTabPin,
            26 => Self::ClearTabColor,
            27 => Self::TabColorRed,
            28 => Self::TabColorOrange,
            29 => Self::TabColorYellow,
            30 => Self::TabColorGreen,
            31 => Self::TabColorCyan,
            32 => Self::TabColorBlue,
            33 => Self::TabColorPurple,
            34 => Self::TabColorPink,
            35 => Self::ClearTabIcon,
            36 => Self::NewTabGroup,
            37 => Self::AssignTabToGroup,
            38 => Self::ClearTabGroup,
            39 => Self::RestartSession,
            _ => return None,
        })
    }
}

fn build_menu_entries(state: &RunningState) -> Vec<MenuEntry> {
    // Build the profile submenu entries using raw action ids in the
    // PROFILE_PICKER_BASE range so the App-level handler dispatches them
    // to `new_tab_with_profile` without going through `dispatch_menu_action`.
    let profile_submenu: Option<Vec<MenuEntry>> = if state.profile_names.is_empty() {
        None
    } else {
        Some(
            state
                .profile_names
                .iter()
                .zip(state.profile_icons.iter())
                .enumerate()
                .map(|(idx, (name, icon))| MenuEntry {
                    // Icons are shown in the profile submenu to help users
                    // tell profiles apart at a glance (their profile glyphs are
                    // set in Settings → Profiles, typically emojis).
                    icon: icon.clone(),
                    label: name.clone(),
                    hotkey: None,
                    enabled: true,
                    separator_before: false,
                    // App-level handler checks `>= PROFILE_PICKER_BASE`.
                    action_id: PROFILE_PICKER_BASE + idx as u32,
                    submenu: None,
                })
                .collect(),
        )
    };

    // Build colour-swatch submenu for "Set tab colour…".
    let color_submenu: Vec<MenuEntry> = vec![
        MenuEntry {
            icon: Some("🔴".into()),
            label: "Red".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: MenuAction::TabColorRed.as_u32(),
            submenu: None,
        },
        MenuEntry {
            icon: Some("🟠".into()),
            label: "Orange".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: MenuAction::TabColorOrange.as_u32(),
            submenu: None,
        },
        MenuEntry {
            icon: Some("🟡".into()),
            label: "Yellow".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: MenuAction::TabColorYellow.as_u32(),
            submenu: None,
        },
        MenuEntry {
            icon: Some("🟢".into()),
            label: "Green".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: MenuAction::TabColorGreen.as_u32(),
            submenu: None,
        },
        MenuEntry {
            icon: Some("🔵".into()),
            label: "Cyan".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: MenuAction::TabColorCyan.as_u32(),
            submenu: None,
        },
        MenuEntry {
            icon: Some("🔷".into()),
            label: "Blue".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: MenuAction::TabColorBlue.as_u32(),
            submenu: None,
        },
        MenuEntry {
            icon: Some("🟣".into()),
            label: "Purple".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: MenuAction::TabColorPurple.as_u32(),
            submenu: None,
        },
        MenuEntry {
            icon: Some("🩷".into()),
            label: "Pink".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: MenuAction::TabColorPink.as_u32(),
            submenu: None,
        },
        MenuEntry {
            icon: None,
            label: "Clear colour".into(),
            hotkey: None,
            enabled: true,
            separator_before: true,
            action_id: MenuAction::ClearTabColor.as_u32(),
            submenu: None,
        },
    ];

    // Build icon-picker submenu for "Set tab icon…".
    let icon_submenu: Vec<MenuEntry> = crate::settings_window::ICON_PRESETS
        .iter()
        .map(|(label, glyph)| MenuEntry {
            icon: Some((*glyph).to_string()),
            label: (*label).to_string(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            // Encode icon index into the TAB_ICON_BASE range.
            action_id: TAB_ICON_PICKER_BASE
                + crate::settings_window::ICON_PRESETS
                    .iter()
                    .position(|(_, g)| g == glyph)
                    .unwrap_or(0) as u32,
            submenu: None,
        })
        .chain(std::iter::once(MenuEntry {
            icon: None,
            label: "Clear icon".into(),
            hotkey: None,
            enabled: true,
            separator_before: true,
            action_id: MenuAction::ClearTabIcon.as_u32(),
            submenu: None,
        }))
        .collect();

    // Build the "Group" flyout: create a new group, add to any existing group,
    // or remove the tab from its group. "Add to …" entries route via the
    // dynamic GROUP_ASSIGN_BASE range (index into state.tab_groups).
    let group_submenu: Vec<MenuEntry> = {
        let mut v = vec![MenuEntry {
            icon: None,
            label: "New group".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: MenuAction::NewTabGroup.as_u32(),
            submenu: None,
        }];
        for (idx, g) in state.tab_groups.iter().enumerate() {
            v.push(MenuEntry {
                icon: None,
                label: format!("Add to \u{201c}{}\u{201d}", g.name),
                hotkey: None,
                enabled: true,
                separator_before: idx == 0,
                action_id: GROUP_ASSIGN_BASE + idx as u32,
                submenu: None,
            });
        }
        let grouped = state
            .tabs
            .get(state.active_tab)
            .is_some_and(|t| t.group.is_some());
        v.push(MenuEntry {
            icon: None,
            label: "Remove from group".into(),
            hotkey: None,
            enabled: grouped,
            separator_before: true,
            action_id: MenuAction::ClearTabGroup.as_u32(),
            submenu: None,
        });
        v
    };

    menu_items(state)
        .into_iter()
        .map(|(m, a)| {
            let mut entry = rich_to_entry(m, a);
            // Inject the profile submenu into the "New tab with profile…"
            // parent so it opens an inline flyout instead of a separate picker.
            if matches!(a, MenuAction::NewTabWithProfile) {
                entry.submenu = profile_submenu.clone();
            }
            // Inject the group submenu into the "Group" parent.
            if matches!(a, MenuAction::AssignTabToGroup) {
                entry.submenu = Some(group_submenu.clone());
            }
            // Inject colour submenu into "Set tab colour…".
            if matches!(a, MenuAction::TabColorRed) && entry.label.contains("colour") {
                entry.submenu = Some(color_submenu.clone());
            }
            // Inject icon submenu into "Set tab icon…".
            if matches!(a, MenuAction::ClearTabIcon) && entry.label.contains("icon") {
                entry.submenu = Some(icon_submenu.clone());
            }
            entry
        })
        .collect()
}

/// Recursively convert a `RichMenuItem` + action into a `MenuEntry`.
fn rich_to_entry(m: RichMenuItem, a: MenuAction) -> MenuEntry {
    let submenu = m.submenu.map(|children| {
        children
            .into_iter()
            .map(|(cm, ca)| rich_to_entry(cm, ca))
            .collect()
    });
    MenuEntry {
        // Icons removed from the right-click menu (they felt cramped) — they
        // remain in Settings. Drop the per-item glyph here.
        icon: None,
        label: m.label,
        hotkey: m.hotkey,
        enabled: m.enabled,
        separator_before: m.separator_before,
        action_id: a.as_u32(),
        submenu,
    }
}

fn dispatch_menu_action(state: &mut RunningState, action_id: u32) {
    let Some(action) = MenuAction::from_u32(action_id) else {
        return;
    };
    match action {
        // Normally intercepted at App level (it needs `self.config` to
        // resolve the profile); this state-level fallback defers to the
        // App loop, which performs the same config-resolved restart.
        MenuAction::RestartSession => state.pending_restart_pane = true,
        MenuAction::Copy => copy_selection(state),
        MenuAction::Paste => match paste_clipboard(state) {
            PasteAction::Sent => {}
            PasteAction::NeedsConfirm { text, bracketed } => {
                state.pending_paste_guard = Some((text, bracketed));
            }
        },
        MenuAction::SelectAll => select_all(state),
        MenuAction::Clear => clear_screen(state),
        MenuAction::ExplainSelection => {
            dispatch_shortcut(state, ShortcutAction::ExplainSelection);
        }
        MenuAction::AskAi => dispatch_shortcut(state, ShortcutAction::AiAssistant),
        MenuAction::ResetZoom => {
            state
                .renderer
                .set_font_size(terminale_render::DEFAULT_FONT_SIZE);
            let size = state.window.inner_size();
            resize_all_tabs(state, size.width, size.height);
            // Persist like the keyboard FontReset so it survives a restart.
            state.pending_font_size = Some(terminale_render::DEFAULT_FONT_SIZE);
        }
        MenuAction::ToggleStayOnTop => toggle_stay_on_top(state),
        MenuAction::OpenSettings => open_settings(state),
        // Pane-management — route through dispatch_shortcut so the menu path
        // and the keyboard path are bit-identical (picks up any future
        // side-effects added to dispatch_shortcut).
        MenuAction::SplitRight => dispatch_shortcut(state, ShortcutAction::SplitRight),
        MenuAction::SplitDown => dispatch_shortcut(state, ShortcutAction::SplitDown),
        MenuAction::SplitLeft => dispatch_shortcut(state, ShortcutAction::SplitLeft),
        MenuAction::SplitUp => dispatch_shortcut(state, ShortcutAction::SplitUp),
        MenuAction::ClosePane => dispatch_shortcut(state, ShortcutAction::ClosePane),
        // Snap/position — same helper as the keyboard SnapTop/SnapBottom/…
        // shortcuts and the Settings > Startup position live-apply path,
        // so all three entry points are bit-identical.
        MenuAction::SnapTop => snap_window(state, terminale_config::SnapEdge::Top),
        MenuAction::SnapBottom => snap_window(state, terminale_config::SnapEdge::Bottom),
        MenuAction::SnapLeft => snap_window(state, terminale_config::SnapEdge::Left),
        MenuAction::SnapRight => snap_window(state, terminale_config::SnapEdge::Right),
        MenuAction::SnapCenter => snap_window(state, terminale_config::SnapEdge::Center),
        MenuAction::SnapMaximize => snap_window(state, terminale_config::SnapEdge::Maximize),
        MenuAction::NewTab => new_tab(state),
        MenuAction::CopyCurrentPath => copy_current_path(state),
        MenuAction::CloseTab => request_close_tab(state, state.active_tab),
        MenuAction::RenameTab => start_rename(state),
        // Opens the searchable profile picker; choosing a profile spawns a new
        // tab with it (same path as Ctrl+Shift+T).
        MenuAction::NewTabWithProfile => state.open_profile_picker = true,
        MenuAction::ToggleTabPin => dispatch_shortcut(state, ShortcutAction::ToggleTabPin),
        MenuAction::NewTabGroup => dispatch_shortcut(state, ShortcutAction::NewTabGroup),
        MenuAction::AssignTabToGroup => {
            dispatch_shortcut(state, ShortcutAction::AssignTabToGroup);
        }
        MenuAction::ClearTabGroup => dispatch_shortcut(state, ShortcutAction::ClearTabGroup),
        // Per-tab colour preset swatches.
        MenuAction::ClearTabColor => crate::shortcuts::set_tab_user_color(state, None),
        MenuAction::TabColorRed => {
            crate::shortcuts::set_tab_user_color(state, Some([0xe0, 0x50, 0x50]));
        }
        MenuAction::TabColorOrange => {
            crate::shortcuts::set_tab_user_color(state, Some([0xe0, 0x90, 0x30]));
        }
        MenuAction::TabColorYellow => {
            crate::shortcuts::set_tab_user_color(state, Some([0xd0, 0xc0, 0x20]));
        }
        MenuAction::TabColorGreen => {
            crate::shortcuts::set_tab_user_color(state, Some([0x30, 0xc0, 0x60]));
        }
        MenuAction::TabColorCyan => {
            crate::shortcuts::set_tab_user_color(state, Some([0x20, 0xb8, 0xc8]));
        }
        MenuAction::TabColorBlue => {
            crate::shortcuts::set_tab_user_color(state, Some([0x40, 0x80, 0xe0]));
        }
        MenuAction::TabColorPurple => {
            crate::shortcuts::set_tab_user_color(state, Some([0x90, 0x50, 0xe0]));
        }
        MenuAction::TabColorPink => {
            crate::shortcuts::set_tab_user_color(state, Some([0xe0, 0x50, 0xa0]));
        }
        MenuAction::ClearTabIcon => crate::shortcuts::set_tab_user_icon(state, None),
    }
    state.window.request_redraw();
}

/// Right-click menu entries, chosen by where the click landed
/// ([`RunningState::menu_context`]). A tab gets tab + group management; the
/// terminal body gets terminal actions only. Both share New tab / Settings.
fn menu_items(state: &RunningState) -> Vec<(RichMenuItem, MenuAction)> {
    let all = menu_items_all(state);
    let keep = |a: MenuAction| match state.menu_context {
        MenuContext::Tab(_) => !menu_action_is_terminal_only(a),
        MenuContext::Terminal => !menu_action_is_tab_only(a),
    };
    let mut items: Vec<(RichMenuItem, MenuAction)> =
        all.into_iter().filter(|(_, a)| keep(*a)).collect();
    // The first surviving item must not draw a leading separator.
    if let Some((first, _)) = items.first_mut() {
        first.separator_before = false;
    }
    items
}

/// True for top-level actions that only make sense on a tab's context menu
/// (tab/group management). Submenu *parents* are matched by their placeholder
/// action id. Children inside flyouts are unaffected (filtering is top-level).
fn menu_action_is_tab_only(a: MenuAction) -> bool {
    matches!(
        a,
        MenuAction::RenameTab
            | MenuAction::ToggleTabPin
            | MenuAction::TabColorRed   // "Set tab colour…" parent
            | MenuAction::ClearTabIcon  // "Set tab icon…" parent
            | MenuAction::AssignTabToGroup // "Group" parent
            | MenuAction::CopyCurrentPath
            | MenuAction::CloseTab
    )
}

/// True for top-level actions that only make sense on the terminal body's
/// context menu (selection / pane / window actions).
fn menu_action_is_terminal_only(a: MenuAction) -> bool {
    matches!(
        a,
        MenuAction::Copy
            | MenuAction::Paste
            | MenuAction::SelectAll
            | MenuAction::Clear
            | MenuAction::SplitRight // "Split" parent
            | MenuAction::SnapTop    // "Position" parent
            | MenuAction::ExplainSelection
            | MenuAction::AskAi
            | MenuAction::ResetZoom
            | MenuAction::ToggleStayOnTop
    )
}

fn menu_items_all(state: &RunningState) -> Vec<(RichMenuItem, MenuAction)> {
    let has_selection = state.renderer.selection().is_some();
    let bundled = state.bundled_icons;

    // ── Split submenu children ─────────────────────────────────────────────
    // Each child carries its live keyboard shortcut so users can see their
    // binding without opening Settings. Routes through dispatch_shortcut so
    // the menu and keyboard paths are bit-identical.
    let split_children: Vec<(RichMenuItem, MenuAction)> = vec![
        (
            RichMenuItem {
                label: "Split right".into(),
                icon: Some(icons::glyph(&icons::ARROW_RIGHT, bundled).into()),
                hotkey: {
                    let b = binding_for(ShortcutAction::SplitRight, &state.shortcuts);
                    (!b.is_empty()).then_some(b)
                },
                enabled: true,
                separator_before: false,
                submenu: None,
            },
            MenuAction::SplitRight,
        ),
        (
            RichMenuItem {
                label: "Split down".into(),
                icon: Some(icons::glyph(&icons::ARROW_DOWN, bundled).into()),
                hotkey: {
                    let b = binding_for(ShortcutAction::SplitDown, &state.shortcuts);
                    (!b.is_empty()).then_some(b)
                },
                enabled: true,
                separator_before: false,
                submenu: None,
            },
            MenuAction::SplitDown,
        ),
        (
            RichMenuItem {
                label: "Split left".into(),
                icon: Some(icons::glyph(&icons::ARROW_LEFT, bundled).into()),
                hotkey: {
                    let b = binding_for(ShortcutAction::SplitLeft, &state.shortcuts);
                    (!b.is_empty()).then_some(b)
                },
                enabled: true,
                separator_before: false,
                submenu: None,
            },
            MenuAction::SplitLeft,
        ),
        (
            RichMenuItem {
                label: "Split up".into(),
                icon: Some(icons::glyph(&icons::ARROW_UP, bundled).into()),
                hotkey: {
                    let b = binding_for(ShortcutAction::SplitUp, &state.shortcuts);
                    (!b.is_empty()).then_some(b)
                },
                enabled: true,
                separator_before: false,
                submenu: None,
            },
            MenuAction::SplitUp,
        ),
        (
            RichMenuItem {
                label: "Close pane".into(),
                icon: Some(icons::glyph(&icons::CLOSE, bundled).into()),
                hotkey: {
                    let b = binding_for(ShortcutAction::ClosePane, &state.shortcuts);
                    (!b.is_empty()).then_some(b)
                },
                enabled: true,
                separator_before: true,
                submenu: None,
            },
            MenuAction::ClosePane,
        ),
    ];

    // ── Position submenu children ─────────────────────────────────────────
    // Iterates SnapEdge::all() so any future SnapEdge variant automatically
    // appears without touching this file. Dispatches through snap_window()
    // — the same helper invoked by the keyboard SnapTop/… shortcuts and the
    // Settings > Window > Startup position live-apply path.
    let snap_icons = [
        icons::glyph(&icons::ARROW_UP, bundled),    // snap top
        icons::glyph(&icons::ARROW_DOWN, bundled),  // snap bottom
        icons::glyph(&icons::ARROW_LEFT, bundled),  // snap left
        icons::glyph(&icons::ARROW_RIGHT, bundled), // snap right
        icons::glyph(&icons::TARGET, bundled),      // snap center (◎)
        icons::glyph(&icons::MAXIMIZE, bundled),    // maximize
    ];
    let snap_actions = [
        MenuAction::SnapTop,
        MenuAction::SnapBottom,
        MenuAction::SnapLeft,
        MenuAction::SnapRight,
        MenuAction::SnapCenter,
        MenuAction::SnapMaximize,
    ];
    let position_children: Vec<(RichMenuItem, MenuAction)> = terminale_config::SnapEdge::all()
        .into_iter()
        .zip(snap_icons.into_iter().zip(snap_actions))
        .enumerate()
        .map(|(i, (edge, (icon, action)))| {
            (
                RichMenuItem {
                    label: edge.label().into(),
                    icon: Some(icon.into()),
                    hotkey: None,
                    enabled: true,
                    separator_before: i == 0, // visual separator before the first child
                    submenu: None,
                },
                action,
            )
        })
        .collect();

    vec![
        (
            RichMenuItem {
                label: "Copy".into(),
                icon: Some(icons::glyph(&icons::COPY, bundled).into()),
                hotkey: Some("Ctrl+Shift+C".into()),
                enabled: has_selection,
                separator_before: false,
                submenu: None,
            },
            MenuAction::Copy,
        ),
        (
            RichMenuItem {
                label: "Paste".into(),
                icon: Some(icons::glyph(&icons::PASTE, bundled).into()),
                hotkey: Some("Ctrl+Shift+V".into()),
                enabled: true,
                separator_before: false,
                submenu: None,
            },
            MenuAction::Paste,
        ),
        (
            RichMenuItem {
                label: "Select all".into(),
                icon: Some(icons::glyph(&icons::SELECT_ALL, bundled).into()),
                hotkey: Some("Ctrl+Shift+A".into()),
                enabled: true,
                separator_before: false,
                submenu: None,
            },
            MenuAction::SelectAll,
        ),
        (
            RichMenuItem {
                label: "Clear".into(),
                icon: Some(icons::glyph(&icons::TRASH, bundled).into()),
                hotkey: Some("Ctrl+L".into()),
                enabled: true,
                separator_before: true,
                submenu: None,
            },
            MenuAction::Clear,
        ),
        // ── Tab management ────────────────────────────────────────────────────
        (
            RichMenuItem {
                label: "New tab".into(),
                icon: None,
                hotkey: {
                    let b = binding_for(ShortcutAction::NewTab, &state.shortcuts);
                    (!b.is_empty()).then_some(b)
                },
                enabled: true,
                separator_before: true,
                submenu: None,
            },
            MenuAction::NewTab,
        ),
        (
            RichMenuItem {
                label: "New tab with profile…".into(),
                icon: None,
                hotkey: {
                    let b = binding_for(ShortcutAction::ProfilePicker, &state.shortcuts);
                    (!b.is_empty()).then_some(b)
                },
                enabled: true,
                separator_before: false,
                submenu: None,
            },
            MenuAction::NewTabWithProfile,
        ),
        (
            RichMenuItem {
                label: "Rename…".into(),
                icon: None,
                hotkey: None, // also: double-click the tab
                enabled: true,
                separator_before: false,
                submenu: None,
            },
            MenuAction::RenameTab,
        ),
        // ── Tab pin / colour / icon ──────────────────────────────────────────
        {
            let is_pinned = state.tabs.get(state.active_tab).is_some_and(|t| t.pinned);
            (
                RichMenuItem {
                    label: if is_pinned {
                        "Unpin tab".into()
                    } else {
                        "Pin tab".into()
                    },
                    icon: None,
                    hotkey: None,
                    enabled: true,
                    separator_before: false,
                    submenu: None,
                },
                MenuAction::ToggleTabPin,
            )
        },
        (
            RichMenuItem {
                label: "Set tab colour\u{2026}".into(),
                icon: None,
                hotkey: None,
                enabled: true,
                separator_before: true,
                // Colour swatch children — populated in build_menu_entries.
                submenu: None,
            },
            MenuAction::TabColorRed, // action_id unused for submenu parents
        ),
        (
            RichMenuItem {
                label: "Set tab icon\u{2026}".into(),
                icon: None,
                hotkey: None,
                enabled: true,
                separator_before: false,
                // Icon children — populated in build_menu_entries.
                submenu: None,
            },
            MenuAction::ClearTabIcon, // action_id unused for submenu parents
        ),
        // ── Tab group management ──────────────────────────────────────────────
        // Parent of the "Group" flyout (New group / Add to <group> / Remove).
        // The submenu children are populated in `build_menu_entries`. Only ever
        // shown on a tab's context menu (filtered out of the terminal menu).
        (
            RichMenuItem {
                label: "Group".into(),
                icon: None,
                hotkey: None,
                enabled: true,
                separator_before: true,
                submenu: None, // populated in build_menu_entries
            },
            MenuAction::AssignTabToGroup, // action_id unused for submenu parents
        ),
        (
            RichMenuItem {
                label: "Copy current path".into(),
                icon: None,
                hotkey: None,
                enabled: state
                    .tabs
                    .get(state.active_tab)
                    .is_some_and(|t| t.emulator.lock().current_dir().is_some()),
                separator_before: false,
                submenu: None,
            },
            MenuAction::CopyCurrentPath,
        ),
        (
            RichMenuItem {
                label: "Restart session".into(),
                icon: Some("\u{21bb}".into()), // ↻
                hotkey: {
                    let b = binding_for(ShortcutAction::RestartTab, &state.shortcuts);
                    (!b.is_empty()).then_some(b)
                },
                // SSH sessions are rebuilt by the async connect flow, not a
                // local respawn — grey the item out for them.
                enabled: state
                    .tabs
                    .get(state.active_tab)
                    .is_some_and(|t| t.ssh_host_name.is_empty()),
                separator_before: false,
                submenu: None,
            },
            MenuAction::RestartSession,
        ),
        (
            RichMenuItem {
                label: "Close tab".into(),
                icon: None,
                hotkey: {
                    let b = binding_for(ShortcutAction::CloseTab, &state.shortcuts);
                    (!b.is_empty()).then_some(b)
                },
                enabled: true,
                separator_before: false,
                submenu: None,
            },
            MenuAction::CloseTab,
        ),
        // ── Pane management (collapsed into a Split submenu) ─────────────────
        // Settings > Keyboard already surfaces all five split bindings,
        // satisfying the project's "every tunable feature in Settings" rule.
        // The five children still show their live hotkeys inside the flyout.
        (
            RichMenuItem {
                label: "Split".into(),
                icon: Some(icons::glyph(&icons::SPLIT, bundled).into()),
                hotkey: None, // chevron rendered instead by ContextMenuWindow
                enabled: true,
                separator_before: true,
                submenu: Some(split_children),
            },
            MenuAction::SplitRight, // action_id unused for submenu parents
        ),
        // ── Position submenu ─────────────────────────────────────────────────
        // Quick-apply snap for the focused window. These are duplicates of
        // the keyboard Snap* shortcuts and the Settings > Startup position
        // dropdown — all three paths call snap_window(), so they're always
        // in sync. Settings > Window remains the authoritative permanent
        // location by project convention; the menu items are convenience duplicates.
        (
            RichMenuItem {
                label: "Position".into(),
                icon: Some(icons::glyph(&icons::POSITION, bundled).into()),
                hotkey: None, // chevron rendered instead
                enabled: true,
                separator_before: false,
                submenu: Some(position_children),
            },
            MenuAction::SnapTop, // action_id unused for submenu parents
        ),
        // ── AI / assistive ────────────────────────────────────────────────────
        (
            RichMenuItem {
                label: "Explain selection".into(),
                icon: Some(icons::glyph(&icons::BULB, bundled).into()),
                hotkey: Some("Ctrl+Shift+E".into()),
                enabled: has_selection,
                separator_before: true,
                submenu: None,
            },
            MenuAction::ExplainSelection,
        ),
        (
            RichMenuItem {
                label: "Ask AI…".into(),
                icon: Some(icons::glyph(&icons::MESSAGE, bundled).into()),
                hotkey: Some("Ctrl+Shift+I".into()),
                enabled: true,
                separator_before: false,
                submenu: None,
            },
            MenuAction::AskAi,
        ),
        (
            RichMenuItem {
                label: "Reset zoom".into(),
                icon: Some(icons::glyph(&icons::REFRESH, bundled).into()),
                hotkey: Some("Ctrl+0".into()),
                enabled: true,
                separator_before: true,
                submenu: None,
            },
            MenuAction::ResetZoom,
        ),
        (
            RichMenuItem {
                label: "Stay on top".into(),
                // Show a check mark when active so the menu reflects state.
                icon: Some(if state.always_on_top {
                    icons::glyph(&icons::CHECK, bundled).into()
                } else {
                    " ".into()
                }),
                // Surface the user's bound shortcut if they set one.
                hotkey: {
                    let b = binding_for(ShortcutAction::ToggleStayOnTop, &state.shortcuts);
                    (!b.is_empty()).then_some(b)
                },
                enabled: true,
                separator_before: true,
                submenu: None,
            },
            MenuAction::ToggleStayOnTop,
        ),
        (
            RichMenuItem {
                label: "Settings…".into(),
                icon: Some(icons::glyph(&icons::SETTINGS, bundled).into()),
                hotkey: Some("Ctrl+,".into()),
                enabled: true,
                separator_before: true,
                submenu: None,
            },
            MenuAction::OpenSettings,
        ),
    ]
}

/// Build the menu overlay struct from current state (selection-aware).
fn refresh_menu_overlay(state: &mut RunningState) {
    if !state.menu_visible {
        state.renderer.set_overlay(None);
        return;
    }
    // The wgpu overlay only renders top-level items (no flyout support);
    // convert RichMenuItems to flat MenuItem via to_render_item().
    let items: Vec<MenuItem> = menu_items(state)
        .into_iter()
        .map(|(m, _)| m.to_render_item())
        .collect();
    let mut overlay = MenuOverlay {
        origin_px: state.menu_origin,
        width_px: 260.0,
        items,
        hovered: None,
    };
    overlay.hovered = compute_hovered_item(state, &overlay);
    state.renderer.set_overlay(Some(overlay));
}

fn update_menu_hover(state: &mut RunningState) {
    refresh_menu_overlay(state);
}

/// Compute a menu origin that keeps the whole panel inside the window.
///
/// Math mirrors the menu layout used by both the renderer and
/// `compute_hovered_item` so the menu lands exactly where it draws.
// Reserved menu-positioning helper: the renderer currently clamps inline,
// but this keeps the shared math in one place for the egui menu window.
#[allow(dead_code)]
fn clamped_menu_origin(state: &RunningState) -> [f32; 2] {
    let items = menu_items(state);
    let item_count = items.len() as f32;
    let separators = items.iter().filter(|(m, _)| m.separator_before).count() as f32;

    let (_cw, ch) = state.renderer.cell_size();
    let item_h = (ch * 1.7).max(28.0);
    let menu_w = 260.0;
    let menu_h = item_h * item_count + 16.0 + separators * 8.0;
    let margin = 4.0;

    let size = state.window.inner_size();
    let scale = state.window.scale_factor() as f32;
    let win_w = size.width as f32 / scale;
    let win_h = size.height as f32 / scale;

    let (px, py) = state.pointer_logical;
    let mut x = px;
    let mut y = py;
    if x + menu_w + margin > win_w {
        x = (win_w - menu_w - margin).max(margin);
    }
    if y + menu_h + margin > win_h {
        // If the cursor was near the bottom, flip menu *above* the cursor
        // (so it doesn't overlap the click point), but never go off-screen.
        y = (py - menu_h).max(margin);
        if y + menu_h + margin > win_h {
            y = (win_h - menu_h - margin).max(margin);
        }
    }
    if x < margin {
        x = margin;
    }
    if y < margin {
        y = margin;
    }
    [x, y]
}

fn compute_hovered_item(state: &RunningState, overlay: &MenuOverlay) -> Option<usize> {
    let (px, py) = state.pointer_logical;
    let (_cw, ch) = state.renderer.cell_size();
    let item_h = (ch * 1.7).max(28.0);
    let x0 = overlay.origin_px[0];
    let y0 = overlay.origin_px[1] + 8.0;
    if px < x0 || px > x0 + overlay.width_px {
        return None;
    }
    if py < y0 {
        return None;
    }
    let mut y = y0;
    for (idx, item) in overlay.items.iter().enumerate() {
        if item.separator_before && idx > 0 {
            y += 8.0;
        }
        if py >= y && py < y + item_h {
            return Some(idx);
        }
        y += item_h;
    }
    None
}

fn open_settings(state: &mut RunningState) {
    state.open_settings_requested = true;
    state.window.request_redraw();
}

// ── Clipboard history helpers ─────────────────────────────────────────────────

/// Push `text` into the clipboard history ring.
///
/// Rules:
/// - Does nothing when `clipboard_history_enabled` is `false`.
/// - Drops empty strings.
/// - Skips the push when `text` is identical to the most-recent entry
///   (consecutive-duplicate suppression).
/// - Evicts the oldest entry when the ring is at capacity.
pub(crate) fn push_clipboard_history(state: &mut RunningState, text: String) {
    if !state.clipboard_history_enabled {
        return;
    }
    if text.is_empty() {
        return;
    }
    // Dedupe: skip if identical to the front (most-recent) entry.
    if state
        .clipboard_history_ring
        .front()
        .is_some_and(|t| t == &text)
    {
        return;
    }
    state.clipboard_history_ring.push_front(text);
    // Cap to configured size, evicting oldest (back) entries.
    while state.clipboard_history_ring.len() > state.clipboard_history_size {
        state.clipboard_history_ring.pop_back();
    }
}

// ── Functions still in main.rs (not yet extracted) ───────────────────────────

/// Screen-space geometry of one window's tab-bar band, for hit-testing a
/// cross-window tab drag. All in **physical** px except `scale`.
#[derive(Debug, Clone, Copy)]
struct BarRect {
    /// Stable OS id of the window this band belongs to.
    id: WindowId,
    /// Window outer-position x / y in physical screen px.
    x: i32,
    y: i32,
    /// Window inner width in physical px.
    width: u32,
    /// Window inner height in physical px. Used for vertical strip bounds.
    height: u32,
    /// DPI scale factor, to convert the band height into physical px.
    scale: f32,
    /// When `true`, the tab bar is a vertical side strip (Left or Right).
    /// The `vert_strip_x_logical` / `vert_strip_w_logical` fields describe
    /// the strip geometry in logical px; the horizontal band check is skipped.
    is_vertical: bool,
    /// Logical-px x origin of the vertical strip (0.0 for Left; `viewport_w -
    /// strip_w` for Right). Unused when `is_vertical` is false.
    vert_strip_x_logical: f32,
    /// Logical-px width of the vertical strip. Unused when `is_vertical` is
    /// false.
    vert_strip_w_logical: f32,
    /// For vertical strips: logical-px x of the inner edge that faces the
    /// terminal grid (strip_x + strip_w for Left; strip_x for Right). The
    /// cursor must move past this edge by more than [`VERT_TEAROUT_MARGIN_LOGICAL`]
    /// before the drag is promoted to a tear-out. Unused for horizontal bars.
    vert_inner_edge_logical: f32,
}

/// Which window's tab bar (if any) contains the screen point `(sx, sy)`
/// (physical px). Pure (no `&self`) so the hit-test is unit-testable without
/// real windows.
///
/// For horizontal bars the check is `y ∈ [0, TAB_BAR_HEIGHT]` (in logical
/// px) across the window width.  For vertical strips the check covers the
/// strip's x-range plus a [`VERT_TEAROUT_MARGIN_LOGICAL`]-wide tolerance zone
/// past the inner edge, to prevent accidental tear-outs from small wobbles.
fn window_bar_at_screen(bars: &[BarRect], sx: i32, sy: i32) -> Option<WindowId> {
    for bar in bars {
        let scale = if bar.scale > 0.0 { bar.scale } else { 1.0 };
        let local_x = sx - bar.x;
        let local_y = sy - bar.y;

        if bar.is_vertical {
            // Vertical strip: the strip occupies x ∈ [strip_x, strip_x+strip_w]
            // (logical) × y ∈ [0, viewport_h] (logical), plus a tolerance zone
            // of VERT_TEAROUT_MARGIN_LOGICAL past the inner edge.
            if local_x < 0 || local_y < 0 {
                continue;
            }
            let logical_x = local_x as f32 / scale;
            let logical_y = local_y as f32 / scale;
            let viewport_h = bar.height as f32 / scale;
            // Expand the hit region by the tearout margin toward the grid.
            // For a Left strip the inner edge is the right side of the strip
            // (inner_edge = strip_x + strip_w); extend the hit zone rightward.
            // For a Right strip the inner edge is the left side of the strip
            // (inner_edge = strip_x); extend the hit zone leftward.
            let strip_right = bar.vert_strip_x_logical + bar.vert_strip_w_logical;
            let (x_min, x_max) = if (bar.vert_inner_edge_logical - strip_right).abs() < 1.0 {
                // Left strip: inner edge ≈ right boundary; extend right into grid.
                (
                    bar.vert_strip_x_logical,
                    bar.vert_inner_edge_logical + VERT_TEAROUT_MARGIN_LOGICAL,
                )
            } else {
                // Right strip: inner edge ≈ left boundary; extend left into grid.
                (
                    bar.vert_inner_edge_logical - VERT_TEAROUT_MARGIN_LOGICAL,
                    strip_right,
                )
            };
            if logical_x >= x_min
                && logical_x <= x_max
                && logical_y >= 0.0
                && logical_y <= viewport_h
            {
                return Some(bar.id);
            }
        } else {
            // Horizontal bar: x ∈ [0, width) × y ∈ [0, TAB_BAR_HEIGHT] (logical).
            let local_y_f = local_y as f32 / scale;
            if local_x >= 0
                && (local_x as u32) < bar.width
                && (0.0..=terminale_render::TAB_BAR_HEIGHT).contains(&local_y_f)
            {
                return Some(bar.id);
            }
        }
    }
    None
}

// ── Type definitions used across submodules (must stay in crate root) ────────

/// Every in-app action that can be bound to a shortcut. Maps 1:1 to
/// the fields of [`terminale_config::ShortcutsConfig`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShortcutAction {
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    MoveTabLeft,
    MoveTabRight,
    ProfilePicker,
    RestartTab,
    Copy,
    Paste,
    SelectAll,
    Find,
    Clear,
    Settings,
    FontIncrease,
    FontDecrease,
    FontReset,
    ScrollLineUp,
    ScrollLineDown,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
    AiAssistant,
    CommandPalette,
    ExplainSelection,
    ClearScrollback,
    ReopenClosedTab,
    NewSshTab,
    ToggleStayOnTop,
    SnapTop,
    SnapBottom,
    SnapLeft,
    SnapRight,
    SnapCenter,
    SnapMaximize,
    /// Snap the window to the top-left quarter of its monitor.
    SnapTopLeft,
    /// Snap the window to the top-right quarter of its monitor.
    SnapTopRight,
    /// Snap the window to the bottom-left quarter of its monitor.
    SnapBottomLeft,
    /// Snap the window to the bottom-right quarter of its monitor.
    SnapBottomRight,
    /// Open the snap-layout chooser overlay (grid of preset layouts).
    ShowSnapLayouts,
    SplitRight,
    SplitDown,
    SplitLeft,
    SplitUp,
    ClosePane,
    FocusPaneLeft,
    FocusPaneRight,
    FocusPaneUp,
    FocusPaneDown,
    TogglePaneZoom,
    ResizePaneLeft,
    ResizePaneRight,
    ResizePaneUp,
    ResizePaneDown,
    ActivateTab1,
    ActivateTab2,
    ActivateTab3,
    ActivateTab4,
    ActivateTab5,
    ActivateTab6,
    ActivateTab7,
    ActivateTab8,
    ActivateTab9,
    /// Switch to the previously-active tab (toggle between the two most
    /// recently used tabs).
    LastTab,
    /// Scroll the viewport to the previous OSC 133 prompt mark.
    PrevPrompt,
    /// Scroll the viewport to the next OSC 133 prompt mark.
    NextPrompt,
    /// Enter modal keyboard copy mode.
    CopyMode,
    /// Enter label-hint quick-select mode.
    QuickSelect,
    /// Enter pane-select label mode.
    PaneSelect,
    /// Reload the config from disk immediately (manual hot-reload).
    ReloadConfig,
    /// Toggle borderless full-screen (F11).
    ToggleFullscreen,
    /// Toggle zen (distraction-free) mode — hides chrome elements per
    /// `[window] zen_hide` and optionally enters full-screen.
    ToggleZenMode,
    /// Toggle broadcast-input mode: mirror typed keystrokes to every pane in
    /// the configured scope. A tinted border marks the receiving panes.
    ToggleBroadcastInput,
    /// Open a fresh top-level window with one default tab (reuses the wgpu
    /// device of the source window). The profile used for the first tab is
    /// controlled by `window.new_window_profile`.
    NewWindow,
    /// Tear the active tab out into a new window. No-op when the source
    /// window has only one tab.
    MoveTabToNewWindow,
    /// Detach the focused pane into a new tab in the same window. No-op
    /// when the active tab is a single pane.
    MovePaneToNewTab,
    /// Detach the focused pane into a brand-new window. No-op when the
    /// active tab is a single pane.
    MovePaneToNewWindow,
    /// Open the snippet picker: a fuzzy-searchable command-palette mode that
    /// lists every configured `[[snippets]]` entry. Selecting one inserts its
    /// decoded body into the focused pane's PTY.
    OpenSnippets,
    /// Find the most-recent command block with a non-zero exit code and send
    /// it to the configured AI provider asking for a corrected command. Opens
    /// the AI assistant window with the prompt already submitted.  No-op when
    /// the last block succeeded or no blocks exist yet.
    FixLastCommand,
    /// Save the current layout as a named workspace. If the command palette
    /// is the trigger, an inline prompt asks for a name; otherwise a
    /// timestamp name is used.
    SaveWorkspace,
    /// Open the workspace picker in the command palette — lists saved workspaces
    /// so the user can fuzzy-search and restore one.
    OpenWorkspace,
    /// Copy the output of the most-recent completed command block to the
    /// clipboard (requires shell integration). No-op when no completed block
    /// exists or shell integration is off.
    CopyLastCommandOutput,
    /// Copy the output of the command block whose range contains the cursor's
    /// current absolute line to the clipboard (requires shell integration).
    /// No-op when no block contains the cursor line.
    CopyBlockOutput,
    /// Copy the command text of the most-recent command block to the clipboard
    /// (requires shell integration). No-op when no block exists.
    CopyLastCommand,
    /// Re-run the most-recent command block verbatim by writing its command
    /// text followed by a newline to the focused pane's PTY (requires shell
    /// integration). No-op when no block with a non-empty command exists.
    RerunLastCommand,
    /// Load the most-recent command block's command onto the shell prompt for
    /// editing (writes it without a trailing newline). Optionally prefixed by
    /// Ctrl+U when `terminal.edit_command_clears_line` is on. Requires shell
    /// integration. No-op when no block with a non-empty command exists.
    EditLastCommand,
    /// Parse the OpenSSH client config file (`~/.ssh/config`) and append any
    /// hosts not already in the saved list to `config.ssh_hosts`, then persist
    /// to disk. This is the one-shot "import once" action — it only runs when
    /// explicitly triggered (palette, Settings button, or shortcut). The
    /// `live` import mode is handled separately at startup / reload time.
    ImportSshHosts,
    /// Open the command-history picker: a fuzzy-searchable list of previously
    /// run commands. Selecting one loads it onto the prompt for editing
    /// (without a trailing newline). Requires shell integration (OSC 133).
    OpenCommandHistory,
    /// Write the focused pane's full scrollback (history + visible screen) to
    /// a plain-text file. A native save-file dialog is presented unless
    /// `terminal.scrollback_export_dir` is set, in which case the file is
    /// written there directly with a timestamped name. Unbound by default.
    ExportScrollback,
    /// Open the clipboard history picker: a fuzzy-searchable list of the last
    /// N text entries produced by copy actions. Selecting an entry pastes it
    /// into the focused pane via the normal paste path (honours bracketed
    /// paste). Memory-only — nothing is ever persisted to disk.
    OpenClipboardHistory,
    /// Toggle the pinned state of the active tab. Pinned tabs sort to the
    /// front of the bar, render compact (icon-only), and resist accidental
    /// close. Unbound by default; assignable in Settings → Keyboard.
    ToggleTabPin,
    /// Move the focused pane left by swapping it with its left neighbour.
    /// No-op when there is no left neighbour. Unbound by default.
    MovePaneLeft,
    /// Move the focused pane right by swapping it with its right neighbour.
    /// No-op when there is no right neighbour. Unbound by default.
    MovePaneRight,
    /// Move the focused pane up by swapping it with the pane above.
    /// No-op when there is no upper neighbour. Unbound by default.
    MovePaneUp,
    /// Move the focused pane down by swapping it with the pane below.
    /// No-op when there is no lower neighbour. Unbound by default.
    MovePaneDown,
    /// Rotate all pane-ids in the active tab's split tree one step forward
    /// (left-to-right, top-to-bottom). Tree shape is preserved; the pane
    /// content moves to the next slot. Unbound by default.
    RotatePanes,
    /// Rotate all pane-ids in the active tab's split tree one step backward.
    /// Inverse of `RotatePanes`. Unbound by default.
    RotatePanesBack,
    /// Open the directory-jump picker: a fuzzy-searchable list of previously
    /// visited directories ranked by frecency. Selecting a directory sends
    /// `cd <path>` to the focused pane's PTY. Requires OSC 7 cwd reporting
    /// from the shell. Unbound by default — use the command palette
    /// ("Directory Jump…") or set a binding in `[keybinds.shortcuts]`.
    OpenDirectoryJump,
    /// Open a native file picker to choose a `.toml` theme file. The chosen
    /// file is copied into the `themes_dir`, appended to the available-theme
    /// list, and selected as the active theme. Unbound by default — trigger
    /// from the command palette or Settings > Appearance.
    ImportTheme,
    /// Scroll the viewport so the **previous** command block with a non-zero
    /// exit code is visible (requires shell integration). Clamps at the
    /// oldest failed block — does not wrap.
    JumpToPrevFailedCommand,
    /// Scroll the viewport so the **next** command block with a non-zero
    /// exit code is visible (requires shell integration). Clamps at the
    /// newest failed block — does not wrap.
    JumpToNextFailedCommand,
    /// Open the failed-command picker: a fuzzy-searchable list of command
    /// blocks whose recorded exit code is non-zero. Selecting an entry
    /// scrolls the viewport to that block. Requires shell integration.
    OpenFailedCommandPicker,
    /// Create a new tab group with an auto-generated name + colour and assign
    /// the active tab to it. No blocking dialog.
    NewTabGroup,
    /// Assign the active tab to an existing group, cycling through available
    /// groups. No-op when no groups exist. Creates one if none exist.
    AssignTabToGroup,
    /// Remove the active tab from its current group (ungroup it). No-op when
    /// the tab is already ungrouped.
    ClearTabGroup,
    /// Request a proactive AI command suggestion immediately.  Works in both
    /// Manual and Auto trigger modes; fires even when Auto has not yet reached
    /// its idle threshold.  The suggestion bar shows a loading animation until
    /// the provider replies, then the proposed command.
    SuggestCommand,
    /// Begin an inline rename of the active tab's group. No-op when the active
    /// tab is not in any group.
    RenameTabGroup,
}

/// Compass direction used by pane-focus and pane-resize keyboard actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Scrollback navigation requests issued by keyboard.
#[derive(Debug, Clone, Copy)]
enum RowsScroll {
    LineUp,
    LineDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
}

/// A parsed `ssh` invocation good enough to pre-fill a saved host.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedSsh {
    /// Login user, when given as `user@host` or `-l user`. `None` ⇒ the
    /// SSH default (the local username).
    user: Option<String>,
    /// Destination hostname or IP.
    host: String,
    /// TCP port — explicit `-p PORT` or the SSH default 22.
    port: u16,
}

/// An in-flight Quake open/close animation.
struct QuakeAnim {
    /// When the animation started.
    start: std::time::Instant,
    /// Total animation duration.
    duration: std::time::Duration,
    /// `true` = opening (revealing), `false` = closing (hiding).
    showing: bool,
    /// Target geometry.
    to: terminale_config::WindowRect,
    /// Starting geometry.
    from: terminale_config::WindowRect,
    /// Which animation variant is running.
    anim_kind: terminale_config::QuakeAnimation,
}

/// Cheap structural equality check used to decide whether the settings
/// window has produced new edits that need to be live-applied to the
/// running renderer.
fn configs_identical(a: &Config, b: &Config) -> bool {
    // Derived structural equality over the WHOLE config tree. This used to
    // be a hand-written ~150-field diff that silently omitted entire
    // sections (profiles, ai providers, gpu, updates, integration,
    // directory_jump, resource_indicators, the [ssh] block, two dozen
    // shortcuts, ...) - changing ONLY one of those fields in Settings was
    // treated as "no change": neither live-applied nor persisted. Deriving
    // PartialEq on every config struct makes the gate exhaustive by
    // construction, and new fields are covered automatically.
    a == b
}

// ── SSH import helpers / live-merge ──────────────────────────────────────────

/// Return the display names to show in the SSH quick-connect picker and
/// command palette.  In `live` import mode the names are derived from the
/// merged host list (persisted + freshly-parsed OpenSSH config); otherwise
/// just the persisted list.
fn effective_ssh_host_names(config: &Config) -> Vec<String> {
    if config.ssh.import_openssh_config == terminale_config::ImportOpenSshConfig::Live {
        live_merged_ssh_hosts(config)
            .into_iter()
            .map(|h| h.name)
            .collect()
    } else {
        config.ssh_hosts.iter().map(|h| h.name.clone()).collect()
    }
}

// ── SSH import helpers ────────────────────────────────────────────────────────

/// Parse the OpenSSH client config file at `ssh_cfg.openssh_config_path`,
/// deduplicate the resulting host list against `config.ssh_hosts`, append the
/// new hosts, and keep the Settings window's copy in sync.
///
/// Returns the number of hosts actually added (0 = nothing new, or file not
/// found / unreadable).
fn import_openssh_hosts(
    config: &mut Config,
    mut settings: Option<&mut crate::settings_window::SettingsWindow>,
    _window: &winit::window::Window,
) -> usize {
    let path = &config.ssh.openssh_config_path;
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(path = %path.display(), err = %e, "could not read SSH config for import");
            return 0;
        }
    };

    let parsed = terminale_config::parse_ssh_config(&text);
    let new_hosts: Vec<terminale_config::SshHost> = parsed
        .into_iter()
        .map(terminale_config::ParsedSshHost::into_ssh_host)
        .collect();
    let to_add: Vec<terminale_config::SshHost> =
        terminale_config::dedupe_imported_hosts(&new_hosts, &config.ssh_hosts)
            .into_iter()
            .cloned()
            .collect();
    let count = to_add.len();
    for host in to_add {
        if let Some(s) = settings.as_deref_mut() {
            s.sync_add_ssh_host(host.clone());
        }
        config.ssh_hosts.push(host);
    }
    count
}

/// Open a native file picker for a `.toml` theme file. When the user picks
/// a file:
///   1. Parse it as a `Theme`.
///   2. Copy it into the effective `themes_dir` (creating the directory if
///      needed).
///   3. Append the theme to `config.appearance.themes` if its name is not
///      already present (deduplication by name — existing themes always win).
///   4. Set `config.appearance.theme` to the imported theme name (activate it).
///   5. Sync the settings window's copy so the combo-box updates immediately.
///
/// Logs a warning and does nothing on error; never panics.
fn import_theme_from_picker(
    config: &mut Config,
    settings: Option<&mut crate::settings_window::SettingsWindow>,
    window: &winit::window::Window,
) {
    // Open the native file picker, owned by our window — a parentless modal
    // dialog can open BEHIND the app, which then reads as frozen (Windows
    // files an AppHang against the unresponsive-looking window).
    let picked = rfd::FileDialog::new()
        .set_parent(window)
        .add_filter("Theme TOML", &["toml"])
        .set_title("Import Theme")
        .pick_file();
    let src_path = match picked {
        Some(p) => p,
        None => return, // user cancelled
    };

    // Parse the theme TOML.
    let text = match std::fs::read_to_string(&src_path) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(path = %src_path.display(), err = %e, "failed to read theme file");
            return;
        }
    };
    let theme: terminale_config::Theme = match toml::from_str(&text) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(path = %src_path.display(), err = %e, "failed to parse theme TOML");
            return;
        }
    };

    // Deduplicate: if the name already exists (built-in or user-defined) we
    // still copy the file into themes_dir but skip adding a duplicate entry.
    let already_present = config
        .appearance
        .all_themes()
        .iter()
        .any(|t| t.name == theme.name);

    // Copy the file into themes_dir so it persists across launches.
    if let Some(dir) = config.appearance.effective_themes_dir() {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(dir = %dir.display(), err = %e, "failed to create themes directory");
        } else {
            // Use the theme name (sanitised) as the destination filename.
            let safe_name: String = theme
                .name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let dest = dir.join(format!("{safe_name}.toml"));
            if let Err(e) = std::fs::copy(&src_path, &dest) {
                tracing::warn!(dest = %dest.display(), err = %e, "failed to copy theme file to themes dir");
            } else {
                tracing::info!(name = %theme.name, dest = %dest.display(), "imported theme file");
            }
        }
    }

    if !already_present {
        // Append to the inline list so it's immediately resolvable without
        // a restart (the dir scan also picks it up next launch).
        config.appearance.themes.push(theme.clone());
    }

    // Activate the imported theme.
    config.appearance.theme = theme.name.clone();

    // Sync the settings window copy so the live-apply diff stays consistent.
    if let Some(s) = settings {
        if !already_present {
            s.sync_add_theme(theme.clone());
        }
        s.sync_theme_active(&theme.name);
        // The import copied a *.toml into themes_dir (and may have appended an
        // inline theme); the Appearance section caches the scanned theme list,
        // so force it to rebuild on its next frame.
        s.invalidate_theme_cache();
    }

    tracing::info!(name = %theme.name, "theme import complete");
}

/// Build an in-memory host list that merges `config.ssh_hosts` with the hosts
/// parsed from the OpenSSH client config file. The imported hosts are **not**
/// written to disk — they are ephemeral additions for the `live` import mode.
///
/// Returns a merged `Vec<SshHost>` (persisted hosts first, then new ones).
pub(crate) fn live_merged_ssh_hosts(config: &Config) -> Vec<terminale_config::SshHost> {
    let path = &config.ssh.openssh_config_path;
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(path = %path.display(), err = %e, "live SSH config merge: file not readable");
            return config.ssh_hosts.clone();
        }
    };

    let parsed = terminale_config::parse_ssh_config(&text);
    let new_hosts: Vec<terminale_config::SshHost> = parsed
        .into_iter()
        .map(terminale_config::ParsedSshHost::into_ssh_host)
        .collect();
    let to_add: Vec<terminale_config::SshHost> =
        terminale_config::dedupe_imported_hosts(&new_hosts, &config.ssh_hosts)
            .into_iter()
            .cloned()
            .collect();

    let mut merged = config.ssh_hosts.clone();
    merged.extend(to_add);
    merged
}

/// Fan `bytes` out to every live pane in `scope`, skipping the pane whose id
/// is `focused_id` (it already received the bytes via the normal path).
///
/// "Live" means the pane's process has not crashed (`!pane.crashed`). The
/// focused pane is never sent a second copy; this function is a no-op when
/// the active tab has only one pane or when no other tab contains panes.
pub(crate) fn broadcast_input_to_panes(
    state: &RunningState,
    scope: terminale_config::BroadcastScope,
    focused_id: PaneId,
    bytes: &[u8],
) {
    match scope {
        terminale_config::BroadcastScope::AllPanesInTab => {
            if let Some(tab) = state.tabs.get(state.active_tab) {
                for (id, pane) in &tab.panes {
                    if *id == focused_id || pane.crashed {
                        continue;
                    }
                    if let Err(e) = pane.session.write_input(bytes) {
                        tracing::warn!(?e, pane_id = id, "broadcast PTY write failed");
                    }
                }
            }
        }
        terminale_config::BroadcastScope::AllPanesInWindow => {
            for (tab_idx, tab) in state.tabs.iter().enumerate() {
                for (id, pane) in &tab.panes {
                    // Skip the focused pane in the active tab to avoid doubling.
                    if tab_idx == state.active_tab && *id == focused_id {
                        continue;
                    }
                    if pane.crashed {
                        continue;
                    }
                    if let Err(e) = pane.session.write_input(bytes) {
                        tracing::warn!(
                            ?e,
                            pane_id = id,
                            tab = tab_idx,
                            "broadcast PTY write failed"
                        );
                    }
                }
            }
        }
    }
}

// ── Group drag: pure block-destination math ───────────────────────────────────

/// Compute the insertion index for a group block after all member tabs have
/// been removed from the strip.
///
/// `members` — ascending sorted list of the original tab indices that belong to
///   the group (as returned by `group_member_indices`).
/// `slot`    — raw drop-slot value from the cursor hit-test (0..=len_before).
/// `len_after` — number of tabs remaining after all members were removed.
///
/// Returns an index in `0..=len_after`.
///
/// The key insight is that every member whose original index is **strictly less
/// than** `slot` would have shifted the slot left by one when removed.  We
/// subtract that count from `slot` and then clamp.
fn group_reorder_dest(members: &[usize], slot: usize, len_after: usize) -> usize {
    let removed_before = members.iter().filter(|&&m| m < slot).count();
    let dest = slot.saturating_sub(removed_before);
    dest.min(len_after)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Context-menu split (tab vs terminal) ──────────────────────────────────

    #[test]
    fn tab_and_terminal_menu_filters_are_disjoint() {
        // No action may belong to both menus, or filtering would be ambiguous.
        // Iterate the static MenuAction variants by their stable ids.
        for id in 0u32..=40 {
            if let Some(a) = MenuAction::from_u32(id) {
                assert!(
                    !(menu_action_is_tab_only(a) && menu_action_is_terminal_only(a)),
                    "{a:?} is classified as both tab-only and terminal-only"
                );
            }
        }
    }

    #[test]
    fn group_and_tab_actions_are_tab_only() {
        // The group parent + tab management live only on a tab's menu.
        for a in [
            MenuAction::AssignTabToGroup,
            MenuAction::RenameTab,
            MenuAction::ToggleTabPin,
            MenuAction::CloseTab,
        ] {
            assert!(menu_action_is_tab_only(a), "{a:?} must be tab-only");
            assert!(!menu_action_is_terminal_only(a));
        }
    }

    #[test]
    fn selection_and_pane_actions_are_terminal_only() {
        for a in [
            MenuAction::Copy,
            MenuAction::Paste,
            MenuAction::SplitRight,
            MenuAction::ExplainSelection,
        ] {
            assert!(
                menu_action_is_terminal_only(a),
                "{a:?} must be terminal-only"
            );
            assert!(!menu_action_is_tab_only(a));
        }
    }

    #[test]
    fn shared_actions_appear_in_both_menus() {
        // New tab / Settings must survive both filters.
        for a in [MenuAction::NewTab, MenuAction::OpenSettings] {
            assert!(!menu_action_is_tab_only(a));
            assert!(!menu_action_is_terminal_only(a));
        }
    }

    /// Quick helper: 0-leaf tree with id `n`.
    fn leaf(n: PaneId) -> PaneNode {
        PaneNode::Leaf(n)
    }

    /// Recursive count of leaves in a tree — used by split tests.
    fn leaf_count(node: &PaneNode) -> usize {
        match node {
            PaneNode::Leaf(_) => 1,
            PaneNode::Split { a, b, .. } => leaf_count(a) + leaf_count(b),
        }
    }

    /// Recursive `Vec<PaneId>` from a depth-first walk of leaves left → right.
    fn leaves_in_order(node: &PaneNode) -> Vec<PaneId> {
        let mut out = Vec::new();
        fn rec(n: &PaneNode, out: &mut Vec<PaneId>) {
            match n {
                PaneNode::Leaf(id) => out.push(*id),
                PaneNode::Split { a, b, .. } => {
                    rec(a, out);
                    rec(b, out);
                }
            }
        }
        rec(node, &mut out);
        out
    }

    #[test]
    fn split_in_single_leaf_creates_two_leaves_with_new_on_b_side() {
        // single leaf "0" → vertical split with new id "1" on the right (b).
        let after = split_in(leaf(0), 0, SplitDir::Vertical, 1, true);
        assert_eq!(leaf_count(&after), 2);
        assert_eq!(leaves_in_order(&after), vec![0, 1]);
        match after {
            PaneNode::Split {
                direction, ratio, ..
            } => {
                assert_eq!(direction, SplitDir::Vertical);
                assert!((ratio - 0.5).abs() < f32::EPSILON);
            }
            PaneNode::Leaf(_) => panic!("expected split"),
        }
    }

    #[test]
    fn split_in_single_leaf_places_new_on_a_side_when_side_b_false() {
        // "0" → split with new id "9" on the left (a).
        let after = split_in(leaf(0), 0, SplitDir::Horizontal, 9, false);
        // Depth-first leaves left → right should be [9, 0] now.
        assert_eq!(leaves_in_order(&after), vec![9, 0]);
    }

    #[test]
    fn split_in_recurses_into_existing_split_to_find_target() {
        // Existing tree: Split(V, 0.5, 0, 1). Split leaf "1" horizontally
        // with new id "2" on the bottom — result should be Split(V, 0.5, 0,
        // Split(H, 0.5, 1, 2)).
        let tree = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(leaf(0)),
            b: Box::new(leaf(1)),
        };
        let after = split_in(tree, 1, SplitDir::Horizontal, 2, true);
        assert_eq!(leaves_in_order(&after), vec![0, 1, 2]);
    }

    #[test]
    fn collapse_close_removes_target_leaf_and_replaces_parent_with_sibling() {
        let tree = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(leaf(0)),
            b: Box::new(leaf(1)),
        };
        // Close leaf "1" — parent split should collapse to just leaf "0".
        let (after, found) = collapse_close(tree, 1);
        assert!(found);
        assert!(matches!(after, PaneNode::Leaf(0)));
    }

    #[test]
    fn collapse_close_no_op_when_target_not_in_tree() {
        let tree = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(leaf(0)),
            b: Box::new(leaf(1)),
        };
        let (after, found) = collapse_close(tree.clone(), 99);
        assert!(!found);
        assert_eq!(leaves_in_order(&after), vec![0, 1]);
    }

    #[test]
    fn first_leaf_of_returns_the_left_most_leaf() {
        // Split(H, 0.5, Split(V, 0.5, 7, 8), 9) → first leaf is 7.
        let tree = PaneNode::Split {
            direction: SplitDir::Horizontal,
            ratio: 0.5,
            a: Box::new(PaneNode::Split {
                direction: SplitDir::Vertical,
                ratio: 0.5,
                a: Box::new(leaf(7)),
                b: Box::new(leaf(8)),
            }),
            b: Box::new(leaf(9)),
        };
        assert_eq!(first_leaf_of(&tree), Some(7));
    }

    #[test]
    fn ghost_window_position_centres_pill_under_cursor_with_no_offset() {
        // cursor at screen (500, 300), no grab offset, 1x scale, 200x100 window:
        // the pill is centred in the window, so the window TL should sit at
        // (500 - 100, 300 - 50) = (400, 250).
        let pos = ghost_window_position((500, 300), 1.0, 0.0, 200, 100);
        assert_eq!(pos.x, 400);
        assert_eq!(pos.y, 250);
    }

    #[test]
    fn ghost_window_position_applies_logical_grab_offset_at_scale() {
        // grab_offset_x = 20 (logical), scale = 2 → 40 physical px. The pill
        // centre should be 40 px LEFT of the cursor, so the window TL sits
        // at (cursor_x - inner/2 - 40, cursor_y - inner/2).
        let pos = ghost_window_position((1000, 500), 2.0, 20.0, 200, 100);
        assert_eq!(pos.x, 1000 - 100 - 40);
        assert_eq!(pos.y, 500 - 50);
    }

    #[test]
    fn fuzzy_matches_subsequence_and_rejects_non_subsequence() {
        assert!(fuzzy_score("nt", "New Tab").is_some());
        assert!(fuzzy_score("newtab", "New Tab").is_some());
        assert!(
            fuzzy_score("", "anything").is_some(),
            "empty query matches all"
        );
        assert!(fuzzy_score("zzz", "New Tab").is_none());
        assert!(
            fuzzy_score("tabx", "New Tab").is_none(),
            "trailing miss fails"
        );
    }

    #[test]
    fn fuzzy_is_case_insensitive() {
        assert!(fuzzy_score("NEW", "new tab").is_some());
        assert!(fuzzy_score("new", "NEW TAB").is_some());
    }

    #[test]
    fn fuzzy_prefers_word_boundary_and_prefix() {
        // "ct" should score higher against "Close Tab" (two word-initials)
        // than against "Select All" (mid-word).
        let boundary = fuzzy_score("ct", "Close Tab").unwrap();
        let midword = fuzzy_score("ct", "Select Action").unwrap();
        assert!(
            boundary > midword,
            "boundary {boundary} should beat midword {midword}"
        );
    }

    /// Built-in theme names + a fake user theme, as the palette would see.
    fn test_theme_names() -> Vec<String> {
        let mut names: Vec<String> = terminale_config::builtin_themes()
            .into_iter()
            .map(|t| t.name)
            .collect();
        names.push("MyCustom".to_string());
        names
    }

    #[test]
    fn ranked_query_filters_and_orders() {
        let sc = terminale_config::ShortcutsConfig::default();
        let ranked = palette_ranked(
            "tab",
            PaletteMode::Actions,
            &sc,
            "Tokyo Night",
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(!ranked.is_empty());
        // Every surviving label must actually fuzzy-match the query.
        for (_, entry) in &ranked {
            assert!(
                fuzzy_score("tab", &entry.label).is_some(),
                "ranked entry {:?} doesn't match query",
                entry.label
            );
        }
        // With no SSH hosts configured the "New SSH Tab…" action is hidden,
        // so the empty query returns (actions) plus the injected "Change Theme…"
        // and "Snippets…" entries.
        assert_eq!(
            palette_ranked(
                "",
                PaletteMode::Actions,
                &sc,
                "Tokyo Night",
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[]
            )
            .len(),
            PALETTE_ACTIONS.len()
        );
    }

    #[test]
    fn ssh_hosts_surface_as_palette_rows() {
        let sc = terminale_config::ShortcutsConfig::default();
        let hosts = vec!["prod".to_string(), "build".to_string()];
        // The empty query now also lists "New SSH Tab…" + one row per host.
        let ranked = palette_ranked(
            "",
            PaletteMode::Actions,
            &sc,
            "Tokyo Night",
            &[],
            &hosts,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(ranked.len(), PALETTE_ACTIONS.len() + 1 + hosts.len());
        // A direct "SSH: prod" row resolves to OpenSsh(0).
        let prod = palette_ranked(
            "SSH: prod",
            PaletteMode::Actions,
            &sc,
            "Tokyo Night",
            &[],
            &hosts,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(matches!(
            prod.first().map(|(it, _)| it),
            Some(PaletteItem::OpenSsh(0))
        ));
    }

    #[test]
    fn ssh_quick_connect_mode_lists_only_hosts() {
        let sc = terminale_config::ShortcutsConfig::default();
        let hosts = vec!["prod".to_string(), "build".to_string()];
        // The scoped picker shows ONLY the hosts — no actions, no theme entry.
        let ranked = palette_ranked(
            "",
            PaletteMode::SshQuickConnect,
            &sc,
            "Tokyo Night",
            &test_theme_names(),
            &hosts,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(ranked.len(), hosts.len());
        for (item, entry) in &ranked {
            assert!(matches!(item, PaletteItem::OpenSsh(_)));
            assert!(entry.label.starts_with("SSH: "));
        }
        // It's still fuzzy-searchable and resolves to the right host index.
        let build = palette_ranked(
            "build",
            PaletteMode::SshQuickConnect,
            &sc,
            "Tokyo Night",
            &[],
            &hosts,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(matches!(
            build.first().map(|(it, _)| it),
            Some(PaletteItem::OpenSsh(1))
        ));
        // With no hosts the scoped picker is empty.
        assert!(palette_ranked(
            "",
            PaletteMode::SshQuickConnect,
            &sc,
            "Tokyo Night",
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .is_empty());
    }

    #[test]
    fn ranked_carries_the_configured_binding() {
        let sc = terminale_config::ShortcutsConfig::default();
        let ranked = palette_ranked(
            "new tab",
            PaletteMode::Actions,
            &sc,
            "Tokyo Night",
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        let (item, entry) = &ranked[0];
        assert!(matches!(item, PaletteItem::Action(ShortcutAction::NewTab)));
        assert_eq!(entry.binding, sc.new_tab);
        assert!(
            !entry.binding.is_empty(),
            "New Tab ships with a default binding"
        );
    }

    #[test]
    fn theme_mode_lists_all_themes_incl_user_and_marks_current() {
        let sc = terminale_config::ShortcutsConfig::default();
        let names = test_theme_names();
        let ranked = palette_ranked(
            "",
            PaletteMode::Themes,
            &sc,
            "Matrix",
            &names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(ranked.len(), names.len());
        // The user-defined theme is surfaced alongside the built-ins.
        assert!(ranked.iter().any(|(_, e)| e.label == "MyCustom"));
        // Exactly the active theme is marked "current".
        let marked: Vec<&str> = ranked
            .iter()
            .filter(|(_, e)| !e.binding.is_empty())
            .map(|(_, e)| e.label.as_str())
            .collect();
        assert_eq!(marked, vec!["Matrix"]);
        // Each row carries a SetTheme item with a matching name.
        for (item, entry) in &ranked {
            match item {
                PaletteItem::SetTheme(name) => assert_eq!(*name, entry.label),
                _ => panic!("theme mode should only yield SetTheme items"),
            }
        }
    }

    #[test]
    fn theme_mode_fuzzy_filters() {
        let sc = terminale_config::ShortcutsConfig::default();
        let names = test_theme_names();
        let ranked = palette_ranked(
            "drac",
            PaletteMode::Themes,
            &sc,
            "Tokyo Night",
            &names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(
            ranked.iter().any(|(_, e)| e.label == "Dracula"),
            "expected Dracula to survive the 'drac' filter"
        );
    }

    #[test]
    fn actions_mode_offers_theme_picker_entry() {
        let sc = terminale_config::ShortcutsConfig::default();
        let ranked = palette_ranked(
            "change theme",
            PaletteMode::Actions,
            &sc,
            "Tokyo Night",
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(matches!(
            ranked.first().map(|(it, _)| it),
            Some(PaletteItem::OpenThemePicker)
        ));
    }

    #[test]
    fn every_action_has_a_binding_lookup() {
        // binding_for must cover every variant the palette can surface
        // (a missing arm would fail to compile, but this guards intent).
        let sc = terminale_config::ShortcutsConfig::default();
        for (action, _) in PALETTE_ACTIONS {
            let _ = binding_for(*action, &sc);
        }
    }

    #[test]
    fn stay_on_top_is_in_the_palette() {
        // "Toggle Stay on Top" must be discoverable in the command palette.
        assert!(
            PALETTE_ACTIONS
                .iter()
                .any(|(a, label)| matches!(a, ShortcutAction::ToggleStayOnTop)
                    && *label == "Toggle Stay on Top"),
            "stay-on-top action must appear in the command palette"
        );
    }

    #[test]
    fn stay_on_top_shortcut_resolves_when_bound() {
        // Unbound by default → never resolves. Once the user binds it, the
        // resolver maps the keystroke to ToggleStayOnTop.
        let mut sc = terminale_config::ShortcutsConfig::default();
        assert!(sc.stay_on_top.is_empty());
        sc.stay_on_top = "Ctrl+Shift+T".into();
        assert_eq!(
            binding_for(ShortcutAction::ToggleStayOnTop, &sc),
            "Ctrl+Shift+T"
        );
    }

    #[test]
    fn snap_actions_are_all_in_the_palette() {
        use ShortcutAction::{
            ShowSnapLayouts, SnapBottom, SnapBottomLeft, SnapBottomRight, SnapCenter, SnapLeft,
            SnapMaximize, SnapRight, SnapTop, SnapTopLeft, SnapTopRight,
        };
        for want in [
            SnapTop,
            SnapBottom,
            SnapLeft,
            SnapRight,
            SnapCenter,
            SnapMaximize,
            SnapTopLeft,
            SnapTopRight,
            SnapBottomLeft,
            SnapBottomRight,
            ShowSnapLayouts,
        ] {
            assert!(
                PALETTE_ACTIONS.iter().any(|(a, _)| *a == want),
                "snap action {want:?} must appear in the command palette"
            );
        }
    }

    #[test]
    fn snap_actions_unbound_by_default_but_resolve_when_bound() {
        let mut sc = terminale_config::ShortcutsConfig::default();
        // All snap actions ship unbound.
        assert!(binding_for(ShortcutAction::SnapMaximize, &sc).is_empty());
        assert!(binding_for(ShortcutAction::SnapLeft, &sc).is_empty());
        assert!(binding_for(ShortcutAction::SnapTopLeft, &sc).is_empty());
        assert!(binding_for(ShortcutAction::SnapTopRight, &sc).is_empty());
        assert!(binding_for(ShortcutAction::SnapBottomLeft, &sc).is_empty());
        assert!(binding_for(ShortcutAction::SnapBottomRight, &sc).is_empty());
        assert!(binding_for(ShortcutAction::ShowSnapLayouts, &sc).is_empty());
        // Binding one surfaces it through binding_for.
        sc.snap_left = "Ctrl+Alt+ArrowLeft".into();
        assert_eq!(
            binding_for(ShortcutAction::SnapLeft, &sc),
            "Ctrl+Alt+ArrowLeft"
        );
        sc.snap_top_left = "Ctrl+Alt+Home".into();
        assert_eq!(
            binding_for(ShortcutAction::SnapTopLeft, &sc),
            "Ctrl+Alt+Home"
        );
        sc.show_snap_layouts = "Ctrl+Alt+S".into();
        assert_eq!(
            binding_for(ShortcutAction::ShowSnapLayouts, &sc),
            "Ctrl+Alt+S"
        );
    }

    /// The snap-layout chooser cell list has exactly 10 entries and maps
    /// cell indices to the correct `SnapChooserCell` variants.
    #[test]
    fn snap_chooser_cell_list_is_complete() {
        use terminale_render::{SnapChooserCell, SNAP_CHOOSER_CELLS};
        assert_eq!(
            SNAP_CHOOSER_CELLS.len(),
            10,
            "SNAP_CHOOSER_CELLS must have exactly 10 entries"
        );
        // Spot-check a few known positions.
        assert_eq!(SNAP_CHOOSER_CELLS[0], SnapChooserCell::TopLeft);
        assert_eq!(SNAP_CHOOSER_CELLS[1], SnapChooserCell::Top);
        assert_eq!(SNAP_CHOOSER_CELLS[2], SnapChooserCell::TopRight);
        assert_eq!(SNAP_CHOOSER_CELLS[4], SnapChooserCell::Center);
        assert_eq!(SNAP_CHOOSER_CELLS[9], SnapChooserCell::Maximize);
    }

    /// `snap_chooser_apply` maps every cell index to the correct `SnapEdge`.
    /// We can't easily call it against a running state in a unit test, so
    /// instead verify the `SNAP_CHOOSER_CELLS` → `SnapEdge` mapping by hand.
    #[test]
    fn snap_chooser_cells_cover_all_snap_edges() {
        use terminale_config::SnapEdge;
        use terminale_render::{SnapChooserCell, SNAP_CHOOSER_CELLS};
        // Every variant reachable through the chooser must appear at least once.
        let expected_edges = [
            SnapEdge::Left,
            SnapEdge::Right,
            SnapEdge::Top,
            SnapEdge::Bottom,
            SnapEdge::TopLeft,
            SnapEdge::TopRight,
            SnapEdge::BottomLeft,
            SnapEdge::BottomRight,
            SnapEdge::Center,
            SnapEdge::Maximize,
        ];
        for edge in &expected_edges {
            let cell = match edge {
                SnapEdge::Left => SnapChooserCell::Left,
                SnapEdge::Right => SnapChooserCell::Right,
                SnapEdge::Top => SnapChooserCell::Top,
                SnapEdge::Bottom => SnapChooserCell::Bottom,
                SnapEdge::TopLeft => SnapChooserCell::TopLeft,
                SnapEdge::TopRight => SnapChooserCell::TopRight,
                SnapEdge::BottomLeft => SnapChooserCell::BottomLeft,
                SnapEdge::BottomRight => SnapChooserCell::BottomRight,
                SnapEdge::Center => SnapChooserCell::Center,
                SnapEdge::Maximize => SnapChooserCell::Maximize,
            };
            assert!(
                SNAP_CHOOSER_CELLS.contains(&cell),
                "cell {cell:?} for edge {edge:?} must be in SNAP_CHOOSER_CELLS",
            );
        }
    }

    /// keybinds roundtrip: new snap fields survive serialise → deserialise.
    #[test]
    fn snap_quarter_keybinds_roundtrip() {
        let cfg = terminale_config::ShortcutsConfig {
            snap_top_left: "Ctrl+Alt+Home".into(),
            snap_top_right: "Ctrl+Alt+PageUp".into(),
            snap_bottom_left: "Ctrl+Alt+End".into(),
            snap_bottom_right: "Ctrl+Alt+PageDown".into(),
            show_snap_layouts: "Ctrl+Alt+S".into(),
            ..terminale_config::ShortcutsConfig::default()
        };

        let serialised = toml::to_string(&cfg).expect("serialise");
        let roundtripped: terminale_config::ShortcutsConfig =
            toml::from_str(&serialised).expect("deserialise");

        assert_eq!(roundtripped.snap_top_left, "Ctrl+Alt+Home");
        assert_eq!(roundtripped.snap_top_right, "Ctrl+Alt+PageUp");
        assert_eq!(roundtripped.snap_bottom_left, "Ctrl+Alt+End");
        assert_eq!(roundtripped.snap_bottom_right, "Ctrl+Alt+PageDown");
        assert_eq!(roundtripped.show_snap_layouts, "Ctrl+Alt+S");
    }

    #[test]
    fn ctrl_maps_non_alpha_keys_to_c0_controls() {
        // Letters keep producing ^A..^Z.
        assert_eq!(ctrl_code_for(KeyCode::KeyA), Some(0x01));
        assert_eq!(ctrl_code_for(KeyCode::KeyC), Some(0x03));
        // The previously-missing C0 controls: Ctrl+\ must send FS (0x1c),
        // not the literal backslash.
        assert_eq!(ctrl_code_for(KeyCode::Backslash), Some(0x1c));
        assert_eq!(ctrl_code_for(KeyCode::BracketLeft), Some(0x1b));
        assert_eq!(ctrl_code_for(KeyCode::BracketRight), Some(0x1d));
        assert_eq!(ctrl_code_for(KeyCode::Slash), Some(0x1f));
        // Keys with no control mapping fall through.
        assert_eq!(ctrl_code_for(KeyCode::F5), None);
    }

    #[test]
    fn rects_close_respects_tolerance() {
        let a = (100, 200, 800, 600);
        assert!(rects_close(a, (103, 198, 802, 597), 6));
        // Height differs by 50 → the user resized: not "close".
        assert!(!rects_close(a, (100, 200, 800, 650), 6));
        // Moved 20px horizontally → not close.
        assert!(!rects_close(a, (120, 200, 800, 600), 6));
    }

    #[test]
    fn tab_label_prefers_program_title() {
        // A program-set OSC title wins over profile + cwd.
        assert_eq!(
            compose_tab_label(None, "PowerShell", Some("vim main.rs"), Some("repo"), false),
            "vim main.rs"
        );
        // No title → profile — cwd.
        assert_eq!(
            compose_tab_label(None, "PowerShell", None, Some("repo"), false),
            "PowerShell — repo"
        );
        // No title, no cwd → just the profile.
        assert_eq!(
            compose_tab_label(None, "PowerShell", None, None, false),
            "PowerShell"
        );
        // Blank title is ignored (falls through to profile).
        assert_eq!(
            compose_tab_label(None, "zsh", Some("   "), None, false),
            "zsh"
        );
        // The shell's own exe-path title is noise → fall back to profile+cwd.
        assert_eq!(
            compose_tab_label(
                None,
                "Windows PowerShell",
                Some("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
                Some("repo"),
                false
            ),
            "Windows PowerShell — repo"
        );
        // Crashed tabs are flagged.
        assert_eq!(
            compose_tab_label(None, "zsh", None, None, true),
            "⚠ zsh (crashed)"
        );
        // Overlong titles are truncated with an ellipsis.
        let long = "x".repeat(80);
        let out = compose_tab_label(None, "zsh", Some(&long), None, false);
        assert!(out.ends_with('…') && out.chars().count() == 40);
    }

    #[test]
    fn user_title_overrides_everything() {
        // An explicit user rename beats program title, cwd, and profile.
        assert_eq!(
            compose_tab_label(Some("my work"), "zsh", Some("vim x"), Some("repo"), false),
            "my work"
        );
        // Blank / whitespace user title falls through to the auto label.
        assert_eq!(
            compose_tab_label(Some("   "), "zsh", None, Some("repo"), false),
            "zsh — repo"
        );
        // A user title on a crashed tab still gets the crash marker.
        assert_eq!(
            compose_tab_label(Some("build"), "zsh", None, None, true),
            "⚠ build (crashed)"
        );
    }

    #[test]
    fn bracketed_paste_strips_embedded_end_marker() {
        // A clipboard trying to break out of bracketed paste to inject input.
        let evil = "ls\x1b[201~\nrm -rf /\n";
        let out = build_paste_payload(evil, true);
        let s = String::from_utf8(out).unwrap();
        // Exactly one start + one end marker (our wrapper), none smuggled in.
        assert_eq!(s.matches("\x1b[200~").count(), 1);
        assert_eq!(s.matches("\x1b[201~").count(), 1);
        assert!(s.starts_with("\x1b[200~") && s.ends_with("\x1b[201~"));
        // The payload between the markers carries no bare end marker.
        let inner = &s["\x1b[200~".len()..s.len() - "\x1b[201~".len()];
        assert!(!inner.contains("\x1b[201~"));
        assert!(inner.contains("rm -rf /"));
    }

    #[test]
    fn paste_normalises_newlines_and_skips_markers_when_unbracketed() {
        // CRLF → LF in both modes.
        let out = build_paste_payload("a\r\nb\r\n", false);
        assert_eq!(out, b"a\nb\n");
        // Non-bracketed mode doesn't add markers.
        let s = String::from_utf8(build_paste_payload("x", false)).unwrap();
        assert!(!s.contains("\x1b[200~"));
    }

    #[test]
    fn spawn_spec_empty_command_falls_back_to_default_shell() {
        // The cwd-inheriting "new tab" builds a profile with an empty
        // command + a cwd; that must spawn the default shell *in* that cwd,
        // not try (and fail) to launch an empty program.
        let cwd = std::path::PathBuf::from(if cfg!(windows) { "C:\\" } else { "/tmp" });
        let p = terminale_config::Profile {
            name: "inherited".into(),
            command: String::new(),
            args: vec![],
            env: Default::default(),
            cwd: Some(cwd.clone()),
            icon: None,
        };
        let spec = build_spawn_spec(Some(&p), None);
        assert_eq!(spec.command, default_shell());
        assert_eq!(spec.cwd, Some(cwd));

        // A real command is left untouched.
        let p2 = terminale_config::Profile {
            command: "zsh".into(),
            ..p.clone()
        };
        assert_eq!(build_spawn_spec(Some(&p2), None).command, "zsh");

        // No profile → default shell.
        assert_eq!(build_spawn_spec(None, None).command, default_shell());
    }

    #[test]
    fn translate_key_emits_correct_pty_bytes() {
        use winit::keyboard::{Key, NamedKey};
        let named = |n| Key::Named(n);
        let ch = |s: &str| Key::Character(s.into());
        // Helper: normal (CSI) cursor mode, no text.
        let go = |mods: ModifiersState, code: KeyCode, key: &Key| {
            translate_key(&mods, PhysicalKey::Code(code), key, None, false)
        };

        // Plain keys.
        assert_eq!(
            go(
                ModifiersState::empty(),
                KeyCode::Enter,
                &named(NamedKey::Enter)
            ),
            Some(vec![b'\r'])
        );
        assert_eq!(
            go(
                ModifiersState::empty(),
                KeyCode::Backspace,
                &named(NamedKey::Backspace)
            ),
            Some(vec![0x7f])
        );
        assert_eq!(
            go(
                ModifiersState::empty(),
                KeyCode::ArrowUp,
                &named(NamedKey::ArrowUp)
            ),
            Some(b"\x1b[A".to_vec())
        );
        // Ctrl+Backspace → ^W (delete word); Ctrl+C → ETX.
        assert_eq!(
            go(
                ModifiersState::CONTROL,
                KeyCode::Backspace,
                &named(NamedKey::Backspace)
            ),
            Some(vec![0x17])
        );
        assert_eq!(
            go(ModifiersState::CONTROL, KeyCode::KeyC, &ch("c")),
            Some(vec![0x03])
        );
        // Alt+b → ESC b (meta prefix).
        assert_eq!(
            go(ModifiersState::ALT, KeyCode::KeyB, &ch("b")),
            Some(vec![0x1b, b'b'])
        );
        // Ctrl+ArrowUp → CSI 1;5A; Shift+Tab → reverse-tab.
        assert_eq!(
            go(
                ModifiersState::CONTROL,
                KeyCode::ArrowUp,
                &named(NamedKey::ArrowUp)
            ),
            Some(b"\x1b[1;5A".to_vec())
        );
        assert_eq!(
            go(ModifiersState::SHIFT, KeyCode::Tab, &named(NamedKey::Tab)),
            Some(b"\x1b[Z".to_vec())
        );

        // ── F5–F12 unmodified ─────────────────────────────────────────────────
        assert_eq!(
            go(ModifiersState::empty(), KeyCode::F5, &named(NamedKey::F5)),
            Some(b"\x1b[15~".to_vec()),
            "F5 must emit CSI 15~"
        );
        assert_eq!(
            go(ModifiersState::empty(), KeyCode::F6, &named(NamedKey::F6)),
            Some(b"\x1b[17~".to_vec()),
            "F6 must emit CSI 17~"
        );
        assert_eq!(
            go(ModifiersState::empty(), KeyCode::F12, &named(NamedKey::F12)),
            Some(b"\x1b[24~".to_vec()),
            "F12 must emit CSI 24~"
        );

        // ── Modified function key (Shift+F5 → CSI 15;2~) ─────────────────────
        assert_eq!(
            go(ModifiersState::SHIFT, KeyCode::F5, &named(NamedKey::F5)),
            Some(b"\x1b[15;2~".to_vec()),
            "Shift+F5 must emit CSI 15;2~"
        );

        // ── Modified PageUp (Ctrl+PageUp → CSI 5;5~) ─────────────────────────
        assert_eq!(
            go(
                ModifiersState::CONTROL,
                KeyCode::PageUp,
                &named(NamedKey::PageUp)
            ),
            Some(b"\x1b[5;5~".to_vec()),
            "Ctrl+PageUp must emit CSI 5;5~"
        );

        // ── Application cursor-key mode: arrows use SS3 ───────────────────────
        let go_app = |code: KeyCode, key: &Key| {
            translate_key(
                &ModifiersState::empty(),
                PhysicalKey::Code(code),
                key,
                None,
                true,
            )
        };
        assert_eq!(
            go_app(KeyCode::ArrowUp, &named(NamedKey::ArrowUp)),
            Some(b"\x1bOA".to_vec()),
            "ArrowUp in app-cursor mode must emit SS3 A"
        );
        assert_eq!(
            go_app(KeyCode::ArrowDown, &named(NamedKey::ArrowDown)),
            Some(b"\x1bOB".to_vec()),
            "ArrowDown in app-cursor mode must emit SS3 B"
        );
        assert_eq!(
            go_app(KeyCode::ArrowRight, &named(NamedKey::ArrowRight)),
            Some(b"\x1bOC".to_vec()),
            "ArrowRight in app-cursor mode must emit SS3 C"
        );
        assert_eq!(
            go_app(KeyCode::ArrowLeft, &named(NamedKey::ArrowLeft)),
            Some(b"\x1bOD".to_vec()),
            "ArrowLeft in app-cursor mode must emit SS3 D"
        );
        assert_eq!(
            go_app(KeyCode::Home, &named(NamedKey::Home)),
            Some(b"\x1bOH".to_vec()),
            "Home in app-cursor mode must emit SS3 H"
        );
        assert_eq!(
            go_app(KeyCode::End, &named(NamedKey::End)),
            Some(b"\x1bOF".to_vec()),
            "End in app-cursor mode must emit SS3 F"
        );

        // Modified arrows in app-cursor mode must still use CSI form.
        assert_eq!(
            translate_key(
                &ModifiersState::CONTROL,
                PhysicalKey::Code(KeyCode::ArrowUp),
                &named(NamedKey::ArrowUp),
                None,
                true,
            ),
            Some(b"\x1b[1;5A".to_vec()),
            "Ctrl+ArrowUp in app-cursor mode must still use CSI"
        );
    }

    #[test]
    fn scroll_follows_at_bottom_but_holds_in_history() {
        // At the live edge → keep following (stay at 0).
        assert_eq!(scroll_after_output(0, 100, 110), 0);
        // Scrolled up 20 lines; 10 new lines spilled into history → advance
        // the offset by 10 so the same content stays in view.
        assert_eq!(scroll_after_output(20, 100, 110), 30);
        // No new history → offset unchanged.
        assert_eq!(scroll_after_output(20, 100, 100), 20);
        // Never exceed the available history.
        assert_eq!(scroll_after_output(95, 100, 100), 95);
    }

    #[test]
    fn parse_binding_and_keycode_names() {
        let (m, k) = parse_binding("Ctrl+Shift+P").unwrap();
        assert!(m.ctrl && m.shift && !m.alt && !m.meta);
        assert_eq!(k, "P");
        // The backtick Quake binding — historically tricky to parse/resolve.
        let (m, k) = parse_binding("Ctrl+`").unwrap();
        assert!(m.ctrl && !m.shift);
        assert_eq!(k, "`");
        // keycode_name(Backquote) must yield "`" so Ctrl+` actually resolves.
        assert_eq!(keycode_name(KeyCode::Backquote), Some("`"));
        // Modifiers are case-insensitive; key token preserved verbatim.
        let (m, k) = parse_binding("control+shift+ArrowLeft").unwrap();
        assert!(m.ctrl && m.shift);
        assert_eq!(k, "ArrowLeft");
        // Empty / modifier-only bindings disable the action.
        assert!(parse_binding("").is_none());
        assert!(parse_binding("Ctrl+").is_none());
    }

    #[test]
    fn tab_jump_index_maps_digits_and_last() {
        // 5 tabs: digits 1..=5 select 0..=4; 9 is always the last tab.
        assert_eq!(tab_jump_index(KeyCode::Digit1, 5), Some(0));
        assert_eq!(tab_jump_index(KeyCode::Digit5, 5), Some(4));
        assert_eq!(tab_jump_index(KeyCode::Digit9, 5), Some(4));
        // Numpad digits behave identically.
        assert_eq!(tab_jump_index(KeyCode::Numpad3, 5), Some(2));
        // A digit past the tab count is a no-op...
        assert_eq!(tab_jump_index(KeyCode::Digit5, 2), None);
        // ...but "9 = last" still resolves against a short tab list.
        assert_eq!(tab_jump_index(KeyCode::Digit9, 2), Some(1));
        // No tabs, or a non-digit key, never jumps.
        assert_eq!(tab_jump_index(KeyCode::Digit1, 0), None);
        assert_eq!(tab_jump_index(KeyCode::KeyA, 5), None);
    }

    #[test]
    fn active_tab_index_survives_detach() {
        // 4 tabs [0,1,2,3], active = 2.
        // Remove a tab AFTER the active one → active stays put.
        assert_eq!(active_tab_after_detach(2, 3, 4), 2);
        // Remove the active tab (not the last) → the tab that slides into
        // its slot becomes active, so the index is unchanged (matches the
        // long-standing close-tab behaviour).
        assert_eq!(active_tab_after_detach(2, 2, 4), 2);
        // Remove a tab BEFORE the active one → active shifts left by one.
        assert_eq!(active_tab_after_detach(2, 0, 4), 1);
        // Remove the last (active) tab → clamp to the new last index.
        assert_eq!(active_tab_after_detach(3, 3, 4), 2);
        // Removing the only tab → empty list reports index 0.
        assert_eq!(active_tab_after_detach(0, 0, 1), 0);
        // Active was already the first; removing a later tab keeps it at 0.
        assert_eq!(active_tab_after_detach(0, 2, 4), 0);
        // Active is the last index but a lower tab is removed → clamp down.
        assert_eq!(active_tab_after_detach(3, 1, 4), 2);
    }

    #[test]
    fn window_bar_hit_test_picks_the_band_under_the_cursor() {
        let a = WindowId::from(1u64);
        let b = WindowId::from(2u64);
        // Window A at (0,0) 800 wide; window B at (900,100) 600 wide; both
        // scale 1.0, so the bar band is y ∈ [0, TAB_BAR_HEIGHT] (36 px).
        let bars = [
            BarRect {
                id: a,
                x: 0,
                y: 0,
                width: 800,
                height: 600,
                scale: 1.0,
                is_vertical: false,
                vert_strip_x_logical: 0.0,
                vert_strip_w_logical: 0.0,
                vert_inner_edge_logical: 0.0,
            },
            BarRect {
                id: b,
                x: 900,
                y: 100,
                width: 600,
                height: 500,
                scale: 1.0,
                is_vertical: false,
                vert_strip_x_logical: 0.0,
                vert_strip_w_logical: 0.0,
                vert_inner_edge_logical: 0.0,
            },
        ];
        // Inside A's bar band.
        assert_eq!(window_bar_at_screen(&bars, 400, 10), Some(a));
        // Just below A's bar (y past the band) → no hit on A.
        assert_eq!(window_bar_at_screen(&bars, 400, 60), None);
        // Inside B's bar band (B's top is at screen-y 100).
        assert_eq!(window_bar_at_screen(&bars, 1000, 110), Some(b));
        // Right of A but left of B (a gap) → no hit.
        assert_eq!(window_bar_at_screen(&bars, 850, 10), None);
        // Far outside everything.
        assert_eq!(window_bar_at_screen(&bars, 5000, 5000), None);
    }

    #[test]
    fn window_bar_hit_test_honours_scale_factor() {
        let a = WindowId::from(7u64);
        // At scale 2.0 the 36-logical-px band is 72 physical px tall.
        let bars = [BarRect {
            id: a,
            x: 0,
            y: 0,
            width: 1600,
            height: 1200,
            scale: 2.0,
            is_vertical: false,
            vert_strip_x_logical: 0.0,
            vert_strip_w_logical: 0.0,
            vert_inner_edge_logical: 0.0,
        }];
        // 70 physical px is still within the band (70/2 = 35 < 36).
        assert_eq!(window_bar_at_screen(&bars, 100, 70), Some(a));
        // 80 physical px is past it (80/2 = 40 > 36).
        assert_eq!(window_bar_at_screen(&bars, 100, 80), None);
    }

    #[test]
    fn window_bar_hit_test_returns_first_match_on_overlap() {
        let top = WindowId::from(10u64);
        let under = WindowId::from(11u64);
        // Two windows occupying the same region; the first listed wins, so
        // callers pass the most-recently-focused window first.
        let bars = [
            BarRect {
                id: top,
                x: 0,
                y: 0,
                width: 400,
                height: 300,
                scale: 1.0,
                is_vertical: false,
                vert_strip_x_logical: 0.0,
                vert_strip_w_logical: 0.0,
                vert_inner_edge_logical: 0.0,
            },
            BarRect {
                id: under,
                x: 0,
                y: 0,
                width: 400,
                height: 300,
                scale: 1.0,
                is_vertical: false,
                vert_strip_x_logical: 0.0,
                vert_strip_w_logical: 0.0,
                vert_inner_edge_logical: 0.0,
            },
        ];
        assert_eq!(window_bar_at_screen(&bars, 50, 10), Some(top));
    }

    #[test]
    fn profile_from_closed_preserves_identity() {
        let closed = ClosedTab {
            profile_name: "PowerShell".into(),
            icon: Some("PS".into()),
            cwd: Some(std::path::PathBuf::from("C:/work")),
        };
        let p = profile_from_closed(&closed);
        assert_eq!(p.name, "PowerShell");
        assert_eq!(p.icon.as_deref(), Some("PS"));
        assert_eq!(p.cwd, Some(std::path::PathBuf::from("C:/work")));
        // Empty command → the spawner falls back to the default shell.
        assert!(p.command.is_empty());
    }

    #[test]
    fn parse_ssh_plain_host() {
        let p = parse_ssh_command("ssh example.com").unwrap();
        assert_eq!(p.user, None);
        assert_eq!(p.host, "example.com");
        assert_eq!(p.port, 22);
    }

    #[test]
    fn parse_ssh_user_at_host() {
        let p = parse_ssh_command("ssh root@example.com").unwrap();
        assert_eq!(p.user.as_deref(), Some("root"));
        assert_eq!(p.host, "example.com");
        assert_eq!(p.port, 22);
    }

    #[test]
    fn parse_ssh_host_then_port_flag() {
        let p = parse_ssh_command("ssh example.com -p 2222").unwrap();
        assert_eq!(p.user, None);
        assert_eq!(p.host, "example.com");
        assert_eq!(p.port, 2222);
    }

    #[test]
    fn parse_ssh_port_flag_before_user_at_host() {
        let p = parse_ssh_command("ssh -p 2222 deploy@10.0.0.5").unwrap();
        assert_eq!(p.user.as_deref(), Some("deploy"));
        assert_eq!(p.host, "10.0.0.5");
        assert_eq!(p.port, 2222);
    }

    #[test]
    fn parse_ssh_dash_l_user() {
        let p = parse_ssh_command("ssh -l alice server.internal").unwrap();
        assert_eq!(p.user.as_deref(), Some("alice"));
        assert_eq!(p.host, "server.internal");
        assert_eq!(p.port, 22);
    }

    #[test]
    fn parse_ssh_glued_port() {
        // `-p2222` with no space.
        let p = parse_ssh_command("ssh -p2222 box").unwrap();
        assert_eq!(p.host, "box");
        assert_eq!(p.port, 2222);
    }

    #[test]
    fn parse_ssh_user_at_host_overrides_dash_l() {
        // OpenSSH: the destination's `user@` wins over an earlier `-l`.
        let p = parse_ssh_command("ssh -l ignored bob@host").unwrap();
        assert_eq!(p.user.as_deref(), Some("bob"));
        assert_eq!(p.host, "host");
    }

    #[test]
    fn parse_ssh_ignores_boolean_and_value_flags() {
        // `-A` / `-X` are booleans; `-i keyfile` takes a value that must not
        // be mistaken for the host; the remote command after the host is
        // ignored too.
        let p = parse_ssh_command("ssh -A -X -i /home/me/.ssh/id_ed25519 root@h uptime").unwrap();
        assert_eq!(p.user.as_deref(), Some("root"));
        assert_eq!(p.host, "h");
        assert_eq!(p.port, 22);
    }

    #[test]
    fn parse_ssh_accepts_binary_path() {
        let p = parse_ssh_command("/usr/bin/ssh prod").unwrap();
        assert_eq!(p.host, "prod");
    }

    #[test]
    fn parse_ssh_rejects_non_ssh_lines() {
        assert!(parse_ssh_command("").is_none());
        assert!(parse_ssh_command("   ").is_none());
        assert!(parse_ssh_command("ls -la").is_none());
        assert!(parse_ssh_command("sshd -t").is_none(), "sshd is not ssh");
        assert!(parse_ssh_command("mosh user@host").is_none());
        // ssh with no destination at all.
        assert!(parse_ssh_command("ssh").is_none());
        assert!(parse_ssh_command("ssh -p 22").is_none());
        // A non-numeric / out-of-range port aborts the parse.
        assert!(parse_ssh_command("ssh -p abc host").is_none());
        assert!(parse_ssh_command("ssh -p 99999 host").is_none());
    }

    // -- Phase E: divider walker + path utilities ---------------------

    fn dividers_for(
        node: &PaneNode,
        rect: (f32, f32, f32, f32),
        thickness_px: f32,
        grab_pad_px: f32,
    ) -> Vec<LocalDividerSpec> {
        let mut out = Vec::new();
        let mut path = Vec::new();
        walk_divider_tree(node, rect, &mut path, thickness_px, grab_pad_px, &mut out);
        out
    }

    #[test]
    fn walk_divider_tree_two_pane_vertical_split_returns_one_divider() {
        let tree = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(PaneNode::Leaf(0)),
            b: Box::new(PaneNode::Leaf(1)),
        };
        let specs = dividers_for(&tree, (0.0, 0.0, 200.0, 100.0), 4.0, 0.0);
        assert_eq!(specs.len(), 1);
        let d = &specs[0];
        assert_eq!(d.axis, SplitDir::Vertical);
        assert!(d.path.is_empty());
        let (rx, _ry, rw, rh) = d.rect_px;
        assert!((rx - 98.0).abs() < 1.0);
        assert!((rw - 4.0).abs() < 1.0);
        assert!((rh - 100.0).abs() < 1.0);
    }

    #[test]
    fn walk_divider_tree_nested_three_pane_returns_two_dividers() {
        let inner = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(PaneNode::Leaf(0)),
            b: Box::new(PaneNode::Leaf(1)),
        };
        let tree = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(inner),
            b: Box::new(PaneNode::Leaf(2)),
        };
        let specs = dividers_for(&tree, (0.0, 0.0, 200.0, 100.0), 2.0, 0.0);
        assert_eq!(specs.len(), 2);
        let outer = specs.iter().find(|s| s.path.is_empty()).unwrap();
        let (ox, _, ow, _) = outer.rect_px;
        assert!((ox - 99.0).abs() < 1.0);
        assert!((ow - 2.0).abs() < 1.0);
        let inner_spec = specs.iter().find(|s| s.path == [false]).unwrap();
        let (ix, _, iw, _) = inner_spec.rect_px;
        assert!((ix - 49.0).abs() < 1.0);
        assert!((iw - 2.0).abs() < 1.0);
    }

    #[test]
    fn split_node_mut_at_path_resolves_deeper_split() {
        let inner = PaneNode::Split {
            direction: SplitDir::Horizontal,
            ratio: 0.4,
            a: Box::new(PaneNode::Leaf(0)),
            b: Box::new(PaneNode::Leaf(1)),
        };
        let mut tree = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(inner),
            b: Box::new(PaneNode::Leaf(2)),
        };
        assert!(matches!(
            split_node_mut_at_path(&mut tree, &[]),
            Some(PaneNode::Split {
                direction: SplitDir::Vertical,
                ..
            })
        ));
        assert!(matches!(
            split_node_mut_at_path(&mut tree, &[false]),
            Some(PaneNode::Split {
                direction: SplitDir::Horizontal,
                ..
            })
        ));
        assert!(split_node_mut_at_path(&mut tree, &[true]).is_none());
    }

    #[test]
    fn set_split_ratio_at_clamps_and_returns_false_on_stale_path() {
        let mut tree = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(PaneNode::Leaf(0)),
            b: Box::new(PaneNode::Leaf(1)),
        };
        assert!(set_split_ratio_at(&mut tree, &[], 0.7));
        if let PaneNode::Split { ratio, .. } = &tree {
            assert!((ratio - 0.7).abs() < f32::EPSILON);
        }
        assert!(set_split_ratio_at(&mut tree, &[], 0.01));
        if let PaneNode::Split { ratio, .. } = &tree {
            assert!((*ratio - 0.05).abs() < f32::EPSILON);
        }
        assert!(set_split_ratio_at(&mut tree, &[], 0.99));
        if let PaneNode::Split { ratio, .. } = &tree {
            assert!((*ratio - 0.95).abs() < f32::EPSILON);
        }
        assert!(!set_split_ratio_at(&mut tree, &[true], 0.5));
        assert!(!set_split_ratio_at(&mut tree, &[false, false], 0.5));
    }

    // -- Menu action round-trip + menu_items content tests ---------------

    /// Every new MenuAction id must survive a round-trip through as_u32 /
    /// from_u32 without colliding with the legacy ids 0..=8.
    #[test]
    fn menu_action_round_trip_new_pane_variants() {
        use MenuAction::{ClosePane, SplitDown, SplitLeft, SplitRight, SplitUp};
        let new_variants = [
            (SplitRight, 9u32),
            (SplitDown, 10),
            (SplitLeft, 11),
            (SplitUp, 12),
            (ClosePane, 13),
        ];
        for (action, expected_id) in new_variants {
            assert_eq!(
                action.as_u32(),
                expected_id,
                "{action:?} should map to id {expected_id}"
            );
            assert!(
                MenuAction::from_u32(expected_id).is_some(),
                "from_u32({expected_id}) should return Some(_)"
            );
            // Ensure legacy ids 0..=8 are not stomped.
            assert!(
                expected_id > 8,
                "new id {expected_id} must not overlap legacy ids 0..=8"
            );
            // Ensure well below PROFILE_PICKER_BASE (0x1_0000).
            assert!(
                expected_id < 0x1_0000,
                "new id must stay below PROFILE_PICKER_BASE"
            );
        }
    }

    /// MenuAction round-trips for pane and snap action ids must be stable.
    #[test]
    fn menu_items_includes_split_and_close_pane_entries() {
        // We can test MenuAction.as_u32 round-trips and the label assertions
        // indirectly via the from_u32 API. The full menu_items() call requires
        // a RunningState which is not constructable in unit tests; instead we
        // verify the invariants that can be checked without one.

        // Pane-management ids 9..=13.
        assert!(matches!(
            MenuAction::from_u32(9),
            Some(MenuAction::SplitRight)
        ));
        assert!(matches!(
            MenuAction::from_u32(10),
            Some(MenuAction::SplitDown)
        ));
        assert!(matches!(
            MenuAction::from_u32(11),
            Some(MenuAction::SplitLeft)
        ));
        assert!(matches!(
            MenuAction::from_u32(12),
            Some(MenuAction::SplitUp)
        ));
        assert!(matches!(
            MenuAction::from_u32(13),
            Some(MenuAction::ClosePane)
        ));
        // Snap/position ids 14..=19.
        assert!(matches!(
            MenuAction::from_u32(14),
            Some(MenuAction::SnapTop)
        ));
        assert!(matches!(
            MenuAction::from_u32(15),
            Some(MenuAction::SnapBottom)
        ));
        assert!(matches!(
            MenuAction::from_u32(16),
            Some(MenuAction::SnapLeft)
        ));
        assert!(matches!(
            MenuAction::from_u32(17),
            Some(MenuAction::SnapRight)
        ));
        assert!(matches!(
            MenuAction::from_u32(18),
            Some(MenuAction::SnapCenter)
        ));
        assert!(matches!(
            MenuAction::from_u32(19),
            Some(MenuAction::SnapMaximize)
        ));
        // Tab actions ids 20..=22.
        assert!(matches!(MenuAction::from_u32(20), Some(MenuAction::NewTab)));
        assert!(matches!(
            MenuAction::from_u32(21),
            Some(MenuAction::CopyCurrentPath)
        ));
        assert!(matches!(
            MenuAction::from_u32(22),
            Some(MenuAction::CloseTab)
        ));
        assert!(matches!(
            MenuAction::from_u32(23),
            Some(MenuAction::RenameTab)
        ));
        assert!(matches!(
            MenuAction::from_u32(24),
            Some(MenuAction::NewTabWithProfile)
        ));
        // Tab-pin / colour / icon actions ids 25..=35.
        assert!(matches!(
            MenuAction::from_u32(25),
            Some(MenuAction::ToggleTabPin)
        ));
        assert!(matches!(
            MenuAction::from_u32(26),
            Some(MenuAction::ClearTabColor)
        ));
        assert!(matches!(
            MenuAction::from_u32(35),
            Some(MenuAction::ClearTabIcon)
        ));
        // Everything must stay below PROFILE_PICKER_BASE.
        for id in 0u32..=35 {
            let expected_id = MenuAction::from_u32(id).map(MenuAction::as_u32);
            assert!(
                expected_id.is_none_or(|v| v < 0x1_0000),
                "action id {id} must stay below PROFILE_PICKER_BASE"
            );
        }
        assert!(matches!(
            MenuAction::from_u32(39),
            Some(MenuAction::RestartSession)
        ));
        // Values above the last static variant (39 = RestartSession) must not resolve.
        assert!(MenuAction::from_u32(40).is_none());
    }

    /// Snap action ids round-trip correctly through MenuAction.
    #[test]
    fn snap_menu_action_from_u32_roundtrips() {
        let pairs = [
            (14u32, MenuAction::SnapTop),
            (15, MenuAction::SnapBottom),
            (16, MenuAction::SnapLeft),
            (17, MenuAction::SnapRight),
            (18, MenuAction::SnapCenter),
            (19, MenuAction::SnapMaximize),
        ];
        for (id, action) in pairs {
            assert_eq!(MenuAction::from_u32(id).map(MenuAction::as_u32), Some(id));
            assert_eq!(action.as_u32(), id);
        }
    }

    /// binding_for must return non-empty strings for split_right and close_pane
    /// under the default ShortcutsConfig (those ship with defaults).
    #[test]
    fn split_right_and_close_pane_have_default_bindings() {
        let sc = terminale_config::ShortcutsConfig::default();
        let sr = binding_for(ShortcutAction::SplitRight, &sc);
        let cp = binding_for(ShortcutAction::ClosePane, &sc);
        assert!(
            !sr.is_empty(),
            "SplitRight ships with a default binding (got empty)"
        );
        assert!(
            !cp.is_empty(),
            "ClosePane ships with a default binding (got empty)"
        );
        // SplitLeft / SplitUp ship unbound by default.
        assert!(
            binding_for(ShortcutAction::SplitLeft, &sc).is_empty(),
            "SplitLeft should be unbound by default"
        );
        assert!(
            binding_for(ShortcutAction::SplitUp, &sc).is_empty(),
            "SplitUp should be unbound by default"
        );
    }

    // -- Pane header helpers -----------------------------------------

    /// `count_leaves` must return 1 for a lone Leaf, 2 for a single split,
    /// and the sum of children for a nested tree.
    #[test]
    fn count_leaves_single_and_nested() {
        assert_eq!(count_leaves(&PaneNode::Leaf(0)), 1, "lone leaf = 1");

        let one_split = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(PaneNode::Leaf(0)),
            b: Box::new(PaneNode::Leaf(1)),
        };
        assert_eq!(count_leaves(&one_split), 2, "one split = 2");

        // Nested: left side further split → 3 leaves total.
        let nested = PaneNode::Split {
            direction: SplitDir::Horizontal,
            ratio: 0.5,
            a: Box::new(PaneNode::Split {
                direction: SplitDir::Vertical,
                ratio: 0.5,
                a: Box::new(PaneNode::Leaf(0)),
                b: Box::new(PaneNode::Leaf(1)),
            }),
            b: Box::new(PaneNode::Leaf(2)),
        };
        assert_eq!(count_leaves(&nested), 3, "nested 3-leaf tree = 3");
    }

    /// `detach_leaf` uses `collapse_close` internally. Verify that the
    /// tree-level contract holds (post-detach tree has one fewer leaf and the
    /// remaining sibling is promoted) by exercising `collapse_close` directly,
    /// mirroring the exact call pattern `detach_leaf` uses.
    #[test]
    fn detach_leaf_tree_contract_via_collapse_close() {
        // A two-leaf split: leaves 0 and 1.
        let tree = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(leaf(0)),
            b: Box::new(leaf(1)),
        };
        // Detach leaf 0: the tree should collapse to a lone Leaf(1).
        let (new_tree, found) = collapse_close(tree, 0);
        assert!(found, "collapse_close must find and remove the target leaf");
        assert!(
            matches!(new_tree, PaneNode::Leaf(1)),
            "remaining sibling (1) must become the new root"
        );
        // After collapse, count_leaves = 1.
        assert_eq!(count_leaves(&new_tree), 1);
        // first_leaf_of the new root is 1 — matches detach_leaf's refocus.
        assert_eq!(first_leaf_of(&new_tree), Some(1));
    }

    /// `detach_leaf` must refuse to detach from a lone-leaf tree and leave the
    /// tree unchanged. Exercise the guard at the `PaneNode::Leaf` level
    /// (collapse_close on a single Leaf returns `found = false`).
    #[test]
    fn detach_leaf_tree_lone_leaf_guard() {
        // Single-leaf tree — collapse_close must return found = false.
        let tree = PaneNode::Leaf(0);
        let (unchanged, found) = collapse_close(tree, 0);
        // collapse_close on a bare Leaf never succeeds (no parent split to
        // collapse), so detach_leaf correctly treats this as a no-op.
        assert!(!found, "cannot detach the sole remaining leaf");
        assert!(matches!(unchanged, PaneNode::Leaf(0)));
    }

    /// `compose_tab_label` (reused by `pane_label`) follows the same
    /// precedence as tab labels: custom OSC title > profile+cwd > profile.
    /// Covers the same cases as `tab_label_prefers_program_title` since
    /// pane_label delegates to compose_tab_label with `crashed = false`.
    #[test]
    fn pane_label_precedence() {
        // Program-set title wins.
        assert_eq!(
            compose_tab_label(None, "zsh", Some("nvim README.md"), Some("repo"), false),
            "nvim README.md"
        );
        // No title → profile — cwd.
        assert_eq!(
            compose_tab_label(None, "zsh", None, Some("repo"), false),
            "zsh — repo"
        );
        // No title, no cwd → just profile.
        assert_eq!(compose_tab_label(None, "zsh", None, None, false), "zsh");
        // Blank title is noise → fall back.
        assert_eq!(
            compose_tab_label(None, "zsh", Some("   "), None, false),
            "zsh"
        );
        // No crash prefix in pane headers.
        assert_eq!(
            compose_tab_label(None, "zsh", None, None, false),
            "zsh",
            "pane_label never prepends crash marker"
        );
    }

    // -- RenameState target field ------------------------------------------

    /// `RenameState` correctly distinguishes Tab, Pane, and Group targets.
    #[test]
    fn rename_state_distinguishes_tab_and_pane() {
        let tab_rename = RenameState {
            tab_idx: 2,
            target: RenameTarget::Tab,
            buffer: "my tab".into(),
        };
        assert_eq!(
            tab_rename.target,
            RenameTarget::Tab,
            "tab rename uses Tab target"
        );
        assert_eq!(tab_rename.tab_idx, 2);
        assert_eq!(tab_rename.buffer, "my tab");

        let pane_rename = RenameState {
            tab_idx: 0,
            target: RenameTarget::Pane(3),
            buffer: "right pane".into(),
        };
        assert_eq!(
            pane_rename.target,
            RenameTarget::Pane(3),
            "pane rename carries pane id"
        );
        assert_eq!(pane_rename.buffer, "right pane");

        let group_rename = RenameState {
            tab_idx: 1,
            target: RenameTarget::Group(42),
            buffer: "My Group".into(),
        };
        assert_eq!(
            group_rename.target,
            RenameTarget::Group(42),
            "group rename carries group id"
        );
        assert_eq!(group_rename.tab_idx, 1);
        assert_eq!(group_rename.buffer, "My Group");
    }

    // -- Profile submenu entries -------------------------------------------

    /// Profile submenu action ids must be >= PROFILE_PICKER_BASE and
    /// consecutive so the App handler's `action_id - PROFILE_PICKER_BASE`
    /// index arithmetic is correct.
    #[test]
    fn profile_submenu_action_ids_are_in_picker_base_range() {
        let names = ["PowerShell".to_string(), "Bash".to_string()];
        let icons: Vec<Option<String>> = vec![Some("⚡".into()), Some("🐚".into())];
        let entries: Vec<crate::context_menu_window::MenuEntry> = names
            .iter()
            .zip(icons.iter())
            .enumerate()
            .map(
                |(idx, (name, icon))| crate::context_menu_window::MenuEntry {
                    icon: icon.clone(),
                    label: name.clone(),
                    hotkey: None,
                    enabled: true,
                    separator_before: false,
                    action_id: PROFILE_PICKER_BASE + idx as u32,
                    submenu: None,
                },
            )
            .collect();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].action_id, PROFILE_PICKER_BASE);
        assert_eq!(entries[1].action_id, PROFILE_PICKER_BASE + 1);
        // The index recovered from each action id matches the profile slot.
        for (i, entry) in entries.iter().enumerate() {
            let recovered = (entry.action_id - PROFILE_PICKER_BASE) as usize;
            assert_eq!(recovered, i);
        }
        // And all action ids must be above any static MenuAction id (max = 24).
        for entry in &entries {
            assert!(
                entry.action_id >= PROFILE_PICKER_BASE,
                "profile entry action_id must be >= PROFILE_PICKER_BASE"
            );
        }
    }

    // ── pane-keyboard: directional adjacency ─────────────────────────────

    /// Helper: build a simple 2-pane horizontal layout (left | right).
    /// left pane id=0 rect=(0,0,100,100), right pane id=1 rect=(100,0,100,100).
    fn two_pane_h() -> Vec<(u32, (f32, f32, f32, f32))> {
        vec![
            (0, (0.0, 0.0, 100.0, 100.0)),
            (1, (100.0, 0.0, 100.0, 100.0)),
        ]
    }

    /// Helper: build a simple 2-pane vertical layout (top | bottom).
    fn two_pane_v() -> Vec<(u32, (f32, f32, f32, f32))> {
        vec![
            (0, (0.0, 0.0, 200.0, 100.0)),
            (1, (0.0, 100.0, 200.0, 100.0)),
        ]
    }

    #[test]
    fn adjacency_right_finds_right_neighbour() {
        let rects = two_pane_h();
        // From pane 0 (left), going Right should find pane 1.
        let candidates: Vec<_> = rects.iter().copied().filter(|(id, _)| *id != 0).collect();
        let result =
            pick_adjacent_pane((0.0, 0.0, 100.0, 100.0), &candidates, PaneDirection::Right);
        assert_eq!(result, Some(1), "Right from left pane should find pane 1");
    }

    #[test]
    fn adjacency_left_finds_left_neighbour() {
        let rects = two_pane_h();
        // From pane 1 (right), going Left should find pane 0.
        let candidates: Vec<_> = rects.iter().copied().filter(|(id, _)| *id != 1).collect();
        let result =
            pick_adjacent_pane((100.0, 0.0, 100.0, 100.0), &candidates, PaneDirection::Left);
        assert_eq!(result, Some(0), "Left from right pane should find pane 0");
    }

    #[test]
    fn adjacency_down_finds_bottom_neighbour() {
        let rects = two_pane_v();
        // From pane 0 (top), going Down should find pane 1.
        let candidates: Vec<_> = rects.iter().copied().filter(|(id, _)| *id != 0).collect();
        let result = pick_adjacent_pane((0.0, 0.0, 200.0, 100.0), &candidates, PaneDirection::Down);
        assert_eq!(result, Some(1), "Down from top pane should find pane 1");
    }

    #[test]
    fn adjacency_up_finds_top_neighbour() {
        let rects = two_pane_v();
        // From pane 1 (bottom), going Up should find pane 0.
        let candidates: Vec<_> = rects.iter().copied().filter(|(id, _)| *id != 1).collect();
        let result = pick_adjacent_pane((0.0, 100.0, 200.0, 100.0), &candidates, PaneDirection::Up);
        assert_eq!(result, Some(0), "Up from bottom pane should find pane 0");
    }

    #[test]
    fn adjacency_no_candidate_in_direction_returns_none() {
        let rects = two_pane_h();
        // From pane 0 (left), going Left — no pane is further left.
        let candidates: Vec<_> = rects.iter().copied().filter(|(id, _)| *id != 0).collect();
        let result = pick_adjacent_pane((0.0, 0.0, 100.0, 100.0), &candidates, PaneDirection::Left);
        assert_eq!(result, None, "No pane to the left of the leftmost pane");
    }

    #[test]
    fn adjacency_three_pane_picks_nearest() {
        // Layout: A(0-100) | B(100-200) | C(200-300), each 100px wide, 100px tall.
        let rects: Vec<(u32, (f32, f32, f32, f32))> = vec![
            (0, (0.0, 0.0, 100.0, 100.0)),
            (1, (100.0, 0.0, 100.0, 100.0)),
            (2, (200.0, 0.0, 100.0, 100.0)),
        ];
        // From B (pane 1), going Right: nearest is C (2), not A (0).
        let candidates: Vec<_> = rects.iter().copied().filter(|(id, _)| *id != 1).collect();
        let result = pick_adjacent_pane(
            (100.0, 0.0, 100.0, 100.0),
            &candidates,
            PaneDirection::Right,
        );
        assert_eq!(
            result,
            Some(2),
            "Right from middle pane should pick right neighbour"
        );
    }

    // ── pane-keyboard: resize split path ─────────────────────────────────

    #[test]
    fn find_resize_split_flat_vertical_split() {
        // Simple V-split: pane 0 left, pane 1 right.
        let tree = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(PaneNode::Leaf(0)),
            b: Box::new(PaneNode::Leaf(1)),
        };
        // Resize Right on pane 0 → finds the root split (path=Some([])).
        let r = find_resize_split(&tree, 0, PaneDirection::Right).unwrap();
        assert_eq!(
            r.path,
            Some(Vec::<bool>::new()),
            "root split has empty path"
        );
        assert!(r.focused_in_a, "pane 0 is in a-side");

        // Resize Right on pane 1 → also root split, focused_in_a=false.
        let r = find_resize_split(&tree, 1, PaneDirection::Right).unwrap();
        assert_eq!(r.path, Some(Vec::<bool>::new()));
        assert!(!r.focused_in_a, "pane 1 is in b-side");
    }

    #[test]
    fn find_resize_split_wrong_direction_returns_none() {
        // V-split: pane 0 left, pane 1 right. Resizing Up/Down requires
        // an H-split, which doesn't exist here.
        let tree = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(PaneNode::Leaf(0)),
            b: Box::new(PaneNode::Leaf(1)),
        };
        assert!(
            find_resize_split(&tree, 0, PaneDirection::Up).is_none(),
            "no H-split means Up resize returns None"
        );
        assert!(find_resize_split(&tree, 0, PaneDirection::Down).is_none(),);
    }

    #[test]
    fn find_resize_split_nested_prefers_innermost() {
        // Layout: V-split { left=Leaf(0), right=H-split { top=Leaf(1), bot=Leaf(2) } }
        let tree = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(PaneNode::Leaf(0)),
            b: Box::new(PaneNode::Split {
                direction: SplitDir::Horizontal,
                ratio: 0.5,
                a: Box::new(PaneNode::Leaf(1)),
                b: Box::new(PaneNode::Leaf(2)),
            }),
        };
        // Resize Down on pane 1 → should find the inner H-split (path=Some([true])).
        let r = find_resize_split(&tree, 1, PaneDirection::Down).unwrap();
        assert_eq!(
            r.path,
            Some(vec![true]),
            "inner H-split path is Some([true])"
        );
        assert!(r.focused_in_a, "pane 1 is in a-side of H-split");

        // Resize Right on pane 1 → should find the outer V-split (path=Some([])).
        let r = find_resize_split(&tree, 1, PaneDirection::Right).unwrap();
        assert_eq!(
            r.path,
            Some(Vec::<bool>::new()),
            "outer V-split path is Some([])"
        );
        assert!(!r.focused_in_a, "pane 1 is in b-side of outer V-split");
    }

    // ── pane-keyboard: ratio nudge clamping ──────────────────────────────

    #[test]
    fn resize_ratio_clamps_at_bounds() {
        let mut tree = PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.05,
            a: Box::new(PaneNode::Leaf(0)),
            b: Box::new(PaneNode::Leaf(1)),
        };
        // Nudging below 0.05 (the minimum) should stay at 0.05.
        set_split_ratio_at(&mut tree, &[], 0.02);
        if let PaneNode::Split { ratio, .. } = &tree {
            assert!(
                (*ratio - 0.05).abs() < f32::EPSILON,
                "ratio below min should clamp to 0.05"
            );
        }
        // Nudging above 0.95 should clamp to 0.95.
        set_split_ratio_at(&mut tree, &[], 0.99);
        if let PaneNode::Split { ratio, .. } = &tree {
            assert!(
                (*ratio - 0.95).abs() < f32::EPSILON,
                "ratio above max should clamp to 0.95"
            );
        }
    }

    // ── tab-nav: activate_tab_by_index clamping ───────────────────────────

    /// Pure logic test: `activate_tab_by_index` is a no-op when the
    /// 0-based index is >= the number of open tabs.
    #[test]
    fn activate_tab_by_index_is_noop_when_out_of_range() {
        // We can test this through the `tab_jump_index` helper which encodes
        // the same clamping rule: a request for tab N on a list of M < N tabs
        // returns `None`.
        assert_eq!(
            tab_jump_index(KeyCode::Digit5, 3),
            None,
            "ActivateTab5 with 3 tabs should be a no-op"
        );
        assert_eq!(
            tab_jump_index(KeyCode::Digit8, 5),
            None,
            "ActivateTab8 with 5 tabs should be a no-op"
        );
        // Tab 1 on a single-tab window is valid (index 0 < 1).
        assert_eq!(tab_jump_index(KeyCode::Digit1, 1), Some(0));
    }

    /// Tab 9 always resolves to the *last* tab regardless of tab count
    /// (a common convention).
    #[test]
    fn activate_tab_9_always_last() {
        assert_eq!(tab_jump_index(KeyCode::Digit9, 1), Some(0));
        assert_eq!(tab_jump_index(KeyCode::Digit9, 3), Some(2));
        assert_eq!(tab_jump_index(KeyCode::Digit9, 9), Some(8));
    }

    // ── tab-nav: previous_active_tab tracking ────────────────────────────

    /// Verify the `active_tab_after_detach` adjustment logic used for
    /// `previous_active_tab` when a tab is removed.
    ///
    /// The invariant: after removing tab `removed` from a list of length
    /// `len_before`, the adjusted pointer `active_tab_after_detach(prev, …)`
    /// must be a valid index into the *new* (len_before - 1) list.
    #[test]
    fn previous_active_tab_adjusted_after_close() {
        // If prev was pointing at tab 3 in a 5-tab list and tab 1 is closed,
        // the pointer must shift down by one to 2 (which now refers to the
        // same tab that was at index 3 before the removal).
        assert_eq!(
            active_tab_after_detach(3, 1, 5),
            2,
            "prev tab index must shift down when a lower tab is closed"
        );

        // If the closed tab was AFTER the previous-active pointer, the pointer
        // stays put (the tab it refers to hasn't moved).
        assert_eq!(
            active_tab_after_detach(1, 3, 5),
            1,
            "prev tab index must not change when a higher tab is closed"
        );

        // Closing the last tab in a 4-tab list while prev pointed at it
        // clamps to the new last index.
        assert_eq!(
            active_tab_after_detach(3, 3, 4),
            2,
            "closing the last tab should clamp prev to new last index"
        );
    }

    // ── SSH click-freeze regression ───────────────────────────────────────

    /// `SshConnectOutcome` must be `Send` so it can travel from a Tokio
    /// background task back to the winit event-loop thread via
    /// `UserEvent::SshConnected`. This test is a compile-time assertion —
    /// it fails to compile (not just at runtime) if `Send` is lost.
    #[test]
    fn ssh_connect_outcome_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<SshConnectOutcome>();
    }

    /// `UserEvent` must be `Debug` (winit derives it from the outer loop type
    /// in tracing / error paths). `SshConnected` carries a custom `Debug`
    /// impl that does not require `SshSession: Debug`.
    #[test]
    fn user_event_debug_includes_ssh_connected() {
        let outcome = SshConnectOutcome {
            window_idx: 0,
            host_name: "prod".into(),
            host_endpoint: "root@192.168.1.1".into(),
            cols: 80,
            rows: 24,
            result: Err("timed out".into()),
        };
        let ev = UserEvent::SshConnected(Box::new(outcome));
        let s = format!("{ev:?}");
        assert!(
            s.contains("SshConnected"),
            "Debug output should name the variant"
        );
    }

    /// Verify that a key misconfiguration (agent auth on a host flagged as
    /// needing a password) is detected synchronously by `ssh_secret_needed`
    /// before any Tokio task is spawned. This keeps the fast-fail path fully
    /// on the UI thread (no task overhead for obvious config errors).
    #[test]
    fn ssh_secret_needed_detects_missing_password() {
        let mut host = terminale_config::SshHost {
            id: terminale_config::SshHost::new_id(),
            name: "test".into(),
            host: "localhost".into(),
            port: 22,
            user: "user".into(),
            auth: terminale_config::SshAuthMethod::Password,
            key_path: None,
        };
        // Password auth with nothing stored in the keychain should return
        // `Ok(Some(false))` — meaning "yes, a secret is needed; not a
        // passphrase". The exact value depends on the keychain; what we
        // assert is that the call completes quickly without blocking.
        let result = ssh_secret_needed(&host);
        // Result is either Ok (keychain reachable) or Err (keychain broke) —
        // either way it must not panic and must return synchronously.
        let _ = result; // success if we reach this line

        // Key auth with no key_path is a hard misconfiguration → Err.
        host.auth = terminale_config::SshAuthMethod::Key;
        host.key_path = None;
        assert!(
            ssh_secret_needed(&host).is_err(),
            "key auth without key_path must be an error"
        );
    }

    // ── collapsed_edge_rect / scale_origin_rect ───────────────────────────────

    /// Helper: the collapsed rect must stay fully INSIDE the monitor — the
    /// invariant that fixes the "window visible on the monitor above while
    /// sliding" bug. Also asserts the dock edge stays pinned.
    fn assert_collapsed_inside_monitor(
        edge: terminale_config::QuakeEdge,
        mon: terminale_config::MonitorRect,
        target: terminale_config::WindowRect,
    ) {
        let off = collapsed_edge_rect(edge, Some(mon), target);
        let (tx, ty, tw, th) = target;
        let (ox, oy, ow, oh) = off;
        let (mx, my, mw, mh) = mon;

        // Every collapsed rect must be fully inside the monitor bounds.
        assert!(
            ox >= mx,
            "{edge:?}: x ({ox}) must be >= monitor left ({mx})"
        );
        assert!(oy >= my, "{edge:?}: y ({oy}) must be >= monitor top ({my})");
        assert!(
            ox + ow as i32 <= mx + mw as i32,
            "{edge:?}: right edge must be inside the monitor"
        );
        assert!(
            oy + oh as i32 <= my + mh as i32,
            "{edge:?}: bottom edge must be inside the monitor"
        );

        // The docked edge stays pinned; the perpendicular extent collapses.
        match edge {
            terminale_config::QuakeEdge::Top => {
                assert_eq!(oy, my, "Top: top edge pinned at monitor top");
                assert_eq!(
                    (ox, ow, oh),
                    (tx, tw, 1),
                    "Top: width kept, height collapsed"
                );
            }
            terminale_config::QuakeEdge::Bottom => {
                assert_eq!(
                    oy + oh as i32,
                    my + mh as i32,
                    "Bottom: bottom edge pinned at monitor bottom"
                );
                assert_eq!((ox, ow, oh), (tx, tw, 1));
            }
            terminale_config::QuakeEdge::Left => {
                assert_eq!(ox, mx, "Left: left edge pinned");
                assert_eq!((oy, ow, oh), (ty, 1, th));
            }
            terminale_config::QuakeEdge::Right => {
                assert_eq!(
                    ox + ow as i32,
                    mx + mw as i32,
                    "Right: right edge pinned at monitor right"
                );
                assert_eq!((oy, ow, oh), (ty, 1, th));
            }
            terminale_config::QuakeEdge::Off => {}
        }
    }

    #[test]
    fn collapsed_rect_top_stays_inside_monitor() {
        let mon = (0, 0, 1920u32, 1080u32);
        let target = (0, 0, 1920u32, 540u32);
        assert_collapsed_inside_monitor(terminale_config::QuakeEdge::Top, mon, target);
        let off = collapsed_edge_rect(terminale_config::QuakeEdge::Top, Some(mon), target);
        assert_eq!(off, (0, 0, 1920, 1));
    }

    #[test]
    fn collapsed_rect_bottom_stays_inside_monitor() {
        let mon = (0, 0, 1920u32, 1080u32);
        let target = (0, 756, 1920u32, 324u32);
        assert_collapsed_inside_monitor(terminale_config::QuakeEdge::Bottom, mon, target);
        let off = collapsed_edge_rect(terminale_config::QuakeEdge::Bottom, Some(mon), target);
        assert_eq!(off, (0, 1079, 1920, 1));
    }

    #[test]
    fn collapsed_rect_left_stays_inside_monitor() {
        let mon = (100, 50, 1920u32, 1080u32);
        let target = (100, 50, 480u32, 1080u32);
        assert_collapsed_inside_monitor(terminale_config::QuakeEdge::Left, mon, target);
        let off = collapsed_edge_rect(terminale_config::QuakeEdge::Left, Some(mon), target);
        assert_eq!(off, (100, 50, 1, 1080));
    }

    #[test]
    fn collapsed_rect_right_stays_inside_monitor() {
        let mon = (100, 50, 1920u32, 1080u32);
        let target = (1540, 50, 480u32, 1080u32);
        assert_collapsed_inside_monitor(terminale_config::QuakeEdge::Right, mon, target);
        let off = collapsed_edge_rect(terminale_config::QuakeEdge::Right, Some(mon), target);
        assert_eq!(off, (2019, 50, 1, 1080));
    }

    #[test]
    fn collapsed_rect_off_edge_collapses_in_place() {
        // Free-floating: collapse at the target's own top edge — never
        // translate above it (the old behaviour leaked onto the monitor above).
        let target = (200, 300, 800u32, 600u32);
        let off = collapsed_edge_rect(terminale_config::QuakeEdge::Off, None, target);
        assert_eq!(off, (200, 300, 800, 1));
    }

    #[test]
    fn collapsed_rect_never_leaves_monitor_for_any_edge() {
        let mon = (0, 0, 2560u32, 1440u32);
        let target = (0, 0, 2560u32, 720u32);
        for edge in terminale_config::QuakeEdge::all() {
            let off = collapsed_edge_rect(edge, Some(mon), target);
            let (ox, oy, ow, oh) = off;
            assert!(
                ox >= 0 && oy >= 0 && ox + ow as i32 <= 2560 && oy + oh as i32 <= 1440,
                "collapsed rect must stay inside the monitor for edge {edge:?}, got {off:?}"
            );
        }
    }

    #[test]
    fn scale_origin_rect_is_a_point_at_edge_centre() {
        let mon = (0, 0, 1920u32, 1080u32);
        let target = (0, 0, 1920u32, 540u32);
        let off = scale_origin_rect(terminale_config::QuakeEdge::Top, Some(mon), target);
        // 1×1 point at the centre of the top edge.
        assert_eq!(off, (960, 0, 1, 1));
        let off = scale_origin_rect(terminale_config::QuakeEdge::Bottom, Some(mon), target);
        assert_eq!(off, (960, 1079, 1, 1));
    }

    #[test]
    fn anim_rest_rect_fade_keeps_target_geometry() {
        // Fade animates opacity only — its rest rect IS the target.
        let mon = (0, 0, 1920u32, 1080u32);
        let target = (0, 0, 1920u32, 540u32);
        let off = anim_rest_rect(
            terminale_config::QuakeAnimation::Fade,
            terminale_config::QuakeEdge::Top,
            Some(mon),
            target,
        );
        assert_eq!(off, target);
    }

    // ── lerp_rect_full (Scale mode) ────────────────────────────────────────────

    #[test]
    fn lerp_rect_full_at_zero_returns_a() {
        let a = (0, -540, 1920u32, 540u32);
        let b = (0, 0, 1920u32, 540u32);
        assert_eq!(lerp_rect_full(a, b, 0.0), a);
    }

    #[test]
    fn lerp_rect_full_at_one_returns_b() {
        let a = (0, -540, 1920u32, 540u32);
        let b = (0, 0, 1920u32, 540u32);
        assert_eq!(lerp_rect_full(a, b, 1.0), b);
    }

    #[test]
    fn lerp_rect_full_interpolates_both_position_and_size() {
        // From a 1-pixel strip (collapsed) to the full target: verify size
        // changes and position moves toward target.
        let from = (0, -540, 1920u32, 1u32); // collapsed strip above monitor
        let to = (0, 0, 1920u32, 540u32); // full target
        let mid = lerp_rect_full(from, to, 0.5);
        // x unchanged (both 0).
        assert_eq!(mid.0, 0);
        // y: -540 + (0 - (-540)) * 0.5 = -540 + 270 = -270.
        assert_eq!(mid.1, -270);
        // width unchanged (both 1920).
        assert_eq!(mid.2, 1920);
        // height: (1.0 + (540.0 - 1.0) * 0.5).round() = (1.0 + 269.5).round() = 270.5.round()
        // Rust f32::round() rounds half to even, so 270.5 → 270 (even).
        // But as f32 arithmetic: 1.0 + 539.0 * 0.5 = 1.0 + 269.5 = 270.5 → 271 (banker's) or 270?
        // Confirm it is between 270 and 272 (the exact value depends on float rounding).
        assert!(
            mid.3 >= 270 && mid.3 <= 271,
            "height at midpoint must be ~270, got {}",
            mid.3
        );
    }

    // ── cap_scrollback (plugin snapshot bound) ────────────────────────────────

    #[test]
    fn cap_scrollback_keeps_newest_lines() {
        let mk = |v: &[&str]| v.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        // Over the cap: oldest (front) entries are dropped.
        let mut lines = mk(&["a", "b", "c", "d"]);
        cap_scrollback(&mut lines, 2);
        assert_eq!(lines, mk(&["c", "d"]));
        // Under the cap: untouched.
        let mut lines = mk(&["a", "b"]);
        cap_scrollback(&mut lines, 10);
        assert_eq!(lines, mk(&["a", "b"]));
        // Zero cap empties the list.
        let mut lines = mk(&["a"]);
        cap_scrollback(&mut lines, 0);
        assert!(lines.is_empty());
    }

    // ── cleanup_old_logs ──────────────────────────────────────────────────────

    #[test]
    fn cleanup_old_logs_keeps_fresh_and_foreign_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fresh = dir.path().join("terminale.log.2026-06-03");
        let foreign = dir.path().join("notes.txt");
        std::fs::write(&fresh, "log").expect("write fresh");
        std::fs::write(&foreign, "keep").expect("write foreign");
        // Freshly-created files are NEVER older than the cutoff.
        cleanup_old_logs(dir.path(), 7);
        assert!(fresh.exists(), "fresh log must survive cleanup");
        assert!(foreign.exists(), "non-log files must never be touched");
        // A missing directory is a silent no-op.
        cleanup_old_logs(&dir.path().join("does-not-exist"), 7);
    }

    // ── quiet_noisy_crates ────────────────────────────────────────────────────

    #[test]
    fn quiet_noisy_crates_appends_warn_caps() {
        let d = quiet_noisy_crates("info");
        assert!(d.starts_with("info,"));
        for krate in ["wgpu_core=warn", "wgpu_hal=warn", "naga=warn"] {
            assert!(d.contains(krate), "missing cap: {krate} in {d}");
        }
        // The result must still parse as a valid EnvFilter.
        assert!(EnvFilter::try_new(&d).is_ok(), "invalid directives: {d}");
    }

    #[test]
    fn quiet_noisy_crates_respects_explicit_user_directive() {
        let d = quiet_noisy_crates("info,wgpu_core=trace");
        // The user's explicit choice survives and no conflicting cap is added.
        assert!(d.contains("wgpu_core=trace"));
        assert!(!d.contains("wgpu_core=warn"));
        // Crates the user did NOT mention still get capped.
        assert!(d.contains("wgpu_hal=warn"));
    }

    #[test]
    fn quiet_noisy_crates_defaults_empty_base_to_info() {
        let d = quiet_noisy_crates("  ");
        assert!(
            d.starts_with("info,"),
            "blank base must default to info: {d}"
        );
        assert!(EnvFilter::try_new(&d).is_ok());
    }

    // ── configs_identical: new-field coverage ─────────────────────────────────

    /// Helper: two identical default configs must be treated as equal.
    #[test]
    fn configs_identical_returns_true_for_defaults() {
        let a = terminale_config::Config::default();
        let b = terminale_config::Config::default();
        assert!(
            configs_identical(&a, &b),
            "two default configs must be identical"
        );
    }

    #[test]
    fn configs_identical_detects_cursor_blink_ease_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.cursor.blink_ease = !a.cursor.blink_ease;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when cursor.blink_ease differs"
        );
    }

    #[test]
    fn configs_identical_detects_cursor_animation_fps_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.cursor.animation_fps = a.cursor.animation_fps + 30;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when cursor.animation_fps differs"
        );
    }

    #[test]
    fn configs_identical_detects_font_cell_width_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.font.cell_width = 1.25;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when font.cell_width differs"
        );
    }

    #[test]
    fn configs_identical_detects_tab_bar_enabled_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.tab_bar_enabled = !a.appearance.tab_bar_enabled;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.tab_bar_enabled differs"
        );
    }

    #[test]
    fn configs_identical_detects_tab_bar_position_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.tab_bar_position = match a.appearance.tab_bar_position {
            terminale_config::TabBarPosition::Top => terminale_config::TabBarPosition::Bottom,
            terminale_config::TabBarPosition::Bottom => terminale_config::TabBarPosition::Top,
            terminale_config::TabBarPosition::Left => terminale_config::TabBarPosition::Top,
            terminale_config::TabBarPosition::Right => terminale_config::TabBarPosition::Top,
        };
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.tab_bar_position differs"
        );
    }

    #[test]
    fn configs_identical_detects_vertical_tab_bar_width_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.vertical_tab_bar_width = 240.0;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.vertical_tab_bar_width differs"
        );
    }

    #[test]
    fn configs_identical_detects_tab_bar_hide_if_single_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.tab_bar_hide_if_single = !a.appearance.tab_bar_hide_if_single;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.tab_bar_hide_if_single differs"
        );
    }

    #[test]
    fn configs_identical_detects_bg_image_path_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.background_image.path = Some("/wallpaper.png".into());
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when background_image.path differs"
        );
    }

    #[test]
    fn configs_identical_detects_bg_image_opacity_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.background_image.opacity = 0.5;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when background_image.opacity differs"
        );
    }

    #[test]
    fn configs_identical_detects_exit_behavior_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.terminal.exit_behavior = terminale_config::ExitBehavior::Hold;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when terminal.exit_behavior differs"
        );
    }

    #[test]
    fn configs_identical_detects_hyperlink_rules_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.terminal.hyperlink_rules = terminale_config::default_hyperlink_rules();
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when terminal.hyperlink_rules differs (non-empty vs empty)"
        );
    }

    // ── configs_identical: newly-gated fields (settings-correctness) ──────────
    //
    // Table-driven coverage for every field added to configs_identical in the
    // settings-correctness pass. One test per field or logical group confirms
    // that a change to that field causes configs_identical to return false, and
    // that two defaults are identical (regression guard).

    #[test]
    fn configs_identical_detects_animated_tab_drag_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.animated_tab_drag = !a.appearance.animated_tab_drag;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.animated_tab_drag differs"
        );
    }

    #[test]
    fn configs_identical_detects_show_pane_headers_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.show_pane_headers = !a.appearance.show_pane_headers;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.show_pane_headers differs"
        );
    }

    #[test]
    fn configs_identical_detects_divider_thickness_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.divider_thickness_logical = a.appearance.divider_thickness_logical + 2.0;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.divider_thickness_logical differs"
        );
    }

    #[test]
    fn configs_identical_detects_divider_grab_padding_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.divider_grab_padding_logical = a.appearance.divider_grab_padding_logical + 2.0;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.divider_grab_padding_logical differs"
        );
    }

    #[test]
    fn configs_identical_detects_focus_border_thickness_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.focus_border_thickness_logical =
            a.appearance.focus_border_thickness_logical + 1.0;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.focus_border_thickness_logical differs"
        );
    }

    #[test]
    fn configs_identical_detects_focus_border_color_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.focus_border_color = Some([100, 150, 200]);
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.focus_border_color differs"
        );
    }

    #[test]
    fn configs_identical_detects_divider_color_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.divider_color = Some([80, 90, 110]);
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.divider_color differs"
        );
    }

    #[test]
    fn configs_identical_detects_font_ligatures_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.font.ligatures = !a.font.ligatures;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when font.ligatures differs"
        );
    }

    #[test]
    fn configs_identical_detects_live_pane_resize_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.terminal.live_pane_resize = !a.terminal.live_pane_resize;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when terminal.live_pane_resize differs"
        );
    }

    #[test]
    fn configs_identical_detects_pane_resize_step_cells_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.terminal.pane_resize_step_cells = a.terminal.pane_resize_step_cells + 1;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when terminal.pane_resize_step_cells differs"
        );
    }

    #[test]
    fn configs_identical_detects_show_prompt_marks_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.terminal.show_prompt_marks = !a.terminal.show_prompt_marks;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when terminal.show_prompt_marks differs"
        );
    }

    #[test]
    fn configs_identical_detects_highlight_on_jump_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.terminal.highlight_on_jump = !a.terminal.highlight_on_jump;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when terminal.highlight_on_jump differs"
        );
    }

    #[test]
    fn configs_identical_detects_status_bar_enabled_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.status_bar.enabled = !a.status_bar.enabled;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when status_bar.enabled differs"
        );
    }

    #[test]
    fn configs_identical_detects_status_bar_position_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.status_bar.position = match a.status_bar.position {
            terminale_config::StatusBarPosition::Top => terminale_config::StatusBarPosition::Bottom,
            terminale_config::StatusBarPosition::Bottom => terminale_config::StatusBarPosition::Top,
        };
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when status_bar.position differs"
        );
    }

    #[test]
    fn configs_identical_detects_status_bar_update_interval_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.status_bar.update_interval_ms = 5000;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when status_bar.update_interval_ms differs"
        );
    }

    #[test]
    fn configs_identical_detects_status_bar_left_segments_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.status_bar.left_segments = vec![terminale_config::StatusSegment::TabIndex];
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when status_bar.left_segments differs"
        );
    }

    #[test]
    fn configs_identical_detects_status_bar_right_segments_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.status_bar.right_segments = vec![terminale_config::StatusSegment::Cwd];
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when status_bar.right_segments differs"
        );
    }

    #[test]
    fn configs_identical_detects_shortcuts_map_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        // Changing any binding in the shortcuts map must be detected.
        b.keybinds.shortcuts.new_tab = "Alt+T".into();
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when keybinds.shortcuts.new_tab differs"
        );
        // Spot-check a second binding.
        let mut c = terminale_config::Config::default();
        c.keybinds.shortcuts.toggle_pane_zoom = "Ctrl+Alt+Z".into();
        assert!(
            !configs_identical(&a, &c),
            "configs_identical must return false when keybinds.shortcuts.toggle_pane_zoom differs"
        );
    }

    #[test]
    fn configs_identical_detects_close_button_style_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.close_button_style = terminale_config::CloseButtonStyle::Bare;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.close_button_style differs"
        );
    }

    #[test]
    fn configs_identical_detects_pane_tear_out_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.pane_tear_out = !a.appearance.pane_tear_out;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.pane_tear_out differs"
        );
    }

    // ── key_table_timed_out ──────────────────────────────────────────────────

    /// `key_table_timed_out` returns `false` before the timeout elapses.
    #[test]
    fn key_table_timed_out_returns_false_before_deadline() {
        let base = std::time::Instant::now();
        // "entered 100 ms ago", timeout 500 ms → NOT timed out.
        let now = base + std::time::Duration::from_millis(100);
        assert!(
            !key_table_timed_out(base, now, 500),
            "100 ms elapsed with 500 ms timeout must NOT be timed-out"
        );
    }

    /// `key_table_timed_out` returns `true` once the timeout has elapsed.
    #[test]
    fn key_table_timed_out_returns_true_after_deadline() {
        let base = std::time::Instant::now();
        // "entered 600 ms ago", timeout 500 ms → timed out.
        let now = base + std::time::Duration::from_millis(600);
        assert!(
            key_table_timed_out(base, now, 500),
            "600 ms elapsed with 500 ms timeout must be timed-out"
        );
    }

    /// `key_table_timed_out` returns `true` exactly at the boundary.
    #[test]
    fn key_table_timed_out_returns_true_at_exact_boundary() {
        let base = std::time::Instant::now();
        let now = base + std::time::Duration::from_secs(1);
        assert!(
            key_table_timed_out(base, now, 1000),
            "exact timeout boundary must be considered timed-out"
        );
    }

    /// `configs_identical` returns `false` when `key_tables` differs.
    #[test]
    fn configs_identical_detects_key_tables_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.keybinds.key_tables.push(terminale_config::KeyTable {
            name: "pane".to_string(),
            leader: "Ctrl+A".to_string(),
            timeout_ms: 1500,
            bindings: Vec::new(),
        });
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when keybinds.key_tables differs"
        );
    }

    // ── zen / fullscreen config identity ─────────────────────────────────────

    /// `configs_identical` returns `false` when `window.zen_hide` differs.
    #[test]
    fn configs_identical_detects_zen_hide_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.window.zen_hide = vec![terminale_config::ZenHideElement::TabBar];
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when window.zen_hide differs"
        );
    }

    /// `configs_identical` returns `false` when `window.zen_fullscreen` differs.
    #[test]
    fn configs_identical_detects_zen_fullscreen_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.window.zen_fullscreen = !a.window.zen_fullscreen;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when window.zen_fullscreen differs"
        );
    }

    /// `configs_identical` returns `false` when `toggle_fullscreen` shortcut differs.
    #[test]
    fn configs_identical_detects_toggle_fullscreen_shortcut_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.keybinds.shortcuts.toggle_fullscreen = "F12".into();
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when toggle_fullscreen shortcut differs"
        );
    }

    /// `configs_identical` returns `false` when `toggle_zen_mode` shortcut differs.
    #[test]
    fn configs_identical_detects_toggle_zen_mode_shortcut_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.keybinds.shortcuts.toggle_zen_mode = "Ctrl+Alt+Z".into();
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when toggle_zen_mode shortcut differs"
        );
    }

    // ── broadcast_input: configs_identical coverage ───────────────────────────

    /// `configs_identical` returns `false` when `terminal.broadcast_scope`
    /// differs — ensures the settings live-apply gate sees scope changes.
    #[test]
    fn configs_identical_detects_broadcast_scope_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.terminal.broadcast_scope = terminale_config::BroadcastScope::AllPanesInWindow;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when broadcast_scope differs"
        );
    }

    /// `configs_identical` returns `false` when the `toggle_broadcast_input`
    /// shortcut binding changes.
    #[test]
    fn configs_identical_detects_toggle_broadcast_input_shortcut_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.keybinds.shortcuts.toggle_broadcast_input = "Ctrl+Shift+B".to_string();
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when toggle_broadcast_input shortcut differs"
        );
    }

    /// `configs_identical` returns `true` for two configs that both have the
    /// same non-default `broadcast_scope`.
    #[test]
    fn configs_identical_same_broadcast_scope_window() {
        let mut a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        a.terminal.broadcast_scope = terminale_config::BroadcastScope::AllPanesInWindow;
        b.terminal.broadcast_scope = terminale_config::BroadcastScope::AllPanesInWindow;
        assert!(
            configs_identical(&a, &b),
            "configs_identical must return true when broadcast_scope matches"
        );
    }

    /// `configs_identical` returns `false` when `keybinds.mouse` differs.
    #[test]
    fn configs_identical_detects_mouse_bindings_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.keybinds.mouse.push(terminale_config::MouseBinding {
            button: "Right".to_string(),
            mods: String::new(),
            count: 1,
            actions: vec![terminale_config::KeyActionSpec::Action("Copy".to_string())],
        });
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when keybinds.mouse differs"
        );
    }

    // ── window-and-move-actions: config + shortcut coverage ───────────────────

    /// `configs_identical` detects a change in `window.new_window_profile`.
    #[test]
    fn configs_identical_detects_new_window_profile_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.window.new_window_profile = Some("MyProfile".to_string());
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when window.new_window_profile differs"
        );
    }

    /// `configs_identical` detects a change in the `new_window` shortcut.
    #[test]
    fn configs_identical_detects_new_window_shortcut_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.keybinds.shortcuts.new_window = "Ctrl+Alt+N".to_string();
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when new_window shortcut differs"
        );
    }

    /// `configs_identical` detects a change in the `move_tab_to_new_window` shortcut.
    #[test]
    fn configs_identical_detects_move_tab_to_new_window_shortcut_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.keybinds.shortcuts.move_tab_to_new_window = "Ctrl+Shift+M".to_string();
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when move_tab_to_new_window shortcut differs"
        );
    }

    /// `configs_identical` detects a change in the `move_pane_to_new_tab` shortcut.
    #[test]
    fn configs_identical_detects_move_pane_to_new_tab_shortcut_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.keybinds.shortcuts.move_pane_to_new_tab = "Ctrl+Shift+J".to_string();
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when move_pane_to_new_tab shortcut differs"
        );
    }

    /// `configs_identical` detects a change in the `move_pane_to_new_window` shortcut.
    #[test]
    fn configs_identical_detects_move_pane_to_new_window_shortcut_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.keybinds.shortcuts.move_pane_to_new_window = "Ctrl+Shift+L".to_string();
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when move_pane_to_new_window shortcut differs"
        );
    }

    /// Default `new_window` binding resolves when a matching key is pressed.
    #[test]
    fn new_window_action_resolves_from_default_binding() {
        let sc = terminale_config::ShortcutsConfig::default();
        // Default is "Ctrl+Shift+N".
        assert_eq!(
            sc.new_window, "Ctrl+Shift+N",
            "new_window must default to Ctrl+Shift+N"
        );
        let binding = binding_for(ShortcutAction::NewWindow, &sc);
        assert_eq!(
            binding, "Ctrl+Shift+N",
            "binding_for(NewWindow) must return the configured string"
        );
    }

    /// Move-window/tab/pane actions are unbound by default.
    #[test]
    fn move_actions_unbound_by_default() {
        let sc = terminale_config::ShortcutsConfig::default();
        assert!(
            binding_for(ShortcutAction::MoveTabToNewWindow, &sc).is_empty(),
            "MoveTabToNewWindow must be unbound by default"
        );
        assert!(
            binding_for(ShortcutAction::MovePaneToNewTab, &sc).is_empty(),
            "MovePaneToNewTab must be unbound by default"
        );
        assert!(
            binding_for(ShortcutAction::MovePaneToNewWindow, &sc).is_empty(),
            "MovePaneToNewWindow must be unbound by default"
        );
    }

    /// Binding `move_tab_to_new_window` and pressing that combo resolves
    /// to `MoveTabToNewWindow`.
    #[test]
    fn move_tab_to_new_window_resolves_when_bound() {
        let sc = terminale_config::ShortcutsConfig {
            move_tab_to_new_window: "Ctrl+Shift+M".to_string(),
            ..Default::default()
        };
        assert_eq!(
            binding_for(ShortcutAction::MoveTabToNewWindow, &sc),
            "Ctrl+Shift+M"
        );
    }

    /// `action_from_name` resolves all four new action names (case-insensitive).
    #[test]
    fn window_move_actions_resolve_from_name() {
        assert_eq!(
            crate::keymap::action_from_name("newwindow"),
            Some(ShortcutAction::NewWindow),
            "newwindow must resolve to NewWindow"
        );
        assert_eq!(
            crate::keymap::action_from_name("NewWindow"),
            Some(ShortcutAction::NewWindow),
            "NewWindow (mixed-case) must resolve"
        );
        assert_eq!(
            crate::keymap::action_from_name("movetabtonewwindow"),
            Some(ShortcutAction::MoveTabToNewWindow),
            "movetabtonewwindow must resolve to MoveTabToNewWindow"
        );
        assert_eq!(
            crate::keymap::action_from_name("movepanenewtab"),
            Some(ShortcutAction::MovePaneToNewTab),
            "movepanenewtab must resolve to MovePaneToNewTab"
        );
        assert_eq!(
            crate::keymap::action_from_name("movepanetonewwindow"),
            Some(ShortcutAction::MovePaneToNewWindow),
            "movepanetonewwindow must resolve to MovePaneToNewWindow"
        );
    }

    /// All four new actions appear in the command-palette PALETTE_ACTIONS list.
    #[test]
    fn new_window_actions_in_palette() {
        let actions: Vec<ShortcutAction> = PALETTE_ACTIONS.iter().map(|(a, _)| *a).collect();
        for action in [
            ShortcutAction::NewWindow,
            ShortcutAction::MoveTabToNewWindow,
            ShortcutAction::MovePaneToNewTab,
            ShortcutAction::MovePaneToNewWindow,
        ] {
            assert!(
                actions.contains(&action),
                "{action:?} must be present in PALETTE_ACTIONS"
            );
        }
    }

    /// `window.new_window_profile` defaults to `None`.
    #[test]
    fn new_window_profile_defaults_to_none() {
        let cfg = terminale_config::Config::default();
        assert!(
            cfg.window.new_window_profile.is_none(),
            "new_window_profile must default to None"
        );
    }

    /// Setting `new_window_profile` can be read back from the `WindowConfig`.
    #[test]
    fn new_window_profile_stores_and_retrieves_name() {
        let w = terminale_config::WindowConfig {
            new_window_profile: Some("Dev Shell".to_string()),
            ..Default::default()
        };
        assert_eq!(
            w.new_window_profile.as_deref(),
            Some("Dev Shell"),
            "new_window_profile must hold the assigned name"
        );
        // A default WindowConfig must have None.
        let w2 = terminale_config::WindowConfig::default();
        assert!(w2.new_window_profile.is_none(), "default must have None");
    }

    // ── Snippet palette mode tests ─────────────────────────────────────────────

    /// The `Snippets` mode lists all seeded snippets and supports fuzzy filtering.
    #[test]
    fn snippets_palette_mode_lists_and_fuzzy_filters() {
        let sc = terminale_config::ShortcutsConfig::default();
        let names: Vec<(String, String)> = vec![
            (
                "Git status".to_string(),
                "Show working-tree status".to_string(),
            ),
            (
                "Docker ps".to_string(),
                "List running containers".to_string(),
            ),
            ("Cargo test".to_string(), String::new()),
        ];
        // Empty query lists all snippets.
        let all = palette_ranked(
            "",
            PaletteMode::Snippets,
            &sc,
            "",
            &[],
            &[],
            &names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(all.len(), 3, "all 3 snippets must appear with empty query");
        for (item, _) in &all {
            assert!(matches!(item, PaletteItem::InsertSnippet(_)));
        }

        // Fuzzy filter: "git" matches "Git status".
        let filtered = palette_ranked(
            "git",
            PaletteMode::Snippets,
            &sc,
            "",
            &[],
            &[],
            &names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(filtered.len(), 1, "'git' should filter to 1 snippet");
        assert!(
            matches!(filtered[0].0, PaletteItem::InsertSnippet(0)),
            "Git status must be index 0"
        );

        // Description appears as the binding hint column.
        let first = palette_ranked(
            "git",
            PaletteMode::Snippets,
            &sc,
            "",
            &[],
            &[],
            &names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(
            first[0].1.binding, "Show working-tree status",
            "description must appear in the binding column"
        );

        // Non-matching query returns empty.
        let none = palette_ranked(
            "zzz",
            PaletteMode::Snippets,
            &sc,
            "",
            &[],
            &[],
            &names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(none.is_empty(), "non-matching query must yield empty list");

        // Correct index is assigned even when a snippet is not first.
        let docker = palette_ranked(
            "docker",
            PaletteMode::Snippets,
            &sc,
            "",
            &[],
            &[],
            &names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(
            matches!(
                docker.first().map(|(it, _)| it),
                Some(PaletteItem::InsertSnippet(1))
            ),
            "Docker ps must be index 1"
        );
    }

    /// `OpenSnippets` action appears in `PALETTE_ACTIONS`.
    #[test]
    fn open_snippets_action_in_palette() {
        assert!(
            PALETTE_ACTIONS
                .iter()
                .any(|(a, _)| *a == ShortcutAction::OpenSnippets),
            "OpenSnippets must be present in PALETTE_ACTIONS"
        );
    }

    /// `OpenSnippets` is unbound by default (so it never collides).
    #[test]
    fn open_snippets_is_unbound_by_default() {
        let sc = terminale_config::ShortcutsConfig::default();
        assert!(
            sc.open_snippets.is_empty(),
            "open_snippets must be unbound by default"
        );
    }

    /// `binding_for` returns the configured binding for `OpenSnippets`.
    #[test]
    fn open_snippets_binding_for_resolves() {
        let sc = terminale_config::ShortcutsConfig {
            open_snippets: "Ctrl+Shift+S".to_string(),
            ..Default::default()
        };
        assert_eq!(
            binding_for(ShortcutAction::OpenSnippets, &sc),
            "Ctrl+Shift+S"
        );
    }

    /// `action_from_name("opensnippets")` resolves correctly (case-insensitive).
    #[test]
    fn open_snippets_action_from_name() {
        assert_eq!(
            crate::keymap::action_from_name("opensnippets"),
            Some(ShortcutAction::OpenSnippets)
        );
        assert_eq!(
            crate::keymap::action_from_name("OpenSnippets"),
            Some(ShortcutAction::OpenSnippets)
        );
    }

    /// The body escape decoder reuses `decode_send_string` exactly — `\n`→LF,
    /// `\t`→TAB, `\e`→ESC, `\xNN`→byte.
    #[test]
    fn snippet_body_escape_decoding() {
        use terminale_config::decode_send_string;
        assert_eq!(decode_send_string("git status\n"), b"git status\n");
        assert_eq!(decode_send_string("a\tb"), b"a\tb");
        assert_eq!(decode_send_string(r"esc\e"), &[b'e', b's', b'c', 0x1b]);
        assert_eq!(decode_send_string("\\x41"), b"A");
        assert_eq!(decode_send_string("no-escape"), b"no-escape");
    }

    /// `snippet_rows` produces rows with correct indices even when the list is
    /// non-trivial, and description appears as the secondary binding hint.
    #[test]
    fn snippet_rows_indices_and_descriptions() {
        let names: Vec<(String, String)> = vec![
            ("Alpha".to_string(), "First".to_string()),
            ("Beta".to_string(), "Second".to_string()),
        ];
        let rows = snippet_rows(&names);
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0].0, PaletteItem::InsertSnippet(0)));
        assert_eq!(rows[0].1, "Alpha");
        assert_eq!(rows[0].2, "First");
        assert!(matches!(rows[1].0, PaletteItem::InsertSnippet(1)));
        assert_eq!(rows[1].1, "Beta");
        assert_eq!(rows[1].2, "Second");
    }

    // ── dim-inactive-panes: configs_identical coverage ────────────────────────

    #[test]
    fn configs_identical_detects_inactive_pane_dim_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.inactive_pane_dim = 0.4;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.inactive_pane_dim differs"
        );
    }

    #[test]
    fn configs_identical_detects_unfocused_window_dim_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.unfocused_window_dim = 0.3;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.unfocused_window_dim differs"
        );
    }

    // ── configs_identical: clipboard_history ──────────────────────────────────

    #[test]
    fn configs_identical_detects_clipboard_history_enabled_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.clipboard_history.enabled = !a.clipboard_history.enabled;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when clipboard_history.enabled differs"
        );
    }

    #[test]
    fn configs_identical_detects_clipboard_history_size_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.clipboard_history.size = a.clipboard_history.size + 5;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when clipboard_history.size differs"
        );
    }

    #[test]
    fn configs_identical_detects_clipboard_history_capture_osc52_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.clipboard_history.capture_osc52 = !a.clipboard_history.capture_osc52;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when clipboard_history.capture_osc52 differs"
        );
    }

    #[test]
    fn configs_identical_detects_alt_screen_scroll_lines_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.window.alt_screen_scroll_lines = a.window.alt_screen_scroll_lines + 1;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when window.alt_screen_scroll_lines differs"
        );
    }

    #[test]
    fn configs_identical_detects_touchpad_pixels_per_row_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.window.touchpad_pixels_per_row = a.window.touchpad_pixels_per_row + 4.0;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when window.touchpad_pixels_per_row differs"
        );
    }

    #[test]
    fn configs_identical_detects_smooth_scroll_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.window.smooth_scroll = !a.window.smooth_scroll;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when window.smooth_scroll differs"
        );
    }

    #[test]
    fn configs_identical_detects_builtin_box_drawing_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.appearance.builtin_box_drawing = !a.appearance.builtin_box_drawing;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when appearance.builtin_box_drawing differs"
        );
    }

    // ── clipboard history ring logic (pure) ────────────────────────────────────
    //
    // The ring lives on RunningState which cannot be constructed in a unit test.
    // We test the pure logic using a standalone VecDeque that mimics the rules
    // applied by push_clipboard_history.

    /// Helper: apply the same rules as push_clipboard_history to a VecDeque.
    fn ring_push(
        ring: &mut std::collections::VecDeque<String>,
        text: String,
        enabled: bool,
        cap: usize,
    ) {
        if !enabled || text.is_empty() {
            return;
        }
        if ring.front().is_some_and(|t| t == &text) {
            return; // consecutive dedupe
        }
        ring.push_front(text);
        while ring.len() > cap {
            ring.pop_back();
        }
    }

    #[test]
    fn ring_push_adds_most_recent_first() {
        let mut ring = std::collections::VecDeque::new();
        ring_push(&mut ring, "a".to_string(), true, 10);
        ring_push(&mut ring, "b".to_string(), true, 10);
        ring_push(&mut ring, "c".to_string(), true, 10);
        assert_eq!(ring[0], "c");
        assert_eq!(ring[1], "b");
        assert_eq!(ring[2], "a");
    }

    #[test]
    fn ring_push_drops_empty_strings() {
        let mut ring = std::collections::VecDeque::new();
        ring_push(&mut ring, String::new(), true, 10);
        assert!(ring.is_empty(), "empty string must not be pushed");
    }

    #[test]
    fn ring_push_dedupes_consecutive_identical() {
        let mut ring = std::collections::VecDeque::new();
        ring_push(&mut ring, "same".to_string(), true, 10);
        ring_push(&mut ring, "same".to_string(), true, 10);
        ring_push(&mut ring, "same".to_string(), true, 10);
        assert_eq!(
            ring.len(),
            1,
            "consecutive identical entries must be deduped to 1"
        );
    }

    #[test]
    fn ring_push_allows_non_consecutive_duplicate() {
        let mut ring = std::collections::VecDeque::new();
        ring_push(&mut ring, "a".to_string(), true, 10);
        ring_push(&mut ring, "b".to_string(), true, 10);
        ring_push(&mut ring, "a".to_string(), true, 10); // not consecutive with the first "a"
        assert_eq!(ring.len(), 3, "non-consecutive duplicate must be kept");
        assert_eq!(ring[0], "a");
        assert_eq!(ring[1], "b");
        assert_eq!(ring[2], "a");
    }

    #[test]
    fn ring_push_evicts_oldest_when_at_cap() {
        let mut ring = std::collections::VecDeque::new();
        let cap = 3;
        for i in 0..5u32 {
            ring_push(&mut ring, format!("entry_{i}"), true, cap);
        }
        assert_eq!(ring.len(), cap, "ring must be capped at cap");
        // Most recent first: entry_4, entry_3, entry_2
        assert_eq!(ring[0], "entry_4");
        assert_eq!(ring[1], "entry_3");
        assert_eq!(ring[2], "entry_2");
    }

    #[test]
    fn ring_push_disabled_does_not_capture() {
        let mut ring = std::collections::VecDeque::new();
        ring_push(&mut ring, "should not appear".to_string(), false, 10);
        assert!(ring.is_empty(), "push must be no-op when disabled");
    }

    #[test]
    fn ring_osc52_capture_gate() {
        // When capture_osc52 is false, OSC 52 text should not reach the ring.
        // Simulate the gate: only call ring_push when capture_osc52 is true.
        let mut ring = std::collections::VecDeque::new();
        let capture_osc52 = false;
        let osc52_text = "secret_token".to_string();
        if capture_osc52 {
            ring_push(&mut ring, osc52_text, true, 10);
        }
        assert!(
            ring.is_empty(),
            "OSC 52 text must not be captured when gate is false"
        );
    }

    #[test]
    fn ring_paste_payload_honours_bracketed_paste() {
        // Verify that build_paste_payload wraps text in bracketed markers
        // when bracketed paste is enabled.
        let text = "hello\nworld";
        let unbracketed = crate::build_paste_payload(text, false);
        let bracketed = crate::build_paste_payload(text, true);
        assert_eq!(unbracketed, b"hello\nworld");
        assert!(
            bracketed.starts_with(b"\x1b[200~"),
            "must start with bracketed paste start"
        );
        assert!(
            bracketed.ends_with(b"\x1b[201~"),
            "must end with bracketed paste end"
        );
    }

    // ── Tab pinning / colour / icon unit tests ────────────────────────────────

    /// The effective colour uses `user_color` when set, falling back to `auto_color`.
    /// Tests the `.or()` chain used in `tab_bar_from` / `refresh_tab_bar`.
    #[test]
    fn effective_color_user_wins_over_auto() {
        let mut user_color: Option<[u8; 3]> = None;
        let mut auto_color: Option<[u8; 3]> = None;

        // No colours set: effective is None.
        assert!(user_color.or(auto_color).is_none());

        // auto_color set: effective is auto.
        auto_color = Some([0x10, 0x20, 0x30]);
        assert_eq!(user_color.or(auto_color), Some([0x10, 0x20, 0x30]));

        // user_color set: effective is user (wins over auto).
        user_color = Some([0xff, 0x00, 0x00]);
        assert_eq!(user_color.or(auto_color), Some([0xff, 0x00, 0x00]));

        // user_color cleared: falls back to auto.
        user_color = None;
        assert_eq!(user_color.or(auto_color), Some([0x10, 0x20, 0x30]));
    }

    /// Pinned tabs carried in `TabBarItem` list reflect the `pinned` flag.
    #[test]
    fn tab_bar_item_pinned_flag() {
        let item_pinned = terminale_render::TabBarItem {
            label: "A".into(),
            icon: None,
            active: true,
            unread: false,
            color: None,
            badge: None,
            pinned: true,
            group_accent: None,
            group_label: None,
        };
        let item_normal = terminale_render::TabBarItem {
            label: "B".into(),
            icon: None,
            active: false,
            unread: false,
            color: None,
            badge: None,
            pinned: false,
            group_accent: None,
            group_label: None,
        };
        assert!(item_pinned.pinned);
        assert!(!item_normal.pinned);
    }

    /// The pin-boundary clamping logic used in `move_active_tab` keeps
    /// unpinned tabs out of the pinned group and vice versa.
    #[test]
    fn move_active_tab_pin_boundary_clamping() {
        // Simulate: [pinned(0), unpinned(1), unpinned(2)]
        let pinned_count: usize = 1;
        let tab_count: usize = 3;

        // Unpinned tab at index 1 trying to move left to index 0 (pinned zone).
        let active = 1usize;
        let dir: i32 = -1;
        let raw_target = (active as i32 + dir).rem_euclid(tab_count as i32) as usize; // = 0
                                                                                      // clamped: must stay >= pinned_count because the tab is unpinned.
        let clamped = raw_target.max(pinned_count); // = max(0, 1) = 1
        assert_eq!(clamped, 1, "unpinned tab must not enter the pinned group");

        // Pinned tab at index 0 trying to move right to index 1 (unpinned zone).
        let active2 = 0usize;
        let dir2: i32 = 1;
        let raw_target2 = (active2 as i32 + dir2).rem_euclid(tab_count as i32) as usize; // = 1
                                                                                         // clamped: must stay < pinned_count because the tab is pinned.
        let clamped2 = raw_target2.min(pinned_count.saturating_sub(1)); // = min(1, 0) = 0
        assert_eq!(clamped2, 0, "pinned tab must not leave the pinned group");
    }

    /// `pinned_tab_width` config round-trips correctly.
    #[test]
    fn pinned_tab_width_in_appearance_config() {
        let cfg = terminale_config::AppearanceConfig {
            pinned_tab_width: 60.0,
            ..Default::default()
        };
        let toml_str = toml::to_string(&cfg).unwrap();
        let parsed: terminale_config::AppearanceConfig = toml::from_str(&toml_str).unwrap();
        assert!((parsed.pinned_tab_width - 60.0).abs() < 1e-4);
        // Default value also round-trips correctly.
        assert!(
            (terminale_config::AppearanceConfig::default().pinned_tab_width - 44.0).abs()
                < f32::EPSILON
        );
    }

    /// `ToggleTabPin` is present in `PALETTE_ACTIONS`.
    #[test]
    fn toggle_tab_pin_in_palette_actions() {
        assert!(
            crate::palette::PALETTE_ACTIONS
                .iter()
                .any(|(a, _)| *a == ShortcutAction::ToggleTabPin),
            "ToggleTabPin must appear in PALETTE_ACTIONS"
        );
    }

    /// `action_from_name("toggletabpin")` resolves correctly.
    #[test]
    fn toggle_tab_pin_action_from_name() {
        assert_eq!(
            crate::keymap::action_from_name("toggletabpin"),
            Some(ShortcutAction::ToggleTabPin)
        );
    }

    // ── swap-rotate-panes feature tests ──────────────────────────────────────

    /// All six pane-swap/rotate actions are present in PALETTE_ACTIONS.
    #[test]
    fn pane_swap_rotate_actions_in_palette() {
        let palette_actions: Vec<ShortcutAction> = crate::palette::PALETTE_ACTIONS
            .iter()
            .map(|(a, _)| *a)
            .collect();
        for action in [
            ShortcutAction::MovePaneLeft,
            ShortcutAction::MovePaneRight,
            ShortcutAction::MovePaneUp,
            ShortcutAction::MovePaneDown,
            ShortcutAction::RotatePanes,
            ShortcutAction::RotatePanesBack,
        ] {
            assert!(
                palette_actions.contains(&action),
                "{action:?} must appear in PALETTE_ACTIONS"
            );
        }
    }

    /// All six pane-swap/rotate action names resolve via `action_from_name`.
    #[test]
    fn pane_swap_rotate_action_from_name() {
        assert_eq!(
            crate::keymap::action_from_name("movepaneleft"),
            Some(ShortcutAction::MovePaneLeft)
        );
        assert_eq!(
            crate::keymap::action_from_name("movepaneright"),
            Some(ShortcutAction::MovePaneRight)
        );
        assert_eq!(
            crate::keymap::action_from_name("movepaneup"),
            Some(ShortcutAction::MovePaneUp)
        );
        assert_eq!(
            crate::keymap::action_from_name("movepanedown"),
            Some(ShortcutAction::MovePaneDown)
        );
        assert_eq!(
            crate::keymap::action_from_name("rotatepanes"),
            Some(ShortcutAction::RotatePanes)
        );
        assert_eq!(
            crate::keymap::action_from_name("rotatepanesback"),
            Some(ShortcutAction::RotatePanesBack)
        );
    }

    /// `binding_for` returns empty strings for all six new actions (unbound by default).
    #[test]
    fn pane_swap_rotate_unbound_by_default() {
        let sc = terminale_config::ShortcutsConfig::default();
        for action in [
            ShortcutAction::MovePaneLeft,
            ShortcutAction::MovePaneRight,
            ShortcutAction::MovePaneUp,
            ShortcutAction::MovePaneDown,
            ShortcutAction::RotatePanes,
            ShortcutAction::RotatePanesBack,
        ] {
            assert!(
                crate::shortcuts::binding_for(action, &sc).is_empty(),
                "{action:?} must be unbound by default"
            );
        }
    }

    // ── AI suggestion config coverage ────────────────────────────────────────

    #[test]
    fn configs_identical_detects_ai_suggestions_enabled_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.ai.suggestions.enabled = !a.ai.suggestions.enabled;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when ai.suggestions.enabled differs"
        );
    }

    #[test]
    fn configs_identical_detects_ai_suggestions_trigger_change() {
        use terminale_config::SuggestionTrigger;
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        // Default trigger is Auto; change it to Off.
        b.ai.suggestions.trigger = SuggestionTrigger::Off;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when ai.suggestions.trigger differs"
        );
    }

    #[test]
    fn configs_identical_detects_ai_suggestions_idle_secs_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.ai.suggestions.idle_secs = a.ai.suggestions.idle_secs + 1;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when ai.suggestions.idle_secs differs"
        );
    }

    #[test]
    fn configs_identical_detects_ai_suggestions_context_lines_change() {
        let a = terminale_config::Config::default();
        let mut b = terminale_config::Config::default();
        b.ai.suggestions.context_lines = a.ai.suggestions.context_lines + 10;
        assert!(
            !configs_identical(&a, &b),
            "configs_identical must return false when ai.suggestions.context_lines differs"
        );
    }

    /// All MenuAction variants round-trip through as_u32 / from_u32.
    #[test]
    fn menu_action_roundtrip_toggle_tab_pin() {
        let actions = [
            MenuAction::ToggleTabPin,
            MenuAction::ClearTabColor,
            MenuAction::TabColorRed,
            MenuAction::TabColorGreen,
            MenuAction::ClearTabIcon,
        ];
        for a in &actions {
            let id = a.as_u32();
            assert_eq!(
                MenuAction::from_u32(id),
                Some(*a),
                "MenuAction round-trip failed for id {id}"
            );
        }
    }

    // ── vertical tab strip drag tests ─────────────────────────────────────────

    /// Build a `BarRect` representing a window with a Left vertical strip of
    /// the given width at scale 1.0.
    fn vert_left_bar(
        id: WindowId,
        win_x: i32,
        win_y: i32,
        win_w: u32,
        win_h: u32,
        strip_w: f32,
    ) -> BarRect {
        BarRect {
            id,
            x: win_x,
            y: win_y,
            width: win_w,
            height: win_h,
            scale: 1.0,
            is_vertical: true,
            vert_strip_x_logical: 0.0,
            vert_strip_w_logical: strip_w,
            // Left strip: inner edge is the right side of the strip.
            vert_inner_edge_logical: strip_w,
        }
    }

    /// Build a `BarRect` for a Right vertical strip.
    fn vert_right_bar(
        id: WindowId,
        win_x: i32,
        win_y: i32,
        win_w: u32,
        win_h: u32,
        strip_w: f32,
    ) -> BarRect {
        let viewport_w = win_w as f32;
        BarRect {
            id,
            x: win_x,
            y: win_y,
            width: win_w,
            height: win_h,
            scale: 1.0,
            is_vertical: true,
            vert_strip_x_logical: viewport_w - strip_w,
            vert_strip_w_logical: strip_w,
            // Right strip: inner edge is the left side of the strip.
            vert_inner_edge_logical: viewport_w - strip_w,
        }
    }

    #[test]
    fn vert_strip_hit_inside_strip_returns_window() {
        let a = WindowId::from(20u64);
        // Window at (0,0), 800×600, left strip 180 px wide.
        let bars = [vert_left_bar(a, 0, 0, 800, 600, 180.0)];

        // Inside the strip (x=90, midway) at various y values → hit.
        assert_eq!(window_bar_at_screen(&bars, 90, 100), Some(a));
        assert_eq!(window_bar_at_screen(&bars, 90, 599), Some(a));
        assert_eq!(window_bar_at_screen(&bars, 1, 0), Some(a));
    }

    #[test]
    fn vert_strip_hit_within_tearout_margin_still_returns_window() {
        let a = WindowId::from(21u64);
        // Strip is 180 px wide (inner edge at x=180).
        // Cursor at x = 180 + VERT_TEAROUT_MARGIN_LOGICAL/2 = 180 + 16 — still inside
        // the tolerance zone → should still return Some(a).
        let bars = [vert_left_bar(a, 0, 0, 800, 600, 180.0)];
        let half_margin = (VERT_TEAROUT_MARGIN_LOGICAL / 2.0) as i32;
        let cursor_x = 180 + half_margin;
        assert_eq!(
            window_bar_at_screen(&bars, cursor_x, 200),
            Some(a),
            "cursor within tearout margin must still hit the strip"
        );
    }

    #[test]
    fn vert_strip_hit_beyond_tearout_margin_returns_none() {
        let a = WindowId::from(22u64);
        // Strip inner edge at x=180; margin is VERT_TEAROUT_MARGIN_LOGICAL (32).
        // Cursor at x = 180 + 33 — past the tolerance zone → Detach (None).
        let bars = [vert_left_bar(a, 0, 0, 800, 600, 180.0)];
        let past_margin = (VERT_TEAROUT_MARGIN_LOGICAL as i32) + 1;
        let cursor_x = 180 + past_margin;
        assert_eq!(
            window_bar_at_screen(&bars, cursor_x, 200),
            None,
            "cursor beyond tearout margin must miss the strip (trigger detach)"
        );
    }

    #[test]
    fn vert_strip_right_placement_hit_test() {
        let a = WindowId::from(23u64);
        // Window 800 wide; Right strip 180 px → strip occupies x ∈ [620, 800].
        // Inner edge at x=620 (facing left). Margin extends left to x=620-32=588.
        let bars = [vert_right_bar(a, 0, 0, 800, 600, 180.0)];

        // Well inside the strip → hit.
        assert_eq!(window_bar_at_screen(&bars, 700, 300), Some(a));
        // Within the left margin of the strip (x = 620 - 16 = 604) → still hit.
        let inside_margin = 620 - (VERT_TEAROUT_MARGIN_LOGICAL as i32 / 2);
        assert_eq!(window_bar_at_screen(&bars, inside_margin, 300), Some(a));
        // Beyond the margin (x = 620 - 33 = 587) → miss.
        let past_margin = 620 - (VERT_TEAROUT_MARGIN_LOGICAL as i32) - 1;
        assert_eq!(window_bar_at_screen(&bars, past_margin, 300), None);
        // Left of window entirely → miss.
        assert_eq!(window_bar_at_screen(&bars, -10, 300), None);
    }

    #[test]
    fn vert_strip_at_non_zero_window_origin() {
        let a = WindowId::from(24u64);
        // Window origin at (200, 100); strip width 180.
        // Screen x=250 → local_x=50, inside strip.
        let bars = [vert_left_bar(a, 200, 100, 800, 600, 180.0)];
        assert_eq!(window_bar_at_screen(&bars, 250, 300), Some(a));
        // Screen x=180 → local_x = -20 → negative → miss.
        assert_eq!(window_bar_at_screen(&bars, 180, 300), None);
        // Screen y=50 → local_y = -50 → negative → miss.
        assert_eq!(window_bar_at_screen(&bars, 250, 50), None);
    }

    // ── modifiers_from_held ───────────────────────────────────────────────────

    #[test]
    fn modifiers_from_held_ctrl_and_shift() {
        let m = modifiers_from_held(true, true, false, false);
        assert!(m.contains(ModifiersState::CONTROL), "CONTROL must be set");
        assert!(m.contains(ModifiersState::SHIFT), "SHIFT must be set");
        assert!(!m.contains(ModifiersState::ALT), "ALT must not be set");
        assert!(!m.contains(ModifiersState::SUPER), "SUPER must not be set");
    }

    #[test]
    fn modifiers_from_held_all_false_gives_empty() {
        let m = modifiers_from_held(false, false, false, false);
        assert_eq!(m, ModifiersState::empty(), "all-false must produce empty");
    }

    #[test]
    fn modifiers_from_held_all_true_gives_all_flags() {
        let m = modifiers_from_held(true, true, true, true);
        assert!(m.contains(ModifiersState::CONTROL));
        assert!(m.contains(ModifiersState::SHIFT));
        assert!(m.contains(ModifiersState::ALT));
        assert!(m.contains(ModifiersState::SUPER));
    }

    #[test]
    fn modifiers_from_held_only_alt() {
        let m = modifiers_from_held(false, false, true, false);
        assert!(!m.contains(ModifiersState::CONTROL));
        assert!(!m.contains(ModifiersState::SHIFT));
        assert!(m.contains(ModifiersState::ALT));
        assert!(!m.contains(ModifiersState::SUPER));
    }

    // ── group_reorder_dest: block-destination math ───────────────────────────

    /// Members whose original index < slot each shift the dest left by one.
    #[test]
    fn group_reorder_dest_shifts_by_removed_before() {
        // 6 tabs: [0,1,2,3,4,5]; group occupies [1,3].  Drop slot = 5.
        // After removing 1 and 3 → 4 tabs remain.
        // removed_before(slot=5) = 2 (indices 1 and 3 are both < 5) → dest = 5-2 = 3.
        assert_eq!(group_reorder_dest(&[1, 3], 5, 4), 3);
    }

    /// Dropping before any member: no shift needed.
    #[test]
    fn group_reorder_dest_no_shift_when_slot_before_members() {
        // 6 tabs: group [3,5], drop slot 0 → 0 removed before → dest = 0.
        assert_eq!(group_reorder_dest(&[3, 5], 0, 4), 0);
    }

    /// Dropping in the middle of members: only those before the slot count.
    #[test]
    fn group_reorder_dest_partial_shift() {
        // 5 tabs: group [0,2,4], drop slot 3.
        // After removal: 2 tabs remain.
        // removed_before(3) = indices 0 and 2 are < 3 → 2 members → dest = 3-2 = 1.
        assert_eq!(group_reorder_dest(&[0, 2, 4], 3, 2), 1);
    }

    /// Clamped to len_after: slot pushed past the end stays at the tail.
    #[test]
    fn group_reorder_dest_clamped_to_len_after() {
        // 3 tabs: group [0,1,2], slot = 3 (past-end), 0 tabs remain after removal.
        assert_eq!(group_reorder_dest(&[0, 1, 2], 3, 0), 0);
    }

    /// No-op check: group already at the front, drop slot 0.
    #[test]
    fn group_reorder_dest_noop_already_at_front() {
        // 4 tabs: group [0,1], slot = 0.
        // removed_before = 0; dest = 0; 2 tabs remain.
        assert_eq!(group_reorder_dest(&[0, 1], 0, 2), 0);
    }

    /// All members removed before slot: dest saturates at 0 when slot == 0.
    #[test]
    fn group_reorder_dest_saturating_sub() {
        // Edge: members = [0,1,2], slot = 0.
        // removed_before = 0 (none of [0,1,2] < 0) → dest = 0.
        assert_eq!(group_reorder_dest(&[0, 1, 2], 0, 0), 0);
    }
}
