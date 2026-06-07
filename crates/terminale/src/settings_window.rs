//! Standalone settings window built with egui + wgpu + winit.
//!
//! Layout: VS Code-style sidebar on the left (sections), content on the right.
//! Inputs use the most ergonomic widget per type — `ComboBox` for enums,
//! `Slider` with value badge for ranges, `Switch` for booleans.

use egui_wgpu::Renderer as EguiRenderer;
use egui_wgpu::ScreenDescriptor;
use egui_winit::State as EguiState;
use std::path::PathBuf;
use std::sync::Arc;
use terminale_config::{auto_detect_profiles, Config, Profile};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

/// Categories shown in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Section {
    Profiles,
    Appearance,
    Font,
    Cursor,
    Window,
    Terminal,
    Gpu,
    Bell,
    Quake,
    Ssh,
    Backup,
    Shortcuts,
    Ai,
    Plugins,
    QuickSelect,
    StatusBar,
    Snippets,
    ContextRules,
    Workspaces,
    ClipboardHistory,
    DirectoryJump,
    Integration,
    About,
}

/// Curated set of popular monospace font families. Users can still type a
/// custom name — the ComboBox is editable.
const FONT_PRESETS: &[&str] = &[
    "JetBrains Mono",
    "Fira Code",
    "Cascadia Code",
    "Cascadia Mono",
    "Consolas",
    "Source Code Pro",
    "Hack",
    "Iosevka",
    "Monaco",
    "Menlo",
    "DejaVu Sans Mono",
    "Ubuntu Mono",
    "IBM Plex Mono",
    "monospace",
];

/// Curated icons users can pick for a profile. Standard emojis chosen so they
/// render in egui's bundled NotoEmoji font without needing a Nerd Font.
pub(crate) const ICON_PRESETS: &[(&str, &str)] = &[
    ("PowerShell", "⚡"),
    ("Command Prompt", "📟"),
    ("Bash", "🐚"),
    ("Git Bash", "🌿"),
    ("WSL / Linux", "🐧"),
    ("macOS", "🍎"),
    ("Windows", "⊞"),
    ("Code", "💻"),
    ("Server", "🖥"),
    ("Cloud", "☁"),
    ("Remote SSH", "🔐"),
    ("Generic", "▶"),
];
// ÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇ
// Per-section sub-modules ÔÇö each owns the impl SettingsWindow { section_xxx }
// block for its section. Adding a new section = new file + one mod line +
// one call in build_ui's match.
// ÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇÔöÇ
mod about;
mod ai;
mod appearance;
mod backup;
mod bell;
mod clipboard_history;
mod context_rules;
mod cursor;
mod directory_jump;
mod font;
mod gpu;
mod integration;
mod plugins;
mod profiles;
mod quake;
mod quick_select;
mod shortcuts;
mod snippets;
mod ssh;
mod status_bar;
mod terminal;
mod window;
mod workspaces;

pub struct SettingsWindow {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    /// String id of the shortcut currently being recorded (e.g.
    /// `"quake"`, `"new_tab"`). `None` = no recording in flight.
    recording_hotkey: Option<String>,
    /// In-progress secret entry for an SSH host: `(host index, typed secret)`.
    /// The secret is held only here (in memory) until the user clicks "Save to
    /// keychain"; it's never written to `config.toml`. `None` when no host's
    /// credential is being edited.
    ssh_secret_edit: Option<(usize, String)>,
    /// Transient per-host status line under the credential controls, e.g.
    /// "Saved to keychain." Keyed by host index.
    ssh_secret_status: Option<(usize, String)>,
    /// Backup section in-memory state: passphrases (held only here, never
    /// persisted), the "include credentials" opt-in, and the last status line.
    backup: BackupUiState,

    egui_ctx: egui::Context,
    egui_state: EguiState,
    egui_renderer: EguiRenderer,

    config: Config,
    config_path: PathBuf,
    dirty: bool,
    /// Set whenever an egui frame ran (`build_ui` is the only place
    /// `config` is edited), consumed by the host's live-apply check in
    /// `about_to_wait` via [`Self::take_config_maybe_changed`]. Gates the
    /// full-`Config` clone + ~150-field diff so it runs once per settings
    /// repaint instead of on every host tick while the panel is open.
    config_maybe_changed: bool,
    status: Option<(StatusKind, String)>,
    section: Section,
    /// Live text in the sidebar search box. Filters which sidebar
    /// entries are shown (case-insensitive substring match against the
    /// label). Empty = show all.
    sidebar_search: String,
    /// Deep-search highlight state: the section + field label that
    /// should pulse after a search match. Cleared when the search box
    /// is empty or a sidebar entry is manually clicked.
    pending_highlight: Option<(Section, &'static str)>,
    /// When the pulse animation started (for fade-out timing).
    highlight_started: Option<std::time::Instant>,
    /// Whether `scroll_to_rect` has already been called for the current
    /// highlight (so we only scroll once per search, not every frame).
    highlight_scrolled: bool,
    detected_shells: Vec<Profile>,
    /// Set by the custom title bar's ✕ to ask the main app to drop this
    /// window on the next event.
    requested_close: bool,
    /// Cached maximized state, refreshed only on `Resized` events. The custom
    /// title bar must NOT call `winit::Window::is_maximized()` per frame: on
    /// macOS that getter round-trips through `-[NSWindow setStyleMask:]`, which
    /// rebuilds the whole AppKit theme frame (~16-20 ms each). Doing it every
    /// repaint pegged a CPU core while the window was merely open.
    cached_maximized: bool,
    /// When egui requests a *delayed* repaint (for in-flight animations
    /// — hover fades, combo transitions, the recorder pulse) we store the
    /// deadline here. The host event loop folds it into its wake timer so
    /// the animation keeps advancing between input events instead of
    /// stuttering.
    next_repaint: Option<std::time::Instant>,
    /// Set by the "Import from SSH config" button in the SSH section. The
    /// host (main.rs) drains this flag on the next frame by calling the
    /// import helper, so the settings window doesn't need a mutable borrow
    /// on the App's config simultaneously.
    pub pending_import_ssh_hosts: bool,
    /// Set by the "Import theme…" button in the Appearance section. The host
    /// (main.rs) drains this flag on the next frame: it opens the file picker,
    /// copies the chosen `.toml` into themes_dir, and appends the theme.
    pub pending_import_theme: bool,
    /// Display names of currently-loaded Lua plugins, synced from the host's
    /// `PluginHost::plugins()` on each `about_to_wait`. Shown in the Plugins
    /// section as a read-only list so the user can see what's active.
    pub loaded_plugin_names: Vec<String>,
    /// Monospace font families actually installed (from the renderer's font
    /// database), populated once in `about_to_wait`. The font pickers list
    /// these so every selectable family resolves instead of falling back.
    pub available_fonts: Vec<String>,
    /// Subset of `available_fonts` that are bundled inside the binary.
    /// Used by the font pickers to append a "(bundled)" label to those
    /// entries so users know they are always available on any machine.
    pub bundled_fonts: Vec<String>,
    /// Cached theme list for the Appearance section. The render closure runs
    /// every frame (and egui repaints continuously while the scroll area has
    /// momentum), so resolving the theme list there used to re-scan the
    /// themes directory from disk — `read_dir` + `read_to_string` + TOML parse
    /// per file — dozens of times a second while scrolling, pegging CPU/IO.
    /// We scan once and cache here; rebuilt only when `theme_cache_dirty` is
    /// set (themes-dir change, theme import, or full config reload).
    cached_all_themes: Vec<terminale_config::Theme>,
    /// Names of the drop-in `*.toml` themes found in the themes directory,
    /// for the read-only list under "Theme import". Part of the same cache.
    cached_dropin_names: Vec<String>,
    /// When set, the next `section_appearance` frame rebuilds the theme cache.
    /// Starts `true` so the cache is populated on first paint.
    theme_cache_dirty: bool,
    /// Cached saved-workspace list `(name, path)` for the Workspaces section.
    /// Same rationale as `cached_all_themes`: the section body runs every
    /// frame, so scanning the workspaces directory there (`read_dir` + sort)
    /// was disk I/O on every repaint while the tab was open. Rebuilt when
    /// `workspace_cache_dirty` is set (section entry or a delete).
    cached_workspaces: Vec<(String, std::path::PathBuf)>,
    /// When set, the next `section_workspaces` frame rebuilds the workspace
    /// cache. Starts `true`; also set on section entry (so an externally-saved
    /// workspace shows up when you navigate to the tab) and after a delete.
    workspace_cache_dirty: bool,
    /// The section rendered last frame, used to detect navigation into a
    /// section so per-visit caches (the workspace list) can refresh once on
    /// entry rather than every frame.
    last_section: Section,
    /// Receiver for the background "Check for updates now" thread. The update
    /// runs off the UI thread (network + disk); it reports back here so the
    /// About section can show a visible result instead of only logging. `Some`
    /// while a check is in flight; drained and reset to `None` once it lands.
    /// The payload is the update outcome, or `Err(message)` on failure.
    update_rx: Option<std::sync::mpsc::Receiver<Result<crate::update::UpdateOutcome, String>>>,
}

#[derive(Debug, Clone, Copy)]
enum StatusKind {
    Success,
    /// Severity level reserved for future non-fatal warning toasts.
    #[allow(dead_code)]
    Warning,
    Error,
}

/// In-memory state for the Backup (encrypted import/export) section.
///
/// Passphrases live here only while the panel is open and are never written to
/// disk or config. They're cleared after a successful export/import.
#[derive(Default)]
struct BackupUiState {
    /// Export passphrase + its confirmation field.
    export_pass: String,
    export_confirm: String,
    /// Opt-in: include SSH credentials (from the keychain) in the export.
    /// Defaults OFF — only set true by an explicit, warned checkbox tick.
    include_credentials: bool,
    /// Import passphrase.
    import_pass: String,
    /// Last action's result, shown under the controls.
    status: Option<(StatusKind, String)>,
}

impl SettingsWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        config: Config,
        config_path: PathBuf,
        instance: Arc<wgpu::Instance>,
        adapter: Arc<wgpu::Adapter>,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Self {
        // Created hidden so we can render the first egui frame to the GPU
        // surface BEFORE the OS gets a chance to show the window. Avoids
        // the white-flash and the show animation.
        let mut attrs = crate::app_icon::with_app_identity(Window::default_attributes())
            .with_title("terminale — settings")
            .with_inner_size(winit::dpi::LogicalSize::new(900.0, 620.0))
            .with_min_inner_size(winit::dpi::LogicalSize::new(580.0, 460.0))
            .with_decorations(false)
            .with_visible(false);
        // When the user has "stay on top" turned on for the terminal, pin
        // Settings above it too — otherwise the moment you click back into
        // the terminal it disappears behind the always-on-top main window.
        if config.window.always_on_top {
            attrs = attrs.with_window_level(winit::window::WindowLevel::AlwaysOnTop);
        }
        if let Some(icon) = crate::app_icon::load_app_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create settings window"),
        );

