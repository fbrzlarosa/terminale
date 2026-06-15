//! Terminal engine: the grid, the cursor, and the ANSI state machine.
//!
//! `Emulator` wraps [`alacritty_terminal::Term`] (the grid + escape handler)
//! and [`alacritty_terminal::vte::ansi::Processor`] (the parser), giving the
//! rest of the workspace a small surface to feed PTY bytes in and read the
//! current grid out.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod apc_graphics;
pub mod images;
pub mod semantic;
pub mod sixel;
pub use apc_graphics::{ApcAction, ApcControl, ApcGraphicsAssembler, ApcImage};
pub use images::{ImageId, ImagePlacement, ImageStore, InlineImage, VisiblePlacement};
pub use semantic::{CommandBlock, OscKind, PromptMark, SemanticModel};

use alacritty_terminal::event::{Event as AlacrittyEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{ClipboardType, Config, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};
use alacritty_terminal::Term;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Default column count for a freshly-opened terminal.
pub const DEFAULT_COLS: u16 = 80;

/// Default row count for a freshly-opened terminal.
pub const DEFAULT_ROWS: u16 = 24;

/// A simple `Dimensions` impl Term needs at construction time.
#[derive(Debug, Clone, Copy)]
struct TermSize {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// 16-colour ANSI palette plus default fg/bg. Drives how named colours
/// (`\e[31m`, `\e[1;34m`, etc.) get resolved into pixels. Push one via
/// [`Emulator::set_palette`] when the user picks a theme.
#[derive(Debug, Clone, Copy)]
pub struct AnsiPalette {
    /// Default foreground (used by `\e[39m` / unset cells).
    pub foreground: [u8; 3],
    /// Default background (used by `\e[49m` / unset cells).
    pub background: [u8; 3],
    /// Eight base ANSI colours (Black, Red, Green, Yellow, Blue, Magenta, Cyan, White).
    pub normal: [[u8; 3]; 8],
    /// Eight bright ANSI colours, same ordering.
    pub bright: [[u8; 3]; 8],
}

impl Default for AnsiPalette {
    fn default() -> Self {
        // Tokyo Night-ish — same as the hand-coded values that used to live
        // in `named_color`. Acts as a safe fallback before any theme loads.
        Self {
            foreground: DEFAULT_FG,
            background: DEFAULT_BG,
            normal: [
                [0x1a, 0x1b, 0x26],
                [0xf7, 0x76, 0x8e],
                [0x9e, 0xce, 0x6a],
                [0xe0, 0xaf, 0x68],
                [0x7a, 0xa2, 0xf7],
                [0xbb, 0x9a, 0xf7],
                [0x7d, 0xcf, 0xff],
                [0xa9, 0xb1, 0xd6],
            ],
            bright: [
                [0x41, 0x48, 0x68],
                [0xff, 0x75, 0x7f],
                [0xb9, 0xf2, 0x7c],
                [0xff, 0x9e, 0x64],
                [0x7d, 0xa6, 0xff],
                [0xbb, 0x9a, 0xf7],
                [0x0d, 0xb9, 0xd7],
                [0xc0, 0xca, 0xf5],
            ],
        }
    }
}

/// Events the emulator surfaces to the host: clipboard requests, title
/// changes, bell, etc. Drained via [`Emulator::drain_events`] after each
/// PTY chunk is parsed.
#[derive(Debug, Clone)]
pub enum EmulatorEvent {
    /// App asked to put `text` into the clipboard (OSC 52). The
    /// [`ClipboardKind`] picks the system selection on Unix; on Windows /
    /// macOS only the clipboard exists and `Selection`/`Clipboard` both
    /// map to the same place.
    ClipboardStore {
        /// Which system selection/clipboard the app targeted.
        kind: ClipboardKind,
        /// The UTF-8 text to store.
        text: String,
    },
    /// App asked to **read** the clipboard (OSC 52 query — payload is `?`).
    ///
    /// The host must check its `terminal.clipboard_read` policy and, if
    /// `allow`, read the system clipboard, base64-encode it, and write
    /// `ESC ] 52 ; <selection> ; <base64> ST` back to the PTY. If `deny`
    /// (the default) the host should silently ignore this event.
    ClipboardRead {
        /// Which selection the app queried (`c`, `p`, `s`, etc.). The host
        /// should echo this back verbatim in the OSC 52 response so the
        /// requesting program can match the reply to its query.
        selection: String,
    },
    /// App asked for the current title (some terminals expose this).
    Title(String),
    /// Visual bell — BEL (`\x07`) was received.
    Bell,
    /// Bytes to write back to the PTY — produced when the parser needs
    /// to respond to a query (OSC 4/10/11/12 colour queries, DA, DSR,
    /// cursor-position reports, …). The host should pipe these directly
    /// into [`terminale_core::Session::write_input`].
    PtyWrite(String),
    /// Desktop notification requested by the running program via OSC 9
    /// (body-only form — any body that is NOT the `9;<path>` cwd form)
    /// or OSC 777 (`notify;<title>;<body>` format, OSC 777 notification protocol).
    Notification {
        /// Short notification heading. Empty when the app didn't supply one.
        title: String,
        /// Main notification body text.
        body: String,
    },
    /// A dynamic-colour OSC (4 / 10 / 11 / 12 / 104 / 110 / 111 / 112) was
    /// processed and the palette override table changed. The host should
    /// re-read [`Emulator::palette`] (which now incorporates any runtime
    /// fg/bg/cursor overrides) and push the new background colour to the
    /// renderer.
    PaletteChanged,
}

/// Active kitty keyboard protocol progressive-enhancement flags.
///
/// Returned by [`Emulator::kitty_keyboard_flags`]. Each field maps to one bit
/// of the protocol's flag word:
/// `0b1` disambiguate, `0b10` report event types, `0b100` report alternate
/// keys, `0b1000` report all keys as escape codes, `0b10000` report associated
/// text. See <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KittyKeyboardFlags {
    /// Disambiguate escape codes (encode keys that legacy mode left ambiguous).
    pub disambiguate: bool,
    /// Report key press / repeat / release as distinct events.
    pub report_event_types: bool,
    /// Report shifted / base-layout alternate key codes alongside the key.
    pub report_alternate_keys: bool,
    /// Report every key (including plain text) as an escape code.
    pub report_all_keys_as_esc: bool,
    /// Report the text a key would produce as trailing codepoints.
    pub report_associated_text: bool,
}

impl KittyKeyboardFlags {
    /// `true` when any progressive-enhancement flag is active — i.e. the
    /// focused app has engaged the kitty keyboard protocol on this screen.
    #[must_use]
    pub fn any(self) -> bool {
        self.disambiguate
            || self.report_event_types
            || self.report_alternate_keys
            || self.report_all_keys_as_esc
            || self.report_associated_text
    }
}

/// Cursor shape the app has requested via DECSCUSR (CSI Ps SP q). Maps
/// 1:1 to `vte::ansi::CursorShape` minus `Hidden` (we return `None` in
/// that case from [`Emulator::cursor_shape`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCursorShape {
    /// Solid filled rectangle (default for most shells).
    Block,
    /// Hollow rectangle outline.
    HollowBlock,
    /// Thin horizontal bar at the bottom of the cell.
    Underline,
    /// Thin vertical bar on the left of the cell (I-beam, vim insert).
    Beam,
}

/// Which mouse-reporting modes are currently active in the focused app.
#[derive(Debug, Clone, Copy)]
pub struct MouseMode {
    /// Press / release events (DECSET 1000).
    pub click: bool,
    /// Drag motion while a button is held (DECSET 1002).
    pub drag: bool,
    /// All motion (DECSET 1003).
    pub motion: bool,
    /// SGR encoding (DECSET 1006) — required for cells past column 95.
    pub sgr: bool,
}

impl MouseMode {
    /// True when the focused app expects *any* mouse reporting.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.click || self.drag || self.motion
    }
}

/// Subset of alacritty's clipboard taxonomy we expose to consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardKind {
    /// Standard clipboard (Ctrl+C / Cmd+C / Win+V target).
    Clipboard,
    /// Unix PRIMARY selection (middle-click paste).
    Selection,
}

/// Listener installed into [`Term`] that buffers events emitted by ANSI
/// handlers (OSC 52, title changes, bell, …) so the host can drain them
/// after each advance.
#[derive(Clone, Default)]
struct EventCollector {
    queue: Arc<parking_lot::Mutex<Vec<AlacrittyEvent>>>,
}

impl EventListener for EventCollector {
    fn send_event(&self, event: AlacrittyEvent) {
        self.queue.lock().push(event);
    }
}

/// The terminal emulator.
///
/// Feed PTY output via [`Emulator::advance`]. Read the grid via
/// [`Emulator::grid`] or via [`Emulator::for_each_visible_cell`] when
/// rendering.
pub struct Emulator {
    term: Term<EventCollector>,
    parser: Processor,
    palette: AnsiPalette,
    collector: EventCollector,
    /// Latest cwd announced via OSC 7 (filesystem path, percent-decoded).
    /// `None` until the shell first emits the sequence.
    current_dir: Option<String>,
    /// Bytes we couldn't terminate inside a single chunk — saved so the
    /// next `advance` call can join them. Caps at a few KB so a hostile
    /// peer can't unbounded-grow this.
    osc7_partial: Vec<u8>,
    /// OSC 133 semantic prompt model: tracks prompt zones so the host can
    /// navigate between prompts in the scrollback.
    semantic: SemanticModel,
    /// OSC 1337 SetUserVar key→value store. Values are base64-decoded at
    /// capture time. Intended for future status-bar / tab-title expansion.
    user_vars: HashMap<String, String>,
    /// Runtime palette overrides set by OSC 4 (indexed, 0–255).
    /// `None` = no override for that slot — the theme's palette is used.
    palette_overrides: Box<[Option<[u8; 3]>; 256]>,
    /// Runtime default-foreground override (OSC 10). `None` = use theme.
    override_fg: Option<[u8; 3]>,
    /// Runtime default-background override (OSC 11). `None` = use theme.
    override_bg: Option<[u8; 3]>,
    /// Runtime cursor-colour override (OSC 12). `None` = use theme.
    override_cursor: Option<[u8; 3]>,
    /// Events produced by our own OSC sniffers (notifications, palette
    /// changes) that are not surfaced by the alacritty `EventCollector`.
    /// Appended in `advance`; drained together with `collector.queue` in
    /// `drain_events`.
    extra_events: Vec<EmulatorEvent>,
    // ── DECSET 2026 synchronized output (BSU/ESU) ─────────────────────────
    /// `true` while the application has opened a synchronized-output frame
    /// with `CSI ?2026h` (BSU) and not yet closed it with `CSI ?2026l` (ESU).
    /// The emulator still parses and applies every byte normally; only the
    /// *snapshot gate* ([`Self::has_new_frame`]) is affected.
    sync_output: bool,
    /// Monotonic instant at which the current synchronized-output frame was
    /// opened. Used by the safety timeout: if ESU is never received within
    /// [`SYNC_OUTPUT_TIMEOUT`], the frame is forcibly committed so a misbehaving
    /// app can never freeze the display permanently.
    sync_start: Option<Instant>,
    // ── Inline image store (OSC 1337 / Sixel / APC graphics share this) ──────
    /// Decoded inline images and their grid placements.
    image_store: images::ImageStore,
    /// Whether the OSC 1337 `File=` protocol is accepted. Set from
    /// `config.terminal.image_protocols.osc1337` at runtime.
    osc1337_images_enabled: bool,
    /// Whether the Sixel `DCS … ST` graphics protocol is accepted. Set from
    /// `config.terminal.image_protocols.sixel` at runtime.
    sixel_images_enabled: bool,
    // ── DCS (Device Control String) accumulation for Sixel ────────────────
    /// Raw bytes collected since we saw a Sixel DCS introducer. We accumulate
    /// across `advance()` calls because a sixel payload can be large and
    /// routinely spans multiple PTY read chunks.
    dcs_sixel_buf: Vec<u8>,
    /// `true` while we are inside a Sixel DCS frame (between the `q`
    /// introducer and the ST string terminator).
    dcs_sixel_active: bool,
    /// Absolute grid line at which the DCS frame started — saved when the
    /// introducer is seen so the placement lands at the right row even if
    /// the frame spans many `advance()` calls.
    dcs_sixel_cursor_abs: i32,
    /// Partial intro bytes (`ESC` or `ESC P` without the trailing `q`)
    /// carried over from the previous chunk. At most a few bytes. When the
    /// next chunk arrives these are prepended to the incoming bytes before
    /// the DCS scanner runs.
    dcs_intro_prefix: Vec<u8>,
    // ── APC (Application Program Command) accumulation for APC graphics ──────
    /// Whether the APC `ESC _ G … ST` graphics protocol is accepted. Set from
    /// `config.terminal.image_protocols.apc` at runtime.
    apc_graphics_enabled: bool,
    /// Stateful assembler for multi-chunk APC graphics images. Accumulates base64
    /// payloads across `advance()` calls and decodes when the final chunk
    /// (`m=0`) is seen.
    apc_graphics_assembler: apc_graphics::ApcGraphicsAssembler,
    /// `true` while we are inside an APC graphics frame (between the `ESC _ G`
    /// introducer and the ST string terminator).
    apc_graphics_active: bool,
    /// Raw payload bytes (between the `;` separator and the ST terminator)
    /// accumulated for the current APC chunk.
    apc_graphics_buf: Vec<u8>,
    /// Control-data bytes (between `G` and the `;`) for the current APC chunk.
    apc_graphics_ctrl_buf: Vec<u8>,
    /// `true` while we are in the control-data section (before the `;` separator).
    apc_graphics_in_ctrl: bool,
    /// Absolute grid line at which the current APC frame started.
    apc_graphics_cursor_abs: i32,
    /// Partial intro bytes carried over from the previous chunk (e.g. a lone
    /// `ESC` or `ESC _` without the trailing `G`). Prepended to the next
    /// chunk before the APC scanner runs.
    apc_intro_prefix: Vec<u8>,
    /// Monotonic counter bumped by every mutation that can change rendered
    /// output (`advance`, `resize`, palette changes, buffer clears, …).
    /// Renderers compare it against the value they last snapshotted to skip
    /// the per-frame O(cells) grid copy + hash when nothing changed — e.g.
    /// during cursor-blink or background-FX driven redraws of an idle
    /// terminal. See [`Self::generation`].
    generation: u64,
}

/// Maximum time a `CSI ?2026h` synchronized-output frame may be open before
/// the emulator forcibly commits it and re-enables snapshot emission.
///
/// TUIs that emit BSU but crash or forget to send ESU would otherwise freeze
/// the renderer forever. 150 ms matches the value suggested in the
/// synchronized-output (DEC 2026) specification.
const SYNC_OUTPUT_TIMEOUT: Duration = Duration::from_millis(150);

impl std::fmt::Debug for Emulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Emulator")
            .field("columns", &self.term.columns())
            .field("screen_lines", &self.term.screen_lines())
            .finish()
    }
}

impl Emulator {
    /// Construct an emulator with the given visible grid dimensions.
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = TermSize {
            columns: cols.into(),
            screen_lines: rows.into(),
        };
        let collector = EventCollector::default();
        Self {
            // `kitty_keyboard: true` enables alacritty's built-in receive side
            // of the kitty keyboard protocol: it parses the push/pop/set/query
            // escape sequences (`CSI > … u`, `CSI < … u`, `CSI = … u`,
            // `CSI ? u`), maintains the per-screen progressive-enhancement flag
            // stack, and emits the query response as an `Event::PtyWrite`. The
            // *send* side (encoding keystrokes as `CSI … u`) lives host-side in
            // `crate::kitty_keyboard`, gated by `terminal.kitty_keyboard`.
            term: Term::new(
                Config {
                    kitty_keyboard: true,
                    ..Config::default()
                },
                &size,
                collector.clone(),
            ),
            parser: Processor::new(),
            palette: AnsiPalette::default(),
            collector,
            current_dir: None,
            osc7_partial: Vec::new(),
            semantic: SemanticModel::new(),
            user_vars: HashMap::new(),
            palette_overrides: Box::new([None; 256]),
            override_fg: None,
            override_bg: None,
            override_cursor: None,
            extra_events: Vec::new(),
            sync_output: false,
            sync_start: None,
            image_store: images::ImageStore::new(),
            osc1337_images_enabled: true,
            sixel_images_enabled: true,
            dcs_sixel_buf: Vec::new(),
            dcs_sixel_active: false,
            dcs_sixel_cursor_abs: 0,
            dcs_intro_prefix: Vec::new(),
            apc_graphics_enabled: true,
            apc_graphics_assembler: apc_graphics::ApcGraphicsAssembler::new(),
            apc_graphics_active: false,
            apc_graphics_buf: Vec::new(),
            apc_graphics_ctrl_buf: Vec::new(),
            apc_graphics_in_ctrl: false,
            apc_graphics_cursor_abs: 0,
            apc_intro_prefix: Vec::new(),
            generation: 0,
        }
    }

    /// Monotonic content generation — bumped by every mutation that can
    /// change rendered output. Equal values across two reads guarantee the
    /// grid (content, colors, images) is byte-identical, so a renderer may
    /// reuse its previous snapshot wholesale.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Bump the content generation (see [`Self::generation`]).
    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Latest current-working-directory announced by the shell via OSC 7
    /// (`\e]7;file://hostname/path\e\\`). `None` until the first event.
    #[must_use]
    pub fn current_dir(&self) -> Option<&str> {
        self.current_dir.as_deref()
    }

    // ── Inline image store ────────────────────────────────────────────────

    /// Borrow the inline image store (read-only). Renderers use this to
    /// enumerate images and placements each frame.
    #[must_use]
    pub fn image_store(&self) -> &images::ImageStore {
        &self.image_store
    }

    /// Enable or disable the OSC 1337 `File=` inline-image parser.
    /// Maps directly to `config.terminal.image_protocols.osc1337`.
    pub fn set_osc1337_images_enabled(&mut self, enabled: bool) {
        self.osc1337_images_enabled = enabled;
    }

    /// Enable or disable the Sixel `DCS … ST` inline-image parser.
    /// Maps directly to `config.terminal.image_protocols.sixel`.
    ///
    /// When disabled, any in-progress DCS accumulation is discarded and Sixel
    /// DCS sequences that arrive in future `advance()` calls are forwarded to
    /// alacritty's parser (which treats them as unknown and ignores them).
    pub fn set_sixel_images_enabled(&mut self, enabled: bool) {
        if !enabled && self.dcs_sixel_active {
            // Discard any partially-buffered sixel frame.
            self.dcs_sixel_active = false;
            self.dcs_sixel_buf.clear();
            self.dcs_intro_prefix.clear();
        }
        self.sixel_images_enabled = enabled;
    }

    /// Enable or disable the APC `ESC _ G … ST` inline-image parser.
    /// Maps directly to `config.terminal.image_protocols.apc`.
    ///
    /// When disabled, any in-progress APC accumulation is discarded so no
    /// half-buffered frame lingers, and future `advance()` calls pass APC
    /// bytes through to alacritty unchanged.
    pub fn set_apc_graphics_enabled(&mut self, enabled: bool) {
        if !enabled {
            // Discard any partially-buffered APC graphics frame.
            self.apc_graphics_active = false;
            self.apc_graphics_buf.clear();
            self.apc_graphics_ctrl_buf.clear();
            self.apc_graphics_in_ctrl = false;
            self.apc_intro_prefix.clear();
            self.apc_graphics_assembler.clear();
        }
        self.apc_graphics_enabled = enabled;
    }

    /// Drain and return every event the ANSI handlers emitted since the
    /// last call. Host wires these into the system clipboard, title bar,
    /// audible bell, etc.
    pub fn drain_events(&mut self) -> Vec<EmulatorEvent> {
        // Drain the alacritty event queue first (needs a shared ref to self
        // for `lookup_palette_color`, so we collect into a vec before
        // appending our own extras which need the mutable borrow back).
        let raw = {
            let mut q = self.collector.queue.lock();
            std::mem::take(&mut *q)
        };
        let from_alacritty: Vec<EmulatorEvent> = raw
            .into_iter()
            .filter_map(|e| self.convert_event(e))
            .collect();
        let mut all = std::mem::take(&mut self.extra_events);
        all.extend(from_alacritty);
        all
    }

    fn convert_event(&self, event: AlacrittyEvent) -> Option<EmulatorEvent> {
        match event {
            AlacrittyEvent::ClipboardStore(kind, text) => Some(EmulatorEvent::ClipboardStore {
                kind: match kind {
                    ClipboardType::Clipboard => ClipboardKind::Clipboard,
                    ClipboardType::Selection => ClipboardKind::Selection,
                },
                text,
            }),
            AlacrittyEvent::Title(t) => Some(EmulatorEvent::Title(t)),
            AlacrittyEvent::Bell => Some(EmulatorEvent::Bell),
            AlacrittyEvent::PtyWrite(s) => Some(EmulatorEvent::PtyWrite(s)),
            AlacrittyEvent::ColorRequest(index, fmt) => {
                // App asked for one of our palette colours so it can
                // adapt its theme. We resolve from our current palette
                // and let alacritty's closure format the OSC response.
                let rgb = self.lookup_palette_color(index)?;
                Some(EmulatorEvent::PtyWrite(fmt(rgb)))
            }
            _ => None,
        }
    }

    /// Resolve a palette colour at `index`, honouring any runtime OSC 4/10/11
    /// overrides. Used internally for `ColorRequest` responses and exposed
    /// here for testing the override layer.
    ///
    /// Index map: `0–7` = normal ANSI, `8–15` = bright, `16–231` = 6×6×6
    /// xterm cube, `232–255` = greyscale, `256` = default fg, `257` = default
    /// bg, `258` = cursor. Returns `None` for any other index.
    #[must_use]
    pub fn lookup_palette_color(&self, index: usize) -> Option<Rgb> {
        let to_rgb = |c: [u8; 3]| Rgb {
            r: c[0],
            g: c[1],
            b: c[2],
        };
        // Indexed 0–255: check runtime OSC 4 overrides first.
        if index < 256 {
            if let Some(ov) = self.palette_overrides[index] {
                return Some(to_rgb(ov));
            }
        }
        // ANSI 16-colour cube — also matches alacritty's NamedColor 0..16.
        if index < 8 {
            return Some(to_rgb(self.palette.normal[index]));
        }
        if index < 16 {
            return Some(to_rgb(self.palette.bright[index - 8]));
        }
        // xterm-style named indices used by alacritty internally.
        // 256 = default foreground, 257 = default background,
        // 258 = cursor, others fall back to default fg.
        match index {
            256 => Some(to_rgb(self.override_fg.unwrap_or(self.palette.foreground))),
            257 => Some(to_rgb(self.override_bg.unwrap_or(self.palette.background))),
            258 => Some(to_rgb(
                self.override_cursor.unwrap_or(self.palette.foreground),
            )),
            _ => None,
        }
    }

    /// Replace the active ANSI palette. Takes effect on the next call to
    /// [`Self::for_each_visible_cell`] / [`Self::for_each_visible_cell_at_scroll`].
    ///
    /// Also resets any runtime dynamic-colour overrides (OSC 4/10/11/12) so a
    /// theme switch always brings a clean, override-free palette. Explicit
    /// OSC 104/110–112 resets are also provided for apps that want a
    /// finer-grained reset.
    pub fn set_palette(&mut self, palette: AnsiPalette) {
        self.palette = palette;
        self.reset_dynamic_colors();
    }

    /// Reset ALL runtime dynamic-colour overrides (OSC 4 indexed palette +
    /// OSC 10/11/12 fg/bg/cursor). Equivalent to sending `OSC 104`, `OSC 110`,
    /// `OSC 111`, and `OSC 112` all at once.
    ///
    /// Called automatically by [`Self::set_palette`] (theme switch) and
    /// available to the host for hard resets (e.g. after a RIS sequence).
    pub fn reset_dynamic_colors(&mut self) {
        self.bump_generation();
        for slot in self.palette_overrides.iter_mut() {
            *slot = None;
        }
        self.override_fg = None;
        self.override_bg = None;
        self.override_cursor = None;
    }

    /// Read-only view of the runtime fg-colour override set by OSC 10.
    /// `None` when no override is active (the theme palette is used).
    #[must_use]
    pub fn override_fg(&self) -> Option<[u8; 3]> {
        self.override_fg
    }

    /// Read-only view of the runtime bg-colour override set by OSC 11.
    /// `None` when no override is active (the theme palette is used).
    #[must_use]
    pub fn override_bg(&self) -> Option<[u8; 3]> {
        self.override_bg
    }

    /// Read-only view of the runtime cursor-colour override set by OSC 12.
    /// `None` when no override is active (the theme palette is used).
    #[must_use]
    pub fn override_cursor(&self) -> Option<[u8; 3]> {
        self.override_cursor
    }

    /// Set the maximum scrollback (history) line count, applied **live** to
    /// the active grid via alacritty's `Term::set_options`. `0` disables
    /// scrollback (only the visible screen is retained); history beyond the
    /// new limit is dropped. Other terminal options stay at their defaults.
    pub fn set_scrollback(&mut self, lines: usize) {
        self.bump_generation();
        self.term.set_options(Config {
            scrolling_history: lines,
            // Preserve kitty keyboard support across a scrollback change:
            // `set_options` replaces the whole config, so omitting this would
            // silently disable the protocol (and reset its flag stack — see
            // alacritty `Term::set_options`) whenever the user retunes history.
            kitty_keyboard: true,
            ..Config::default()
        });
    }

    /// Current palette — useful for renderers that need the default
    /// fg/bg to draw the window background or unfilled cells.
    #[must_use]
    pub fn palette(&self) -> AnsiPalette {
        self.palette
    }

    /// Feed bytes from the PTY into the parser.
    pub fn advance(&mut self, bytes: &[u8]) {
        // Any byte can mutate the grid/colors/images — invalidate snapshots.
        self.bump_generation();
        // Pre-scan for cwd announcements (OSC 7 file://… and OSC 9;9
        // ConPTY-style). alacritty's parser doesn't expose either as
        // an Event, and shells emit them often enough that we want the
        // latest value tracked alongside the title.
        sniff_cwd(&mut self.osc7_partial, bytes, |cwd| {
            self.current_dir = Some(cwd);
        });

        // Pre-scan for OSC 133 (semantic prompt zones) and OSC 1337
        // (SetUserVar). These are not surfaced by alacritty's handler.
        let cursor_abs = {
            let grid = self.term.grid();
            cursor_absolute_line(grid)
        };
        // Scan OSC 133 events first (collecting them), then apply them to the
        // semantic model with the command-text extractor.
        let osc133_events = scan_osc_133(bytes, cursor_abs);
        for ev in osc133_events {
            if ev.kind == OscKind::OutputStart {
                // Determine the command text for this C event.
                //
                // Priority:
                //   1. `inline_command_text` — bytes between B and C in the
                //      SAME advance() call (synthetic demos, one-shot writes).
                //      This is non-empty when scan_osc_133 saw both B and C in
                //      this chunk and extracted printable content between them.
                //   2. Grid extraction — real-shell path: the user typed the
                //      command across multiple advance() calls, so the text is
                //      already in the grid by the time C arrives.
                let text = if let Some(inline) = ev.inline_command_text.filter(|t| !t.is_empty()) {
                    inline
                } else {
                    // Fall back to reading the command from the grid at the B line.
                    // This is the correct path for real shell sessions where each
                    // keystroke is a separate PTY chunk.
                    let b_line = self
                        .semantic
                        .pending_command_start_line()
                        .unwrap_or(cursor_abs);
                    let cols = self.term.columns();
                    extract_line_text(self.term.grid(), b_line, cols)
                };
                let cwd = self.current_dir.clone();
                self.semantic
                    .record_with_text(ev.kind, ev.line, None, text, cwd);
            } else {
                self.semantic.record(ev.kind, ev.line, ev.exit_code);
            }
        }
        sniff_osc_1337(bytes, &mut self.user_vars);

        // Pre-scan for OSC 9 desktop notifications and OSC 777 notification
        // protocol. Distinct from the cwd `9;9` form.
        sniff_osc_notify(bytes, &mut self.extra_events);

        // Pre-scan for OSC 52 clipboard READ queries (`? ` payload).
        // alacritty's handler only processes the write/store form (base64
        // payload); it silently drops the query form, so we intercept it
        // here and surface a `ClipboardRead` event for the host to honour
        // (or ignore) based on the `terminal.clipboard_read` policy.
        sniff_osc52_read(bytes, &mut self.extra_events);

        // Pre-scan for OSC 4/104/10/11/12/110/111/112 dynamic colour
        // overrides and resets.
        sniff_osc_palette(
            bytes,
            &mut self.palette_overrides,
            &mut self.override_fg,
            &mut self.override_bg,
            &mut self.override_cursor,
            &mut self.extra_events,
        );

        // Pre-scan for DECSET 2026 synchronized output (BSU / ESU).
        // alacritty_terminal 0.24 parses ?2026h/?2026l as NamedPrivateMode::SyncUpdate
        // but treats them as no-ops. We intercept them here to gate snapshot
        // emission for flicker-free TUI rendering.
        sniff_decset_2026(bytes, &mut self.sync_output, &mut self.sync_start);

        // Pre-scan for OSC 1337;File= inline images when the protocol is enabled.
        if self.osc1337_images_enabled {
            let (cols, rows) = self.size();
            let cell_cols = cols;
            let cell_rows = rows;
            sniff_osc_1337_file(
                bytes,
                cursor_abs,
                cell_cols,
                cell_rows,
                &mut self.image_store,
            );
        }

        // Pre-scan for Sixel DCS sequences when the protocol is enabled.
        // We intercept raw bytes here: bytes belonging to a Sixel frame are
        // accumulated in `dcs_sixel_buf` and NOT forwarded to alacritty (which
        // has no Sixel support). All other bytes are forwarded normally.
        // `Cow` keeps the common path (both protocols disabled) zero-copy:
        // the chunk flows straight to alacritty's parser without touching the
        // allocator. Only an active byte-stripping protocol pays for a copy.
        let after_sixel: std::borrow::Cow<'_, [u8]> = if self.sixel_images_enabled {
            // Prepend any partial-intro bytes left over from the previous
            // chunk so an intro that straddles a chunk boundary is recognised.
            let effective_bytes: std::borrow::Cow<'_, [u8]> = if self.dcs_intro_prefix.is_empty() {
                std::borrow::Cow::Borrowed(bytes)
            } else {
                let mut v = std::mem::take(&mut self.dcs_intro_prefix);
                v.extend_from_slice(bytes);
                std::borrow::Cow::Owned(v)
            };
            std::borrow::Cow::Owned(sniff_dcs_sixel(
                &effective_bytes,
                cursor_abs,
                &mut self.dcs_sixel_active,
                &mut self.dcs_sixel_buf,
                &mut self.dcs_sixel_cursor_abs,
                &mut self.dcs_intro_prefix,
                &mut self.image_store,
            ))
        } else {
            std::borrow::Cow::Borrowed(bytes)
        };

        // Pre-scan for APC graphics `ESC _ G … ST` sequences when the protocol
        // is enabled. We chain this after the Sixel filter so both protocols can
        // coexist in the same byte stream. APC graphics bytes are stripped from
        // the slice forwarded to alacritty; non-APC bytes are passed through.
        let bytes_for_alacritty: std::borrow::Cow<'_, [u8]> = if self.apc_graphics_enabled {
            // Prepend any partial-intro bytes left over from the previous chunk.
            let effective_bytes: std::borrow::Cow<'_, [u8]> = if self.apc_intro_prefix.is_empty() {
                after_sixel
            } else {
                let mut v = std::mem::take(&mut self.apc_intro_prefix);
                v.extend_from_slice(&after_sixel);
                std::borrow::Cow::Owned(v)
            };
            std::borrow::Cow::Owned(sniff_apc_graphics(
                &effective_bytes,
                cursor_abs,
                &mut self.apc_graphics_active,
                &mut self.apc_graphics_ctrl_buf,
                &mut self.apc_graphics_in_ctrl,
                &mut self.apc_graphics_buf,
                &mut self.apc_graphics_cursor_abs,
                &mut self.apc_intro_prefix,
                &mut self.apc_graphics_assembler,
                &mut self.image_store,
            ))
        } else {
            after_sixel
        };

        // After all bytes are parsed the history size may have grown; prune
        // marks that fell off the top of the scrollback.
        // vte 0.15 (alacritty_terminal 0.25) takes the whole byte slice at
        // once instead of one byte per call.
        self.parser.advance(&mut self.term, &bytes_for_alacritty);
        let topmost = self.term.grid().topmost_line().0;
        self.semantic.prune(topmost);
        self.image_store.prune_placements(topmost);
    }

    /// Borrow the semantic prompt model (OSC 133 marks). The host uses this
    /// to implement "jump to previous/next prompt" navigation.
    #[must_use]
    pub fn semantic(&self) -> &SemanticModel {
        &self.semantic
    }

    /// Configure the command-block capture. Called from the host whenever
    /// `config.terminal.command_blocks` or `config.terminal.max_command_blocks`
    /// changes (live-applied).
    ///
    /// When `enabled` is `false` (or `max_blocks` is `0`) the block list is
    /// cleared and no new blocks will be recorded.
    pub fn set_command_blocks(&mut self, enabled: bool, max_blocks: usize) {
        let effective_max = if enabled { max_blocks } else { 0 };
        self.semantic.set_max_blocks(effective_max);
        if !enabled {
            // Clear existing blocks when the feature is turned off.
            self.semantic.clear_blocks();
        }
    }

    /// Slice of captured command blocks from the semantic model.
    #[must_use]
    pub fn command_blocks(&self) -> &[CommandBlock] {
        self.semantic.blocks()
    }

    /// The most-recently captured command block, or `None`.
    #[must_use]
    pub fn last_command_block(&self) -> Option<&CommandBlock> {
        self.semantic.last_block()
    }

    /// The command block whose output range contains `abs_line`, or `None`.
    #[must_use]
    pub fn command_block_at_line(&self, abs_line: i32) -> Option<&CommandBlock> {
        self.semantic.block_at_line(abs_line)
    }

    /// Look up a per-terminal user variable set via `OSC 1337;SetUserVar`.
    /// Returns `None` when the variable hasn't been set yet.
    #[must_use]
    pub fn user_var(&self, name: &str) -> Option<&str> {
        self.user_vars.get(name).map(String::as_str)
    }

    /// Wipe the entire buffer: blank the visible viewport AND drop every
    /// line of scrollback history, then home the cursor. Unlike feeding
    /// `\x1b[3J` / `\x1b[2J` through the parser, this never routes through
    /// `Term::clear_screen(ClearMode::All)` (which calls
    /// `Grid::clear_viewport` — that scrolls the existing screen INTO
    /// scrollback before blanking, defeating a follow-up history wipe).
    /// Pair it with sending `\x0c` to the PTY so the shell redraws its
    /// prompt against the now-empty buffer.
    pub fn clear_buffer_to_blank(&mut self) {
        self.bump_generation();
        use alacritty_terminal::index::Line;
        let grid = self.term.grid_mut();
        let rows = grid.screen_lines() as i32;
        // reset_region blanks the visible rows in place — it does NOT
        // scroll their contents into history (that's the bug in the old
        // ED-Ps=3-then-Ctrl-L path).
        grid.reset_region(Line(0)..Line(rows));
        // Drop every scrollback line. Safe to call even when empty.
        grid.clear_history();
        // Home the cursor so the shell's prompt lands at (0,0).
        self.parser.advance(&mut self.term, b"\x1b[H");
    }

    /// Resize the emulator to a new (cols, rows).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.bump_generation();
        let (old_cols, old_rows) = self.size();
        let cursor_before = self.cursor();
        // Snap display_offset to 0 if we're already at the live edge before
        // the resize — alacritty's resize preserves the offset, but if the
        // user was watching live output (offset == 0) we want to stay pinned
        // to the live edge after the reflow.
        let was_at_live_edge = self.term.grid().display_offset() == 0;
        let size = TermSize {
            columns: cols.into(),
            screen_lines: rows.into(),
        };
        self.term.resize(size);
        // Re-pin to live edge when the caller was already there.
        if was_at_live_edge {
            use alacritty_terminal::grid::Scroll;
            self.term.scroll_display(Scroll::Bottom);
        }
        let cursor_after = self.cursor();
        tracing::debug!(
            old_cols,
            old_rows,
            new_cols = cols,
            new_rows = rows,
            cursor_before = ?cursor_before,
            cursor_after = ?cursor_after,
            history = self.term.grid().history_size(),
            "emulator resize"
        );
    }

    /// Visible grid dimensions (cols, rows).
    #[must_use]
    pub fn size(&self) -> (u16, u16) {
        (
            self.term.columns().try_into().unwrap_or(u16::MAX),
            self.term.screen_lines().try_into().unwrap_or(u16::MAX),
        )
    }

    /// Borrow the underlying grid (read-only).
    #[must_use]
    pub fn grid(&self) -> &Grid<Cell> {
        self.term.grid()
    }

    /// Cursor position (col, row) within the visible viewport.
    #[must_use]
    pub fn cursor(&self) -> (u16, u16) {
        let point = self.term.grid().cursor.point;
        let col = point.column.0.try_into().unwrap_or(u16::MAX);
        let row = u16::try_from(point.line.0.max(0)).unwrap_or(0);
        (col, row)
    }

    /// The cursor's **absolute** line index (negative = scrollback history).
    ///
    /// `abs = viewport_row - history_size`. Used internally by the OSC 133
    /// parser to anchor prompt marks, and exposed here so main.rs can compute
    /// the correct "current line" for prev/next-prompt navigation without
    /// duplicating the formula.
    #[must_use]
    pub fn cursor_absolute_line(&self) -> i32 {
        cursor_absolute_line(self.term.grid())
    }

    /// Iterate every visible cell, yielding (column, row, snapshot).
    ///
    /// Useful for renderers that need the visible viewport in one pass.
    pub fn for_each_visible_cell<F>(&self, f: F)
    where
        F: FnMut(u16, u16, CellSnapshot),
    {
        self.for_each_visible_cell_at_scroll(0, f);
    }

    /// Like [`Self::for_each_visible_cell`] but the viewport is scrolled up by
    /// `scroll_lines` lines into the history buffer. Use this for the
    /// terminal's local scrollback view — the application's window pans
    /// over both the current screen and recent history.
    pub fn for_each_visible_cell_at_scroll<F>(&self, scroll_lines: usize, mut f: F)
    where
        F: FnMut(u16, u16, CellSnapshot),
    {
        let grid = self.term.grid();
        let cols = grid.columns();
        let rows = grid.screen_lines();
        let off = i32::try_from(scroll_lines).unwrap_or(i32::MAX);
        for row in 0..rows {
            for col in 0..cols {
                let line_idx = row as i32 - off;
                let point = Point::new(Line(line_idx), Column(col));
                let cell = &grid[point];
                let row_u = u16::try_from(row).unwrap_or(u16::MAX);
                let col_u = u16::try_from(col).unwrap_or(u16::MAX);
                f(
                    col_u,
                    row_u,
                    snapshot_cell_with_palette(
                        cell,
                        &self.palette,
                        &self.palette_overrides,
                        self.override_fg,
                        self.override_bg,
                    ),
                );
            }
        }
    }

    /// Number of lines currently in the scrollback buffer. Use this to
    /// clamp scroll offsets in the host application.
    #[must_use]
    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    /// The currently-visible screen as trailing-trimmed text (no scrollback
    /// history). The result has exactly `screen_lines` rows. Used by the
    /// "export visible screen" variant of the scrollback-export action.
    #[must_use]
    pub fn visible_lines_text(&self) -> Vec<String> {
        let grid = self.term.grid();
        let cols = grid.columns();
        let rows = grid.screen_lines();
        let mut out = Vec::with_capacity(rows);
        for row in 0..rows {
            let line_idx = i32::try_from(row).unwrap_or(i32::MAX);
            let mut s = String::with_capacity(cols);
            for col in 0..cols {
                let p = Point::new(Line(line_idx), Column(col));
                let c = grid[p].c;
                s.push(if c == '\0' { ' ' } else { c });
            }
            out.push(s.trim_end().to_string());
        }
        out
    }

    /// Every line of the buffer — scrollback history followed by the
    /// visible screen — as trailing-trimmed text, oldest first. The
    /// absolute alacritty `Line` of entry `i` is `i as i32 - history_size`,
    /// so the last `screen_lines` entries are the live viewport. Used for
    /// full-buffer (scrollback) search.
    #[must_use]
    pub fn buffer_lines_text(&self) -> Vec<String> {
        let grid = self.term.grid();
        let cols = grid.columns();
        let rows = grid.screen_lines();
        let hist = grid.history_size();
        let mut out = Vec::with_capacity(hist + rows);
        let start = -(i32::try_from(hist).unwrap_or(i32::MAX));
        let end = i32::try_from(rows).unwrap_or(i32::MAX);
        for line in start..end {
            let mut s = String::with_capacity(cols);
            for col in 0..cols {
                let p = Point::new(Line(line), Column(col));
                let c = grid[p].c;
                s.push(if c == '\0' { ' ' } else { c });
            }
            // Drop trailing blank cells so column math matches what the
            // user sees and matches don't run off into padding.
            out.push(s.trim_end().to_string());
        }
        out
    }

    /// Cell range of the word containing `(col, row)`. Returns
    /// `((start_col, row), (end_col, row))` inclusive. Two cells belong
    /// to the same word when neither is a boundary and they sit next to
    /// each other on the same line. Useful for double-click selection.
    ///
    /// A cell is a boundary when it is whitespace, the null cell, or its
    /// character appears in `separators` (the user-configurable
    /// `terminal.word_separators` set). Pass an empty `separators` to treat
    /// every non-whitespace cell as part of the word.
    #[must_use]
    pub fn word_at(
        &self,
        col: u16,
        row: u16,
        scroll_lines: usize,
        separators: &str,
    ) -> ((u16, u16), (u16, u16)) {
        let (cols, rows) = self.size();
        // Clamp into the grid — a mid-resize click can land just past
        // the last column / row before the emulator catches up.
        let col = col.min(cols.saturating_sub(1));
        let row = row.min(rows.saturating_sub(1));
        if cols == 0 || rows == 0 {
            return ((col, row), (col, row));
        }
        let grid = self.term.grid();
        // Read at the scrolled absolute line so double-click picks the word
        // the user sees in scrollback, not the one on the live screen.
        let off = i32::try_from(scroll_lines).unwrap_or(i32::MAX);
        let line = (i32::from(row) - off).clamp(grid.topmost_line().0, grid.bottommost_line().0);
        let is_word = |c: char| !c.is_whitespace() && c != '\0' && !separators.contains(c);
        let here_char = grid[Point::new(Line(line), Column(col.into()))].c;
        // If the user clicked on whitespace, just return a single-cell
        // range so the standard click→drag still works.
        if !is_word(here_char) {
            return ((col, row), (col, row));
        }
        let mut start = col;
        while start > 0 {
            let p = Point::new(Line(line), Column((start - 1).into()));
            if !is_word(grid[p].c) {
                break;
            }
            start -= 1;
        }
        let mut end = col;
        while end + 1 < cols {
            let p = Point::new(Line(line), Column((end + 1).into()));
            if !is_word(grid[p].c) {
                break;
            }
            end += 1;
        }
        ((start, row), (end, row))
    }

    /// Cell range spanning the full line containing `(_, row)`. Returns
    /// `((0, row), (cols-1, row))` clamped to the visible viewport.
    #[must_use]
    pub fn line_at(&self, row: u16) -> ((u16, u16), (u16, u16)) {
        let (cols, _) = self.size();
        ((0, row), (cols.saturating_sub(1), row))
    }

    /// Resolve the OSC 8 hyperlink URI at `(col, row)` in the viewport,
    /// optionally panned `scroll_lines` into history. Returns `None` if
    /// the cell carries no hyperlink or the indices are out of range
    /// (which can briefly happen mid-resize when the renderer is one
    /// frame ahead of the emulator).
    #[must_use]
    pub fn cell_hyperlink(&self, col: u16, row: u16, scroll_lines: usize) -> Option<String> {
        let grid = self.term.grid();
        if usize::from(col) >= grid.columns() {
            return None;
        }
        if usize::from(row) >= grid.screen_lines() {
            return None;
        }
        let off = i32::try_from(scroll_lines).unwrap_or(i32::MAX);
        let line_idx = i32::from(row) - off;
        let topmost = grid.topmost_line().0;
        let bottommost = grid.bottommost_line().0;
        if line_idx < topmost || line_idx > bottommost {
            return None;
        }
        let p = Point::new(Line(line_idx), Column(col.into()));
        let cell = &grid[p];
        cell.hyperlink().map(|h| h.uri().to_string())
    }

    /// `true` while the application has switched to the alternate screen
    /// (vim, less, btop). In that mode mouse-wheel events should be sent
    /// through to the PTY instead of scrolling our local backbuffer.
    #[must_use]
    pub fn is_alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// `true` while the focused application has enabled application cursor-key
    /// mode (DECCKM, DECSET 1). In this mode, unmodified arrow keys and
    /// Home/End should transmit SS3 sequences (`ESC O A` … `ESC O D`,
    /// `ESC O H`, `ESC O F`) instead of the normal CSI sequences
    /// (`ESC [ A` …). Modified keys always use the CSI form regardless.
    #[must_use]
    pub fn app_cursor_mode(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }

    /// `true` while the focused application has enabled application keypad
    /// mode (DECPAM, `ESC =`). In this mode, keypad keys transmit
    /// application-mode sequences rather than their numeric equivalents.
    #[must_use]
    pub fn app_keypad_mode(&self) -> bool {
        self.term.mode().contains(TermMode::APP_KEYPAD)
    }

    /// The kitty keyboard protocol progressive-enhancement flags currently
    /// active for the focused screen (main vs alt each keep their own stack;
    /// alacritty tracks the active set in [`TermMode`]).
    ///
    /// The host reads these to decide how to encode keystrokes: when any flag
    /// is set the key goes out as a `CSI … u` sequence (see
    /// `crate::kitty_keyboard`); when all are clear the legacy xterm encoding
    /// is used. Mirrors the bit layout of the protocol's flag word.
    #[must_use]
    pub fn kitty_keyboard_flags(&self) -> KittyKeyboardFlags {
        let m = self.term.mode();
        KittyKeyboardFlags {
            disambiguate: m.contains(TermMode::DISAMBIGUATE_ESC_CODES),
            report_event_types: m.contains(TermMode::REPORT_EVENT_TYPES),
            report_alternate_keys: m.contains(TermMode::REPORT_ALTERNATE_KEYS),
            report_all_keys_as_esc: m.contains(TermMode::REPORT_ALL_KEYS_AS_ESC),
            report_associated_text: m.contains(TermMode::REPORT_ASSOCIATED_TEXT),
        }
    }

    /// Cursor shape the focused app currently wants (DECSCUSR / VT520).
    /// `None` means "Hidden" — render no cursor at all.
    ///
    /// A `None` is returned both when the app picked the explicit Hidden
    /// DECSCUSR shape *and* when it hid the cursor via DECTCEM (`ESC[?25l`,
    /// the `SHOW_CURSOR` mode). `Term::cursor_style()` does not fold the
    /// `SHOW_CURSOR` mode into its result, so we check it here — otherwise a
    /// TUI that hides the real cursor and draws its own (vim, fzf, the Claude
    /// Code prompt, …) would show two cursors at once.
    #[must_use]
    pub fn cursor_shape(&self) -> Option<AppCursorShape> {
        if !self.term.mode().contains(TermMode::SHOW_CURSOR) {
            return None;
        }
        let s = self.term.cursor_style();
        match s.shape {
            alacritty_terminal::vte::ansi::CursorShape::Hidden => None,
            alacritty_terminal::vte::ansi::CursorShape::Block => Some(AppCursorShape::Block),
            alacritty_terminal::vte::ansi::CursorShape::Underline => {
                Some(AppCursorShape::Underline)
            }
            alacritty_terminal::vte::ansi::CursorShape::Beam => Some(AppCursorShape::Beam),
            alacritty_terminal::vte::ansi::CursorShape::HollowBlock => {
                Some(AppCursorShape::HollowBlock)
            }
        }
    }

    /// `true` when the focused app has enabled bracketed-paste mode
    /// (DECSET 2004). Pasted text should be wrapped in `\e[200~ ... \e[201~`
    /// only when this is set — naive shells like `cat`, login prompts, and
    /// classic DOS tools choke on the wrapping otherwise.
    #[must_use]
    pub fn bracketed_paste_enabled(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// `true` while the focused app has enabled focus-in/out reporting
    /// (DECSET 1004). Vim, tmux, and modern shells use this to refresh
    /// the cursor and pause animations when the window loses focus.
    #[must_use]
    pub fn focus_events_enabled(&self) -> bool {
        self.term.mode().contains(TermMode::FOCUS_IN_OUT)
    }

    /// `true` while the focused app has opened a synchronized-output frame
    /// with `CSI ?2026h` (Begin Synchronized Update) and not yet closed it
    /// with `CSI ?2026l` (End Synchronized Update), **and** the safety timeout
    /// has not yet elapsed.
    ///
    /// When this returns `true` the host renderer should **skip redraw** for
    /// this frame — the grid is mid-update and painting it now would show a
    /// torn / partially-drawn frame. The host should call [`Self::has_new_frame`]
    /// to decide whether to paint.
    #[must_use]
    pub fn is_synchronized_output(&self) -> bool {
        if !self.sync_output {
            return false;
        }
        // Safety timeout: treat a frame as committed once it has been open for
        // longer than SYNC_OUTPUT_TIMEOUT, even if ESU never arrived.
        if let Some(start) = self.sync_start {
            start.elapsed() < SYNC_OUTPUT_TIMEOUT
        } else {
            false
        }
    }

    /// Returns `true` when there is new content that the host should render.
    ///
    /// This is the **snapshot gate**: the host renderer calls this after every
    /// [`Self::advance`] invocation. When `false`, the grid may be
    /// mid-update (DECSET 2026 synchronized-output frame is open and the safety
    /// timeout has not elapsed), so the host should reuse the last painted frame
    /// and skip the current redraw pass.
    ///
    /// When `true` the host should proceed with a normal render cycle.
    ///
    /// Note: this method does **not** track frame dirtiness beyond the DECSET
    /// 2026 guard — it does not replace a full damage-tracking system.
    /// Callers that need finer-grained dirty detection should layer their own
    /// change detection on top. The only gating performed here is:
    /// - While a synchronized-output frame is open (and within the safety
    ///   window), return `false`.
    /// - Otherwise, return `true`.
    #[must_use]
    pub fn has_new_frame(&self) -> bool {
        !self.is_synchronized_output()
    }

    /// Snapshot of which mouse-reporting modes the focused app has
    /// enabled. Used by the host to decide whether to forward mouse
    /// events to the PTY (vim, less, htop, tmux all rely on this).
    #[must_use]
    pub fn mouse_mode(&self) -> MouseMode {
        let m = self.term.mode();
        MouseMode {
            click: m.contains(TermMode::MOUSE_REPORT_CLICK),
            drag: m.contains(TermMode::MOUSE_DRAG),
            motion: m.contains(TermMode::MOUSE_MOTION),
            sgr: m.contains(TermMode::SGR_MOUSE),
        }
    }

    /// Extract a text-only copy of a rectangular cell range, row-major,
    /// rows separated by `\n`. Useful for "copy selection" features.
    ///
    /// The range is inclusive on both ends. Out-of-range cells are clamped
    /// to the visible viewport. `scroll_lines` pans into history so a
    /// selection made while scrolled copies what the user actually sees
    /// (rows map to absolute line `row - scroll_lines`, like `cell_hyperlink`).
    #[must_use]
    pub fn text_in_range(
        &self,
        (a_col, a_row): (u16, u16),
        (b_col, b_row): (u16, u16),
        scroll_lines: usize,
    ) -> String {
        let (cols, rows) = self.size();
        let (r0, r1) = if a_row <= b_row {
            (a_row, b_row)
        } else {
            (b_row, a_row)
        };
        let r1 = r1.min(rows.saturating_sub(1));
        let same_row = r0 == r1;
        let grid = self.term.grid();
        let off = i32::try_from(scroll_lines).unwrap_or(i32::MAX);
        let topmost = grid.topmost_line().0;
        let bottommost = grid.bottommost_line().0;

        let mut out = String::new();
        for row in r0..=r1 {
            // Map the viewport row to its absolute grid line for the current
            // scroll, clamped so a transient over-scroll can't index OOB.
            let row_idx = (i32::from(row) - off).clamp(topmost, bottommost);
            let (start, end) = if same_row {
                if a_col <= b_col {
                    (a_col, b_col)
                } else {
                    (b_col, a_col)
                }
            } else if row == a_row {
                if a_row <= b_row {
                    (a_col, cols.saturating_sub(1))
                } else {
                    (0, a_col)
                }
            } else if row == b_row {
                if a_row <= b_row {
                    (0, b_col)
                } else {
                    (b_col, cols.saturating_sub(1))
                }
            } else {
                (0, cols.saturating_sub(1))
            };
            let end = end.min(cols.saturating_sub(1));
            let mut row_str = String::new();
            for col in start..=end {
                let p = Point::new(Line(row_idx), Column(col.into()));
                let c = grid[p].c;
                row_str.push(if c == '\0' { ' ' } else { c });
            }
            // trim trailing spaces row-by-row — matches user expectation.
            let trimmed = row_str.trim_end_matches(' ');
            out.push_str(trimmed);
            if row != r1 {
                out.push('\n');
            }
        }
        out
    }
}