        let surface = instance
            .create_surface(Arc::clone(&window))
            .expect("settings surface");

        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        // egui expects a LINEAR (non-sRGB) framebuffer and warns +
        // does an extra conversion otherwise. Prefer Bgra8Unorm /
        // Rgba8Unorm; only fall back to whatever's available.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                )
            })
            .or_else(|| caps.formats.iter().copied().find(|f| !f.is_srgb()))
            .unwrap_or(caps.formats[0]);
        // A non-blocking present mode keeps window DRAGS smooth. With
        // AutoVsync + 1 frame of latency, `get_current_texture()` parks the
        // thread on the compositor for a whole refresh every frame, so while
        // the user is moving the window the redraws lag (~30ms → dropped
        // frames). Mailbox/Immediate let acquire return at once; egui still
        // only repaints on demand, so this doesn't busy-loop.
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else if caps.present_modes.contains(&wgpu::PresentMode::Immediate) {
            wgpu::PresentMode::Immediate
        } else {
            wgpu::PresentMode::AutoVsync
        };
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let egui_ctx = egui::Context::default();
        configure_visuals(&egui_ctx);
        // Install Hack as a Proportional fallback so geometric/arrow icons
        // (↑ ↓ ▲ ▼ ⊕ ● etc.) never render as tofu in this window.
        crate::egui_icons::install_icon_font(&egui_ctx);
        let viewport_id = egui_ctx.viewport_id();
        let egui_state = EguiState::new(
            egui_ctx.clone(),
            viewport_id,
            &*window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = EguiRenderer::new(&device, format, None, 1, false);

        let mut this = Self {
            window,
            surface,
            surface_config,
            device,
            queue,
            egui_ctx,
            egui_state,
            egui_renderer,
            config,
            config_path,
            dirty: false,
            // Start true so the host's first live-apply check after the
            // panel opens runs once even before the first egui frame.
            config_maybe_changed: true,
            status: None,
            section: Section::Profiles,
            sidebar_search: String::new(),
            pending_highlight: None,
            highlight_started: None,
            highlight_scrolled: false,
            detected_shells: auto_detect_profiles(),
            requested_close: false,
            cached_maximized: false,
            next_repaint: None,
            recording_hotkey: None,
            ssh_secret_edit: None,
            ssh_secret_status: None,
            backup: BackupUiState::default(),
            pending_import_ssh_hosts: false,
            pending_import_theme: false,
            loaded_plugin_names: Vec::new(),
            available_fonts: Vec::new(),
            bundled_fonts: Vec::new(),
            cached_all_themes: Vec::new(),
            cached_dropin_names: Vec::new(),
            theme_cache_dirty: true,
            cached_workspaces: Vec::new(),
            workspace_cache_dirty: true,
            last_section: Section::Profiles,
            update_rx: None,
        };

        // Pre-render one frame into the swap chain while the window is still
        // hidden — this populates the surface with content so when we show
        // it, the OS doesn't display the empty (white) client area.
        this.render_frame();

        #[cfg(windows)]
        set_dwm_cloak(&this.window, true);

        this.window.set_visible(true);

        #[cfg(windows)]
        set_dwm_cloak(&this.window, false);

        this.window.focus_window();

        this
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn current_config(&self) -> &Config {
        &self.config
    }

    /// Consume the "config may have changed" flag — `true` when at least
    /// one egui frame (the only context that edits `config`) ran since the
    /// last call. The host gates its clone + 150-field live-apply diff on
    /// this instead of running it every `about_to_wait` tick.
    pub fn take_config_maybe_changed(&mut self) -> bool {
        std::mem::take(&mut self.config_maybe_changed)
    }

    /// Sync an externally-applied font size (a live Ctrl+± zoom) into this
    /// window's config copy, so the host's live-apply diff doesn't see a
    /// stale value and revert the zoom while the panel is open.
    pub fn sync_font_size(&mut self, size: f32) {
        self.config.font.size = size;
    }

    /// Sync an externally-applied "stay on top" toggle (from the command
    /// palette or right-click menu) into this window's config copy, so the
    /// host's live-apply diff doesn't see a stale value and revert it while
    /// the panel is open. Also re-applies the level to the Settings window
    /// itself, so the panel tracks the user's choice instead of sitting
    /// behind the always-on-top terminal.
    pub fn sync_always_on_top(&mut self, on: bool) {
        self.config.window.always_on_top = on;
        self.apply_own_window_level();
    }

    /// Apply the current `always_on_top` flag to this Settings window. Called
    /// after construction and whenever the user toggles the flag from inside
    /// Settings (or via an external sync).
    fn apply_own_window_level(&self) {
        let level = if self.config.window.always_on_top {
            winit::window::WindowLevel::AlwaysOnTop
        } else {
            winit::window::WindowLevel::Normal
        };
        self.window.set_window_level(level);
    }

    /// Append an SSH host added outside the settings panel (the "Save this
    /// SSH host?" prompt) into this window's config copy, so the host's
    /// live-apply diff doesn't see a stale list and drop the new host while
    /// the panel is open. The new host is then visible in Settings → SSH
    /// hosts immediately.
    pub fn sync_add_ssh_host(&mut self, host: terminale_config::SshHost) {
        self.config.ssh_hosts.push(host);
    }

    /// Sync a newly-imported drop-in theme into this window's config copy so
    /// the theme picker shows it immediately and the live-apply diff doesn't
    /// revert it while the panel is open. Silently no-ops when the theme's
    /// name is already present in the inline list.
    pub fn sync_add_theme(&mut self, theme: terminale_config::Theme) {
        // Only add to the inline list if not already there (by name). Drop-ins
        // live on disk and are re-scanned by all_themes(), so we only need to
        // ensure the active-theme name can be resolved inside the panel.
        if !self
            .config
            .appearance
            .themes
            .iter()
            .any(|t| t.name == theme.name)
        {
            self.config.appearance.themes.push(theme);
        }
    }

    /// Sync an externally-applied active-theme change into this window's
    /// config copy so the combo-box selection reflects it immediately.
    pub fn sync_theme_active(&mut self, name: &str) {
        self.config.appearance.theme = name.to_owned();
    }

    /// Sync an externally-applied "offer to save SSH hosts" toggle (from the
    /// prompt's "don't ask again" checkbox) into this window's config copy,
    /// so the host's live-apply diff doesn't revert it while the panel is open.
    pub fn sync_offer_save_ssh_hosts(&mut self, on: bool) {
        self.config.terminal.offer_save_ssh_hosts = on;
    }

    /// Replace the entire settings config with a freshly-reloaded copy
    /// (from a disk reload). This keeps the settings panel in sync so the
    /// live-apply diff in `about_to_wait` sees no difference and doesn't
    /// immediately re-write the reloaded values back. Clears any unsaved edits
    /// in the panel.
    pub fn sync_config(&mut self, new_config: Config) {
        self.config = new_config;
        // If the user had unsaved in-progress edits, they've been superseded
        // by the reload. Mark not dirty so we don't auto-save stale values.
        self.dirty = false;
        // A reload can change themes_dir or the inline theme list, so the
        // cached theme picker contents must be rebuilt.
        self.theme_cache_dirty = true;
    }

    /// Mark the cached theme list stale so the Appearance section rebuilds it
    /// (re-scans the themes directory) on its next frame. Called by the host
    /// after importing a theme — the import copies a `*.toml` into the themes
    /// directory and may append an inline theme, neither of which the panel
    /// would otherwise notice without a disk re-scan.
    pub fn invalidate_theme_cache(&mut self) {
        self.theme_cache_dirty = true;
    }

    /// Rebuild the cached theme list if it's been marked stale. Cheap no-op on
    /// every clean frame; does the disk scan only when `theme_cache_dirty` is
    /// set. Keeps the per-frame `section_appearance` render closure free of
    /// disk I/O (see `cached_all_themes`).
    pub(super) fn ensure_theme_cache(&mut self) {
        if !self.theme_cache_dirty {
            return;
        }
        self.cached_all_themes = self.config.appearance.all_themes();
        self.cached_dropin_names = self
            .config
            .appearance
            .effective_themes_dir()
            .map(|dir| {
                terminale_config::scan_themes_dir(&dir)
                    .into_iter()
                    .map(|t| t.name)
                    .collect()
            })
            .unwrap_or_default();
        self.theme_cache_dirty = false;
    }

    /// Rebuild the cached saved-workspace list if it's been marked stale.
    /// Cheap no-op on clean frames; scans the workspaces directory only when
    /// `workspace_cache_dirty` is set. Keeps the per-frame `section_workspaces`
    /// render closure free of disk I/O (see `cached_workspaces`).
    pub(super) fn ensure_workspace_cache(&mut self) {
        if !self.workspace_cache_dirty {
            return;
        }
        self.cached_workspaces = terminale_config::paths::workspaces_dir()
            .and_then(|d| std::fs::read_dir(&d).ok())
            .map(|rd| {
                let mut list: Vec<(String, std::path::PathBuf)> = rd
                    .flatten()
                    .filter_map(|e| {
                        let p = e.path();
                        if p.extension()? == "toml" {
                            let name = p.file_stem()?.to_string_lossy().into_owned();
                            Some((name, p))
                        } else {
                            None
                        }
                    })
                    .collect();
                list.sort_by(|a, b| a.0.cmp(&b.0));
                list
            })
            .unwrap_or_default();
        self.workspace_cache_dirty = false;
    }

    /// If egui asked for a delayed repaint, this is the deadline. The
    /// host folds it into its event-loop wake timer.
    #[must_use]
    pub fn next_repaint(&self) -> Option<std::time::Instant> {
        self.next_repaint
    }

    /// Called by the host each loop tick: if the egui animation deadline
    /// has elapsed, queue a repaint so the animation advances.
    pub fn pump_repaint(&mut self) {
        if let Some(deadline) = self.next_repaint {
            if std::time::Instant::now() >= deadline {
                self.next_repaint = None;
                self.window.request_redraw();
            }
        }
    }

    /// Returns `true` if the window was asked to close.
    pub fn handle_event(&mut self, event: &WindowEvent) -> bool {
        if matches!(event, WindowEvent::CloseRequested) {
            return true;
        }

        // Edge-resize: this window has no system frame, so we drive resize
        // manually via cursor-edge hit-testing.
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.window.scale_factor() as f32;
                let lx = position.x as f32 / scale;
                let ly = position.y as f32 / scale;
                let icon =
                    match detect_window_resize_edge(lx, ly, &self.window, self.cached_maximized) {
                        Some(dir) => cursor_icon_for_resize_settings(dir),
                        None => winit::window::CursorIcon::Default,
                    };
                self.window.set_cursor(icon);
            }
            WindowEvent::MouseInput {
                state: winit::event::ElementState::Pressed,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                // We don't know cursor position from this event — read it
                // from the last egui state. Simpler: query Win32… or just
                // detect on the last position egui reported.
                let pp = self.egui_ctx.pointer_latest_pos();
                if let Some(pos) = pp {
                    if let Some(dir) =
                        detect_window_resize_edge(pos.x, pos.y, &self.window, self.cached_maximized)
                    {
                        let _ = self.window.drag_resize_window(dir);
                        return false;
                    }
                }
            }
            _ => {}
        }

        // `RedrawRequested` is a "paint now" signal, not input — yet egui-winit
        // still reports `repaint == true` for it. Honouring that here calls
        // `request_redraw()`, which produces another `RedrawRequested`, and so
        // on: an unbroken ~60 fps repaint loop that pegs a CPU core while the
        // window merely sits open (no input, no animation). The paint itself
        // happens in the `RedrawRequested` arm below, and `render_frame`
        // schedules its own follow-up repaint whenever an animation needs one,
        // so suppress the self-retriggering redraw for that event.
        let response = self.egui_state.on_window_event(&self.window, event);
        if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
            self.window.request_redraw();
        }

        if let WindowEvent::Resized(size) = event {
            self.surface_config.width = size.width.max(1);
            self.surface_config.height = size.height.max(1);
            self.surface.configure(&self.device, &self.surface_config);
            // Maximize/restore both arrive as a resize — refresh the cached
            // flag here (off the per-frame path) so the title bar's
            // maximize/restore glyph stays correct without the per-repaint
            // `is_maximized()` cost. See `cached_maximized`.
            self.cached_maximized = self.window.is_maximized();
            self.window.request_redraw();
        }

        if matches!(event, WindowEvent::RedrawRequested) {
            self.render_frame();
        }

        // Custom title-bar's ✕ button takes effect here.
        if self.requested_close {
            self.requested_close = false;
            return true;
        }

        false
    }

    fn render_frame(&mut self) {
        let frame_start = std::time::Instant::now();
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let ctx = self.egui_ctx.clone();
        let ui_start = std::time::Instant::now();
        let full_output = ctx.run(raw_input, |ctx| self.build_ui(ctx));
        let ui_ms = ui_start.elapsed().as_secs_f32() * 1000.0;
        // `build_ui` is the only place `self.config` is edited — flag the
        // frame so the host's live-apply diff runs (once) after it.
        self.config_maybe_changed = true;

        // Honour egui's requested repaint cadence. Without this, in-flight
        // animations (hover fades, combo open/close, the recorder pulse)
        // only advance when a raw input event happens to arrive — which
        // reads as stutter / frame drops. Zero delay = repaint ASAP
        // (keep the animation loop self-sustaining); a finite delay = wake
        // the host loop at that deadline; ~never = idle.
        let repaint_delay = full_output
            .viewport_output
            .values()
            .map(|v| v.repaint_delay)
            .min()
            .unwrap_or(std::time::Duration::MAX);
        if repaint_delay.is_zero() {
            self.next_repaint = None;
            self.window.request_redraw();
        } else if repaint_delay < std::time::Duration::from_secs(1) {
            self.next_repaint = std::time::Instant::now().checked_add(repaint_delay);
        } else {
            self.next_repaint = None;
        }

        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);

        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }
        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        let screen = ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("settings encoder"),
            });

        self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen,
        );

        // `get_current_texture` blocks until the compositor hands back a
        // back-buffer. With AutoVsync + 1 frame of latency that wait is the
        // *normal* vsync park (up to a full refresh, and more while DWM is
        // also compositing the main window) — it is not work we did, so we
        // time it apart and exclude it from the slow-frame budget below.
        let acquire_start = std::time::Instant::now();
        let frame = if let Ok(f) = self.surface.get_current_texture() {
            f
        } else {
            self.surface.configure(&self.device, &self.surface_config);
            return;
        };
        let acquire_ms = acquire_start.elapsed().as_secs_f32() * 1000.0;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("settings pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.043,
                                g: 0.050,
                                b: 0.071,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            self.egui_renderer.render(&mut pass, &paint_jobs, &screen);
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();

        // Surface slow frames so we can see whether the cost is in egui
        // layout (ui_ms) or our GPU submit. We deliberately judge on
        // *work* time — total minus the vsync-blocking surface acquire —
        // because the panel renders at vsync cadence and the acquire wait
        // (often the bulk of total_ms when idle) is the compositor pacing
        // us, not a stall we caused. Logging total alone produced noisy
        // 20–30 ms "slow frame" warnings on an essentially free static
        // panel. We still surface acquire_ms so a genuinely starved swap
        // chain stays visible in the logs.
        let total_ms = frame_start.elapsed().as_secs_f32() * 1000.0;
        let work_ms = total_ms - acquire_ms;
        if work_ms > 16.0 {
            tracing::warn!(
                work_ms = format!("{work_ms:.1}"),
                total_ms = format!("{total_ms:.1}"),
                acquire_ms = format!("{acquire_ms:.1}"),
                ui_ms = format!("{ui_ms:.1}"),
                section = ?self.section,
                "settings slow frame"
            );
        }
    }

    fn build_ui(&mut self, ctx: &egui::Context) {
        let mut save_now = false;

        // Drain the background update check (started by "Check for updates
        // now"). While it's in flight we keep repainting so the result lands
        // promptly; once it arrives we turn it into a visible status line.
        if let Some(rx) = &self.update_rx {
            match rx.try_recv() {
                Ok(result) => {
                    use crate::update::UpdateOutcome;
                    self.status = Some(match result {
                        Ok(UpdateOutcome::Staged(v)) => (
                            StatusKind::Success,
                            format!("Update {v} downloaded — restart terminale to apply."),
                        ),
                        Ok(UpdateOutcome::SwitchRequired(v)) => (
                            StatusKind::Error,
                            format!(
                                "{v} is available, but this legacy system-wide install can't \
                                 be upgraded in place — use \"Switch to self-updating \
                                 install\" below (one-time, keeps your settings)."
                            ),
                        ),
                        Ok(UpdateOutcome::InstallerRequired(v)) => (
                            StatusKind::Success,
                            format!(
                                "Update {v} is available but this install location isn't \
                                 writable — update it the way it was installed."
                            ),
                        ),
                        Ok(UpdateOutcome::UpToDate) => {
                            (StatusKind::Success, "terminale is up to date.".to_owned())
                        }
                        Err(e) => (StatusKind::Error, format!("Update failed: {e}")),
                    });
                    self.update_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // Still running — keep the frame loop alive so we notice
                    // completion without waiting for the next input event.
                    ctx.request_repaint();
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.update_rx = None;
                }
            }
        }

        // ── Custom title bar ──
        self.build_title_bar(ctx);

        // ── Sidebar (grouped) ──
        egui::SidePanel::left("sidebar")
            .resizable(false)
            .exact_width(220.0)
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(15, 17, 25))
                    .inner_margin(egui::Margin::symmetric(12.0, 16.0)),
            )
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("terminale")
                        .heading()
                        .color(egui::Color32::from_rgb(220, 230, 255)),
                );
                ui.label(
                    egui::RichText::new("settings")
                        .small()
                        .color(egui::Color32::from_rgb(120, 130, 160)),
                );
                ui.add_space(10.0);

                // About is pinned to the very bottom of the sidebar; the
                // grouped nav scrolls above it so the list never overruns the
                // panel on a short window.
                // Sidebar search box — filters which sidebar entries are
                // shown (case-insensitive substring match against the
                // visible label). Empty = show everything. The box sits
                // ABOVE the scroll area so it stays visible regardless of
                // sidebar length, mirroring the settings-search pattern used
                // by common editors.
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.sidebar_search)
                        .hint_text("Search settings\u{2026}")
                        .desired_width(f32::INFINITY),
                );
                if r.changed() {
                    let q = self.sidebar_search.to_lowercase();
                    if q.is_empty() {
                        // Clear highlight when search box is emptied.
                        self.pending_highlight = None;
                        self.highlight_started = None;
                        self.highlight_scrolled = false;
                    } else {
                        let tokens = query_tokens(&q);
                        // Keep `self.section` valid — if the current section
                        // is no longer visible (neither its sidebar label/group
                        // nor any of its fields match), switch to the first
                        // section that still has a match.
                        let entries = sidebar_entries();
                        let current_visible = entries.iter().any(|e| {
                            e.section == self.section
                                && section_matches(e.section, e.group, e.label, &tokens)
                        });
                        if !current_visible {
                            if let Some(first) = entries
                                .iter()
                                .find(|e| section_matches(e.section, e.group, e.label, &tokens))
                            {
                                self.section = first.section;
                            }
                        }
                        // Deep search: find the first field in declaration order
                        // that matches all tokens and jump to it.
                        if let Some(entry) = search_index()
                            .iter()
                            .find(|e| tokens_match_label(e.label, &tokens))
                        {
                            self.section = entry.section;
                            self.pending_highlight = Some((entry.section, entry.label));
                            self.highlight_started = Some(std::time::Instant::now());
                            self.highlight_scrolled = false;
                        } else {
                            self.pending_highlight = None;
                            self.highlight_started = None;
                            self.highlight_scrolled = false;
                        }
                    }
                }
                ui.add_space(6.0);

                // About is pinned to the very bottom of the sidebar even
                // when the search is active.
                egui::TopBottomPanel::bottom("sidebar_about")
                    .frame(egui::Frame::default().inner_margin(egui::Margin::ZERO))
                    .show_separator_line(false)
                    .show_inside(ui, |ui| {
                        let q = self.sidebar_search.to_lowercase();
                        let tokens = query_tokens(&q);
                        // Only render About when the search matches it,
                        // or when the search is empty. Deep search also
                        // surfaces About when any of its fields match.
                        if section_matches(Section::About, "About", "About", &tokens) {
                            ui.add_space(4.0);
                            let section_before = self.section;
                            sidebar_link(
                                ui,
                                &mut self.section,
                                Section::About,
                                crate::icons::glyph(
                                    ABOUT_ICON,
                                    self.config.appearance.bundled_icons,
                                ),
                                "About",
                            );
                            if self.section != section_before {
                                self.pending_highlight = None;
                                self.highlight_started = None;
                                self.highlight_scrolled = false;
                            }
                        }
                    });

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        let q = self.sidebar_search.to_lowercase();
                        let tokens = query_tokens(&q);
                        let entries = sidebar_entries();
                        // Walk the entries once, emitting a group label only
                        // when the group has at least one visible child after
                        // filtering.
                        let mut last_group: Option<&'static str> = None;
                        let section_before = self.section;
                        for entry in entries {
                            if !section_matches(entry.section, entry.group, entry.label, &tokens) {
                                continue;
                            }
                            if last_group != Some(entry.group) {
                                sidebar_group_label(ui, entry.group);
                                last_group = Some(entry.group);
                            }
                            sidebar_link(
                                ui,
                                &mut self.section,
                                entry.section,
                                crate::icons::glyph(
                                    entry.icon,
                                    self.config.appearance.bundled_icons,
                                ),
                                entry.label,
                            );
                        }
                        // If the user manually clicked a sidebar entry (section
                        // changed by a click, not by the search handler), clear
                        // the pending highlight so it doesn't haunt the new section.
                        if self.section != section_before {
                            self.pending_highlight = None;
                            self.highlight_started = None;
                            self.highlight_scrolled = false;
                        }
                    });
            });

        // ── Pinned save / status bar ──
        // Lives in a bottom panel *outside* the scroll area so it stays
        // visible at any window height. The scrollable section content sits in
        // the CentralPanel above it.
        egui::TopBottomPanel::bottom("save_bar")
            .resizable(false)
            .min_height(56.0)
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(13, 15, 22))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(28, 33, 48)))
                    .inner_margin(egui::Margin::symmetric(28.0, 10.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Status text on the left.
                    if let Some((kind, msg)) = &self.status {
                        let color = match kind {
                            StatusKind::Success => egui::Color32::from_rgb(120, 220, 140),
                            StatusKind::Warning => egui::Color32::from_rgb(230, 200, 110),
                            StatusKind::Error => egui::Color32::from_rgb(230, 110, 110),
                        };
                        ui.label(egui::RichText::new(msg).color(color));
                    } else if self.dirty {
                        // U+2022 bullet — present in egui's proportional
                        // Ubuntu face (unlike U+25CF, which lives only in
                        // the monospace Hack face and renders as tofu here).
                        ui.label(
                            egui::RichText::new("\u{2022} Unsaved changes")
                                .color(egui::Color32::from_rgb(230, 200, 110)),
                        );
                    } else {
                        // U+2714 heavy check — covered by the bundled
                        // NotoEmoji; U+2713 is in none of egui's fonts.
                        ui.label(
                            egui::RichText::new("\u{2714} Up to date")
                                .color(egui::Color32::from_rgb(120, 130, 160)),
                        );
                    }

                    // Save button right-aligned.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                self.dirty,
                                egui::Button::new(
                                    egui::RichText::new("  💾  Save changes  ")
                                        .strong()
                                        .color(egui::Color32::WHITE),
                                )
                                .fill(egui::Color32::from_rgb(60, 110, 230))
                                .min_size(egui::vec2(0.0, 34.0))
                                .rounding(0.0),
                            )
                            .clicked()
                        {
                            save_now = true;
                        }
                    });
                });
            });

        // ── Main content (scrollable) ──
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(11, 13, 18))
                    .inner_margin(egui::Margin::symmetric(28.0, 24.0)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .show(ui, |ui| {
                        // Stretch the content to the full panel width so rows
                        // and cards span edge-to-edge and the scrollbar pins to
                        // the right border instead of hugging narrow content.
                        ui.set_width(ui.available_width());
                        // Detect navigation into a different section so
                        // per-visit caches (the workspace list) refresh once on
                        // entry rather than scanning the disk every frame.
                        if self.section != self.last_section {
                            self.last_section = self.section;
                            self.workspace_cache_dirty = true;
                        }
                        match self.section {
                            Section::Profiles => self.section_profiles(ui),
                            Section::Appearance => self.section_appearance(ui),
                            Section::Font => self.section_font(ui),
                            Section::Cursor => self.section_cursor(ui),
                            Section::Window => self.section_window(ui),
                            Section::Terminal => self.section_terminal(ui),
                            Section::Gpu => self.section_gpu(ui),
                            Section::Bell => self.section_bell(ui),
                            Section::Quake => self.section_quake(ui),
                            Section::Ssh => self.section_ssh(ui),
                            Section::Backup => self.section_backup(ui),
                            Section::Shortcuts => self.section_shortcuts(ui),
                            Section::Ai => self.section_ai(ui),
                            Section::Plugins => self.section_plugins(ui),
                            Section::QuickSelect => self.section_quick_select(ui),
                            Section::StatusBar => self.section_status_bar(ui),
                            Section::Snippets => self.section_snippets(ui),
                            Section::ContextRules => self.section_context_rules(ui),
                            Section::Workspaces => self.section_workspaces(ui),
                            Section::ClipboardHistory => self.section_clipboard_history(ui),
                            Section::DirectoryJump => self.section_directory_jump(ui),
                            Section::Integration => self.section_integration(ui),
                            Section::About => self.section_about(ui),
                        }
                    });
            });

        if save_now {
            self.save();
        }
    }

    fn build_title_bar(&mut self, ctx: &egui::Context) {
        let mut minimize = false;
        let mut toggle_max = false;
        let mut close = false;
        let mut start_drag = false;
        let mut dbl_max = false;

        egui::TopBottomPanel::top("title_bar")
            .exact_height(34.0)
            .resizable(false)
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(8, 10, 16))
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ctx, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), terminale_render::WINDOW_CTRL_HEIGHT),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        // Zero gap between the three title buttons — the
                        // Windows convention is buttons that touch.
                        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                        // Close button right-most.
                        if title_button(ui, TitleIcon::Close).clicked() {
                            close = true;
                        }
                        let max_icon = if self.cached_maximized {
                            TitleIcon::Restore
                        } else {
                            TitleIcon::Maximize
                        };
                        if title_button(ui, max_icon).clicked() {
                            toggle_max = true;
                        }
                        if title_button(ui, TitleIcon::Minimize).clicked() {
                            minimize = true;
                        }

                        // Drag area = the rest of the row.
                        let drag_size =
                            egui::vec2(ui.available_width(), terminale_render::WINDOW_CTRL_HEIGHT);
                        let drag_resp =
                            ui.allocate_response(drag_size, egui::Sense::click_and_drag());
                        if drag_resp.is_pointer_button_down_on() {
                            start_drag = true;
                        }
                        if drag_resp.double_clicked() {
                            dbl_max = true;
                        }
                        let drag_rect = drag_resp.rect;
                        ui.painter().text(
                            egui::pos2(drag_rect.left() + 14.0, drag_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            "terminale — settings",
                            egui::FontId::new(13.0, egui::FontFamily::Proportional),
                            egui::Color32::from_rgb(190, 200, 230),
                        );
                    },
                );
            });

        if minimize {
            self.window.set_minimized(true);
        }
        if toggle_max || dbl_max {
            // Toggle from the cached state (avoids an extra `is_maximized()`
            // round-trip); the authoritative refresh still happens on the
            // resulting `Resized` event.
            let max = !self.cached_maximized;
            self.window.set_maximized(max);
            self.cached_maximized = max;
        }
        if start_drag {
            let _ = self.window.drag_window();
        }
        if close {
            self.requested_close = true;
        }
    }

    // ──────────────────────────────────────────────────────────────────
    // Sections
    // ──────────────────────────────────────────────────────────────────

    /// Convenience wrapper around [`maybe_highlight_row`] that forwards
    /// the window's highlight state. Call after a `ui.horizontal(…)`
    /// block that renders a field, passing the captured `row_rect`.
    fn highlight_row(
        &mut self,
        ui: &mut egui::Ui,
        row_rect: egui::Rect,
        section: Section,
        label: &str,
    ) {
        maybe_highlight_row(
            ui,
            row_rect,
            section,
            label,
            &mut self.pending_highlight,
            &mut self.highlight_started,
            &mut self.highlight_scrolled,
            &mut self.next_repaint,
        );
    }

    // ──────────────────────────────────────────────────────────────────
    // Actions
    // ──────────────────────────────────────────────────────────────────

    fn add_blank_profile(&mut self) {
        let n = self.config.profiles.profiles.len();
        self.config.profiles.profiles.push(Profile {
            name: format!("New profile {}", n + 1),
            command: if cfg!(windows) {
                "cmd.exe".into()
            } else {
                "/bin/bash".into()
            },
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            icon: Some(ICON_PRESETS[0].1.to_string()),
        });
        self.dirty = true;
    }

    fn save(&mut self) {
        match self.config.write_to(&self.config_path) {
            Ok(()) => {
                self.dirty = false;
                self.status = Some((StatusKind::Success, "Settings saved.".into()));
            }
            Err(e) => {
                tracing::warn!(?e, "settings save failed");
                self.status = Some((StatusKind::Error, format!("Save failed: {e}")));
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────

/// Icon glyph for the bottom-pinned About entry (kept as a module
/// constant so the search filter and the sidebar both reference the same
/// value).
const ABOUT_ICON: &crate::icons::Icon = &crate::icons::BOOK;

/// One row in the sidebar — a group label, an icon (Tabler + legacy pair),
/// the human label, and the matching [`Section`].
#[derive(Clone, Copy)]
struct SidebarEntry {
    group: &'static str,
    section: Section,
    icon: &'static crate::icons::Icon,
    label: &'static str,
}

/// Every sidebar entry in display order. Centralised so the global
/// search box (which filters this list) and the sidebar UI walk the
/// same data — no risk of the filter referring to a label the sidebar
/// doesn't actually render.
///
/// Each entry stores a reference to a [`crate::icons::Icon`] so the
/// sidebar can render the Tabler glyph (`bundled_icons = true`) or the
/// legacy emoji (`bundled_icons = false`) via [`crate::icons::glyph`].
fn sidebar_entries() -> &'static [SidebarEntry] {
    use crate::icons;
    &[
        SidebarEntry {
            group: "General",
            section: Section::Profiles,
            icon: &icons::FOLDER,
            label: "Profiles",
        },
        SidebarEntry {
            group: "General",
            section: Section::Ssh,
            icon: &icons::WORLD,
            label: "SSH hosts",
        },
        SidebarEntry {
            group: "General",
            section: Section::Backup,
            icon: &icons::PACKAGE,
            label: "Backup",
        },
        SidebarEntry {
            group: "Look & feel",
            section: Section::Appearance,
            icon: &icons::PALETTE,
            label: "Appearance",
        },
        SidebarEntry {
            group: "Look & feel",
            section: Section::Font,
            icon: &icons::TYPOGRAPHY,
            label: "Font",
        },
        SidebarEntry {
            group: "Look & feel",
            section: Section::Cursor,
            icon: &icons::EDIT,
            label: "Cursor",
        },
        SidebarEntry {
            group: "Terminal",
            section: Section::Terminal,
            icon: &icons::TERMINAL,
            label: "Terminal",
        },
        SidebarEntry {
            group: "Terminal",
            section: Section::Window,
            icon: &icons::PHOTO,
            label: "Window",
        },
        SidebarEntry {
            group: "Terminal",
            section: Section::Bell,
            icon: &icons::BELL,
            label: "Bell",
        },
        SidebarEntry {
            group: "Modes",
            section: Section::Quake,
            icon: &icons::DOWNLOAD,
            label: "Quake",
        },
        SidebarEntry {
            group: "Input",
            section: Section::Shortcuts,
            icon: &icons::KEY,
            label: "Shortcuts",
        },
        SidebarEntry {
            group: "Input",
            section: Section::QuickSelect,
            icon: &icons::SEARCH,
            label: "Quick select",
        },
        SidebarEntry {
            group: "Integrations",
            section: Section::Ai,
            icon: &icons::AI,
            label: "AI",
        },
        SidebarEntry {
            group: "Integrations",
            section: Section::Plugins,
            icon: &icons::PLUG,
            label: "Plugins",
        },
        SidebarEntry {
            group: "System",
            section: Section::Gpu,
            icon: &icons::GAMEPAD,
            label: "GPU",
        },
        SidebarEntry {
            group: "Look & feel",
            section: Section::StatusBar,
            icon: &icons::CHART_BAR,
            label: "Status bar",
        },
        SidebarEntry {
            group: "Input",
            section: Section::Snippets,
            icon: &icons::CLIPBOARD,
            label: "Snippets",
        },
        SidebarEntry {
            group: "Terminal",
            section: Section::ContextRules,
            icon: &icons::TAGS,
            label: "Context rules",
        },
        SidebarEntry {
            group: "General",
            section: Section::Workspaces,
            icon: &icons::FLOPPY,
            label: "Workspaces",
        },
        SidebarEntry {
            group: "Input",
            section: Section::ClipboardHistory,
            icon: &icons::FOLDER,
            label: "Clipboard history",
        },
        SidebarEntry {
            group: "Input",
            section: Section::DirectoryJump,
            icon: &icons::MAP,
            label: "Directory jump",
        },
        SidebarEntry {
            group: "General",
            section: Section::Integration,
            icon: &icons::PLUG,
            label: "Desktop integration",
        },
    ]
}

/// One entry in the deep-search index — a field label exactly as
/// passed to `field_label()` and the section it lives in.
///
/// The label is `&'static str` so the index is zero-cost at runtime and
/// the highlight check in `highlight_field` is a pointer-equality-
/// friendly string compare against the same literals.
#[derive(Clone, Copy)]
struct SearchEntry {
    /// The [`Section`] this field belongs to.
    section: Section,
    /// Exact label string as passed to `field_label()` in the
    /// corresponding `section_*` function.
    label: &'static str,
}

/// Static index of every field label across all `section_*` functions.
///
/// IMPORTANT: every `label` here MUST match verbatim the string literal
/// passed to `field_label()` in the corresponding section function so
/// that the highlight pulse fires correctly. A unit test (`test_search_index_labels`)
/// asserts this invariant at compile time via `include_str!`.
#[must_use]
#[allow(clippy::too_many_lines)]
fn search_index() -> &'static [SearchEntry] {
    &[
        // section_profiles
        SearchEntry {
            section: Section::Profiles,
            label: "Default profile",
        },
        // section_profiles -> profile_card
        SearchEntry {
            section: Section::Profiles,
            label: "Shell",
        },
        SearchEntry {
            section: Section::Profiles,
            label: "Command path",
        },
        SearchEntry {
            section: Section::Profiles,
            label: "Arguments",
        },
        SearchEntry {
            section: Section::Profiles,
            label: "Working dir",
        },
        // section_appearance
        SearchEntry {
            section: Section::Appearance,
            label: "Theme",
        },
        // section_appearance -> Theme import
        SearchEntry {
            section: Section::Appearance,
            label: "Themes directory",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Import theme",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Tab min width",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Tab max width",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Pinned tab width",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Show pane headers",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Activity spinner on busy tabs",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Tear out panes",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Divider thickness",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Divider grab padding",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Divider colour",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Focus border thickness",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Focus border colour",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Dim inactive panes",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Dim unfocused window",
        },
        // section_appearance -> Background image
        SearchEntry {
            section: Section::Appearance,
            label: "Image path",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Image fit",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Image opacity",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Image brightness",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Image saturation",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Image hue",
        },
        // section_appearance -> Background FX
        SearchEntry {
            section: Section::Appearance,
            label: "Enable background FX",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Background style",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Background intensity",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Background speed",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "React to keystrokes",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Band lifetime",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Matrix band width",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Matrix fall speed",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Max concurrent bands",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Custom colors",
        },
        // section_appearance -> Close button style
        SearchEntry {
            section: Section::Appearance,
            label: "Close button style",
        },
        // section_appearance -> Text rendering
        SearchEntry {
            section: Section::Appearance,
            label: "Faint/dim intensity",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Minimum contrast",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Builtin box drawing",
        },
        // section_appearance -> Tab bar
        SearchEntry {
            section: Section::Appearance,
            label: "Tab bar enabled",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Tab bar position",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Vertical tab bar width",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Hide tab bar if single tab",
        },
        SearchEntry {
            section: Section::Appearance,
            label: "Show tab group labels",
        },
        // section_appearance -> Icons
        SearchEntry {
            section: Section::Appearance,
            label: "Use bundled icon set",
        },
        // section_font
        SearchEntry {
            section: Section::Font,
            label: "Font family",
        },
        SearchEntry {
            section: Section::Font,
            label: "Bold font",
        },
        SearchEntry {
            section: Section::Font,
            label: "Italic font",
        },
        SearchEntry {
            section: Section::Font,
            label: "Bold-italic font",
        },
        SearchEntry {
            section: Section::Font,
            label: "Font size",
        },
        SearchEntry {
            section: Section::Font,
            label: "Line height",
        },
        SearchEntry {
            section: Section::Font,
            label: "Ligatures",
        },
        SearchEntry {
            section: Section::Font,
            label: "Underline thickness",
        },
        SearchEntry {
            section: Section::Font,
            label: "Cell width",
        },
        // section_cursor
        SearchEntry {
            section: Section::Cursor,
            label: "Style",
        },
        SearchEntry {
            section: Section::Cursor,
            label: "Blink",
        },
        SearchEntry {
            section: Section::Cursor,
            label: "Rate",
        },
        SearchEntry {
            section: Section::Cursor,
            label: "Thickness",
        },
        SearchEntry {
            section: Section::Cursor,
            label: "Opacity",
        },
        SearchEntry {
            section: Section::Cursor,
            label: "Cell tint",
        },
        SearchEntry {
            section: Section::Cursor,
            label: "Custom colour",
        },
        SearchEntry {
            section: Section::Cursor,
            label: "Colour",
        },
        SearchEntry {
            section: Section::Cursor,
            label: "Blink ease",
        },
        SearchEntry {
            section: Section::Cursor,
            label: "Animation FPS",
        },
        // section_window
        SearchEntry {
            section: Section::Window,
            label: "Opacity",
        },
        SearchEntry {
            section: Section::Window,
            label: "Padding",
        },
        SearchEntry {
            section: Section::Window,
            label: "Confirm close",
        },
        SearchEntry {
            section: Section::Window,
            label: "Stay on top",
        },
        SearchEntry {
            section: Section::Window,
            label: "Startup position",
        },
        SearchEntry {
            section: Section::Window,
            label: "Auto reload config",
        },
        // section_window -> Zen mode
        SearchEntry {
            section: Section::Window,
            label: "Enter full-screen",
        },
        // section_terminal
        SearchEntry {
            section: Section::Terminal,
            label: "Scroll step",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Alt-screen scroll step",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Trackpad pixels per row",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Smooth scroll",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Scrollback",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Copy on select",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Word separators",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Underline links",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Link hover tooltip",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Link hover delay",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Resize panes live while dragging",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Keyboard pane resize step",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Open file links with",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Command",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Prompt marks in gutter",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "OS notifications",
        },
        // ux-polish-a: exit behavior and hyperlink rules
        SearchEntry {
            section: Section::Terminal,
            label: "Exit behavior",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Hyperlink rules",
        },
        // inline image protocol toggles (OSC 1337, Sixel, APC graphics)
        SearchEntry {
            section: Section::Terminal,
            label: "Inline image protocols",
        },
        // keyboard encoding / DECCKM mode
        SearchEntry {
            section: Section::Terminal,
            label: "Keyboard encoding",
        },
        // broadcast input scope
        SearchEntry {
            section: Section::Terminal,
            label: "Broadcast input scope",
        },
        // OSC 52 clipboard read permission policy
        SearchEntry {
            section: Section::Terminal,
            label: "Clipboard read",
        },
        // shell integration: command blocks
        SearchEntry {
            section: Section::Terminal,
            label: "Shell integration",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Capture command blocks",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Max command blocks",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Edit command clears line",
        },
        // command-history picker
        SearchEntry {
            section: Section::Terminal,
            label: "History picker scope",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "History picker max entries",
        },
        // scrollback export
        SearchEntry {
            section: Section::Terminal,
            label: "Scrollback export",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Export format",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Export directory",
        },
        // paste safety
        SearchEntry {
            section: Section::Terminal,
            label: "Paste safety",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Confirm multi-line paste",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Confirm when unbracketed",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Strip control characters",
        },
        SearchEntry {
            section: Section::Terminal,
            label: "Highlight on jump",
        },
        // section_quake
        SearchEntry {
            section: Section::Quake,
            label: "Global hotkey",
        },
        SearchEntry {
            section: Section::Quake,
            label: "Dock to edge",
        },
        SearchEntry {
            section: Section::Quake,
            label: "Display",
        },
        SearchEntry {
            section: Section::Quake,
            label: "Size",
        },
        SearchEntry {
            section: Section::Quake,
            label: "Margin",
        },
        SearchEntry {
            section: Section::Quake,
            label: "Hide on focus loss",
        },
        SearchEntry {
            section: Section::Quake,
            label: "Animation",
        },
        SearchEntry {
            section: Section::Quake,
            label: "Duration",
        },
        // section_ssh
        SearchEntry {
            section: Section::Ssh,
            label: "Host key policy",
        },
        SearchEntry {
            section: Section::Ssh,
            label: "known_hosts file",
        },
        SearchEntry {
            section: Section::Ssh,
            label: "Offer to save typed SSH hosts",
        },
        SearchEntry {
            section: Section::Ssh,
            label: "Import SSH config",
        },
        SearchEntry {
            section: Section::Ssh,
            label: "SSH config path",
        },
        SearchEntry {
            section: Section::Ssh,
            label: "Name",
        },
        SearchEntry {
            section: Section::Ssh,
            label: "Host",
        },
        SearchEntry {
            section: Section::Ssh,
            label: "Port",
        },
        SearchEntry {
            section: Section::Ssh,
            label: "User",
        },
        SearchEntry {
            section: Section::Ssh,
            label: "Auth method",
        },
        SearchEntry {
            section: Section::Ssh,
            label: "Key path",
        },
        SearchEntry {
            section: Section::Ssh,
            label: "Password",
        },
        // section_shortcuts (all labels from the groups data structure)
        SearchEntry {
            section: Section::Shortcuts,
            label: "New tab",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Close tab",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Next tab",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Previous tab",
        },
        // Labels here must use the same escape sequences as the shortcuts groups
        // data so include_str!-based invariant tests find them in the source.
        SearchEntry {
            section: Section::Shortcuts,
            label: "Move tab ⬅",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Move tab ➡",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Profile picker",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Restart session",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Reopen closed tab",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "New SSH tab",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Go to last-used tab",
        },
        // Tab-index shortcuts.
        SearchEntry {
            section: Section::Shortcuts,
            label: "Go to tab 1",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Go to tab 2",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Go to tab 3",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Go to tab 4",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Go to tab 5",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Go to tab 6",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Go to tab 7",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Go to tab 8",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Go to last tab (tab 9)",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Copy",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Paste",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Select all",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Find in buffer",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Clear screen",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Clear scrollback",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Enter copy mode",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Quick select",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Pane select",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Open settings",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Increase font",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Decrease font",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Reset font",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Toggle stay on top",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Reload config",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Snap top half",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Snap bottom half",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Snap left half",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Snap right half",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Center on monitor",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Maximize to monitor",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Snap top-left quarter",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Snap top-right quarter",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Snap bottom-left quarter",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Snap bottom-right quarter",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Show snap layout chooser",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Split pane right",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Split pane down",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Split pane left",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Split pane up",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Close focused pane",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Toggle broadcast input",
        },
        // Pane focus shortcuts.
        SearchEntry {
            section: Section::Shortcuts,
            label: "Focus pane left",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Focus pane right",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Focus pane up",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Focus pane down",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Toggle pane zoom",
        },
        // Pane resize shortcuts.
        SearchEntry {
            section: Section::Shortcuts,
            label: "Resize pane left",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Resize pane right",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Resize pane up",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Resize pane down",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "AI assistant",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Command palette",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Explain selection (AI)",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Fix last failed command (AI)",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Line up",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Line down",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Page up",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Page down",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Jump to top",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Jump to bottom",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Jump to previous prompt",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Jump to next prompt",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Export scrollback to file",
        },
        // section_shortcuts -> block actions group
        SearchEntry {
            section: Section::Shortcuts,
            label: "Block actions",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Copy last command output",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Copy block output",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Copy last command",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Re-run last command",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Edit last command",
        },
        // section_shortcuts -> pane arrangement group
        SearchEntry {
            section: Section::Shortcuts,
            label: "Move pane left (swap)",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Move pane right (swap)",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Move pane up (swap)",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Move pane down (swap)",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Rotate panes forward",
        },
        SearchEntry {
            section: Section::Shortcuts,
            label: "Rotate panes backward",
        },
        // section_shortcuts -> custom multi-action keybinds
        SearchEntry {
            section: Section::Shortcuts,
            label: "Custom keybinds",
        },
        // section_shortcuts -> modal key-tables (leader mode)
        SearchEntry {
            section: Section::Shortcuts,
            label: "Key tables (leader mode)",
        },
        // section_shortcuts -> custom mouse bindings
        SearchEntry {
            section: Section::Shortcuts,
            label: "Custom mouse bindings",
        },
        // section_ai
        SearchEntry {
            section: Section::Ai,
            label: "Default provider",
        },
        SearchEntry {
            section: Section::Ai,
            label: "Render markdown",
        },
        SearchEntry {
            section: Section::Ai,
            label: "Offer fix on failure",
        },
        // section_ai -> Command suggestions
        SearchEntry {
            section: Section::Ai,
            label: "Command suggestions",
        },
        SearchEntry {
            section: Section::Ai,
            label: "When to suggest",
        },
        SearchEntry {
            section: Section::Ai,
            label: "Idle delay",
        },
        SearchEntry {
            section: Section::Ai,
            label: "Context lines",
        },
        SearchEntry {
            section: Section::Ai,
            label: "API key",
        },
        SearchEntry {
            section: Section::Ai,
            label: "Model",
        },
        SearchEntry {
            section: Section::Ai,
            label: "Max tokens",
        },
        SearchEntry {
            section: Section::Ai,
            label: "Endpoint",
        },
        // section_plugins
        SearchEntry {
            section: Section::Plugins,
            label: "Enabled",
        },
        SearchEntry {
            section: Section::Plugins,
            label: "Directory",
        },
        SearchEntry {
            section: Section::Plugins,
            label: "Allow scrollback read",
        },
        SearchEntry {
            section: Section::Plugins,
            label: "Scrollback read cap",
        },
        SearchEntry {
            section: Section::Plugins,
            label: "Allow plugin keybindings",
        },
        SearchEntry {
            section: Section::Plugins,
            label: "Loaded plugins",
        },
        // section_gpu
        SearchEntry {
            section: Section::Gpu,
            label: "Backend",
        },
        SearchEntry {
            section: Section::Gpu,
            label: "Power",
        },
        // section_bell
        SearchEntry {
            section: Section::Bell,
            label: "Mode",
        },
        // section_backup
        SearchEntry {
            section: Section::Backup,
            label: "Passphrase",
        },
        SearchEntry {
            section: Section::Backup,
            label: "Confirm",
        },
        // section_about
        SearchEntry {
            section: Section::About,
            label: "Diagnostics",
        },
        SearchEntry {
            section: Section::About,
            label: "Config file",
        },
        // section_quick_select
        SearchEntry {
            section: Section::QuickSelect,
            label: "Label alphabet",
        },
        SearchEntry {
            section: Section::QuickSelect,
            label: "Regex patterns",
        },
        SearchEntry {
            section: Section::QuickSelect,
            label: "Overlay dim",
        },
        SearchEntry {
            section: Section::QuickSelect,
            label: "Quick select",
        },
        SearchEntry {
            section: Section::QuickSelect,
            label: "Pane select",
        },
        // section_status_bar
        SearchEntry {
            section: Section::StatusBar,
            label: "Enable status bar",
        },
        SearchEntry {
            section: Section::StatusBar,
            label: "Status bar position",
        },
        SearchEntry {
            section: Section::StatusBar,
            label: "Left segments",
        },
        SearchEntry {
            section: Section::StatusBar,
            label: "Right segments",
        },
        SearchEntry {
            section: Section::StatusBar,
            label: "Update interval",
        },
        // section_snippets
        SearchEntry {
            section: Section::Snippets,
            label: "Name",
        },
        SearchEntry {
            section: Section::Snippets,
            label: "Description",
        },
        SearchEntry {
            section: Section::Snippets,
            label: "Body",
        },
        // section_context_rules
        SearchEntry {
            section: Section::ContextRules,
            label: "Rule name",
        },
        SearchEntry {
            section: Section::ContextRules,
            label: "Host glob",
        },
        SearchEntry {
            section: Section::ContextRules,
            label: "Cwd glob",
        },
        SearchEntry {
            section: Section::ContextRules,
            label: "Tab color",
        },
        SearchEntry {
            section: Section::ContextRules,
            label: "Badge",
        },
        // section_workspaces
        SearchEntry {
            section: Section::Workspaces,
            label: "Restore session",
        },
        SearchEntry {
            section: Section::Workspaces,
            label: "Restore working dirs",
        },
        SearchEntry {
            section: Section::Workspaces,
            label: "Workspaces list",
        },
        // section_clipboard_history
        SearchEntry {
            section: Section::ClipboardHistory,
            label: "Enable clipboard history",
        },
        SearchEntry {
            section: Section::ClipboardHistory,
            label: "History size",
        },
        SearchEntry {
            section: Section::ClipboardHistory,
            label: "Capture OSC 52 writes",
        },
        // section_directory_jump
        SearchEntry {
            section: Section::DirectoryJump,
            label: "Enable directory jump",
        },
        SearchEntry {
            section: Section::DirectoryJump,
            label: "Max tracked directories",
        },
        SearchEntry {
            section: Section::DirectoryJump,
            label: "Persist history to disk",
        },
        // section_integration
        SearchEntry {
            section: Section::Integration,
            label: "Register application-menu entry",
        },
    ]
}

/// Split a lowercase query string into whitespace-separated tokens.
///
/// Used by the deep-search engine: all tokens must be present in a label
/// for it to match (AND logic).
fn query_tokens(query_lower: &str) -> Vec<&str> {
    query_lower.split_whitespace().collect()
}

/// True when ALL tokens in `tokens` are substrings of `label` (case-insensitive,
/// caller must pass a lowercase label for correct results).
fn tokens_match_label(label: &str, tokens: &[&str]) -> bool {
    if tokens.is_empty() {
        return true;
    }
    let label_lower = label.to_lowercase();
    tokens.iter().all(|t| label_lower.contains(t))
}

/// True when a sidebar entry is visible for the given query tokens.
///
/// Matches when:
/// - `tokens` is empty (show everything), OR
/// - any token matches the entry's `group` or `label`, OR
/// - any field in [`search_index()`] whose section equals `section`
///   has a label that contains all tokens (deep field search).
fn section_matches(section: Section, group: &str, label: &str, tokens: &[&str]) -> bool {
    if tokens.is_empty() {
        return true;
    }
    let group_lower = group.to_lowercase();
    let label_lower = label.to_lowercase();
    // Quick check: does the sidebar entry's own label/group match?
    let sidebar_hit = tokens.iter().all(|t| group_lower.contains(t))
        || tokens.iter().all(|t| label_lower.contains(t));
    if sidebar_hit {
        return true;
    }
    // Deep check: does any field in this section match?
    search_index()
        .iter()
        .any(|e| e.section == section && tokens_match_label(e.label, tokens))
}

fn sidebar_link(
    ui: &mut egui::Ui,
    current: &mut Section,
    target: Section,
    icon: &str,
    label: &str,
) {
    let selected = *current == target;
    // Denser sidebar — 28px rows (was 34) — and a square, no-rounding hit
    // area so the whole Settings UI reads as "tools panel" not "design
    // showcase".
    let desired = egui::vec2(196.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let bg = if selected {
            egui::Color32::from_rgb(38, 50, 92)
        } else if hovered {
            egui::Color32::from_rgb(24, 30, 48)
        } else {
            egui::Color32::TRANSPARENT
        };
        ui.painter().rect(rect, 0.0, bg, egui::Stroke::NONE);

        // Accent bar on left when selected (square, no rounding).
        if selected {
            let bar_rect = egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.top() + 4.0),
                egui::vec2(3.0, rect.height() - 8.0),
            );
            ui.painter()
                .rect_filled(bar_rect, 0.0, egui::Color32::from_rgb(110, 160, 240));
        }

        let fg = if selected {
            egui::Color32::WHITE
        } else if hovered {
            egui::Color32::from_rgb(220, 230, 250)
        } else {
            egui::Color32::from_rgb(170, 180, 210)
        };
        // Icon glyph (NotoEmoji is loaded by the host on Settings init, so
        // U+1F4D1-class symbols render correctly here).
        ui.painter().text(
            egui::pos2(rect.left() + 14.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            icon,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
            fg,
        );
        ui.painter().text(
            egui::pos2(rect.left() + 36.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
            fg,
        );
    }

    if response.clicked() {
        *current = target;
    }
}

/// Small uppercase caption that introduces a group of sidebar links.
fn sidebar_group_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .small()
            .strong()
            .color(egui::Color32::from_rgb(95, 105, 135)),
    );
    ui.add_space(1.0);
}