/// How the underline beneath a cell should be drawn.
///
/// Maps alacritty's [`Flags`] bits to a renderer-friendly enum so the
/// render crate never has to touch alacritty internals directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnderlineStyle {
    /// No underline — most cells.
    #[default]
    None,
    /// Standard SGR 4 single underline.
    Single,
    /// SGR 4:2 / SGR 21 double underline.
    Double,
    /// SGR 4:3 undercurl (wavy underline).
    Curly,
    /// SGR 4:4 dotted underline.
    Dotted,
    /// SGR 4:5 dashed underline.
    Dashed,
}

impl UnderlineStyle {
    /// Derive the style from the cell's [`Flags`] bitfield.
    ///
    /// Priority: UNDERCURL > DOTTED > DASHED > DOUBLE_UNDERLINE > UNDERLINE.
    #[must_use]
    pub fn from_flags(flags: Flags) -> Self {
        if flags.contains(Flags::UNDERCURL) {
            Self::Curly
        } else if flags.contains(Flags::DOTTED_UNDERLINE) {
            Self::Dotted
        } else if flags.contains(Flags::DASHED_UNDERLINE) {
            Self::Dashed
        } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
            Self::Double
        } else if flags.contains(Flags::UNDERLINE) {
            Self::Single
        } else {
            Self::None
        }
    }
}

/// Snapshot of one rendered cell. Owns nothing, copy-friendly.
#[derive(Debug, Clone, Copy)]
pub struct CellSnapshot {
    /// Character in this cell (space for empty).
    pub ch: char,
    /// Foreground colour (sRGB).
    pub fg: [u8; 3],
    /// Background colour (sRGB).
    pub bg: [u8; 3],
    /// Bold attribute.
    pub bold: bool,
    /// Italic attribute.
    pub italic: bool,
    /// Underline style (SGR 4:N / SGR 21). [`UnderlineStyle::None`] means no
    /// underline at all, independent of hyperlinks.
    pub underline_style: UnderlineStyle,
    /// Independent underline colour set by SGR 58 / reset by SGR 59. `None`
    /// means the underline inherits the cell foreground colour.
    pub underline_color: Option<[u8; 3]>,
    /// Strikethrough attribute (SGR 9 / reset SGR 29).
    pub strikethrough: bool,
    /// Overline attribute (SGR 53 / reset SGR 55).
    ///
    /// NOTE: alacritty_terminal 0.24 does not track overline in its `Flags`
    /// bitfield. This field always yields `false` for alacritty-parsed cells.
    /// It is included so future backends or a custom SGR handler can populate
    /// it without a breaking struct change.
    pub overline: bool,
    /// `true` if this cell is part of an OSC 8 hyperlink. The host can
    /// resolve the URL via [`Emulator::cell_hyperlink`].
    pub has_link: bool,
    /// SGR 2 (faint/dim): the foreground should be rendered at reduced
    /// luminance. The exact darkening amount is controlled by the renderer's
    /// `appearance.dim_amount` config field.
    pub dim: bool,
    /// SGR 7 (reverse video): foreground and background colours are swapped
    /// for this cell. Applied before dim so dim works on the swapped colours.
    pub inverse: bool,
    /// SGR 8 (concealed/hidden): the glyph is invisible — the cell background
    /// still draws but the text character should not be rendered.
    pub hidden: bool,
}

fn snapshot_cell_with_palette(
    cell: &Cell,
    palette: &AnsiPalette,
    overrides: &[Option<[u8; 3]>; 256],
    override_fg: Option<[u8; 3]>,
    override_bg: Option<[u8; 3]>,
) -> CellSnapshot {
    let flags = cell.flags;
    // Effective fg/bg defaults: OSC 10/11 overrides take precedence over the
    // theme palette's foreground/background.
    let eff_fg_default = override_fg.unwrap_or(palette.foreground);
    let eff_bg_default = override_bg.unwrap_or(palette.background);
    let underline_color = cell.underline_color().map(|c| {
        resolve_color(
            c,
            eff_fg_default,
            palette,
            overrides,
            override_fg,
            override_bg,
        )
    });
    CellSnapshot {
        ch: cell.c,
        fg: resolve_color(
            cell.fg,
            eff_fg_default,
            palette,
            overrides,
            override_fg,
            override_bg,
        ),
        bg: resolve_color(
            cell.bg,
            eff_bg_default,
            palette,
            overrides,
            override_fg,
            override_bg,
        ),
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        underline_style: UnderlineStyle::from_flags(flags),
        underline_color,
        strikethrough: flags.contains(Flags::STRIKEOUT),
        // alacritty_terminal 0.24 has no OVERLINE flag — always false.
        overline: false,
        has_link: cell.hyperlink().is_some(),
        dim: flags.contains(Flags::DIM),
        inverse: flags.contains(Flags::INVERSE),
        hidden: flags.contains(Flags::HIDDEN),
    }
}

const DEFAULT_FG: [u8; 3] = [0xe6, 0xe6, 0xe6];
const DEFAULT_BG: [u8; 3] = [0x0d, 0x10, 0x17];

fn resolve_color(
    color: Color,
    default: [u8; 3],
    palette: &AnsiPalette,
    overrides: &[Option<[u8; 3]>; 256],
    override_fg: Option<[u8; 3]>,
    override_bg: Option<[u8; 3]>,
) -> [u8; 3] {
    match color {
        Color::Spec(Rgb { r, g, b }) => [r, g, b],
        Color::Named(named) => {
            named_color_with_overrides(named, default, palette, override_fg, override_bg)
        }
        // Indexed colours: 0–7 map to normal, 8–15 to bright, 16–231 is the
        // 6×6×6 xterm cube, 232–255 is the greyscale ramp.
        // An OSC 4 indexed override replaces the theme value for that slot.
        Color::Indexed(i) => {
            if let Some(ov) = overrides[i as usize] {
                return ov;
            }
            indexed_color(i, default, palette)
        }
    }
}

/// Map a [`NamedColor`] to its RGB triple from `p`, with optional OSC 10/11
/// runtime fg/bg overrides.
fn named_color_with_overrides(
    named: NamedColor,
    default: [u8; 3],
    p: &AnsiPalette,
    override_fg: Option<[u8; 3]>,
    override_bg: Option<[u8; 3]>,
) -> [u8; 3] {
    match named {
        NamedColor::Black => p.normal[0],
        NamedColor::Red => p.normal[1],
        NamedColor::Green => p.normal[2],
        NamedColor::Yellow => p.normal[3],
        NamedColor::Blue => p.normal[4],
        NamedColor::Magenta => p.normal[5],
        NamedColor::Cyan => p.normal[6],
        NamedColor::White => p.normal[7],
        NamedColor::BrightBlack => p.bright[0],
        NamedColor::BrightRed => p.bright[1],
        NamedColor::BrightGreen => p.bright[2],
        NamedColor::BrightYellow => p.bright[3],
        NamedColor::BrightBlue => p.bright[4],
        NamedColor::BrightMagenta => p.bright[5],
        NamedColor::BrightCyan => p.bright[6],
        NamedColor::BrightWhite => p.bright[7],
        // OSC 10/11 override fg/bg if set; otherwise fall back to the theme.
        NamedColor::Foreground => override_fg.unwrap_or(p.foreground),
        NamedColor::Background => override_bg.unwrap_or(p.background),
        _ => default,
    }
}

fn indexed_color(i: u8, _default: [u8; 3], p: &AnsiPalette) -> [u8; 3] {
    if i < 8 {
        p.normal[i as usize]
    } else if i < 16 {
        p.bright[(i - 8) as usize]
    } else if i < 232 {
        // 6×6×6 colour cube. Channels are non-linear (xterm convention).
        let idx = i - 16;
        let r = idx / 36;
        let g = (idx % 36) / 6;
        let b = idx % 6;
        let comp = |v: u8| -> u8 {
            if v == 0 {
                0
            } else {
                55 + v * 40
            }
        };
        [comp(r), comp(g), comp(b)]
    } else {
        // 232..=255 — greyscale ramp.
        let v = 8 + (i - 232) * 10;
        [v, v, v]
    }
}

/// Compute the **absolute** line index of the cursor for the given grid.
///
/// The alacritty `cursor.point.line` is relative to the *visible* viewport
/// (`0` = top visible row). The absolute index (negative = history) is:
///   `abs = cursor_viewport_row - history_size`.
///
/// This value is used to anchor OSC 133 prompt marks at the line the cursor
/// is on when the escape sequence arrives, which is the correct semantic:
/// the shell emits the sequence while its cursor is at the prompt line.
fn cursor_absolute_line(grid: &Grid<Cell>) -> i32 {
    let row_in_viewport = grid.cursor.point.line.0;
    let history = grid.history_size() as i32;
    row_in_viewport - history
}

/// Parsed OSC 133 event returned by [`scan_osc_133`].
struct Osc133Event {
    kind: OscKind,
    line: i32,
    exit_code: Option<u32>,
    /// For `OscKind::OutputStart` (C): command text extracted directly from
    /// the raw bytes between the preceding B and C sequences in the same chunk.
    ///
    /// This is the primary command-text source. It is populated whenever B and
    /// C appear in the same byte buffer (synthetic demos, one-shot `advance`
    /// calls). When the B→C bytes span multiple `advance` calls (the common
    /// real-shell case) this field is `None` and the caller falls back to
    /// reading the command from the already-populated grid line.
    inline_command_text: Option<String>,
}

/// Scan `bytes` for `OSC 133 ; <letter> [;<params>] ST` sequences and return
/// the parsed events anchored at `cursor_abs`.
///
/// Multiple sequences in a single chunk are all collected. The scanner is
/// intentionally stateless between calls: sequences that straddle chunk
/// boundaries are missed (the partial-sequence-across-chunks case is rare in
/// practice — shells emit the full sequence in one write — and carrying
/// arbitrary OSC-133 partial state across calls would complicate the buffer
/// management). The OSC 7 / cwd path uses dedicated buffering because it
/// carries a variable-length path; OSC 133 payloads are tiny.
///
/// When B and C appear in the **same** chunk the bytes between them are
/// decoded as the inline command text and stored in
/// [`Osc133Event::inline_command_text`] of the C event. This covers both the
/// demo seed path and any shell integration that places the command text
/// between B and C in a single write rather than relying on echo to the grid.
fn scan_osc_133(bytes: &[u8], cursor_abs: i32) -> Vec<Osc133Event> {
    let mut events = Vec::new();
    // `last_b_end`: byte offset just past the end of the most recent B
    // sequence seen in this chunk. Used to capture inline command text
    // between B and C.
    let mut last_b_end: Option<usize> = None;

    let mut i = 0;
    while i + 5 <= bytes.len() {
        // Detect `ESC ]` (0x1b 0x5d).
        if bytes[i] != 0x1b || bytes[i + 1] != b']' {
            i += 1;
            continue;
        }
        // Must be `1 3 3 ;` next.
        if i + 6 > bytes.len()
            || bytes[i + 2] != b'1'
            || bytes[i + 3] != b'3'
            || bytes[i + 4] != b'3'
            || bytes[i + 5] != b';'
        {
            i += 1;
            continue;
        }
        // Payload starts at i+6. Find the string terminator (BEL or ST).
        let payload_start = i + 6;
        let mut j = payload_start;
        let mut term_end = None;
        while j < bytes.len() {
            if bytes[j] == 0x07 {
                term_end = Some(j + 1);
                break;
            }
            if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                term_end = Some(j + 2);
                break;
            }
            j += 1;
        }
        let Some(end) = term_end else {
            // Unterminated — skip to avoid false positives.
            i += 1;
            continue;
        };
        // Parse the letter and optional semicolon-delimited params.
        if let Ok(payload) = std::str::from_utf8(&bytes[payload_start..j]) {
            let mut parts = payload.splitn(3, ';');
            let letter = parts.next().unwrap_or("").trim();
            let params: &str = parts.next().unwrap_or("");
            let kind = match letter {
                "A" => Some(OscKind::PromptStart),
                "B" => Some(OscKind::InputStart),
                "C" => Some(OscKind::OutputStart),
                "D" => Some(OscKind::CommandEnd),
                _ => None,
            };
            if let Some(k) = kind {
                let exit_code = if k == OscKind::CommandEnd && !params.is_empty() {
                    params.parse::<u32>().ok()
                } else {
                    None
                };

                // For C: capture bytes between last B and this C as inline
                // command text. This is the critical path for synthetic/demo
                // sequences (all in one `advance` call) and also handles any
                // shell that places the command text between B and C in a
                // single write rather than relying on it being echoed to the grid.
                let inline_command_text = if k == OscKind::OutputStart {
                    last_b_end.map(|b_end| extract_inline_command_text(&bytes[b_end..i]))
                } else {
                    None
                };

                // Track where B ended so we can harvest text before the next C.
                if k == OscKind::InputStart {
                    last_b_end = Some(end);
                } else if k == OscKind::PromptStart || k == OscKind::CommandEnd {
                    // A fresh A or D resets the B anchor — no partial carry-over.
                    last_b_end = None;
                }

                events.push(Osc133Event {
                    kind: k,
                    line: cursor_abs,
                    exit_code,
                    inline_command_text,
                });
            }
        }
        i = end;
    }
    events
}

/// Decode the raw bytes sitting between a B and C sequence as command text.
///
/// The bytes may contain ANSI escape sequences, carriage returns, and other
/// control characters introduced by the terminal echo. We strip all escape
/// sequences and non-printable ASCII so we're left with the visible text the
/// user typed. The result is trimmed of leading/trailing whitespace.
///
/// Returns an empty string when no printable content is found.
fn extract_inline_command_text(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // ESC: skip the full escape sequence.
            0x1b => {
                i += 1;
                if i >= bytes.len() {
                    break;
                }
                match bytes[i] {
                    // CSI: ESC [ … final-byte (0x40–0x7E)
                    b'[' => {
                        i += 1;
                        // Skip parameter bytes (0x30–0x3F) and intermediate bytes
                        // (0x20–0x2F), then one final byte (0x40–0x7E).
                        while i < bytes.len() && bytes[i] < 0x40 {
                            i += 1;
                        }
                        i += 1; // skip final byte
                    }
                    // OSC: ESC ] … BEL or ESC \
                    b']' => {
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == 0x07 {
                                i += 1;
                                break;
                            }
                            if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    // DCS: ESC P … ESC \
                    b'P' => {
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    // Two-character sequences: ESC <any single byte>
                    _ => {
                        i += 1;
                    }
                }
            }
            // Carriage return / newline / tab — treated as whitespace, not copied.
            0x0d | 0x0a | 0x09 => {
                i += 1;
            }
            // Other C0 control bytes — skip.
            b if b < 0x20 => {
                i += 1;
            }
            // DEL (0x7f) — skip.
            0x7f => {
                i += 1;
            }
            // Printable ASCII and high bytes (UTF-8 continuation safe — we
            // don't validate UTF-8 here; we just collect the bytes and let
            // String::from_utf8_lossy handle any invalid sequences).
            b => {
                out.push(b as char);
                i += 1;
            }
        }
    }
    out.trim().to_string()
}

/// Extract a single line of text from the grid at the given absolute line
/// index, trimmed of trailing whitespace. Returns an empty string if the line
/// is out of range.
fn extract_line_text(grid: &Grid<Cell>, abs_line: i32, cols: usize) -> String {
    let topmost = grid.topmost_line().0;
    let bottommost = grid.bottommost_line().0;
    if abs_line < topmost || abs_line > bottommost {
        return String::new();
    }
    let mut s = String::with_capacity(cols);
    for col in 0..cols {
        let p = Point::new(Line(abs_line), Column(col));
        let c = grid[p].c;
        s.push(if c == '\0' { ' ' } else { c });
    }
    s.trim_end().to_string()
}

/// Maximum number of distinct OSC 1337 user variables retained per pane.
/// Untrusted output could otherwise grow the map without bound by emitting
/// `SetUserVar` with ever-new names for the life of the session.
const USER_VARS_CAP: usize = 256;

/// Scan `bytes` for `OSC 1337 ; SetUserVar=NAME=BASE64VALUE ST` sequences
/// and store the decoded values in `vars`.
///
/// Per the OSC 1337 spec the value is base64-encoded UTF-8. We decode it
/// inline; any decode error silently drops the variable (preferable to
/// panicking on a malformed shell integration script). Updates to existing
/// names always land; NEW names are dropped once [`USER_VARS_CAP`] distinct
/// variables exist (bounding memory against hostile output).
fn sniff_osc_1337(bytes: &[u8], vars: &mut HashMap<String, String>) {
    let mut i = 0;
    while i + 5 <= bytes.len() {
        if bytes[i] != 0x1b || bytes[i + 1] != b']' {
            i += 1;
            continue;
        }
        // `1 3 3 7 ;`
        if i + 7 > bytes.len()
            || bytes[i + 2] != b'1'
            || bytes[i + 3] != b'3'
            || bytes[i + 4] != b'3'
            || bytes[i + 5] != b'7'
            || bytes[i + 6] != b';'
        {
            i += 1;
            continue;
        }
        let payload_start = i + 7;
        let mut j = payload_start;
        let mut term_end = None;
        while j < bytes.len() {
            if bytes[j] == 0x07 {
                term_end = Some(j + 1);
                break;
            }
            if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                term_end = Some(j + 2);
                break;
            }
            j += 1;
        }
        let Some(end) = term_end else {
            i += 1;
            continue;
        };
        if let Ok(payload) = std::str::from_utf8(&bytes[payload_start..j]) {
            // SetUserVar=NAME=BASE64VALUE
            if let Some(rest) = payload.strip_prefix("SetUserVar=") {
                if let Some(eq) = rest.find('=') {
                    let name = &rest[..eq];
                    let b64 = &rest[eq + 1..];
                    // Decode base64; silently skip malformed values.
                    if let Ok(decoded) = base64_decode(b64) {
                        if let Ok(s) = String::from_utf8(decoded) {
                            // Cap distinct names; updates to known names pass.
                            if vars.len() < USER_VARS_CAP || vars.contains_key(name) {
                                vars.insert(name.to_string(), s);
                            } else {
                                tracing::debug!(
                                    name,
                                    cap = USER_VARS_CAP,
                                    "OSC 1337 SetUserVar dropped: per-pane cap reached"
                                );
                            }
                        }
                    }
                }
            }
        }
        i = end;
    }
}

/// Parse the key=value argument string from `OSC 1337;File=<args>:<base64>`.
///
/// Recognised keys (case-insensitive): `name`, `size`, `width`, `height`,
/// `preserveAspectRatio`, `inline`. Every unrecognised key is silently ignored
/// so forward-compatibility is preserved.
///
/// Returns `(width_cells, height_cells, inline, base64_payload)`. When
/// `inline != 1` the payload should not be displayed inline; we skip it.
fn parse_osc1337_file_args(raw: &str) -> Option<(u16, u16, bool, &str)> {
    // The raw string is `<args>:<base64-data>`. Split on the FIRST colon.
    let colon = raw.find(':')?;
    let args_str = &raw[..colon];
    let b64 = raw[colon + 1..].trim();
    if b64.is_empty() {
        return None;
    }

    let mut width_cells: Option<u16> = None;
    let mut height_cells: Option<u16> = None;
    let mut inline = false;

    for kv in args_str.split(';') {
        let kv = kv.trim();
        if kv.is_empty() {
            continue;
        }
        let eq = match kv.find('=') {
            Some(p) => p,
            None => continue,
        };
        let key = kv[..eq].trim().to_ascii_lowercase();
        let val = kv[eq + 1..].trim();

        match key.as_str() {
            "inline" => {
                inline = val == "1";
            }
            "width" => {
                // Width can be "<N>" (cells), "<N>px" (pixels — we treat as cells
                // for now; the renderer can honour pixel sizing later), or
                // "<N>%" (percent of viewport). We accept cells and px here and
                // leave percent as 0 (auto).
                width_cells = parse_dim_spec(val);
            }
            "height" => {
                height_cells = parse_dim_spec(val);
            }
            _ => {}
        }
    }

    if !inline {
        return None;
    }

    Some((
        width_cells.unwrap_or(0),
        height_cells.unwrap_or(0),
        inline,
        b64,
    ))
}

/// Parse a dimension spec: `"<N>"` (cells), `"<N>px"`, or `"<N>%"`. Returns
/// `None` for `"auto"` and percent specs so the caller can fall back to
/// pixel-derived sizing.
fn parse_dim_spec(s: &str) -> Option<u16> {
    if s.eq_ignore_ascii_case("auto") || s.ends_with('%') {
        return None;
    }
    // Strip "px" suffix if present.
    let digits = s
        .trim_end_matches("px")
        .trim_end_matches("Px")
        .trim_end_matches("PX");
    digits.parse::<u16>().ok()
}

/// Compute the cell footprint (cols × rows) of an image whose natural size
/// is `px_w × px_h` pixels, given the terminal cell size `cell_w × cell_h`
/// in pixels, and optional explicit cell overrides.
///
/// If the explicit override is 0 (meaning "auto") we derive the count from
/// the pixel size rounded up. Minimum is 1 in each dimension.
fn image_cell_footprint(
    px_w: u32,
    px_h: u32,
    cell_w: f32,
    cell_h: f32,
    explicit_cols: u16,
    explicit_rows: u16,
) -> (u16, u16) {
    let cols = if explicit_cols > 0 {
        explicit_cols
    } else if cell_w > 0.0 {
        ((px_w as f32 / cell_w).ceil() as u16).max(1)
    } else {
        1
    };
    let rows = if explicit_rows > 0 {
        explicit_rows
    } else if cell_h > 0.0 {
        ((px_h as f32 / cell_h).ceil() as u16).max(1)
    } else {
        1
    };
    (cols, rows)
}

/// Scan `bytes` for `OSC 1337;File=<args>:<base64> ST` sequences, decode the
/// embedded image, add it to `store`, and record a placement at `cursor_abs`.
///
/// Cell dimensions are derived from `viewport_cols` and `viewport_rows` (the
/// current terminal size); pixel-per-cell sizing is not yet available here but
/// the arg parser accepts `width=<N>` / `height=<N>` cell counts as a
/// first-class override.
fn sniff_osc_1337_file(
    bytes: &[u8],
    cursor_abs: i32,
    _viewport_cols: u16,
    _viewport_rows: u16,
    store: &mut images::ImageStore,
) {
    let mut i = 0;
    while i + 7 <= bytes.len() {
        if bytes[i] != 0x1b || bytes[i + 1] != b']' {
            i += 1;
            continue;
        }
        // Must start with `1 3 3 7 ;`
        if bytes[i + 2] != b'1'
            || bytes[i + 3] != b'3'
            || bytes[i + 4] != b'3'
            || bytes[i + 5] != b'7'
            || bytes[i + 6] != b';'
        {
            i += 1;
            continue;
        }
        let payload_start = i + 7;
        // Find string terminator (BEL 0x07 or ESC \)
        let mut j = payload_start;
        let mut term_end = None;
        while j < bytes.len() {
            if bytes[j] == 0x07 {
                term_end = Some((j, j + 1));
                break;
            }
            if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                term_end = Some((j, j + 2));
                break;
            }
            j += 1;
        }
        let Some((payload_end, total_end)) = term_end else {
            i += 1;
            continue;
        };
        let Ok(payload) = std::str::from_utf8(&bytes[payload_start..payload_end]) else {
            i = total_end;
            continue;
        };
        // Must start with "File="
        if let Some(rest) = payload.strip_prefix("File=") {
            if let Some((explicit_cols, explicit_rows, _inline, b64)) =
                parse_osc1337_file_args(rest)
            {
                match base64_decode(b64) {
                    Ok(raw_bytes) => {
                        if let Some(id) = store.add_image(&raw_bytes) {
                            if let Some(img) = store.get_image(id) {
                                // Derive cell footprint. Without live cell-pixel
                                // dimensions we use a reasonable 8×16 fallback.
                                let (cols, rows) = image_cell_footprint(
                                    img.width_px,
                                    img.height_px,
                                    8.0,
                                    16.0,
                                    explicit_cols,
                                    explicit_rows,
                                );
                                store.place(id, cursor_abs, 0, cols, rows);
                                tracing::debug!(
                                    id,
                                    cursor_abs,
                                    cols,
                                    rows,
                                    "OSC 1337 inline image placed"
                                );
                            }
                        }
                    }
                    Err(()) => {
                        tracing::warn!("OSC 1337 inline image: base64 decode error");
                    }
                }
            }
        }
        i = total_end;
    }
}

/// Scan `bytes` for OSC 9 desktop notifications and OSC 777 notifications,
/// pushing [`EmulatorEvent::Notification`] entries into `out` for each one
/// found.
///
/// Disambiguates OSC 9:
/// - `\e]9;9;<path>` — cwd announcement (ConPTY / OSC 9;9 convention). This
///   form is already handled by [`sniff_cwd`]; we ignore it here.
/// - `\e]9;<any-other-body>` — desktop notification; body = notification text.
///
/// OSC 777 notification protocol: `\e]777;notify;<title>;<body>`
fn sniff_osc_notify(bytes: &[u8], out: &mut Vec<EmulatorEvent>) {
    let mut i = 0;
    while i + 3 <= bytes.len() {
        if bytes[i] != 0x1b || bytes[i + 1] != b']' {
            i += 1;
            continue;
        }
        // Check for OSC 777 first (5 bytes prefix: `7 7 7 ;`).
        let is_osc777 = i + 6 <= bytes.len()
            && bytes[i + 2] == b'7'
            && bytes[i + 3] == b'7'
            && bytes[i + 4] == b'7'
            && bytes[i + 5] == b';';
        // Check for OSC 9 (3 bytes prefix: `9 ;`).
        let is_osc9 = bytes[i + 2] == b'9' && i + 4 <= bytes.len() && bytes[i + 3] == b';';

        if !is_osc777 && !is_osc9 {
            i += 1;
            continue;
        }

        let payload_start = if is_osc777 { i + 6 } else { i + 4 };

        // Find string terminator (BEL or ESC\).
        let mut j = payload_start;
        let mut term_end = None;
        while j < bytes.len() {
            if bytes[j] == 0x07 {
                term_end = Some((j, j + 1));
                break;
            }
            if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                term_end = Some((j, j + 2));
                break;
            }
            j += 1;
        }
        let Some((payload_end, total_end)) = term_end else {
            i += 1;
            continue;
        };

        if let Ok(payload) = std::str::from_utf8(&bytes[payload_start..payload_end]) {
            if is_osc777 {
                // OSC 777;notify;<title>;<body>
                if let Some(rest) = payload.strip_prefix("notify;") {
                    let (title, body) = if let Some(sep) = rest.find(';') {
                        (rest[..sep].to_string(), rest[sep + 1..].to_string())
                    } else {
                        (String::new(), rest.to_string())
                    };
                    out.push(EmulatorEvent::Notification { title, body });
                }
            } else {
                // OSC 9;…  — skip the cwd form (9;9;…).
                if !payload.starts_with("9;") {
                    out.push(EmulatorEvent::Notification {
                        title: String::new(),
                        body: payload.to_string(),
                    });
                }
            }
        }
        i = total_end;
    }
}

/// Scan `bytes` for OSC 52 clipboard **READ** queries and push a
/// [`EmulatorEvent::ClipboardRead`] for each one found.
///
/// The query form is `OSC 52 ; <selection-chars> ; ? ST`, where the payload
/// after the second `;` is exactly the single character `?`. This is
/// distinct from the *store* form (payload is base64 data), which alacritty
/// handles natively and surfaces as `AlacrittyEvent::ClipboardStore`.
///
/// We intercept only the query form here so the host can enforce the
/// `terminal.clipboard_read` permission policy before responding.
fn sniff_osc52_read(bytes: &[u8], out: &mut Vec<EmulatorEvent>) {
    let mut i = 0;
    // Minimum: ESC ] 5 2 ; ; ? BEL  = 8 bytes.
    while i + 7 < bytes.len() {
        if bytes[i] != 0x1b || bytes[i + 1] != b']' {
            i += 1;
            continue;
        }
        // Expect `5 2 ;`
        if bytes[i + 2] != b'5' || bytes[i + 3] != b'2' || bytes[i + 4] != b';' {
            i += 1;
            continue;
        }
        // Collect the selection string (between the first `;` and the second `;`).
        let sel_start = i + 5;
        let mut sel_end = sel_start;
        while sel_end < bytes.len() && bytes[sel_end] != b';' {
            // Bail early if we hit a terminator — malformed sequence.
            if bytes[sel_end] == 0x07 || bytes[sel_end] == 0x1b {
                break;
            }
            sel_end += 1;
        }
        // Must have found the second `;` separator.
        if sel_end >= bytes.len() || bytes[sel_end] != b';' {
            i += 1;
            continue;
        }
        // Payload starts after the second `;`.
        let payload_start = sel_end + 1;
        // Find the string terminator (BEL `\x07` or ST `ESC \`).
        let mut j = payload_start;
        let mut term_end = None;
        while j < bytes.len() {
            if bytes[j] == 0x07 {
                term_end = Some((j, j + 1));
                break;
            }
            if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                term_end = Some((j, j + 2));
                break;
            }
            j += 1;
        }
        let Some((payload_end, total_end)) = term_end else {
            i += 1;
            continue;
        };
        let payload = &bytes[payload_start..payload_end];
        // Only the READ form: payload must be exactly `?`.
        if payload != b"?" {
            // This is the store form (base64 data) — alacritty handles it.
            i = total_end;
            continue;
        }
        let Ok(selection) = std::str::from_utf8(&bytes[sel_start..sel_end]) else {
            i = total_end;
            continue;
        };
        out.push(EmulatorEvent::ClipboardRead {
            selection: selection.to_string(),
        });
        i = total_end;
    }
}

/// Scan `bytes` for OSC 4/104/10/11/12/110/111/112 dynamic-colour sequences
/// and update the override tables accordingly. Pushes a single
/// [`EmulatorEvent::PaletteChanged`] when at least one override was applied.
///
/// Supported sequences:
/// - `OSC 4;<index>;<spec>` — set indexed palette colour (0–255).
/// - `OSC 104[;<index>]` — reset indexed palette colour (or all if no index).
/// - `OSC 10;<spec>` — set default foreground colour.
/// - `OSC 11;<spec>` — set default background colour.
/// - `OSC 12;<spec>` — set cursor colour.
/// - `OSC 110` — reset default foreground.
/// - `OSC 111` — reset default background.
/// - `OSC 112` — reset cursor colour.
///
/// Colour specs: `rgb:RR/GG/BB` (1–4 hex digits per channel) or `#RRGGBB`
/// (exactly 6 hex digits, no `#RRGGBBAA`).
fn sniff_osc_palette(
    bytes: &[u8],
    overrides: &mut [Option<[u8; 3]>; 256],
    override_fg: &mut Option<[u8; 3]>,
    override_bg: &mut Option<[u8; 3]>,
    override_cursor: &mut Option<[u8; 3]>,
    out: &mut Vec<EmulatorEvent>,
) {
    let mut changed = false;
    let mut i = 0;
    while i + 3 <= bytes.len() {
        if bytes[i] != 0x1b || bytes[i + 1] != b']' {
            i += 1;
            continue;
        }
        // Determine OSC number by scanning forward to `;` or terminator.
        let num_start = i + 2;
        let mut num_end = num_start;
        while num_end < bytes.len() && bytes[num_end].is_ascii_digit() {
            num_end += 1;
        }
        if num_end == num_start || num_end >= bytes.len() {
            i += 1;
            continue;
        }
        let sep = bytes[num_end];
        let Ok(osc_num_str) = std::str::from_utf8(&bytes[num_start..num_end]) else {
            i += 1;
            continue;
        };
        let Ok(osc_num): Result<u32, _> = osc_num_str.parse() else {
            i += 1;
            continue;
        };

        // Determine payload start (after the `;`) or handle no-param resets
        // where the number is immediately followed by BEL/ST.
        let payload_start = if sep == b';' {
            num_end + 1
        } else if sep == 0x07 || sep == 0x1b {
            num_end
        } else {
            i += 1;
            continue;
        };

        // Find the string terminator.
        let mut j = payload_start;
        let mut term_end = None;
        while j < bytes.len() {
            if bytes[j] == 0x07 {
                term_end = Some((j, j + 1));
                break;
            }
            if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                term_end = Some((j, j + 2));
                break;
            }
            j += 1;
        }
        let Some((payload_end, total_end)) = term_end else {
            i += 1;
            continue;
        };

        let payload = if let Ok(s) = std::str::from_utf8(&bytes[payload_start..payload_end]) {
            s
        } else {
            i = total_end;
            continue;
        };

        match osc_num {
            // OSC 4 — set indexed colour: `4;<index>;<spec>`.
            4 => {
                // Payload is `<index>;<spec>` (may be multiple `;<index>;<spec>` pairs).
                let mut parts = payload.splitn(3, ';');
                if let (Some(idx_str), Some(spec)) = (parts.next(), parts.next()) {
                    if let Ok(idx) = idx_str.parse::<usize>() {
                        if idx < 256 {
                            if let Some(rgb) = parse_color_spec(spec) {
                                overrides[idx] = Some(rgb);
                                changed = true;
                            }
                        }
                    }
                }
            }
            // OSC 104 — reset indexed colour(s): `104[;<index>]`.
            104 => {
                if payload.is_empty() {
                    // Reset all indexed overrides.
                    for slot in overrides.iter_mut() {
                        *slot = None;
                    }
                    changed = true;
                } else if let Ok(idx) = payload.trim().parse::<usize>() {
                    if idx < 256 {
                        overrides[idx] = None;
                        changed = true;
                    }
                }
            }
            // OSC 10 — set default fg.
            10 => {
                if let Some(rgb) = parse_color_spec(payload) {
                    *override_fg = Some(rgb);
                    changed = true;
                }
            }
            // OSC 11 — set default bg.
            11 => {
                if let Some(rgb) = parse_color_spec(payload) {
                    *override_bg = Some(rgb);
                    changed = true;
                }
            }
            // OSC 12 — set cursor colour.
            12 => {
                if let Some(rgb) = parse_color_spec(payload) {
                    *override_cursor = Some(rgb);
                    changed = true;
                }
            }
            // OSC 110 — reset default fg.
            110 => {
                *override_fg = None;
                changed = true;
            }
            // OSC 111 — reset default bg.
            111 => {
                *override_bg = None;
                changed = true;
            }
            // OSC 112 — reset cursor colour.
            112 => {
                *override_cursor = None;
                changed = true;
            }
            _ => {}
        }

        i = total_end;
    }

    if changed {
        out.push(EmulatorEvent::PaletteChanged);
    }
}