fn page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(
        egui::RichText::new(title)
            .heading()
            .size(20.0)
            .color(egui::Color32::from_rgb(230, 235, 250)),
    );
    if !subtitle.is_empty() {
        ui.label(
            egui::RichText::new(subtitle)
                .small()
                .color(egui::Color32::from_rgb(140, 150, 175)),
        );
    }
    ui.add_space(10.0);
}

fn card(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::default()
        .fill(egui::Color32::from_rgb(18, 22, 32))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(32, 38, 54)))
        // Square — no rounding anywhere in Settings.
        .rounding(0.0)
        // Denser — was (18, 14) px symmetric.
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
        .show(ui, |ui| {
            // Stretch the card to the full available width so it spans the
            // panel edge-to-edge instead of shrinking to its content.
            ui.set_min_width(ui.available_width());
            body(ui);
        });
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(140.0, 22.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(
                egui::RichText::new(text)
                    .color(egui::Color32::from_rgb(195, 205, 230))
                    .strong(),
            );
        },
    );
}

/// Accent colour used for the search-highlight pulse outline.
const HIGHLIGHT_COLOR: egui::Color32 = egui::Color32::from_rgb(91, 176, 255);

/// Duration (ms) of the highlight fade-out animation.
const HIGHLIGHT_DURATION_MS: f32 = 800.0;