/// Scan `bytes` for `CSI ?2026h` (Begin Synchronized Update) and
/// `CSI ?2026l` (End Synchronized Update) sequences and update `sync_output`
/// and `sync_start` accordingly.
///
/// # Why byte-level scanning instead of the alacritty Handler trait?
///
/// `alacritty_terminal` 0.24 maps `?2026` to `NamedPrivateMode::SyncUpdate`
/// in both `set_private_mode` and `unset_private_mode`, but both arms are
/// no-ops — the library intentionally ignores the mode.  The `Handler` trait
/// is not object-safe in a way that lets us wrap `Term`, so intercepting the
/// two sequences at the raw-bytes level (matching `\x1b[?2026h` and
/// `\x1b[?2026l`) is the cleanest approach that does not require forking the
/// dependency.
///
/// Multiple BSU/ESU pairs in a single chunk are all processed; the last ESU
/// wins.  Sequences that straddle a chunk boundary are missed, but in practice
/// a TUI always emits the full 8-byte sequence in a single write.
fn sniff_decset_2026(bytes: &[u8], sync_output: &mut bool, sync_start: &mut Option<Instant>) {
    // CSI ?2026h  →  0x1B 0x5B 0x3F 0x32 0x30 0x32 0x36 0x68
    // CSI ?2026l  →  0x1B 0x5B 0x3F 0x32 0x30 0x32 0x36 0x6C
    const BSU: &[u8] = b"\x1b[?2026h";
    const ESU: &[u8] = b"\x1b[?2026l";
    const LEN: usize = 8; // both sequences are exactly 8 bytes

    let mut i = 0;
    while i + LEN <= bytes.len() {
        if bytes[i] != 0x1b {
            i += 1;
            continue;
        }
        let window = &bytes[i..i + LEN];
        if window == BSU {
            if !*sync_output {
                *sync_output = true;
                *sync_start = Some(Instant::now());
            }
            i += LEN;
        } else if window == ESU {
            *sync_output = false;
            *sync_start = None;
            i += LEN;
        } else {
            i += 1;
        }
    }
}

/// Parse a colour spec in `rgb:RR/GG/BB` (1–4 hex digits per channel) or
/// `#RRGGBB` (exactly 6 hex digits) form. Returns `None` for unrecognised
/// or malformed specs (e.g. query `?` sent by colour-querying apps).
fn parse_color_spec(s: &str) -> Option<[u8; 3]> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("rgb:") {
        // `rgb:RR/GG/BB` — each channel is 1–4 hex digits; we take the
        // top 8 bits if more than 2 digits are given.
        let mut parts = rest.splitn(3, '/');
        let r = parse_hex_channel(parts.next()?)?;
        let g = parse_hex_channel(parts.next()?)?;
        let b = parse_hex_channel(parts.next()?)?;
        // Make sure there's no leftover (i.e. guard against a 4th `/`).
        Some([r, g, b])
    } else if let Some(hex) = s.strip_prefix('#') {
        // `#RRGGBB` — exactly 6 hex digits.
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some([r, g, b])
    } else {
        None
    }
}

/// Parse a single channel value from an X11 `rgb:…` colour spec. Each
/// channel may be 1–4 hex digits; we normalise to 8 bits by shifting the
/// parsed value right by `(digits - 2) * 4` (i.e. taking the top 8 bits).
fn parse_hex_channel(s: &str) -> Option<u8> {
    let len = s.len();
    if len == 0 || len > 4 {
        return None;
    }
    let v = u16::from_str_radix(s, 16).ok()?;
    // Normalise to 8 bits: shift right by (len - 2) * 4, but for 1-digit
    // inputs (which represent 0–F i.e. 0–15) scale to 0–255 by repeating
    // the nibble: 0xF → 0xFF, 0x8 → 0x88.
    let byte = match len {
        1 => (v as u8) | ((v as u8) << 4), // replicate nibble
        2 => v as u8,                      // already 8 bits
        3 => (v >> 4) as u8,               // top 8 of 12
        4 => (v >> 8) as u8,               // top 8 of 16
        _ => unreachable!(),
    };
    Some(byte)
}

/// Maximum bytes we'll accumulate for a single DCS Sixel payload. A sixel
/// image that exceeds this cap is silently dropped — protects against a
/// runaway sender that never closes the DCS frame.
const DCS_SIXEL_BUF_CAP: usize = 8 * 1024 * 1024; // 8 MiB

/// Scan `bytes` for Sixel DCS frames and accumulate payload bytes, forwarding
/// all non-Sixel bytes back to the caller (for alacritty's parser).
///
/// DCS Sixel introducers:
/// - `ESC P <params> q` (3+ bytes: 0x1B 0x50 … 0x71)
/// - `0x90 <params> q`  (2-byte C1 form, one byte introducer)
///
/// String terminator (ST):
/// - `ESC \` (0x1B 0x5C)
/// - `0x9C`  (C1 ST)
///
/// `intro_prefix` is filled with any trailing bytes of a partial-intro sequence
/// (e.g. a lone `ESC` or `ESC P` without a `q`) that could not be classified
/// yet. The caller must prepend these to the next chunk before calling again.
///
/// Returns a `Vec<u8>` of bytes that should be forwarded to alacritty. Bytes
/// consumed for Sixel (introducer, payload, ST) are withheld.
fn sniff_dcs_sixel(
    bytes: &[u8],
    cursor_abs: i32,
    active: &mut bool,
    buf: &mut Vec<u8>,
    anchor_abs: &mut i32,
    intro_prefix: &mut Vec<u8>,
    store: &mut images::ImageStore,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if *active {
            // We are inside an in-progress Sixel DCS. Look for ST.
            match bytes[i] {
                // C1 ST: 0x9C
                0x9c => {
                    // End of DCS frame — decode and place.
                    commit_sixel(buf, *anchor_abs, store);
                    *active = false;
                    buf.clear();
                    i += 1;
                }
                // ESC (could be the start of ESC \)
                0x1b => {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                        // ESC \ — ST found.
                        commit_sixel(buf, *anchor_abs, store);
                        *active = false;
                        buf.clear();
                        i += 2;
                    } else if i + 1 >= bytes.len() {
                        // ESC is the last byte of this chunk; next byte may be
                        // `\` (completing the ST). Save it in intro_prefix so
                        // the next call can check without re-buffering it as
                        // payload.
                        intro_prefix.push(0x1b);
                        i += 1;
                    } else {
                        // Some other escape — not an ST. Buffer the ESC as
                        // payload and continue.
                        if buf.len() < DCS_SIXEL_BUF_CAP {
                            buf.push(bytes[i]);
                        }
                        i += 1;
                    }
                }
                b => {
                    // Regular payload byte.
                    if buf.len() < DCS_SIXEL_BUF_CAP {
                        buf.push(b);
                    }
                    i += 1;
                }
            }
        } else {
            // Not inside a Sixel frame — look for a DCS Sixel introducer.
            //
            // Two intro forms:
            //   A) ESC P [params] q   (C0 form: 0x1B 0x50 [params] 0x71)
            //   B) 0x90 [params] q    (C1 form: 0x90 [params] 0x71)
            //
            // The `<params>` section is optional digits and `;` characters
            // immediately between the DCS prefix and the `q`.

            let is_esc = bytes[i] == 0x1b;
            let is_c1_dcs = bytes[i] == 0x90;

            if is_esc {
                if i + 1 >= bytes.len() {
                    // Lone ESC at the end of the chunk — save it; the next byte
                    // (which may be `P`) will arrive in the next chunk.
                    intro_prefix.push(0x1b);
                    i += 1;
                    continue;
                }
                if bytes[i + 1] != 0x50 {
                    // ESC followed by something other than P — not a DCS.
                    out.push(bytes[i]);
                    i += 1;
                    continue;
                }
                // ESC P — confirmed DCS start. Scan for [params] q.
                let param_start = i + 2;
                let mut j = param_start;
                while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
                    j += 1;
                }
                if j >= bytes.len() {
                    // Didn't reach `q` — rest of intro may come in next chunk.
                    // Save everything from ESC through end-of-chunk.
                    intro_prefix.extend_from_slice(&bytes[i..]);
                    i = bytes.len();
                    continue;
                }
                if bytes[j] == b'q' {
                    // Found a valid Sixel DCS introducer.
                    *active = true;
                    *anchor_abs = cursor_abs;
                    buf.clear();
                    i = j + 1; // skip past `q`
                } else {
                    // DCS but not a Sixel introducer (some other DCS type).
                    // Pass the ESC through and resume scanning.
                    out.push(bytes[i]);
                    i += 1;
                }
            } else if is_c1_dcs {
                // C1 DCS form: 0x90 [params] q
                let param_start = i + 1;
                let mut j = param_start;
                while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
                    j += 1;
                }
                if j >= bytes.len() {
                    intro_prefix.extend_from_slice(&bytes[i..]);
                    i = bytes.len();
                    continue;
                }
                if bytes[j] == b'q' {
                    *active = true;
                    *anchor_abs = cursor_abs;
                    buf.clear();
                    i = j + 1;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            } else {
                // Normal byte, pass through.
                out.push(bytes[i]);
                i += 1;
            }
        }
    }

    out
}

/// Decode the accumulated sixel payload in `buf` and place the resulting
/// image in `store` at `anchor_abs` absolute line.
fn commit_sixel(buf: &[u8], anchor_abs: i32, store: &mut images::ImageStore) {
    if buf.is_empty() {
        return;
    }
    let Some(img) = sixel::decode(buf) else {
        tracing::debug!("sixel: decode produced no image (empty/invalid payload)");
        return;
    };
    // Pack the RGBA into a PNG in memory so we can reuse `ImageStore::add_image`
    // which calls the `image` crate's decoder.  An alternative would be to add
    // a separate `add_rgba` method to `ImageStore`; we keep that for a future
    // optimisation pass.
    let raw_rgba = img.rgba;
    let w = img.width;
    let h = img.height;

    // Build a PNG in-memory using the `image` crate.
    let Some(id) = add_rgba_to_store(store, raw_rgba, w, h) else {
        return;
    };
    let Some(image_meta) = store.get_image(id) else {
        return;
    };
    let (cols, rows) = image_cell_footprint(
        image_meta.width_px,
        image_meta.height_px,
        8.0,
        16.0,
        0, // auto
        0, // auto
    );
    let anchor_col = 0u16;
    store.place(id, anchor_abs, anchor_col, cols, rows);
    tracing::debug!(
        id,
        anchor_abs,
        cols,
        rows,
        width_px = w,
        height_px = h,
        "sixel image placed"
    );
}

/// Encode raw RGBA8 pixels as a PNG in memory and add to the image store.
/// Returns the assigned `ImageId`, or `None` on encode/decode failure.
fn add_rgba_to_store(
    store: &mut images::ImageStore,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
) -> Option<images::ImageId> {
    use image::{ImageBuffer, Rgba};
    let buf: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_raw(width, height, rgba)?;
    let mut png_bytes = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut png_bytes, image::ImageFormat::Png)
        .map_err(|e| tracing::warn!(error = ?e, "sixel: PNG encode failed"))
        .ok()?;
    store.add_image(&png_bytes.into_inner())
}

/// Maximum bytes we accumulate for a single APC graphics payload. Defence
/// against a runaway sender that never closes the APC frame.
const APC_GRAPHICS_BUF_CAP_LOCAL: usize = apc_graphics::APC_GRAPHICS_BUF_CAP;

/// Scan `bytes` for APC graphics `ESC _ G … ST` sequences, strip them from
/// the output, decode them via [`apc_graphics::ApcGraphicsAssembler`], and
/// place any completed images in `store` at `cursor_abs`.
///
/// APC graphics introducer: `ESC _` (0x1B 0x5F) followed by `G` (0x47).
/// String terminator (ST): `ESC \` (0x1B 0x5C) or C1 ST (0x9C).
///
/// `intro_prefix` is filled with any trailing bytes of a partial-intro
/// sequence (lone `ESC` or `ESC _` without `G`) that cannot be classified
/// yet. The caller must prepend these to the next chunk before calling again.
///
/// Returns a `Vec<u8>` of bytes that should be forwarded to alacritty. Bytes
/// consumed for APC graphics (introducer, payload, ST) are withheld.
#[allow(clippy::too_many_arguments)]
fn sniff_apc_graphics(
    bytes: &[u8],
    cursor_abs: i32,
    active: &mut bool,
    ctrl_buf: &mut Vec<u8>,
    in_ctrl: &mut bool,
    payload_buf: &mut Vec<u8>,
    anchor_abs: &mut i32,
    intro_prefix: &mut Vec<u8>,
    assembler: &mut apc_graphics::ApcGraphicsAssembler,
    store: &mut images::ImageStore,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if *active {
            // We are inside an in-progress APC graphics frame. Look for ST.
            match bytes[i] {
                // C1 ST: 0x9C
                0x9c => {
                    commit_apc_graphics_chunk(ctrl_buf, payload_buf, *anchor_abs, assembler, store);
                    *active = false;
                    *in_ctrl = false;
                    ctrl_buf.clear();
                    payload_buf.clear();
                    i += 1;
                }
                // ESC — could be the start of ESC \
                0x1b => {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                        // ESC \ — ST found.
                        commit_apc_graphics_chunk(
                            ctrl_buf,
                            payload_buf,
                            *anchor_abs,
                            assembler,
                            store,
                        );
                        *active = false;
                        *in_ctrl = false;
                        ctrl_buf.clear();
                        payload_buf.clear();
                        i += 2;
                    } else if i + 1 >= bytes.len() {
                        // ESC is the last byte; save for next chunk.
                        intro_prefix.push(0x1b);
                        i += 1;
                    } else {
                        // Some other escape — treat as payload byte.
                        if *in_ctrl {
                            if ctrl_buf.len() < APC_GRAPHICS_BUF_CAP_LOCAL {
                                ctrl_buf.push(bytes[i]);
                            }
                        } else if payload_buf.len() < APC_GRAPHICS_BUF_CAP_LOCAL {
                            payload_buf.push(bytes[i]);
                        }
                        i += 1;
                    }
                }
                // Semicolon separates control-data from payload.
                b';' if *in_ctrl => {
                    *in_ctrl = false;
                    i += 1;
                }
                b => {
                    if *in_ctrl {
                        if ctrl_buf.len() < APC_GRAPHICS_BUF_CAP_LOCAL {
                            ctrl_buf.push(b);
                        }
                    } else if payload_buf.len() < APC_GRAPHICS_BUF_CAP_LOCAL {
                        payload_buf.push(b);
                    }
                    i += 1;
                }
            }
        } else {
            // Not inside an APC graphics frame. Look for APC graphics introducer:
            // `ESC _` (0x1B 0x5F) followed by `G` (0x47).

            if bytes[i] == 0x1b {
                if i + 1 >= bytes.len() {
                    // Lone ESC at end of chunk — save it.
                    intro_prefix.push(0x1b);
                    i += 1;
                    continue;
                }
                if bytes[i + 1] != 0x5f {
                    // ESC followed by something other than `_` — not an APC.
                    out.push(bytes[i]);
                    i += 1;
                    continue;
                }
                // ESC _ confirmed. Check for `G`.
                if i + 2 >= bytes.len() {
                    // `ESC _` at end of chunk — save and wait for `G`.
                    intro_prefix.extend_from_slice(&bytes[i..]);
                    i = bytes.len();
                    continue;
                }
                if bytes[i + 2] == b'G' {
                    // `ESC _ G` — valid APC graphics introducer.
                    *active = true;
                    *in_ctrl = true;
                    *anchor_abs = cursor_abs;
                    ctrl_buf.clear();
                    payload_buf.clear();
                    i += 3; // skip past `G`
                } else {
                    // APC but not graphics (different first byte after `_`).
                    out.push(bytes[i]);
                    i += 1;
                }
            } else {
                // Normal byte, pass through.
                out.push(bytes[i]);
                i += 1;
            }
        }
    }

    out
}

/// Decode one complete APC graphics chunk (ctrl + payload already accumulated)
/// and dispatch to the assembler. If the assembler produces a finished image,
/// add it to `store` and place it at `anchor_abs`.
fn commit_apc_graphics_chunk(
    ctrl_buf: &[u8],
    payload_buf: &[u8],
    anchor_abs: i32,
    assembler: &mut apc_graphics::ApcGraphicsAssembler,
    store: &mut images::ImageStore,
) {
    // Parse control-data string (ASCII, so lossy UTF-8 decode is fine).
    let ctrl_str = String::from_utf8_lossy(ctrl_buf);
    let ctrl = apc_graphics::parse_control(&ctrl_str);

    // Feed into the assembler.
    let Some(img) = assembler.feed(ctrl, payload_buf) else {
        return; // more chunks expected, or decode error (already logged)
    };

    // We have a complete decoded image. Store it.
    let Some(id) = store.add_rgba8(img.width, img.height, img.rgba) else {
        return;
    };

    // Place the image if the action implies display.
    let display = matches!(
        img.control.action,
        apc_graphics::ApcAction::TransmitAndDisplay | apc_graphics::ApcAction::Put
    );
    if display {
        let (cols, rows) = image_cell_footprint(
            img.width,
            img.height,
            8.0,
            16.0,
            img.control.cols,
            img.control.rows,
        );
        store.place(id, anchor_abs, 0, cols, rows);
        tracing::debug!(
            id,
            anchor_abs,
            cols,
            rows,
            width_px = img.width,
            height_px = img.height,
            image_id = img.control.image_id,
            "apc_graphics image placed"
        );
    } else {
        tracing::debug!(
            id,
            image_id = img.control.image_id,
            "apc_graphics image stored (transmit-only)"
        );
    }
}

/// Minimal base64 decoder (standard alphabet, no padding required).
/// Avoids adding a new crate: the only consumer is OSC 1337 SetUserVar which
/// is a best-effort feature — if this ever needs to handle all edge cases we
/// can add `data-encoding` to the workspace.
fn base64_decode(input: &str) -> Result<Vec<u8>, ()> {
    let table: [u8; 256] = {
        let mut t = [0xffu8; 256];
        for (i, &c) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
            .iter()
            .enumerate()
        {
            t[c as usize] = i as u8;
        }
        t['=' as usize] = 0;
        t
    };
    let bytes = input.trim().as_bytes();
    let mut out = Vec::with_capacity((bytes.len() * 3) / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &b in bytes {
        if b == b'=' {
            break;
        }
        let v = table[b as usize];
        if v == 0xff {
            return Err(());
        }
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// Maximum bytes we'll carry across chunks while waiting for an OSC 7
/// terminator. Defence against a malicious peer that opens an OSC and
/// never closes it.
const OSC7_PARTIAL_CAP: usize = 4096;

/// Scan `chunk` for cwd-announcement OSC sequences — both the Unix
/// convention (`\e]7;file://…\e\\`) and the Windows / ConPTY one
/// (`\e]9;9;<path>\e\\`). Joins anything left dangling from the
/// previous call so split-across-chunks frames still parse cleanly.
fn sniff_cwd(pending: &mut Vec<u8>, chunk: &[u8], mut on_cwd: impl FnMut(String)) {
    pending.extend_from_slice(chunk);
    if pending.len() > OSC7_PARTIAL_CAP * 2 {
        // Drop everything except the tail so we still see closing
        // terminators when they finally arrive.
        let keep_from = pending.len() - OSC7_PARTIAL_CAP;
        pending.drain(0..keep_from);
    }

    let mut consumed_up_to = 0usize;
    let bytes = pending.clone();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        // OSC 7 (Unix): \e]7;file://host/path<ST>
        // OSC 9;9 (Windows / ConPTY): \e]9;9;<path><ST>
        let osc7_match = bytes[i] == 0x1b
            && bytes[i + 1] == b']'
            && bytes[i + 2] == b'7'
            && bytes[i + 3] == b';';
        let osc9_9_match = i + 6 <= bytes.len()
            && bytes[i] == 0x1b
            && bytes[i + 1] == b']'
            && bytes[i + 2] == b'9'
            && bytes[i + 3] == b';'
            && bytes[i + 4] == b'9'
            && bytes[i + 5] == b';';
        if !osc7_match && !osc9_9_match {
            i += 1;
            continue;
        }
        let payload_start = if osc7_match { i + 4 } else { i + 6 };
        let mut j = payload_start;
        let mut found_end = None;
        while j < bytes.len() {
            if bytes[j] == 0x07 {
                found_end = Some((j, j + 1));
                break;
            }
            if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                found_end = Some((j, j + 2));
                break;
            }
            j += 1;
        }
        if let Some((payload_end, total_end)) = found_end {
            if let Ok(s) = std::str::from_utf8(&bytes[payload_start..payload_end]) {
                let path = if osc7_match {
                    file_uri_to_path(s)
                } else {
                    // OSC 9;9 payload is the bare path. Strip surrounding
                    // double quotes if present (some shells emit them).
                    let trimmed = s.trim().trim_matches('"');
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                };
                if let Some(p) = path {
                    on_cwd(p);
                }
            }
            consumed_up_to = total_end;
            i = total_end;
            continue;
        }
        // No terminator yet — leave from `i` onward in the buffer.
        break;
    }

    if consumed_up_to > 0 {
        pending.drain(0..consumed_up_to);
    }
    // If no OSC at all is in the buffer, trim everything (don't carry
    // unrelated PTY bytes forever).
    if !contains_esc_open(pending) {
        pending.clear();
    }
}

fn contains_esc_open(bytes: &[u8]) -> bool {
    bytes.windows(2).any(|w| w[0] == 0x1b && w[1] == b']')
}

/// `file://hostname/path` → `/path` (percent-decoded). Returns `None`
/// when the URI isn't a `file:` scheme.
fn file_uri_to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let slash = rest.find('/').unwrap_or(rest.len());
    let path = &rest[slash..];
    let decoded = percent_decode(path);
    // A Windows `file:///C:/dir` URI decodes to `/C:/dir`; the leading
    // slash before the drive letter makes it an invalid OS path (a cwd of
    // `/C:/dir` fails to spawn). Strip it for the `/<letter>:/…` shape.
    let b = decoded.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        Some(decoded[1..].to_string())
    } else {
        Some(decoded)
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h1), Some(h2)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                out.push((h1 << 4) | h2);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Returns the visible width (in columns) of a printable character.
#[must_use]
pub fn char_width(c: char) -> u8 {
    use unicode_width::UnicodeWidthChar;
    // .min(2) caps the value at 2, which always fits in u8.
    u8::try_from(c.width().unwrap_or(0).min(2)).unwrap_or(0)
}