/// Total time after which the highlight is cleared entirely (ms).
const HIGHLIGHT_LIFETIME_MS: f32 = 1200.0;

/// Check whether a field row should be highlighted and, if so, draw
/// the highlight pulse and schedule a repaint.
///
/// Call this AFTER the `ui.horizontal(…)` block that renders the field.
/// Pass the `rect` returned by `InnerResponse::response.rect` (or
/// `ui.min_rect()` captured after the block) as `row_rect`.
///
/// The function is a free function (not a method) to avoid borrow
/// conflicts: the caller passes the three highlight-state fields
/// individually.
#[allow(clippy::too_many_arguments)]
fn maybe_highlight_row(
    ui: &mut egui::Ui,
    row_rect: egui::Rect,
    section: Section,
    label: &str,
    pending_highlight: &mut Option<(Section, &'static str)>,
    highlight_started: &mut Option<std::time::Instant>,
    highlight_scrolled: &mut bool,
    next_repaint: &mut Option<std::time::Instant>,
) {
    // Is this the highlighted field?
    let is_target = matches!(*pending_highlight, Some((s, l)) if s == section && l == label);
    if !is_target {
        return;
    }
    let Some(started) = *highlight_started else {
        return;
    };
    let elapsed_ms = started.elapsed().as_millis() as f32;
    if elapsed_ms > HIGHLIGHT_LIFETIME_MS {
        *pending_highlight = None;
        *highlight_started = None;
        *highlight_scrolled = false;
        return;
    }

    // Auto-scroll: fire once on the first frame the field paints.
    if !*highlight_scrolled {
        ui.scroll_to_rect(row_rect, Some(egui::Align::Center));
        *highlight_scrolled = true;
    }

    // Fade-out alpha 1.0 → 0.0 over HIGHLIGHT_DURATION_MS.
    let alpha = (1.0 - (elapsed_ms / HIGHLIGHT_DURATION_MS).min(1.0)).max(0.0);

    // Inflate the rect so the outline sits outside the content.
    let inflated = row_rect.expand(3.0);

    let outline_alpha = (alpha * 255.0) as u8;
    let outline_color = egui::Color32::from_rgba_unmultiplied(
        HIGHLIGHT_COLOR.r(),
        HIGHLIGHT_COLOR.g(),
        HIGHLIGHT_COLOR.b(),
        outline_alpha,
    );

    // Brief flash fill for the first 150 ms.
    if elapsed_ms < 150.0 {
        let fill_alpha = (alpha * 0.15 * 255.0) as u8;
        let fill_color = egui::Color32::from_rgba_unmultiplied(
            HIGHLIGHT_COLOR.r(),
            HIGHLIGHT_COLOR.g(),
            HIGHLIGHT_COLOR.b(),
            fill_alpha,
        );
        ui.painter().rect_filled(inflated, 0.0, fill_color);
    }

    // 2 px outline.
    ui.painter()
        .rect_stroke(inflated, 0.0, egui::Stroke::new(2.0, outline_color));

    // Keep scheduling repaints while the animation is live.
    let deadline = started + std::time::Duration::from_millis(HIGHLIGHT_LIFETIME_MS as u64);
    *next_repaint = Some(match *next_repaint {
        Some(existing) => existing.min(deadline),
        None => deadline,
    });
}

fn sublabel(ui: &mut egui::Ui, text: &str) {
    // Feature-description text under each control. egui's `.small()` read as
    // too tiny, so size it relative to Body: 90% of Body plus one point
    // (default Body 14.0 → 13.6pt) — still secondary, but legible, and it
    // tracks the configured body font size.
    let size = sublabel_size(ui);
    ui.label(
        egui::RichText::new(text)
            .size(size)
            .color(egui::Color32::from_rgb(120, 130, 160)),
    );
}

/// The font size `sublabel` renders at — exposed so description-like text
/// that needs a custom color (e.g. warnings) can match it exactly.
fn sublabel_size(ui: &egui::Ui) -> f32 {
    ui.style()
        .text_styles
        .get(&egui::TextStyle::Body)
        .map_or(14.0, |f| f.size * 0.9 + 1.0)
}

/// A small bold sub-header inside a settings page — used to group related
/// controls (e.g. "Split panes") without a full `page_header`.
fn section_subheader(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .strong()
            .color(egui::Color32::from_rgb(180, 190, 220)),
    );
}