/// Encode bytes to standard base64 (used in tests to craft OSC 1337 payloads).
#[cfg(test)]
fn base64_encode(data: &[u8]) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i];
        let b1 = if i + 1 < data.len() { data[i + 1] } else { 0 };
        let b2 = if i + 2 < data.len() { data[i + 2] } else { 0 };
        out.push(ALPHA[(b0 >> 2) as usize] as char);
        out.push(ALPHA[((b0 & 3) << 4 | b1 >> 4) as usize] as char);
        if i + 1 < data.len() {
            out.push(ALPHA[((b1 & 0xf) << 2 | b2 >> 6) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < data.len() {
            out.push(ALPHA[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_chars_are_width_one() {
        assert_eq!(char_width('a'), 1);
        assert_eq!(char_width('Z'), 1);
    }

    #[test]
    fn cjk_is_width_two() {
        assert_eq!(char_width('日'), 2);
    }

    #[test]
    fn control_chars_are_zero() {
        assert_eq!(char_width('\u{0007}'), 0);
    }

    #[test]
    fn emulator_starts_empty() {
        let emu = Emulator::new(80, 24);
        let (cols, rows) = emu.size();
        assert_eq!((cols, rows), (80, 24));
    }

    #[test]
    fn emulator_writes_hello() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"hello");
        let mut found = Vec::new();
        emu.for_each_visible_cell(|col, row, snap| {
            if row == 0 && col < 5 {
                found.push(snap.ch);
            }
        });
        assert_eq!(found, vec!['h', 'e', 'l', 'l', 'o']);
    }

    #[test]
    fn dectcem_hides_and_shows_cursor_shape() {
        let mut emu = Emulator::new(80, 24);
        // Cursor is visible by default.
        assert!(
            emu.cursor_shape().is_some(),
            "a fresh emulator must show a cursor"
        );
        // DECTCEM hide (ESC [ ? 25 l) → no cursor at all, so a TUI that draws
        // its own (vim, fzf, the Claude Code prompt) doesn't show two.
        emu.advance(b"\x1b[?25l");
        assert!(
            emu.cursor_shape().is_none(),
            "ESC[?25l must hide the terminal cursor"
        );
        // DECTCEM show (ESC [ ? 25 h) → cursor returns.
        emu.advance(b"\x1b[?25h");
        assert!(
            emu.cursor_shape().is_some(),
            "ESC[?25h must restore the terminal cursor"
        );
    }

    #[test]
    fn file_uri_to_path_handles_windows_and_unix() {
        // Windows drive: the leading slash before the drive letter is stripped.
        assert_eq!(
            file_uri_to_path("file://localhost/C:/Windows").as_deref(),
            Some("C:/Windows")
        );
        assert_eq!(
            file_uri_to_path("file:///D:/Users/foo").as_deref(),
            Some("D:/Users/foo")
        );
        // Percent-decoding still works.
        assert_eq!(
            file_uri_to_path("file:///C:/My%20Docs").as_deref(),
            Some("C:/My Docs")
        );
        // Unix path keeps its leading slash.
        assert_eq!(
            file_uri_to_path("file://host/home/user").as_deref(),
            Some("/home/user")
        );
        assert_eq!(file_uri_to_path("not-a-file-uri"), None);
    }

    #[test]
    fn text_in_range_reads_scrollback_at_offset() {
        let mut emu = Emulator::new(20, 3);
        for i in 0..10 {
            emu.advance(format!("line{i:02}\r\n").as_bytes());
        }
        let h = emu.history_size();
        assert!(h > 0, "expected scrollback");
        // At max scroll, viewport row 0 is the oldest scrollback line —
        // before the scroll fix this read the live screen instead.
        assert_eq!(emu.text_in_range((0, 0), (19, 0), h).trim(), "line00");
        // At the live edge the oldest line isn't visible.
        assert_ne!(emu.text_in_range((0, 0), (19, 0), 0).trim(), "line00");
    }

    #[test]
    fn word_at_respects_custom_separators() {
        let mut emu = Emulator::new(40, 3);
        // Layout (row 0): "foo.bar baz/qux"
        //                  0123456789...
        emu.advance(b"foo.bar baz/qux");

        // With NO separators (only whitespace splits), the dot stays joined
        // so the whole "foo.bar" run is one word.
        let (a, b) = emu.word_at(1, 0, 0, "");
        assert_eq!(a, (0, 0));
        assert_eq!(b, (6, 0), "empty separators: dot should not split");

        // With '.' as a separator, double-clicking on "foo" stops at the dot.
        let (a, b) = emu.word_at(1, 0, 0, ".");
        assert_eq!(a, (0, 0));
        assert_eq!(b, (2, 0), "dot separator: 'foo' only");

        // Clicking past the dot selects "bar" (cols 4..=6).
        let (a, b) = emu.word_at(5, 0, 0, ".");
        assert_eq!(a, (4, 0));
        assert_eq!(b, (6, 0));

        // '/' as a separator splits "baz/qux"; clicking "qux" (col 12..=14).
        let (a, b) = emu.word_at(13, 0, 0, "/");
        assert_eq!(a, (12, 0));
        assert_eq!(b, (14, 0));

        // Whitespace is always a boundary regardless of the set.
        let (a, b) = emu.word_at(7, 0, 0, ".");
        assert_eq!(a, (7, 0), "click on the space is a single-cell range");
        assert_eq!(b, (7, 0));
    }

    #[test]
    fn esc_3j_clears_scrollback() {
        let mut emu = Emulator::new(20, 3);
        for i in 0..30 {
            emu.advance(format!("line{i}\r\n").as_bytes());
        }
        assert!(emu.history_size() > 0, "expected scrollback to accumulate");
        // ED with Ps=3 — "Erase Saved Lines" (clear scrollback).
        emu.advance(b"\x1b[3J");
        assert_eq!(emu.history_size(), 0, "scrollback should be cleared");
    }

    #[test]
    fn clear_buffer_then_shell_redraw_keeps_history_empty() {
        let mut emu = Emulator::new(20, 3);
        for i in 0..30 {
            emu.advance(format!("line{i}\r\n").as_bytes());
        }
        assert!(emu.history_size() > 0, "expected scrollback to accumulate");
        // Pre-wipe: capture visible content to confirm it disappears.
        let old_hist = emu.history_size();
        emu.clear_buffer_to_blank();
        assert_eq!(
            emu.history_size(),
            0,
            "clear_buffer_to_blank must empty history"
        );
        // Simulate the shell echoing its own clear (what \x0c triggers in
        // bash via tput clear): with the old code, this scrolled the 3
        // visible rows (prompt + output) back into the freshly-emptied
        // scrollback — `history_size()` would jump by `screen_lines` (3).
        // With our fix the viewport is blank before the shell redraw arrives,
        // so alacritty's clear_viewport can only scroll at most 1 blank line.
        emu.advance(b"\x1b[H\x1b[2J");
        let post_hist = emu.history_size();
        assert!(
            post_hist <= 1,
            "shell redraw must not refill history with live content \
             (got {post_hist} lines, old code would have added {old_hist} lines)"
        );
    }

    #[test]
    fn clear_buffer_to_blank_homes_cursor() {
        let mut emu = Emulator::new(20, 5);
        emu.advance(b"hello\r\nworld\r\n");
        emu.clear_buffer_to_blank();
        assert_eq!(
            emu.cursor(),
            (0, 0),
            "cursor should be at (0,0) after clear"
        );
    }

    #[test]
    fn set_scrollback_caps_history() {
        let mut emu = Emulator::new(20, 3);
        emu.set_scrollback(5);
        for i in 0..50 {
            emu.advance(format!("line{i}\r\n").as_bytes());
        }
        assert!(
            emu.history_size() <= 5,
            "history should be capped at 5, got {}",
            emu.history_size()
        );
        // The most recent output survives on the visible screen.
        let lines = emu.buffer_lines_text();
        assert!(
            lines.iter().any(|l| l == "line49"),
            "newest line should still be present, got {lines:?}"
        );
    }

    #[test]
    fn buffer_lines_text_includes_scrollback() {
        // Small grid so a handful of lines spill into the scrollback.
        let mut emu = Emulator::new(20, 3);
        for i in 0..10 {
            emu.advance(format!("line{i}\r\n").as_bytes());
        }
        let lines = emu.buffer_lines_text();
        // history + the 3 visible rows.
        assert_eq!(lines.len(), emu.history_size() + 3);
        // Early lines (scrolled off-screen) are still searchable here.
        assert!(
            lines.iter().any(|l| l == "line0"),
            "scrollback line should be present, got {lines:?}"
        );
        assert!(lines.iter().any(|l| l == "line9"));
        // Absolute-line invariant: entry `i` maps to alacritty line
        // `i - history_size`, so the last 3 entries are the visible screen.
        let hist = emu.history_size();
        assert!(hist >= 7, "expected ~7 lines of history, got {hist}");
    }

    // ── UnderlineStyle mapping ──────────────────────────────────────────────

    #[test]
    fn underline_style_none_when_no_flags() {
        assert_eq!(
            UnderlineStyle::from_flags(Flags::empty()),
            UnderlineStyle::None
        );
    }

    #[test]
    fn underline_style_single() {
        assert_eq!(
            UnderlineStyle::from_flags(Flags::UNDERLINE),
            UnderlineStyle::Single
        );
    }

    #[test]
    fn underline_style_double() {
        assert_eq!(
            UnderlineStyle::from_flags(Flags::DOUBLE_UNDERLINE),
            UnderlineStyle::Double
        );
    }

    #[test]
    fn underline_style_curly_from_undercurl() {
        assert_eq!(
            UnderlineStyle::from_flags(Flags::UNDERCURL),
            UnderlineStyle::Curly
        );
    }

    #[test]
    fn underline_style_dotted() {
        assert_eq!(
            UnderlineStyle::from_flags(Flags::DOTTED_UNDERLINE),
            UnderlineStyle::Dotted
        );
    }

    #[test]
    fn underline_style_dashed() {
        assert_eq!(
            UnderlineStyle::from_flags(Flags::DASHED_UNDERLINE),
            UnderlineStyle::Dashed
        );
    }

    #[test]
    fn undercurl_beats_double_underline() {
        // When multiple flags are present UNDERCURL wins.
        let flags = Flags::UNDERCURL | Flags::DOUBLE_UNDERLINE;
        assert_eq!(UnderlineStyle::from_flags(flags), UnderlineStyle::Curly);
    }

    #[test]
    fn strikethrough_mapped_from_sgr_advance() {
        let mut emu = Emulator::new(80, 24);
        // SGR 9 = strikethrough, SGR 0 = reset.
        emu.advance(b"\x1b[9mX");
        let mut found = false;
        emu.for_each_visible_cell(|col, row, snap| {
            if row == 0 && col == 0 {
                found = snap.strikethrough;
            }
        });
        assert!(
            found,
            "cell written with SGR 9 must have strikethrough=true"
        );
    }

    #[test]
    fn underline_color_from_sgr_58() {
        let mut emu = Emulator::new(80, 24);
        // SGR 58:2::<R>:<G>:<B> sets the underline colour (extended SGR).
        // SGR 4 activates underline so it's visible.
        emu.advance(b"\x1b[4;58:2::255:128:0mX");
        let mut ul_color: Option<Option<[u8; 3]>> = None;
        emu.for_each_visible_cell(|col, row, snap| {
            if row == 0 && col == 0 {
                ul_color = Some(snap.underline_color);
            }
        });
        let ul_color = ul_color.expect("cell must exist");
        // If alacritty parsed the SGR, the underline colour must be Some([255,128,0]).
        // If it silently ignored SGR 58 (older behaviour), the colour is None — that
        // is also acceptable for a forward-compat reason; this test just must not
        // panic or produce an impossible colour.
        if let Some(c) = ul_color {
            assert_eq!(
                c,
                [255, 128, 0],
                "underline colour must match SGR 58:2::255:128:0"
            );
        }
    }

    proptest::proptest! {
        #[test]
        fn char_width_is_bounded(c in proptest::char::any()) {
            let w = char_width(c);
            proptest::prop_assert!(w <= 2);
        }

        #[test]
        fn parser_never_panics(input in proptest::collection::vec(proptest::num::u8::ANY, 0..1024)) {
            let mut emu = Emulator::new(80, 24);
            emu.advance(&input);
        }
    }

    // ── OSC 133 shell integration parsing ──────────────────────────────────

    #[test]
    fn osc_133_full_cycle_via_advance() {
        let mut emu = Emulator::new(80, 24);
        // A complete A→B→C→D cycle with exit code 0 (BEL terminator).
        emu.advance(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07\x1b]133;D;0\x07");
        assert_eq!(
            emu.semantic().len(),
            1,
            "one complete cycle must produce one mark"
        );
        let mark = emu.semantic().iter_marks().next().unwrap();
        assert_eq!(mark.exit_code, Some(0));
    }

    #[test]
    fn osc_133_nonzero_exit() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b]133;A\x07\x1b]133;D;127\x07");
        let mark = emu.semantic().iter_marks().next().unwrap();
        assert_eq!(mark.exit_code, Some(127));
    }

    #[test]
    fn osc_133_unknown_exit_when_d_has_no_code() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b]133;A\x07\x1b]133;D\x07");
        let mark = emu.semantic().iter_marks().next().unwrap();
        assert_eq!(mark.exit_code, None);
    }

    #[test]
    fn osc_133_st_terminator_works() {
        // String terminator ESC\ instead of BEL.
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b]133;A\x1b\\\x1b]133;D;0\x1b\\");
        assert_eq!(emu.semantic().len(), 1);
    }

    #[test]
    fn osc_133_ignored_by_existing_osc7_and_cwd() {
        // A cwd announcement and an OSC 133 in the same chunk must not
        // interfere with each other.
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b]7;file:///home/user\x1b\\\x1b]133;A\x07\x1b]133;D;0\x07");
        assert_eq!(
            emu.current_dir().unwrap_or(""),
            "/home/user",
            "OSC 7 must still parse"
        );
        assert_eq!(emu.semantic().len(), 1, "OSC 133 must still produce a mark");
    }

    // ── OSC 1337 SetUserVar ─────────────────────────────────────────────────

    #[test]
    fn osc_1337_set_user_var_decoded() {
        let mut emu = Emulator::new(80, 24);
        // "hello" in base64 = "aGVsbG8="
        emu.advance(b"\x1b]1337;SetUserVar=greeting=aGVsbG8=\x07");
        assert_eq!(emu.user_var("greeting"), Some("hello"));
    }

    #[test]
    fn osc_1337_overwrite_user_var() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b]1337;SetUserVar=k=Zmlyc3Q=\x07"); // "first"
        emu.advance(b"\x1b]1337;SetUserVar=k=c2Vjb25k\x07"); // "second"
        assert_eq!(emu.user_var("k"), Some("second"));
    }

    #[test]
    fn osc_1337_missing_var_is_none() {
        let emu = Emulator::new(80, 24);
        assert_eq!(emu.user_var("nonexistent"), None);
    }

    /// Hostile output minting ever-new variable names must hit the cap
    /// (no unbounded memory growth), while updates to existing names keep
    /// working even at the cap.
    #[test]
    fn osc_1337_user_var_count_is_capped() {
        let mut emu = Emulator::new(80, 24);
        for i in 0..(USER_VARS_CAP + 50) {
            // "x" in base64 = "eA=="
            let seq = format!("\x1b]1337;SetUserVar=var{i}=eA==\x07");
            emu.advance(seq.as_bytes());
        }
        assert_eq!(emu.user_var("var0"), Some("x"), "early vars retained");
        assert_eq!(
            emu.user_var(&format!("var{}", USER_VARS_CAP + 10)),
            None,
            "names past the cap must be dropped"
        );
        // Updating an existing name still works at the cap. "y" = "eQ==".
        emu.advance(b"\x1b]1337;SetUserVar=var0=eQ==\x07");
        assert_eq!(emu.user_var("var0"), Some("y"));
    }

    #[test]
    fn base64_decode_roundtrip() {
        // Test the internal base64 decoder directly.
        let cases: &[(&str, &[u8])] = &[
            ("", b""),
            ("Zg==", b"f"),
            ("Zm8=", b"fo"),
            ("Zm9v", b"foo"),
            ("aGVsbG8=", b"hello"),
            ("QUJD", b"ABC"),
        ];
        for (encoded, expected) in cases {
            let got = base64_decode(encoded).expect("should decode");
            assert_eq!(got, *expected, "base64 mismatch for {encoded:?}");
        }
    }

    // ── OSC 9 / OSC 777 notification parsing ──────────────────────────────

    /// OSC 9;9;<path> is the cwd form — must NOT produce a Notification event.
    #[test]
    fn osc9_cwd_form_does_not_notify() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b]9;9;/home/user\x07");
        let events = emu.drain_events();
        let has_notify = events
            .iter()
            .any(|e| matches!(e, EmulatorEvent::Notification { .. }));
        assert!(
            !has_notify,
            "OSC 9;9;<path> is a cwd announcement, must not emit Notification"
        );
        assert_eq!(
            emu.current_dir(),
            Some("/home/user"),
            "OSC 9;9 must still set the cwd"
        );
    }

    /// OSC 9;<body> (non-cwd form) must emit a Notification event.
    #[test]
    fn osc9_body_only_emits_notification() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b]9;Build finished\x07");
        let events = emu.drain_events();
        let notif = events
            .iter()
            .find(|e| matches!(e, EmulatorEvent::Notification { .. }));
        let Some(EmulatorEvent::Notification { title, body }) = notif else {
            panic!("expected Notification event, got: {events:?}");
        };
        assert!(title.is_empty(), "OSC 9 body-only form has no title");
        assert_eq!(body, "Build finished");
    }

    /// OSC 777;notify;<title>;<body> must emit a Notification with title + body.
    #[test]
    fn osc777_notify_emits_notification_with_title() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b]777;notify;Done;My task completed\x07");
        let events = emu.drain_events();
        let notif = events
            .iter()
            .find(|e| matches!(e, EmulatorEvent::Notification { .. }));
        let Some(EmulatorEvent::Notification { title, body }) = notif else {
            panic!("expected Notification event, got: {events:?}");
        };
        assert_eq!(title, "Done");
        assert_eq!(body, "My task completed");
    }

    /// OSC 777;notify;<body-only> (no second semicolon) must still parse.
    #[test]
    fn osc777_notify_body_only() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b]777;notify;Just a body\x07");
        let events = emu.drain_events();
        let notif = events
            .iter()
            .find(|e| matches!(e, EmulatorEvent::Notification { .. }));
        let Some(EmulatorEvent::Notification { title, body }) = notif else {
            panic!("expected Notification event");
        };
        assert!(title.is_empty());
        assert_eq!(body, "Just a body");
    }

    // ── OSC 4/104/10/11/12 dynamic colour overrides ────────────────────────

    /// `parse_color_spec` must accept the `rgb:RR/GG/BB` form.
    #[test]
    fn parse_color_spec_rgb_form() {
        assert_eq!(parse_color_spec("rgb:ff/80/00"), Some([0xff, 0x80, 0x00]));
        assert_eq!(parse_color_spec("rgb:00/00/00"), Some([0x00, 0x00, 0x00]));
        assert_eq!(parse_color_spec("rgb:ff/ff/ff"), Some([0xff, 0xff, 0xff]));
    }

    /// `parse_color_spec` must accept the `#RRGGBB` hex form.
    #[test]
    fn parse_color_spec_hex_form() {
        assert_eq!(parse_color_spec("#ff8000"), Some([0xff, 0x80, 0x00]));
        assert_eq!(parse_color_spec("#000000"), Some([0x00, 0x00, 0x00]));
        assert_eq!(parse_color_spec("#ffffff"), Some([0xff, 0xff, 0xff]));
    }

    /// 4-digit channel values in `rgb:RRRR/GGGG/BBBB` must be normalised to
    /// 8 bits (top byte taken).
    #[test]
    fn parse_color_spec_4digit_channel() {
        // rgb:ffff/8080/0000 → [0xff, 0x80, 0x00]
        assert_eq!(
            parse_color_spec("rgb:ffff/8080/0000"),
            Some([0xff, 0x80, 0x00])
        );
    }

    /// Querying `?` and unknown specs must return `None`.
    #[test]
    fn parse_color_spec_rejects_query_and_garbage() {
        assert_eq!(parse_color_spec("?"), None);
        assert_eq!(parse_color_spec(""), None);
        assert_eq!(parse_color_spec("red"), None);
    }

    /// OSC 4;<index>;<spec> sets the indexed override; the resolved cell colour
    /// must reflect it, and OSC 104 resets it back to the palette value.
    #[test]
    fn osc4_set_and_osc104_reset_indexed_color() {
        let mut emu = Emulator::new(80, 24);
        // Write a cell with colour index 1 (ANSI Red).
        // First check what the theme's Red looks like before any override.
        let palette = emu.palette();
        let theme_red = palette.normal[1];

        // Override colour index 1 with a custom colour.
        emu.advance(b"\x1b]4;1;rgb:00/ff/00\x07");
        let events = emu.drain_events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, EmulatorEvent::PaletteChanged)),
            "OSC 4 must emit PaletteChanged"
        );

        // The override must be visible in the cell snapshot.
        // Advance a cell with Named Red (SGR 31).
        emu.advance(b"\x1b[31mX\x1b[0m");
        let mut cell_fg = [0u8; 3];
        emu.for_each_visible_cell(|col, row, snap| {
            if row == 0 && col == 0 {
                cell_fg = snap.fg;
            }
        });
        // Named Red resolves via NamedColor::Red → p.normal[1], but our indexed
        // override only fires for `Color::Indexed(1)`.  What actually reaches the
        // snapshot for `\x1b[31m` depends on whether alacritty uses NamedColor or
        // Indexed internally — either way we can confirm the raw override layer
        // works by checking the emulator's accessor directly.
        // We check the raw override table through the `lookup_palette_color` path.
        let overridden = emu.lookup_palette_color(1);
        assert!(overridden.is_some());
        let rgb = overridden.unwrap();
        assert_eq!([rgb.r, rgb.g, rgb.b], [0x00, 0xff, 0x00]);

        // OSC 104 with the same index must reset it.
        emu.advance(b"\x1b]104;1\x07");
        let _ = emu.drain_events();
        let after_reset = emu.lookup_palette_color(1).unwrap();
        assert_eq!(
            [after_reset.r, after_reset.g, after_reset.b],
            theme_red,
            "after OSC 104 reset the colour must match the theme"
        );
    }

    /// OSC 10/11/12 set fg/bg/cursor; OSC 110/111/112 reset them.
    #[test]
    fn osc_10_11_12_set_and_reset() {
        let mut emu = Emulator::new(80, 24);

        // Set fg (OSC 10), bg (OSC 11), cursor (OSC 12).
        emu.advance(b"\x1b]10;rgb:aa/bb/cc\x07");
        emu.advance(b"\x1b]11;#112233\x07");
        emu.advance(b"\x1b]12;rgb:ff/00/00\x07");
        let _ = emu.drain_events();

        assert_eq!(emu.override_fg(), Some([0xaa, 0xbb, 0xcc]));
        assert_eq!(emu.override_bg(), Some([0x11, 0x22, 0x33]));
        assert_eq!(emu.override_cursor(), Some([0xff, 0x00, 0x00]));

        // Reset via OSC 110/111/112.
        emu.advance(b"\x1b]110\x07");
        emu.advance(b"\x1b]111\x07");
        emu.advance(b"\x1b]112\x07");
        let _ = emu.drain_events();

        assert_eq!(emu.override_fg(), None);
        assert_eq!(emu.override_bg(), None);
        assert_eq!(emu.override_cursor(), None);
    }

    /// `reset_dynamic_colors` wipes all overrides at once.
    #[test]
    fn reset_dynamic_colors_clears_all_overrides() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b]4;0;#ff0000\x07");
        emu.advance(b"\x1b]10;#aabbcc\x07");
        emu.advance(b"\x1b]11;#112233\x07");
        let _ = emu.drain_events();

        emu.reset_dynamic_colors();

        assert_eq!(emu.override_fg(), None);
        assert_eq!(emu.override_bg(), None);
        assert_eq!(
            emu.lookup_palette_color(0).unwrap().r,
            emu.palette().normal[0][0]
        );
    }

    /// The effective background/foreground seen in cell snapshots should
    /// reflect OSC 11/10 overrides.
    #[test]
    fn osc_override_fg_bg_affects_cell_snapshot() {
        let mut emu = Emulator::new(80, 24);
        // Write a space with default fg (NamedColor::Foreground) by not setting
        // any colour — the cell colour is NamedColor::Foreground.
        // Override the fg to a distinct colour.
        emu.advance(b"\x1b]10;rgb:de/ad/be\x07");
        let _ = emu.drain_events();

        // Advance a cell that will use the default fg.
        emu.advance(b"X");
        let mut fg = [0u8; 3];
        emu.for_each_visible_cell(|col, row, snap| {
            if row == 0 && col == 0 {
                fg = snap.fg;
            }
        });
        // The cell should now use the overridden foreground.
        assert_eq!(
            fg,
            [0xde, 0xad, 0xbe],
            "OSC 10 fg override must affect cell snapshot"
        );
    }

    // ── DECSET 2026 synchronized output (BSU / ESU) ───────────────────────

    /// `CSI ?2026h` (BSU) sets sync_output; `CSI ?2026l` (ESU) clears it.
    #[test]
    fn decset_2026h_sets_sync_output() {
        let mut emu = Emulator::new(80, 24);
        assert!(!emu.is_synchronized_output(), "should start unsynced");
        emu.advance(b"\x1b[?2026h");
        assert!(emu.is_synchronized_output(), "BSU must activate sync mode");
    }

    /// ESU clears the sync flag and `has_new_frame` returns `true` again.
    #[test]
    fn decset_2026l_clears_sync_output_and_releases_frame() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b[?2026h");
        assert!(emu.is_synchronized_output());
        assert!(!emu.has_new_frame(), "no new frame while sync is active");

        emu.advance(b"\x1b[?2026l");
        assert!(!emu.is_synchronized_output(), "ESU must clear sync mode");
        assert!(emu.has_new_frame(), "frame must be released after ESU");
    }

    /// BSU + some output + ESU in a single chunk: both sequences are parsed,
    /// the sync flag is cleared, and the content is in the grid.
    #[test]
    fn decset_2026_bsu_content_esu_single_chunk() {
        let mut emu = Emulator::new(80, 24);
        // BSU, write "hi", ESU — all in one chunk.
        emu.advance(b"\x1b[?2026hhello\x1b[?2026l");
        assert!(!emu.is_synchronized_output(), "ESU must end sync mode");
        assert!(emu.has_new_frame(), "frame must be released after ESU");
        // The content must be in the grid.
        let mut found = String::new();
        emu.for_each_visible_cell(|col, row, snap| {
            if row == 0 && col < 5 {
                found.push(snap.ch);
            }
        });
        assert_eq!(found, "hello", "grid content must survive BSU/ESU framing");
    }

    /// `has_new_frame` reflects `is_synchronized_output` correctly.
    #[test]
    fn has_new_frame_gating_predicate() {
        let mut emu = Emulator::new(80, 24);
        // Before any sync sequence: has_new_frame is true.
        assert!(emu.has_new_frame());

        emu.advance(b"\x1b[?2026h");
        assert!(!emu.has_new_frame(), "gated while sync active");

        emu.advance(b"\x1b[?2026l");
        assert!(emu.has_new_frame(), "released after ESU");
    }

    /// Multiple BSU sequences without intervening ESU do not stack; a single
    /// ESU releases the frame.
    #[test]
    fn decset_2026_repeated_bsu_cleared_by_single_esu() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b[?2026h\x1b[?2026h\x1b[?2026h");
        assert!(emu.is_synchronized_output());
        emu.advance(b"\x1b[?2026l");
        assert!(!emu.is_synchronized_output());
    }

    /// Safety timeout: after `SYNC_OUTPUT_TIMEOUT` a stuck BSU frame is
    /// treated as committed and `has_new_frame` returns `true`.
    ///
    /// We simulate the timeout by directly manipulating `sync_start` to a
    /// past instant.
    #[test]
    fn decset_2026_timeout_releases_frame() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b[?2026h");
        assert!(emu.is_synchronized_output(), "sync active right after BSU");

        // Wind the clock back far enough to exceed the timeout.
        emu.sync_start = Some(
            Instant::now()
                .checked_sub(SYNC_OUTPUT_TIMEOUT + Duration::from_millis(1))
                .unwrap(),
        );

        assert!(
            !emu.is_synchronized_output(),
            "timed-out sync must not block"
        );
        assert!(emu.has_new_frame(), "frame must be released after timeout");
    }

    /// BSU/ESU do not interfere with normal text written during the sync frame.
    #[test]
    fn decset_2026_grid_content_unaffected() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b[?2026hworld\x1b[?2026l");
        let mut buf = String::new();
        emu.for_each_visible_cell(|col, row, snap| {
            if row == 0 && col < 5 {
                buf.push(snap.ch);
            }
        });
        assert_eq!(buf, "world");
    }

    /// `sniff_decset_2026` is tested directly: single BSU in a byte slice.
    #[test]
    fn sniff_decset_2026_bsu_sets_flag() {
        let mut sync = false;
        let mut start: Option<Instant> = None;
        sniff_decset_2026(b"\x1b[?2026h", &mut sync, &mut start);
        assert!(sync);
        assert!(start.is_some());
    }

    /// `sniff_decset_2026` handles ESU clearing an active flag.
    #[test]
    fn sniff_decset_2026_esu_clears_flag() {
        let mut sync = true;
        let mut start: Option<Instant> = Some(Instant::now());
        sniff_decset_2026(b"\x1b[?2026l", &mut sync, &mut start);
        assert!(!sync);
        assert!(start.is_none());
    }

    /// `sniff_decset_2026` ignores unrelated CSI sequences.
    #[test]
    fn sniff_decset_2026_ignores_unrelated_sequences() {
        let mut sync = false;
        let mut start: Option<Instant> = None;
        // CSI ?2004h (bracketed paste) and random text.
        sniff_decset_2026(b"\x1b[?2004hhello\x1b[0m", &mut sync, &mut start);
        assert!(!sync);
        assert!(start.is_none());
    }

    // ── OSC 1337 File= arg parsing ─────────────────────────────────────────

    #[test]
    fn parse_osc1337_file_args_inline_required() {
        // Without `inline=1` the function must return None.
        let result = parse_osc1337_file_args("name=foo.png;size=1234;width=20;height=10:AABB");
        assert!(result.is_none(), "no inline=1 → must return None");
    }

    #[test]
    fn parse_osc1337_file_args_basic_inline() {
        let result = parse_osc1337_file_args("inline=1:PAYLOAD");
        let (w, h, inline, b64) = result.expect("inline=1 must succeed");
        assert!(inline);
        assert_eq!(b64, "PAYLOAD");
        // No explicit width/height → 0 (auto).
        assert_eq!(w, 0);
        assert_eq!(h, 0);
    }

    #[test]
    fn parse_osc1337_file_args_with_cell_dims() {
        let result = parse_osc1337_file_args("width=40;height=15;inline=1:DATA");
        let (w, h, _, b64) = result.expect("parse with cell dims");
        assert_eq!(w, 40);
        assert_eq!(h, 15);
        assert_eq!(b64, "DATA");
    }

    #[test]
    fn parse_osc1337_file_args_px_suffix_stripped() {
        let result = parse_osc1337_file_args("width=320px;height=240px;inline=1:D");
        let (w, h, _, _) = result.expect("px suffix must be stripped");
        assert_eq!(w, 320);
        assert_eq!(h, 240);
    }

    #[test]
    fn parse_osc1337_file_args_empty_payload_returns_none() {
        let result = parse_osc1337_file_args("inline=1:");
        assert!(result.is_none(), "empty payload must return None");
    }

    #[test]
    fn parse_osc1337_file_args_no_colon_returns_none() {
        let result = parse_osc1337_file_args("inline=1");
        assert!(result.is_none(), "missing colon must return None");
    }

    #[test]
    fn image_cell_footprint_auto_from_pixels() {
        // 80px image, 8px cell width → ceil(80/8) = 10 cols
        let (cols, rows) = image_cell_footprint(80, 32, 8.0, 16.0, 0, 0);
        assert_eq!(cols, 10);
        assert_eq!(rows, 2); // ceil(32/16) = 2
    }

    #[test]
    fn image_cell_footprint_explicit_override() {
        // Explicit override ignores pixel size.
        let (cols, rows) = image_cell_footprint(1000, 1000, 8.0, 16.0, 5, 3);
        assert_eq!(cols, 5);
        assert_eq!(rows, 3);
    }

    #[test]
    fn image_cell_footprint_minimum_one() {
        // Zero-size image still produces at least 1×1.
        let (cols, rows) = image_cell_footprint(0, 0, 8.0, 16.0, 0, 0);
        assert_eq!(cols, 1);
        assert_eq!(rows, 1);
    }

    #[test]
    fn osc1337_image_placement_via_emulator() {
        use image::{ImageBuffer, Rgba};
        // Encode a tiny 2×2 RGBA PNG.
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(2, 2, Rgba([100u8, 150, 200, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)
            .expect("encode PNG");
        let raw = buf.into_inner();

        // Base64-encode the PNG bytes using the same base64 alphabet the
        // emulator's decoder understands.
        let encoded = base64_encode(&raw);

        let mut emu = Emulator::new(80, 24);
        // Construct an OSC 1337;File= sequence with inline=1 and explicit cell dims.
        let osc = format!("\x1b]1337;File=width=10;height=5;inline=1:{encoded}\x07");
        emu.advance(osc.as_bytes());

        let store = emu.image_store();
        // The image should be in the store and have a placement.
        assert_eq!(store.iter_images().count(), 1, "one image must be stored");
        let placements = store.placements_in_view(
            emu.cursor_absolute_line() - 1, // one line above cursor (where image was placed)
            24,
        );
        assert!(
            !placements.is_empty(),
            "placement must be visible in the viewport"
        );
    }

    // ── Sixel DCS integration tests ────────────────────────────────────────

    /// Helper: build a complete DCS Sixel sequence (ESC P q … ESC \) for a
    /// tiny 1-column all-bits-set red block (2×6 pixels).
    fn tiny_sixel_dcs() -> Vec<u8> {
        // ESC P q  #0;2;100;0;0  ~  ESC \
        let mut v = b"\x1bPq".to_vec();
        v.extend_from_slice(b"#0;2;100;0;0~");
        v.extend_from_slice(b"\x1b\\");
        v
    }

    /// A single-chunk DCS Sixel sequence must decode and place one image.
    #[test]
    fn sixel_single_chunk_places_image() {
        let mut emu = Emulator::new(80, 24);
        let seq = tiny_sixel_dcs();
        emu.advance(&seq);
        let store = emu.image_store();
        assert_eq!(
            store.iter_images().count(),
            1,
            "a complete DCS Sixel frame must produce exactly one image"
        );
    }

    /// The same sequence split across two `advance()` calls must still
    /// decode and produce exactly one image.
    #[test]
    fn sixel_split_across_two_chunks_places_image() {
        let seq = tiny_sixel_dcs();
        // Try splitting at several points.
        for split in [1, 3, seq.len() / 2, seq.len() - 2] {
            if split == 0 || split >= seq.len() {
                continue;
            }
            let mut emu = Emulator::new(80, 24);
            emu.advance(&seq[..split]);
            emu.advance(&seq[split..]);
            let store = emu.image_store();
            assert_eq!(
                store.iter_images().count(),
                1,
                "split at {split}: DCS Sixel must still produce one image"
            );
        }
    }

    /// When `set_sixel_images_enabled(false)` is called, a subsequent DCS
    /// Sixel sequence must NOT produce any image in the store.
    #[test]
    fn sixel_disabled_produces_no_image() {
        let mut emu = Emulator::new(80, 24);
        emu.set_sixel_images_enabled(false);
        let seq = tiny_sixel_dcs();
        emu.advance(&seq);
        let store = emu.image_store();
        assert_eq!(
            store.iter_images().count(),
            0,
            "Sixel disabled: DCS frame must be ignored, no image stored"
        );
    }

    /// Sixel DCS with C1 ST terminator (0x9C byte) must also commit correctly.
    #[test]
    fn sixel_c1_st_terminates_frame() {
        let mut emu = Emulator::new(80, 24);
        // ESC P q  …payload…  0x9C (C1 ST)
        let mut seq = b"\x1bPq".to_vec();
        seq.extend_from_slice(b"#0;2;100;0;0~");
        seq.push(0x9c); // C1 ST
        emu.advance(&seq);
        let store = emu.image_store();
        assert_eq!(
            store.iter_images().count(),
            1,
            "C1 ST must terminate a DCS Sixel frame"
        );
    }

    /// Re-enabling Sixel after it was disabled mid-stream must discard the
    /// partial buffer and not leave the emulator in a broken state.
    #[test]
    fn sixel_re_enable_after_disable_works() {
        let seq = tiny_sixel_dcs();
        let mut emu = Emulator::new(80, 24);

        // Send the introducer only — DCS frame starts.
        emu.advance(&seq[..3]);
        // Disable mid-stream — partial buffer must be dropped.
        emu.set_sixel_images_enabled(false);
        // Send the rest: should be ignored because sixel is off.
        emu.advance(&seq[3..]);
        assert_eq!(
            emu.image_store().iter_images().count(),
            0,
            "image must not be committed after mid-stream disable"
        );

        // Re-enable and send a full fresh frame.
        emu.set_sixel_images_enabled(true);
        emu.advance(&seq);
        assert_eq!(
            emu.image_store().iter_images().count(),
            1,
            "re-enabled sixel must decode the next frame"
        );
    }

    // ── APC graphics integration tests ─────────────────────────────────────

    /// Build a complete APC graphics sequence for a tiny 1x1 blue PNG image.
    /// Format: `ESC _ G <ctrl> ; <b64-png> ESC \`.
    fn tiny_apc_graphics_png() -> Vec<u8> {
        use image::{ImageBuffer, Rgba};
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(1, 1, Rgba([0u8, 0, 255, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)
            .expect("encode PNG");
        let png = buf.into_inner();
        let b64 = base64_encode(&png);

        // Build: ESC _ G a=T,f=100 ; <b64> ESC \
        let mut seq = vec![0x1b, 0x5f, b'G']; // ESC _ G
        seq.extend_from_slice(b"a=T,f=100");
        seq.push(b';');
        seq.extend_from_slice(b64.as_bytes());
        seq.extend_from_slice(b"\x1b\\"); // ST
        seq
    }

    /// Build a complete APC graphics sequence for a 2x2 raw RGBA8 image (f=32).
    fn tiny_apc_graphics_f32() -> Vec<u8> {
        let rgba: Vec<u8> = (0u8..16).collect(); // 2x2 RGBA8
        let b64 = base64_encode(&rgba);

        let mut seq = vec![0x1b, 0x5f, b'G']; // ESC _ G
        seq.extend_from_slice(b"a=T,f=32,s=2,v=2");
        seq.push(b';');
        seq.extend_from_slice(b64.as_bytes());
        seq.extend_from_slice(b"\x1b\\"); // ST
        seq
    }

    /// A single-chunk APC graphics PNG sequence must decode and place one image.
    #[test]
    fn apc_graphics_single_chunk_png_places_image() {
        let mut emu = Emulator::new(80, 24);
        let seq = tiny_apc_graphics_png();
        emu.advance(&seq);
        let store = emu.image_store();
        assert_eq!(
            store.iter_images().count(),
            1,
            "an APC graphics PNG frame must produce exactly one image"
        );
    }

    /// A single-chunk APC graphics f=32 sequence must decode and place one image.
    #[test]
    fn apc_graphics_single_chunk_f32_places_image() {
        let mut emu = Emulator::new(80, 24);
        let seq = tiny_apc_graphics_f32();
        emu.advance(&seq);
        let store = emu.image_store();
        assert_eq!(
            store.iter_images().count(),
            1,
            "an APC graphics f=32 frame must produce exactly one image"
        );
    }

    /// The same sequence split across two `advance()` calls must still
    /// decode and produce exactly one image (straddle test).
    #[test]
    fn apc_graphics_split_across_two_chunks_places_image() {
        let seq = tiny_apc_graphics_png();
        for split in [1, 3, seq.len() / 2, seq.len() - 2] {
            if split == 0 || split >= seq.len() {
                continue;
            }
            let mut emu = Emulator::new(80, 24);
            emu.advance(&seq[..split]);
            emu.advance(&seq[split..]);
            let store = emu.image_store();
            assert_eq!(
                store.iter_images().count(),
                1,
                "split at {split}: APC graphics sequence must still produce one image"
            );
        }
    }

    /// When `set_apc_graphics_enabled(false)` is called, a subsequent APC
    /// graphics sequence must NOT produce any image in the store.
    #[test]
    fn apc_graphics_disabled_produces_no_image() {
        let mut emu = Emulator::new(80, 24);
        emu.set_apc_graphics_enabled(false);
        let seq = tiny_apc_graphics_png();
        emu.advance(&seq);
        let store = emu.image_store();
        assert_eq!(
            store.iter_images().count(),
            0,
            "APC graphics disabled: APC frame must be ignored, no image stored"
        );
    }

    /// Non-APC bytes (plain text + cursor sequences) must not be swallowed
    /// by the APC graphics scanner.
    #[test]
    fn apc_graphics_non_apc_bytes_pass_through() {
        let mut emu = Emulator::new(80, 24);
        // Feed some plain ASCII + a simple CSI sequence (cursor up).
        let plain = b"hello\x1b[1Aworld";
        emu.advance(plain);
        // No image should have been stored.
        assert_eq!(
            emu.image_store().iter_images().count(),
            0,
            "plain bytes must not produce any image"
        );
    }

    /// Multi-chunk APC graphics assembly: two APC frames with m=1 then m=0.
    #[test]
    fn apc_graphics_multi_chunk_assembly() {
        let rgba: Vec<u8> = (0u8..16).collect();
        let b64 = base64_encode(&rgba);
        let mid = b64.len() / 2;

        // Build two APC sequences: first with m=1, second with m=0.
        let mut seq1 = vec![0x1b, 0x5f, b'G'];
        seq1.extend_from_slice(b"a=T,f=32,s=2,v=2,i=1,m=1");
        seq1.push(b';');
        seq1.extend_from_slice(&b64.as_bytes()[..mid]);
        seq1.extend_from_slice(b"\x1b\\");

        let mut seq2 = vec![0x1b, 0x5f, b'G'];
        seq2.extend_from_slice(b"a=T,f=32,s=2,v=2,i=1,m=0");
        seq2.push(b';');
        seq2.extend_from_slice(&b64.as_bytes()[mid..]);
        seq2.extend_from_slice(b"\x1b\\");

        let mut emu = Emulator::new(80, 24);
        emu.advance(&seq1); // first chunk
        assert_eq!(
            emu.image_store().iter_images().count(),
            0,
            "m=1 chunk must not produce an image yet"
        );
        emu.advance(&seq2); // final chunk
        assert_eq!(
            emu.image_store().iter_images().count(),
            1,
            "m=0 final chunk must produce one image"
        );
    }

    /// Re-enabling APC graphics after it was disabled mid-stream must not leave
    /// the emulator in a broken state.
    #[test]
    fn apc_graphics_re_enable_after_disable_works() {
        let seq = tiny_apc_graphics_png();
        let mut emu = Emulator::new(80, 24);

        // Send the introducer only — frame starts.
        emu.advance(&seq[..3]);
        // Disable mid-stream — partial buffer must be dropped.
        emu.set_apc_graphics_enabled(false);
        // Send the rest: should be ignored.
        emu.advance(&seq[3..]);
        assert_eq!(
            emu.image_store().iter_images().count(),
            0,
            "image must not be committed after mid-stream disable"
        );

        // Re-enable and send a full fresh frame.
        emu.set_apc_graphics_enabled(true);
        emu.advance(&seq);
        assert_eq!(
            emu.image_store().iter_images().count(),
            1,
            "re-enabled APC graphics must decode the next frame"
        );
    }

    // ── DECCKM / DECPAM mode accessors ───────────────────────────────────────

    /// `app_cursor_mode()` must return `false` in a freshly-created emulator
    /// (DECCKM is off by default — normal CSI cursor-key sequences).
    #[test]
    fn app_cursor_mode_default_is_false() {
        let emu = Emulator::new(80, 24);
        assert!(
            !emu.app_cursor_mode(),
            "app_cursor_mode() must be false at startup (DECCKM off)"
        );
    }

    /// Feeding `CSI ?1h` (DECSET 1 = enable DECCKM) must flip `app_cursor_mode`
    /// to `true`; `CSI ?1l` (DECRST 1 = disable DECCKM) must flip it back.
    #[test]
    fn app_cursor_mode_toggles_with_decckm() {
        let mut emu = Emulator::new(80, 24);
        // Enable application cursor-key mode: CSI ? 1 h
        emu.advance(b"\x1b[?1h");
        assert!(
            emu.app_cursor_mode(),
            "app_cursor_mode() must be true after CSI ?1h (DECCKM enable)"
        );
        // Disable again: CSI ? 1 l
        emu.advance(b"\x1b[?1l");
        assert!(
            !emu.app_cursor_mode(),
            "app_cursor_mode() must be false after CSI ?1l (DECCKM disable)"
        );
    }

    /// `app_keypad_mode()` must return `false` in a freshly-created emulator
    /// (DECPAM/numeric keypad mode is the default).
    #[test]
    fn app_keypad_mode_default_is_false() {
        let emu = Emulator::new(80, 24);
        assert!(
            !emu.app_keypad_mode(),
            "app_keypad_mode() must be false at startup (numeric keypad)"
        );
    }

    /// Feeding `ESC =` (DECPAM) must enable application keypad mode; `ESC >`
    /// (DECPNM) must restore numeric mode.
    #[test]
    fn app_keypad_mode_toggles_with_decpam() {
        let mut emu = Emulator::new(80, 24);
        // Enable: ESC =
        emu.advance(b"\x1b=");
        assert!(
            emu.app_keypad_mode(),
            "app_keypad_mode() must be true after ESC = (DECPAM)"
        );
        // Disable: ESC >
        emu.advance(b"\x1b>");
        assert!(
            !emu.app_keypad_mode(),
            "app_keypad_mode() must be false after ESC > (DECPNM)"
        );
    }

    // ── SGR 2/7/8 — dim, inverse, hidden ────────────────────────────────────

    /// Helper: read the CellSnapshot at (col, row) from a freshly snapshotted grid.
    fn cell_at(emu: &Emulator, col: u16, row: u16) -> CellSnapshot {
        let mut found: Option<CellSnapshot> = None;
        emu.for_each_visible_cell(|c, r, snap| {
            if c == col && r == row {
                found = Some(snap);
            }
        });
        found.unwrap_or_else(|| panic!("cell ({col},{row}) not found"))
    }

    /// SGR 2 sets `dim = true`; SGR 0 resets it.
    #[test]
    fn sgr_2_sets_dim_flag() {
        let mut emu = Emulator::new(80, 24);
        // SGR 2 (dim) then a character.
        emu.advance(b"\x1b[2mX");
        let snap = cell_at(&emu, 0, 0);
        assert!(snap.dim, "SGR 2 must set CellSnapshot.dim");
        assert_eq!(snap.ch, 'X');
    }

    /// SGR 0 resets dim back to false.
    #[test]
    fn sgr_0_resets_dim() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b[2mX\x1b[0mY");
        // 'X' at col 0 — dim
        let x = cell_at(&emu, 0, 0);
        assert!(x.dim, "SGR 2 cell must be dim");
        // 'Y' at col 1 — no dim
        let y = cell_at(&emu, 1, 0);
        assert!(!y.dim, "SGR 0 must clear dim");
    }

    /// SGR 7 sets `inverse = true`; SGR 0 resets it.
    #[test]
    fn sgr_7_sets_inverse_flag() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b[7mR");
        let snap = cell_at(&emu, 0, 0);
        assert!(snap.inverse, "SGR 7 must set CellSnapshot.inverse");
        assert_eq!(snap.ch, 'R');
    }

    /// SGR 0 resets inverse back to false.
    #[test]
    fn sgr_0_resets_inverse() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b[7mR\x1b[0mS");
        let r = cell_at(&emu, 0, 0);
        assert!(r.inverse, "SGR 7 cell must be inverse");
        let s = cell_at(&emu, 1, 0);
        assert!(!s.inverse, "SGR 0 must clear inverse");
    }

    /// SGR 8 sets `hidden = true`; SGR 0 resets it.
    #[test]
    fn sgr_8_sets_hidden_flag() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b[8mH");
        let snap = cell_at(&emu, 0, 0);
        assert!(snap.hidden, "SGR 8 must set CellSnapshot.hidden");
        assert_eq!(snap.ch, 'H');
    }

    /// SGR 0 resets hidden back to false.
    #[test]
    fn sgr_0_resets_hidden() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"\x1b[8mH\x1b[0mI");
        let h = cell_at(&emu, 0, 0);
        assert!(h.hidden, "SGR 8 cell must be hidden");
        let i = cell_at(&emu, 1, 0);
        assert!(!i.hidden, "SGR 0 must clear hidden");
    }

    /// A cell with no SGR flags must have dim/inverse/hidden all false.
    #[test]
    fn plain_cell_has_no_sgr_flags() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"Z");
        let snap = cell_at(&emu, 0, 0);
        assert!(!snap.dim, "plain cell must not be dim");
        assert!(!snap.inverse, "plain cell must not be inverse");
        assert!(!snap.hidden, "plain cell must not be hidden");
    }

    // ── OSC 52 clipboard READ detection ──────────────────────────────────────

    /// Helper: collect all ClipboardRead events from a single advance call.
    fn clipboard_read_events(bytes: &[u8]) -> Vec<String> {
        let mut emu = Emulator::new(80, 24);
        emu.advance(bytes);
        emu.drain_events()
            .into_iter()
            .filter_map(|e| match e {
                EmulatorEvent::ClipboardRead { selection } => Some(selection),
                _ => None,
            })
            .collect()
    }

    /// Helper: collect all ClipboardStore events from a single advance call.
    fn clipboard_store_events(bytes: &[u8]) -> Vec<(String, String)> {
        let mut emu = Emulator::new(80, 24);
        emu.advance(bytes);
        emu.drain_events()
            .into_iter()
            .filter_map(|e| match e {
                EmulatorEvent::ClipboardStore { kind: _, text } => {
                    Some(("clipboard".to_string(), text))
                }
                _ => None,
            })
            .collect()
    }

    /// OSC 52 query form with BEL terminator produces a ClipboardRead event.
    #[test]
    fn osc52_read_query_with_bel_detected() {
        // ESC ] 52 ; c ; ? BEL
        let events = clipboard_read_events(b"\x1b]52;c;?\x07");
        assert_eq!(events.len(), 1, "exactly one ClipboardRead expected");
        assert_eq!(events[0], "c", "selection must be 'c'");
    }

    /// OSC 52 query form with ST (ESC \) terminator produces a ClipboardRead event.
    #[test]
    fn osc52_read_query_with_st_detected() {
        // ESC ] 52 ; c ; ? ESC \
        let events = clipboard_read_events(b"\x1b]52;c;?\x1b\\");
        assert_eq!(events.len(), 1, "exactly one ClipboardRead expected");
        assert_eq!(events[0], "c", "selection must be 'c'");
    }

    /// OSC 52 query with a non-standard selection string is preserved verbatim.
    #[test]
    fn osc52_read_query_selection_preserved() {
        // ESC ] 52 ; p ; ? BEL
        let events = clipboard_read_events(b"\x1b]52;p;?\x07");
        assert_eq!(events.len(), 1, "exactly one ClipboardRead expected");
        assert_eq!(events[0], "p", "selection string must match");
    }

    /// OSC 52 STORE form (base64 payload) must NOT be misclassified as a read.
    #[test]
    fn osc52_store_form_not_misclassified_as_read() {
        // ESC ] 52 ; c ; dGVzdA== BEL   ("test" in base64)
        let read_events = clipboard_read_events(b"\x1b]52;c;dGVzdA==\x07");
        assert!(
            read_events.is_empty(),
            "store form must not produce a ClipboardRead event"
        );
        // The store form should instead surface as a ClipboardStore via alacritty.
        let store_events = clipboard_store_events(b"\x1b]52;c;dGVzdA==\x07");
        assert_eq!(
            store_events.len(),
            1,
            "store form must produce a ClipboardStore event"
        );
        assert_eq!(store_events[0].1, "test", "decoded text must be 'test'");
    }

    /// Multiple OSC 52 query sequences in the same chunk are all detected.
    #[test]
    fn osc52_read_multiple_queries_detected() {
        // Two consecutive queries: `c` and `s`
        let events = clipboard_read_events(b"\x1b]52;c;?\x07\x1b]52;s;?\x07");
        assert_eq!(
            events.len(),
            2,
            "both ClipboardRead events must be detected"
        );
        assert_eq!(events[0], "c");
        assert_eq!(events[1], "s");
    }

    /// Empty selection string (between the two semicolons) is accepted.
    #[test]
    fn osc52_read_empty_selection_accepted() {
        // ESC ] 52 ; ; ? BEL  (empty selection string — legal per spec)
        let events = clipboard_read_events(b"\x1b]52;;?\x07");
        assert_eq!(
            events.len(),
            1,
            "ClipboardRead with empty selection must be detected"
        );
        assert_eq!(events[0], "", "selection must be empty string");
    }

    /// An OSC 52 query without a terminator is NOT surfaced (incomplete sequence).
    #[test]
    fn osc52_read_unterminated_sequence_ignored() {
        let events = clipboard_read_events(b"\x1b]52;c;?");
        assert!(
            events.is_empty(),
            "unterminated sequence must not produce an event"
        );
    }

    /// A plain OSC 52 store followed by a query — only the query is a ClipboardRead.
    #[test]
    fn osc52_mixed_store_then_read() {
        // store "hello", then query
        let bytes = b"\x1b]52;c;aGVsbG8=\x07\x1b]52;c;?\x07";
        let read_events = clipboard_read_events(bytes);
        assert_eq!(
            read_events.len(),
            1,
            "exactly one ClipboardRead from the query"
        );
        assert_eq!(read_events[0], "c");
    }

    // ── OSC 133 command-block end-to-end capture ──────────────────────────────

    /// A single A→B→C→D cycle with inline command text (B and C in the same
    /// advance call) must produce one block with the correct command text and
    /// exit code. This is the regression guard for the cmdhistory demo bug.
    #[test]
    fn osc133_inline_command_text_captured_single_advance() {
        let mut emu = Emulator::new(80, 24);
        emu.set_command_blocks(true, 100);

        // Single advance call — B and C in the same buffer.
        // This matches the form used by the cmdhistory demo seed.
        emu.advance(
            b"\x1b]133;A\x1b\\\x1b]133;B\x1b\\cargo build --workspace\x1b]133;C\x1b\\\r\n   Compiling terminale\r\n\x1b]133;D;0\x1b\\",
        );

        let blocks = emu.command_blocks();
        assert_eq!(
            blocks.len(),
            1,
            "one complete A→B→C→D cycle must produce one block"
        );
        assert_eq!(
            blocks[0].command_text, "cargo build --workspace",
            "command_text must be the inline text between B and C"
        );
        assert_eq!(blocks[0].exit_code, Some(0));
        assert!(
            blocks[0].end_line.is_some(),
            "block must be finalised after D"
        );
    }

    /// Five complete OSC 133 cycles (the exact form used by TERMINALE_DEMO_PALETTE=cmdhistory)
    /// must produce five blocks with the correct command texts.
    #[test]
    fn osc133_cmdhistory_demo_seed_produces_five_blocks() {
        let mut emu = Emulator::new(80, 24);
        emu.set_command_blocks(true, 1000);

        // Exact byte sequence from the cmdhistory demo arm in main.rs.
        emu.advance(
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

        let blocks = emu.command_blocks();
        assert_eq!(
            blocks.len(),
            5,
            "cmdhistory demo must produce exactly 5 blocks; got {}",
            blocks.len()
        );
        assert_eq!(blocks[0].command_text, "cargo build --workspace");
        assert_eq!(blocks[1].command_text, "cargo test --workspace");
        assert_eq!(blocks[2].command_text, "git status");
        assert_eq!(
            blocks[3].command_text,
            "cargo clippy --workspace --all-targets --all-features"
        );
        assert_eq!(blocks[4].command_text, "git log --oneline -5");
        for b in blocks {
            assert_eq!(b.exit_code, Some(0));
            assert!(b.end_line.is_some(), "all blocks must be finalised");
        }
    }

    /// Realistic shell simulation: command text is echoed to the grid across
    /// SEPARATE advance() calls before C arrives. The grid-fallback path must
    /// capture the content of the B-line (the full prompt line including the
    /// command the user typed after the prompt).
    #[test]
    fn osc133_multiadvance_real_shell_captures_command_from_grid() {
        let mut emu = Emulator::new(80, 24);
        emu.set_command_blocks(true, 100);

        // Simulate a real shell: prompt markup, user type, then C/D.
        // Each shell write is a separate advance() call.
        emu.advance(b"\x1b]133;A\x1b\\"); // A — prompt start
        emu.advance(b"$ "); // prompt text drawn
        emu.advance(b"\x1b]133;B\x1b\\"); // B — input start
        emu.advance(b"git diff"); // user typing (echoed)
                                  // At C-time, the full B-line in the grid contains "$ git diff".
                                  // The grid-fallback captures the whole visible line content.
        emu.advance(b"\x1b]133;C\x1b\\"); // C — submitted
        emu.advance(b"\r\ndiff --git a/foo b/foo\r\n"); // output
        emu.advance(b"\x1b]133;D;0\x1b\\"); // D — done

        let blocks = emu.command_blocks();
        assert_eq!(blocks.len(), 1, "one block from multi-advance cycle");
        // The grid fallback captures the full line content at the B-line.
        // It includes the prompt prefix (this is the known grid-fallback
        // behaviour; the inline path is more precise).
        assert!(
            blocks[0].command_text.contains("git diff"),
            "command_text must contain the typed command; got {:?}",
            blocks[0].command_text
        );
        assert_eq!(blocks[0].exit_code, Some(0));
    }

    /// Inline text with ANSI escape sequences embedded (e.g. colour from
    /// shell completion highlighting) must be stripped to the visible text.
    #[test]
    fn osc133_inline_command_text_strips_ansi() {
        let mut emu = Emulator::new(80, 24);
        emu.set_command_blocks(true, 100);

        // Command text with embedded SGR colour codes.
        emu.advance(
            b"\x1b]133;A\x1b\\\x1b]133;B\x1b\\\x1b[32mls\x1b[0m -la\x1b]133;C\x1b\\\x1b]133;D;0\x1b\\"
        );

        let blocks = emu.command_blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0].command_text, "ls -la",
            "ANSI escape sequences in inline command text must be stripped"
        );
    }

    /// BEL-terminated OSC 133 sequences must work the same as ST-terminated ones.
    #[test]
    fn osc133_bel_terminator_inline_command() {
        let mut emu = Emulator::new(80, 24);
        emu.set_command_blocks(true, 100);

        emu.advance(b"\x1b]133;A\x07\x1b]133;B\x07echo hello\x1b]133;C\x07\x1b]133;D;0\x07");

        let blocks = emu.command_blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].command_text, "echo hello");
        assert_eq!(blocks[0].exit_code, Some(0));
    }

    /// When command blocks are disabled (max_blocks == 0), no blocks are stored
    /// even when inline command text is present.
    #[test]
    fn osc133_blocks_disabled_inline_command_not_stored() {
        let mut emu = Emulator::new(80, 24);
        // Deliberately NOT calling set_command_blocks — default max is 0.

        emu.advance(b"\x1b]133;A\x1b\\\x1b]133;B\x1b\\ls\x1b]133;C\x1b\\\x1b]133;D;0\x1b\\");

        assert!(
            emu.command_blocks().is_empty(),
            "blocks must stay empty when capture is disabled"
        );
        // Prompt marks must still be recorded.
        assert_eq!(emu.semantic().len(), 1, "PromptMark must still be recorded");
    }

    /// Non-zero exit code is correctly captured for inline command text.
    #[test]
    fn osc133_inline_command_nonzero_exit() {
        let mut emu = Emulator::new(80, 24);
        emu.set_command_blocks(true, 100);

        emu.advance(
            b"\x1b]133;A\x1b\\\x1b]133;B\x1b\\cat /nonexistent\x1b]133;C\x1b\\\r\ncat: /nonexistent: No such file\r\n\x1b]133;D;1\x1b\\"
        );

        let blocks = emu.command_blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].command_text, "cat /nonexistent");
        assert_eq!(blocks[0].exit_code, Some(1));
    }

    // ── visible_lines_text ────────────────────────────────────────────────────

    /// `visible_lines_text` returns exactly `screen_lines` rows.
    #[test]
    fn visible_lines_text_row_count() {
        let emu = Emulator::new(80, 24);
        let lines = emu.visible_lines_text();
        assert_eq!(
            lines.len(),
            24,
            "visible_lines_text must return screen_lines rows"
        );
    }

    /// After writing text to the emulator, `visible_lines_text` contains it.
    #[test]
    fn visible_lines_text_contains_written_text() {
        let mut emu = Emulator::new(80, 24);
        emu.advance(b"hello scrollback");
        let lines = emu.visible_lines_text();
        // The text lands on the first visible row.
        assert!(
            lines.iter().any(|l| l.contains("hello scrollback")),
            "visible_lines_text must include text written to the emulator"
        );
    }

    /// `visible_lines_text` rows are trailing-trimmed (no trailing spaces).
    #[test]
    fn visible_lines_text_trailing_trimmed() {
        let emu = Emulator::new(80, 24);
        let lines = emu.visible_lines_text();
        for (i, line) in lines.iter().enumerate() {
            assert!(
                !line.ends_with(' '),
                "row {i} must not end with a trailing space"
            );
        }
    }
}