#[allow(clippy::too_many_arguments)]
fn profile_card(
    ui: &mut egui::Ui,
    idx: usize,
    profile: &mut Profile,
    is_default: bool,
    detected_shells: &[Profile],
    dirty_flag: &mut bool,
    mut remove: impl FnMut(),
    mut duplicate: impl FnMut(),
) {
    let outline = if is_default {
        egui::Stroke::new(2.0, egui::Color32::from_rgb(80, 130, 240))
    } else {
        egui::Stroke::new(1.0, egui::Color32::from_rgb(32, 38, 54))
    };

    egui::Frame::default()
        .fill(if is_default {
            egui::Color32::from_rgb(24, 32, 56)
        } else {
            egui::Color32::from_rgb(18, 22, 32)
        })
        .stroke(outline)
        .rounding(0.0)
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
        .show(ui, |ui| {
            // Stretch the card to the full available width so it spans the
            // panel edge-to-edge instead of shrinking to its content.
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                // Icon picker (combo).
                let mut chosen_icon: Option<String> = None;
                egui::ComboBox::from_id_salt(("icon", idx))
                    .selected_text(profile.icon.clone().unwrap_or_else(|| "·".into()))
                    .width(60.0)
                    .show_ui(ui, |ui| {
                        for (label, glyph) in ICON_PRESETS {
                            if ui
                                .selectable_label(
                                    profile.icon.as_deref() == Some(*glyph),
                                    format!("{glyph}  {label}"),
                                )
                                .clicked()
                            {
                                chosen_icon = Some((*glyph).to_string());
                            }
                        }
                    });
                if let Some(glyph) = chosen_icon {
                    profile.icon = Some(glyph);
                    *dirty_flag = true;
                }
                ui.add_space(6.0);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut profile.name)
                        .desired_width(220.0)
                        .font(egui::TextStyle::Heading),
                );
                if r.changed() {
                    *dirty_flag = true;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("🗑")
                                    .color(egui::Color32::from_rgb(220, 130, 130)),
                            )
                            .fill(egui::Color32::from_rgb(40, 26, 30))
                            .rounding(0.0),
                        )
                        .on_hover_text("Remove this profile")
                        .clicked()
                    {
                        remove();
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                // U+1F4D1 (bookmark tabs) — covered by the
                                // bundled NotoEmoji. U+29C9 is in none of
                                // egui's fonts and rendered as a tofu box.
                                egui::RichText::new("\u{1F4D1}")
                                    .color(egui::Color32::from_rgb(180, 190, 220)),
                            )
                            .fill(egui::Color32::from_rgb(28, 32, 46))
                            .rounding(0.0),
                        )
                        .on_hover_text("Duplicate")
                        .clicked()
                    {
                        duplicate();
                    }
                    let badge_text = if is_default {
                        "★ Default"
                    } else {
                        "Make default"
                    };
                    let badge_fill = if is_default {
                        egui::Color32::from_rgb(60, 110, 230)
                    } else {
                        egui::Color32::from_rgb(28, 32, 46)
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(badge_text).color(egui::Color32::WHITE),
                            )
                            .fill(badge_fill)
                            .rounding(0.0),
                        )
                        .clicked()
                        && !is_default
                    {
                        // Communicate via the dirty flag — the caller picks up the
                        // selected name by re-reading after this method.
                        // Simpler: signal by setting an out-of-band sentinel via
                        // the icon? No — caller has &mut on us, so we mutate a side
                        // channel. Here we just leave `is_default` untouched and
                        // the caller (section_profiles) listens on click via
                        // selectable_label in the top combo. Instead we track via
                        // a stored field on the profile — there's none — so we
                        // expose a hidden mechanism: shove "DEFAULT_REQUEST" into
                        // a special field. To keep things simple, set the name
                        // intact and let user pick from the top combo.
                        *dirty_flag = true;
                    }
                    let _ = badge_text;
                });
            });

            ui.add_space(8.0);

            // Shell picker.
            ui.horizontal(|ui| {
                field_label(ui, "Shell");
                let mut chosen: Option<(String, Vec<String>)> = None;
                egui::ComboBox::from_id_salt(("shell", idx))
                    .selected_text(short_path(&profile.command))
                    .width(360.0)
                    .show_ui(ui, |ui| {
                        for shell in detected_shells {
                            if ui
                                .selectable_label(
                                    profile.command == shell.command,
                                    format!("{}  {}", shell.name, short_path(&shell.command)),
                                )
                                .clicked()
                            {
                                chosen = Some((shell.command.clone(), shell.args.clone()));
                            }
                        }
                        ui.separator();
                        ui.label(
                            egui::RichText::new("Or enter a custom command path below.")
                                .small()
                                .color(egui::Color32::from_rgb(140, 150, 175)),
                        );
                    });
                if let Some((cmd, args)) = chosen {
                    profile.command = cmd;
                    if !args.is_empty() {
                        profile.args = args;
                    }
                    *dirty_flag = true;
                }
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                field_label(ui, "Command path");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut profile.command)
                        .desired_width(360.0)
                        .font(egui::TextStyle::Monospace),
                );
                if r.changed() {
                    *dirty_flag = true;
                }
            });

            ui.add_space(4.0);
            // Quote-aware join/split so an argument that legitimately contains
            // a space (a path, `-c "echo hi"`) round-trips intact instead of
            // being shattered into multiple argv slots.
            let mut args_joined = join_args(&profile.args);
            ui.horizontal(|ui| {
                field_label(ui, "Arguments");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut args_joined)
                        .desired_width(360.0)
                        .hint_text("arguments — quote ones containing spaces"),
                );
                if r.changed() {
                    profile.args = split_args(&args_joined);
                    *dirty_flag = true;
                }
            });

            ui.add_space(4.0);
            let mut cwd_str = profile
                .cwd
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            ui.horizontal(|ui| {
                field_label(ui, "Working dir");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut cwd_str)
                        .desired_width(360.0)
                        .hint_text("leave empty to inherit"),
                );
                if r.changed() {
                    profile.cwd = if cwd_str.is_empty() {
                        None
                    } else {
                        Some(cwd_str.clone().into())
                    };
                    *dirty_flag = true;
                }
            });

            ui.add_space(4.0);
            // Environment variables — one KEY=VALUE per line. This editor was
            // missing entirely: `profile.env` was config-file-only.
            //
            // While the editor has focus the text lives in egui temp memory so
            // incomplete lines ("FOO" with no `=` yet) survive between frames;
            // only complete KEY=VALUE lines are committed to `profile.env`.
            // On blur the text is rebuilt (sorted) from the canonical map.
            let env_id = ui.make_persistent_id(("profile-env-edit", idx));
            let rebuilt = {
                let mut lines = profile
                    .env
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>();
                lines.sort();
                lines.join("\n")
            };
            let focused = ui.memory(|m| m.has_focus(env_id));
            let mut env_joined = if focused {
                ui.data(|d| d.get_temp::<String>(env_id)).unwrap_or(rebuilt)
            } else {
                rebuilt
            };
            ui.horizontal_top(|ui| {
                field_label(ui, "Environment");
                let r = ui.add(
                    egui::TextEdit::multiline(&mut env_joined)
                        .id(env_id)
                        .desired_width(360.0)
                        .desired_rows(2)
                        .font(egui::TextStyle::Monospace)
                        .hint_text("KEY=VALUE, one per line"),
                );
                if r.changed() {
                    profile.env = env_joined
                        .lines()
                        .filter_map(|l| {
                            let (k, v) = l.trim().split_once('=')?;
                            let k = k.trim();
                            if k.is_empty() {
                                return None;
                            }
                            Some((k.to_string(), v.to_string()))
                        })
                        .collect();
                    *dirty_flag = true;
                }
            });
            ui.data_mut(|d| d.insert_temp(env_id, env_joined));
        });
}

/// Join argv entries into a single editable line, double-quoting any entry
/// that contains whitespace or quotes (escaping embedded `"` as `\"`).
fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.is_empty() || a.chars().any(char::is_whitespace) || a.contains('"') {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Split an arguments line into argv entries, honouring double/single quotes
/// (and `\"` escapes inside double quotes) so quoted arguments keep their
/// spaces. Inverse of [`join_args`].
fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some('"') => match c {
                '"' => quote = None,
                '\\' if chars.peek() == Some(&'"') => {
                    cur.push('"');
                    chars.next();
                }
                _ => cur.push(c),
            },
            Some(_) => {
                if c == '\'' {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    in_token = true;
                }
                c if c.is_whitespace() => {
                    if in_token {
                        out.push(std::mem::take(&mut cur));
                        in_token = false;
                    }
                }
                _ => {
                    cur.push(c);
                    in_token = true;
                }
            },
        }
    }
    if in_token {
        out.push(cur);
    }
    out
}

/// Which title-bar icon to paint. Thin alias kept for the call sites
/// — the actual geometry comes from
/// [`terminale_render::system_icons::SystemIcon`].
type TitleIcon = terminale_render::system_icons::SystemIcon;

/// One window-control button. Draws the *exact* same geometry as the
/// main window's title bar by consuming the shared
/// [`terminale_render::system_icons`] spec.
fn title_button(ui: &mut egui::Ui, icon: TitleIcon) -> egui::Response {
    use terminale_render::system_icons::{
        icon_lines, SystemIcon, BG_CLOSE_HOVER, BG_HOVER, STROKE_CLOSE_HOVER, STROKE_DEFAULT,
        STROKE_PX,
    };
    let desired = egui::vec2(
        terminale_render::WINDOW_CTRL_WIDTH,
        terminale_render::WINDOW_CTRL_HEIGHT,
    );
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let is_close = matches!(icon, SystemIcon::Close);
        let bg_rgb = if is_close && hovered {
            Some(BG_CLOSE_HOVER)
        } else if hovered {
            Some(BG_HOVER)
        } else {
            None
        };
        if let Some(rgb) = bg_rgb {
            ui.painter()
                .rect_filled(rect, 0.0, egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
        }

        let stroke_rgb = if is_close && hovered {
            STROKE_CLOSE_HOVER
        } else {
            STROKE_DEFAULT
        };
        let stroke = egui::Stroke::new(
            STROKE_PX,
            egui::Color32::from_rgb(stroke_rgb[0], stroke_rgb[1], stroke_rgb[2]),
        );
        let c = rect.center();
        for line in icon_lines(icon, c.x, c.y) {
            ui.painter().line_segment(
                [
                    egui::pos2(line.from.0, line.from.1),
                    egui::pos2(line.to.0, line.to.1),
                ],
                stroke,
            );
        }
    }
    response
}

fn color_swatch(ui: &mut egui::Ui, rgb: [u8; 3], tooltip: &str) {
    let size = egui::vec2(18.0, 18.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect(
            rect,
            4.0,
            egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
            egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 46, 60)),
        );
    }
    response.on_hover_text(tooltip);
}

fn short_path(path: &str) -> String {
    let p = std::path::Path::new(path);
    p.file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// A draws-as-pill toggle. Caller checks `clicked()` to flip the source value.
fn toggle_switch(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let desired_size = egui::vec2(44.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, on);
        let radius = 0.5 * rect.height();
        let bg_color = if on {
            egui::Color32::from_rgb(60, 130, 230)
        } else {
            egui::Color32::from_rgb(50, 55, 75)
        };
        ui.painter()
            .rect(rect, radius, bg_color, egui::Stroke::NONE);
        let knob_x = if on {
            rect.right() - radius
        } else {
            rect.left() + radius
        };
        let knob_pos = egui::pos2(knob_x, rect.center().y);
        ui.painter().circle(
            knob_pos,
            radius - 3.0,
            egui::Color32::WHITE,
            visuals.fg_stroke,
        );
    }
    response
}

fn configure_visuals(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.window_fill = egui::Color32::from_rgb(11, 13, 18);
    style.visuals.panel_fill = egui::Color32::from_rgb(11, 13, 18);
    style.visuals.window_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(32, 38, 54));
    style.visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(18, 22, 32);
    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(28, 32, 44);
    style.visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 46, 64));
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(70, 100, 170);
    style.visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(50, 70, 130);
    style.visuals.widgets.hovered.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(110, 150, 230));
    style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(80, 120, 200);
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(60, 110, 230);
    style.visuals.override_text_color = Some(egui::Color32::from_rgb(220, 226, 240));
    // Square everything — no rounded corners anywhere.
    style.visuals.window_rounding = 0.0.into();
    style.visuals.menu_rounding = 0.0.into();
    style.visuals.widgets.noninteractive.rounding = 0.0.into();
    style.visuals.widgets.inactive.rounding = 0.0.into();
    style.visuals.widgets.hovered.rounding = 0.0.into();
    style.visuals.widgets.active.rounding = 0.0.into();
    style.visuals.widgets.open.rounding = 0.0.into();
    // Denser default spacing — was (10, 8) item and (14, 6) button.
    style.spacing.item_spacing = egui::vec2(8.0, 4.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.slider_width = 200.0;
    style.spacing.combo_width = 280.0;
    ctx.set_style(style);
}

fn segment_kind_label(seg: &terminale_config::StatusSegment) -> &'static str {
    use terminale_config::StatusSegment::*;
    match seg {
        Cwd => "cwd",
        Clock { .. } => "clock",
        Profile => "profile",
        TabIndex => "tab_index",
        UserVar { .. } => "user_var",
        Literal { .. } => "literal",
        Spacer => "spacer",
    }
}

fn bell_mode_label(mode: terminale_config::BellMode) -> &'static str {
    use terminale_config::BellMode::*;
    match mode {
        Visual => "Visual flash",
        Audio => "System beep",
        Both => "Visual + audio",
        None => "Silenced",
    }
}

fn ai_provider_label(value: &str) -> &'static str {
    match value {
        "claude" => "Anthropic Claude",
        "openai" => "OpenAI",
        "ollama" => "Ollama (local)",
        _ => "Custom",
    }
}

/// Render a click-to-record hotkey button. When the user clicks it,
/// the next key combination they press becomes the binding. Esc
/// cancels recording; clicking again also cancels. Empty bindings
/// are shown as "(disabled)".
///
/// `id` uniquely identifies *this* widget so multiple recorders in the
/// same view don't fight over the focus. Returns `true` if the binding
/// changed.
fn hotkey_recorder(
    ui: &mut egui::Ui,
    id: &str,
    binding: &mut String,
    recording: &mut Option<String>,
) -> bool {
    let is_recording = recording.as_deref() == Some(id);
    let label = if is_recording {
        "  Press a key…  ".to_string()
    } else if binding.is_empty() {
        "  (disabled)  ".to_string()
    } else {
        format!("  {binding}  ")
    };

    let bg = if is_recording {
        egui::Color32::from_rgb(80, 40, 50)
    } else {
        egui::Color32::from_rgb(28, 32, 46)
    };
    let stroke = if is_recording {
        egui::Stroke::new(1.5, egui::Color32::from_rgb(232, 70, 90))
    } else {
        egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 56, 78))
    };

    let resp = ui.add(
        egui::Button::new(
            egui::RichText::new(&label)
                .monospace()
                .color(egui::Color32::from_rgb(220, 226, 240)),
        )
        .min_size(egui::vec2(220.0, 28.0))
        .fill(bg)
        .stroke(stroke)
        .rounding(0.0),
    );

    let mut changed = false;

    if resp.clicked() {
        if is_recording {
            *recording = None;
        } else {
            *recording = Some(id.to_string());
        }
    }

    if is_recording {
        // Hold focus on the recording button so key events (notably
        // Tab) don't leak into egui's focus navigation while we're
        // capturing the binding.
        resp.request_focus();
        // Drain key events and convert the first non-modifier press
        // into a binding string. Esc cancels.
        let captured = ui.ctx().input(|i| {
            for ev in &i.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    repeat: false,
                    ..
                } = ev
                {
                    if *key == egui::Key::Escape && !modifiers.any() {
                        return Some(None);
                    }
                    if let Some(name) = egui_key_name(*key) {
                        let mut parts: Vec<&str> = Vec::new();
                        if modifiers.ctrl {
                            parts.push("Ctrl");
                        }
                        if modifiers.shift {
                            parts.push("Shift");
                        }
                        if modifiers.alt {
                            parts.push("Alt");
                        }
                        // Only the PHYSICAL Cmd key — NOT `modifiers.command`,
                        // which on Windows/Linux aliases Ctrl and would
                        // emit a phantom "Cmd" on every Ctrl combo.
                        if modifiers.mac_cmd {
                            parts.push("Cmd");
                        }
                        parts.push(name);
                        return Some(Some(parts.join("+")));
                    }
                }
            }
            None
        });

        if let Some(result) = captured {
            if let Some(new_binding) = result {
                *binding = new_binding;
                changed = true;
            } else {
                // User pressed Escape — cancel without changing.
            }
            *recording = None;
        }
    }

    changed
}

/// Map egui's [`egui::Key`] enum onto the same names accepted by
/// `parse_keycode` in the host. Returns `None` for keys that are
/// modifiers only or that we don't bind.
fn egui_key_name(key: egui::Key) -> Option<&'static str> {
    use egui::Key;
    Some(match key {
        Key::A => "A",
        Key::B => "B",
        Key::C => "C",
        Key::D => "D",
        Key::E => "E",
        Key::F => "F",
        Key::G => "G",
        Key::H => "H",
        Key::I => "I",
        Key::J => "J",
        Key::K => "K",
        Key::L => "L",
        Key::M => "M",
        Key::N => "N",
        Key::O => "O",
        Key::P => "P",
        Key::Q => "Q",
        Key::R => "R",
        Key::S => "S",
        Key::T => "T",
        Key::U => "U",
        Key::V => "V",
        Key::W => "W",
        Key::X => "X",
        Key::Y => "Y",
        Key::Z => "Z",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        Key::F1 => "F1",
        Key::F2 => "F2",
        Key::F3 => "F3",
        Key::F4 => "F4",
        Key::F5 => "F5",
        Key::F6 => "F6",
        Key::F7 => "F7",
        Key::F8 => "F8",
        Key::F9 => "F9",
        Key::F10 => "F10",
        Key::F11 => "F11",
        Key::F12 => "F12",
        Key::ArrowUp => "ArrowUp",
        Key::ArrowDown => "ArrowDown",
        Key::ArrowLeft => "ArrowLeft",
        Key::ArrowRight => "ArrowRight",
        Key::Space => "Space",
        Key::Enter => "Enter",
        Key::Tab => "Tab",
        Key::Backspace => "Backspace",
        Key::Delete => "Delete",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "PageUp",
        Key::PageDown => "PageDown",
        Key::Insert => "Insert",
        Key::Backtick => "`",
        Key::Minus => "-",
        Key::Equals => "=",
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        Key::Backslash => "\\",
        Key::Semicolon => ";",
        Key::Quote => "'",
        Key::Comma => ",",
        Key::Period => ".",
        Key::Slash => "/",
        _ => return None,
    })
}

const SETTINGS_RESIZE_BORDER: f32 = 5.0;

fn detect_window_resize_edge(
    logical_x: f32,
    logical_y: f32,
    window: &Window,
    maximized: bool,
) -> Option<winit::window::ResizeDirection> {
    use winit::window::ResizeDirection::*;
    // `maximized` is passed in (the cached flag) rather than read via
    // `window.is_maximized()`: this runs on every CursorMoved over the window,
    // and that getter round-trips through `-[NSWindow setStyleMask:]` on macOS
    // (expensive — see `cached_maximized`).
    if maximized || window.fullscreen().is_some() {
        return None;
    }
    let size = window.inner_size();
    let scale = window.scale_factor() as f32;
    let w = size.width as f32 / scale;
    let h = size.height as f32 / scale;
    let b = SETTINGS_RESIZE_BORDER;
    let on_left = logical_x <= b;
    let on_right = logical_x >= w - b;
    let on_top = logical_y <= b;
    let on_bot = logical_y >= h - b;
    match (on_top, on_bot, on_left, on_right) {
        (true, _, true, _) => Some(NorthWest),
        (true, _, _, true) => Some(NorthEast),
        (_, true, true, _) => Some(SouthWest),
        (_, true, _, true) => Some(SouthEast),
        (true, _, _, _) => Some(North),
        (_, true, _, _) => Some(South),
        (_, _, true, _) => Some(West),
        (_, _, _, true) => Some(East),
        _ => None,
    }
}

fn cursor_icon_for_resize_settings(
    dir: winit::window::ResizeDirection,
) -> winit::window::CursorIcon {
    use winit::window::CursorIcon;
    use winit::window::ResizeDirection::*;
    match dir {
        North | South => CursorIcon::NsResize,
        East | West => CursorIcon::EwResize,
        NorthEast | SouthWest => CursorIcon::NeswResize,
        NorthWest | SouthEast => CursorIcon::NwseResize,
    }
}

#[cfg(windows)]
fn set_dwm_cloak(window: &Window, cloaked: bool) {
    use std::ffi::c_void;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut c_void,
            dwAttribute: u32,
            pvAttribute: *const c_void,
            cbAttribute: u32,
        ) -> i32;
    }
    const DWMWA_CLOAK: u32 = 13;

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = h.hwnd.get() as *mut c_void;
    let value: i32 = i32::from(cloaked);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            std::ptr::from_ref::<i32>(&value) as *const c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assert that every label in [`search_index()`] appears as a
    /// literal string somewhere across the settings source files.
    ///
    /// After the per-section modularisation each `field_label("Foo")` call
    /// lives in its own sub-module file (`settings_window/<section>.rs`).
    /// The test therefore concatenates the root file *and* all section
    /// files before counting occurrences, so the invariant is:
    ///   - exactly one occurrence in `search_index()` (in this root file)
    ///   - exactly one occurrence at the `field_label()` / label call site
    ///     (in one of the section files)
    ///     → combined count >= 2.
    ///
    /// This catches the common drift where someone renames a
    /// `field_label("Foo")` call without updating the search index,
    /// silently breaking highlight and search for that field.
    #[test]
    fn test_search_index_labels_present_in_source() {
        // Root file (contains search_index() with one occurrence per label).
        let root = include_str!("settings_window.rs");
        // All per-section sub-module files (each contains the field_label()
        // call site — the second occurrence we need).
        let sections = concat!(
            include_str!("settings_window/profiles.rs"),
            include_str!("settings_window/appearance.rs"),
            include_str!("settings_window/font.rs"),
            include_str!("settings_window/cursor.rs"),
            include_str!("settings_window/window.rs"),
            include_str!("settings_window/terminal.rs"),
            include_str!("settings_window/quake.rs"),
            include_str!("settings_window/ssh.rs"),
            include_str!("settings_window/shortcuts.rs"),
            include_str!("settings_window/ai.rs"),
            include_str!("settings_window/plugins.rs"),
            include_str!("settings_window/gpu.rs"),
            include_str!("settings_window/bell.rs"),
            include_str!("settings_window/quick_select.rs"),
            include_str!("settings_window/snippets.rs"),
            include_str!("settings_window/status_bar.rs"),
            include_str!("settings_window/context_rules.rs"),
            include_str!("settings_window/about.rs"),
            include_str!("settings_window/backup.rs"),
            include_str!("settings_window/workspaces.rs"),
            include_str!("settings_window/clipboard_history.rs"),
            include_str!("settings_window/directory_jump.rs"),
            include_str!("settings_window/integration.rs"),
        );
        // Use a single owned string so `matches()` scans one contiguous slice.
        let combined = format!("{root}{sections}");
        let mut missing: Vec<&'static str> = Vec::new();
        for entry in search_index() {
            // Each label must appear at least twice in the combined source:
            // once in the search_index() literal and once at the field_label /
            // label call site.
            let occurrences = combined.matches(entry.label).count();
            if occurrences < 2 {
                missing.push(entry.label);
            }
        }
        assert!(
            missing.is_empty(),
            "search_index() labels not found in field_label() calls (< 2 occurrences):\n  {}",
            missing.join("\n  ")
        );
    }

    /// Smoke-test that `query_tokens` splits correctly.
    #[test]
    fn test_query_tokens_basic() {
        assert_eq!(query_tokens("stay on top"), vec!["stay", "on", "top"]);
        assert_eq!(query_tokens("scrollback"), vec!["scrollback"]);
        assert_eq!(query_tokens("  "), Vec::<&str>::new());
    }

    /// Smoke-test that `tokens_match_label` implements AND logic.
    #[test]
    fn test_tokens_match_label() {
        let tokens = query_tokens("stay top");
        assert!(tokens_match_label("Stay on top", &tokens));
        assert!(!tokens_match_label("Dock to edge", &tokens));
    }

    /// Verify that `section_matches` surfaces deep fields even when the
    /// sidebar label/group don't match.
    #[test]
    fn test_section_matches_deep() {
        // "scrollback" is a field in Terminal but not in the sidebar
        // label/group ("Terminal" / "Terminal").
        let tokens = query_tokens("scrollback");
        // Terminal section — should match via search_index deep check.
        assert!(section_matches(
            Section::Terminal,
            "Terminal",
            "Terminal",
            &tokens
        ));
        // Appearance section — should NOT match.
        assert!(!section_matches(
            Section::Appearance,
            "Look & feel",
            "Appearance",
            &tokens
        ));
    }
}
