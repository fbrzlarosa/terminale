//! GPU rendering for `terminale`.
//!
//! Each frame is composed in two passes inside one render pass:
//!   1. The background pipeline draws per-cell ANSI background quads,
//!      the cursor block, and selection / overlay rectangles.
//!   2. [`glyphon`] draws every visible row as its own pixel-positioned
//!      [`TextArea`] so the grid stays grid-aligned and never drifts.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod bg_fx;
mod bg_image;
mod bg_pipeline;
pub mod box_drawing;
pub mod bundled_fonts;
pub mod image_blit;
pub mod system_icons;

pub use bg_fx::{BgFxParams, BgFxPipeline, GpuEmitter, MAX_EMITTERS};
pub use bg_image::{apply_hsb_cpu, compute_uv_cpu, BgImageFit, BgImageParams, BgImagePipeline};
pub use bg_pipeline::{BgPipeline, Quad};
pub use bundled_fonts::{bundled_family_names, load_bundled_fonts};
pub use image_blit::ImageBlitPipeline;

use glyphon::{
    Attrs, Buffer, Cache as GlyphonCache, Color as GlyphonColor, ColorMode, Family, FontSystem,
    Metrics, Resolution, Shaping, SwashCache, SwashContent, TextArea, TextAtlas, TextBounds,
    TextRenderer, Viewport as GlyphonViewport, Wrap,
};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::sync::Arc;
use terminale_term::{AppCursorShape, CellSnapshot, Emulator, UnderlineStyle};
use thiserror::Error;
use wgpu::{
    Color, CompositeAlphaMode, Device, DeviceDescriptor, Instance, InstanceDescriptor, LoadOp,
    MultisampleState, Operations, PowerPreference, PresentMode, Queue, RenderPassColorAttachment,
    RenderPassDescriptor, RequestAdapterOptions, StoreOp, Surface, SurfaceConfiguration,
    TextureUsages, TextureViewDescriptor,
};

/// GPU adapter / instance selection knobs, resolved from the user's
/// `[gpu]` config and handed to [`Renderer::new`]. Keeps this crate
/// config-agnostic: the binary maps `terminale_config::GpuConfig` onto
/// these raw wgpu types.
#[derive(Debug, Clone, Copy)]
pub struct GpuOptions {
    /// Backend bitflags wgpu is allowed to use. Restrict to a single bit
    /// to force one API, or pass [`wgpu::Backends::all`] for "auto".
    pub backends: wgpu::Backends,
    /// Adapter power-preference hint.
    pub power_preference: PowerPreference,
    /// Request a CPU fallback adapter (software rendering / GPU disabled).
    pub force_fallback_adapter: bool,
}

impl Default for GpuOptions {
    fn default() -> Self {
        Self {
            backends: wgpu::Backends::PRIMARY,
            power_preference: PowerPreference::HighPerformance,
            force_fallback_adapter: false,
        }
    }
}

/// Background colour used to clear the screen each frame.
pub const BACKGROUND_RGB: [u8; 3] = [0x0d, 0x10, 0x17];

/// Default font size in points.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

/// Default line-height multiplier.
pub const DEFAULT_LINE_HEIGHT: f32 = 1.25;

/// Compile-time default logical padding (px) used only as the initial seed for
/// `Renderer::padding_px` and in tests.  At runtime the live knob is
/// `window.padding` (applied via [`Renderer::set_padding`]); this constant is
/// never added on top of `padding_px`.
pub const PADDING_PX: f32 = 12.0;

/// Fallback RGB accent used for the focus-border stroke when no explicit
/// override is set in `appearance.focus_border_color`.
const ACCENT_FOCUS_BORDER: [u8; 3] = [0x7d, 0xa6, 0xff];

/// How long the global Aurora/Starfield/PixelCRT keystroke "pulse" energy
/// takes to decay back to zero (seconds).
const BG_FX_PULSE_SECS: f32 = 0.8;

/// CPU-side representation of a single per-keystroke emitter band.
///
/// Each call to [`Renderer::spawn_bg_fx_emitter`] pushes one of these; the
/// list is pruned every frame once emitters exceed their configured lifetime.
#[derive(Debug, Clone, Copy)]
struct CpuEmitter {
    /// Scaled time at birth (same timescale as the GPU `time` uniform).
    birth: f32,
    /// Normalised horizontal position `0..=1`.
    col: f32,
    /// Per-band pseudo-random seed `0..=1`.
    seed: f32,
    /// Mode at spawn time (mirrors `BgFxParams::mode`).
    kind: f32,
}

/// A rasterized katakana glyph atlas for the Matrix background effect.
struct MatrixAtlas {
    /// R8 coverage, `width * height` bytes.
    data: Vec<u8>,
    width: u32,
    height: u32,
    cols: u32,
    rows: u32,
    count: u32,
}

/// Rasterize a set of half-width katakana + digits into a single-channel atlas
/// (one glyph per grid cell), for the Matrix "digital rain" background. Uses
/// the font system's own fallback so the glyphs are real characters.
fn build_matrix_atlas(font_system: &mut FontSystem, swash_cache: &mut SwashCache) -> MatrixAtlas {
    let chars: Vec<char> = "ｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾖﾗﾘﾙﾚﾛﾜ0123456789"
        .chars()
        .collect();
    let count = chars.len();
    let cell: usize = 32; // atlas cell size in px
    let glyph_px: f32 = 26.0;
    let cols: usize = 8;
    let rows = count.div_ceil(cols);
    let width = cols * cell;
    let height = rows * cell;
    let mut data = vec![0u8; width * height];
    let metrics = Metrics::new(glyph_px, glyph_px * 1.2);

    for (i, ch) in chars.iter().enumerate() {
        let mut buf = Buffer::new(font_system, metrics);
        buf.set_size(font_system, Some(cell as f32), Some(cell as f32));
        buf.set_text(
            font_system,
            ch.encode_utf8(&mut [0u8; 4]),
            Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
        );
        buf.shape_until_scroll(font_system, false);
        let cx = (i % cols) * cell;
        let cy = (i / cols) * cell;

        let mut done = false;
        for run in buf.layout_runs() {
            for glyph in run.glyphs {
                let pg = glyph.physical((0.0, 0.0), 1.0);
                if let Some(img) = swash_cache.get_image_uncached(font_system, pg.cache_key) {
                    let gw = img.placement.width as usize;
                    let gh = img.placement.height as usize;
                    if gw > 0 && gh > 0 {
                        let stride = if img.content == SwashContent::Color {
                            4
                        } else {
                            1
                        };
                        let ox = cx + cell.saturating_sub(gw) / 2;
                        let oy = cy + cell.saturating_sub(gh) / 2;
                        for yy in 0..gh {
                            for xx in 0..gw {
                                let src = (yy * gw + xx) * stride + (stride - 1);
                                let cov = img.data.get(src).copied().unwrap_or(0);
                                let px = ox + xx;
                                let py = oy + yy;
                                if px < width && py < height {
                                    data[py * width + px] = cov;
                                }
                            }
                        }
                    }
                    done = true;
                    break;
                }
            }
            if done {
                break;
            }
        }
    }

    MatrixAtlas {
        data,
        width: width as u32,
        height: height as u32,
        cols: cols as u32,
        rows: rows as u32,
        count: count as u32,
    }
}

/// Renderer error type.
#[derive(Debug, Error)]
pub enum RenderError {
    /// wgpu reported a surface error (lost, outdated, etc.).
    #[error("surface error: {0}")]
    Surface(#[from] wgpu::SurfaceError),
    /// glyphon could not lay out the frame.
    #[error("glyphon prepare error: {0}")]
    Prepare(#[from] glyphon::PrepareError),
    /// glyphon could not render the frame.
    #[error("glyphon render error: {0}")]
    Render(#[from] glyphon::RenderError),
    /// No suitable GPU adapter could be acquired.
    #[error("no GPU adapter matched the requested options")]
    NoAdapter,
    /// Failed to construct a wgpu device.
    #[error("device init error: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
    /// Failed to create the surface.
    #[error("surface creation error: {0}")]
    SurfaceCreate(#[from] wgpu::CreateSurfaceError),
    /// The adapter reported no usable surface formats / alpha modes.
    /// Observed in the wild on virtual/remote display adapters (RDP,
    /// headless) — fail renderer init gracefully instead of panicking.
    #[error("adapter reported no usable surface capabilities")]
    EmptySurfaceCaps,
}

/// Rectangle of cells, inclusive on both ends, in (col, row) coordinates.
#[derive(Debug, Clone, Copy)]
pub struct CellRect {
    /// Anchor cell — the cell the user clicked first.
    pub anchor: (u16, u16),
    /// Current cell — the cell the user is hovering / released on.
    pub cursor: (u16, u16),
    /// When `true`, the selection is a rectangular block (xterm
    /// Alt+drag semantic) rather than the default flowing row-major
    /// run.
    pub block: bool,
}

impl CellRect {
    /// Iterate every cell inside the selection in row-major order.
    pub fn cells(&self) -> Box<dyn Iterator<Item = (u16, u16)>> {
        let (a_col, a_row) = self.anchor;
        let (c_col, c_row) = self.cursor;
        let (r_start, r_end) = if a_row <= c_row {
            (a_row, c_row)
        } else {
            (c_row, a_row)
        };
        if self.block {
            let (col_lo, col_hi) = if a_col <= c_col {
                (a_col, c_col)
            } else {
                (c_col, a_col)
            };
            return Box::new(
                (r_start..=r_end).flat_map(move |row| (col_lo..=col_hi).map(move |col| (col, row))),
            );
        }
        Box::new((r_start..=r_end).flat_map(move |row| {
            let (start, end) = if row == a_row && row == c_row {
                if a_col <= c_col {
                    (a_col, c_col)
                } else {
                    (c_col, a_col)
                }
            } else if row == a_row {
                if a_row <= c_row {
                    (a_col, u16::MAX)
                } else {
                    (0, a_col)
                }
            } else if row == c_row {
                if a_row <= c_row {
                    (0, c_col)
                } else {
                    (c_col, u16::MAX)
                }
            } else {
                (0, u16::MAX)
            };
            (start..=end).map(move |col| (col, row))
        }))
    }
}

/// Map a selection cell's viewport row, captured at `(sel_scroll,
/// sel_history)`, to the viewport row where the SAME text line sits now, at
/// `(scroll, history)`. Returns `None` when the line has moved above the
/// viewport top (the caller bounds-checks the bottom edge against the grid).
///
/// Derivation: with `H` history lines above the live screen and the viewport
/// panned up by `S`, the buffer line shown at viewport row `r` is
/// `g = H - S + r`. Solving for the new row of the same `g` at `(S', H')`
/// gives `r' = r + (S' - S) - (H' - H)` — scrolling deeper into history
/// moves the text down the screen, new output arriving at the live bottom
/// moves it up.
fn reanchored_row(
    row: u16,
    sel_scroll: usize,
    sel_history: usize,
    scroll: usize,
    history: usize,
) -> Option<usize> {
    let dy = scroll as i64 - sel_scroll as i64 - (history as i64 - sel_history as i64);
    usize::try_from(i64::from(row) + dy).ok()
}

/// When the scrollback scrollbar is shown. Mirrors `window.scrollbar`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollbarMode {
    /// Shown while panning history, or when hovering the right edge —
    /// so it can be grabbed even from the live bottom. (default)
    #[default]
    Auto,
    /// Shown whenever any scrollback history exists.
    Always,
    /// Never drawn (and never grabbable).
    Never,
}

/// Geometry of the scrollback scrollbar as last computed by the draw pass,
/// in physical pixels, plus the scroll state it was derived from. Cached per
/// frame so the mouse handler can hit-test the thumb and convert a drag into
/// a scroll offset without recomputing renderer internals.
#[derive(Debug, Clone, Copy)]
pub struct ScrollbarGeom {
    /// Left edge of the track.
    pub track_x: f32,
    /// Top of the track.
    pub track_top: f32,
    /// Track width.
    pub track_w: f32,
    /// Track height.
    pub track_h: f32,
    /// Top of the thumb.
    pub thumb_top: f32,
    /// Thumb height.
    pub thumb_h: f32,
    /// Scrollback length the geometry was computed against.
    pub history: usize,
    /// Visible grid rows the geometry was computed against.
    pub rows: usize,
}

/// Scroll offset (lines into history; `0` = live bottom) for a thumb dragged
/// so its TOP sits at `thumb_top`. Inverse of the draw-pass mapping, linear
/// over the thumb's travel range so the full history is always reachable —
/// thumb at the track top ⇒ deepest history, thumb at the bottom ⇒ live.
#[must_use]
pub fn scrollbar_scroll_for_thumb(geom: &ScrollbarGeom, thumb_top: f32) -> usize {
    if geom.history == 0 {
        return 0;
    }
    let total = (geom.history + geom.rows).max(1) as f32;
    #[allow(clippy::cast_precision_loss)]
    let thumb_frac = (geom.rows as f32 / total).clamp(0.04, 1.0);
    let max_frac = (1.0 - thumb_frac).max(f32::EPSILON);
    let top_frac = ((thumb_top - geom.track_top) / geom.track_h.max(1.0)).clamp(0.0, max_frac);
    #[allow(clippy::cast_precision_loss)]
    let scroll = (geom.history as f32 * (1.0 - top_frac / max_frac)).round();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (scroll as usize).min(geom.history)
    }
}

/// One menu item shown in the right-click overlay.
#[derive(Debug, Clone)]
pub struct MenuItem {
    /// Label displayed in the menu.
    pub label: String,
    /// Optional Unicode icon (glyph) shown before the label.
    pub icon: Option<String>,
    /// Hotkey hint (optional, right-aligned).
    pub hotkey: Option<String>,
    /// Disabled items render dim and don't react to clicks.
    pub enabled: bool,
    /// Render a thin separator above this item.
    pub separator_before: bool,
}

/// One tab in the top tab bar.
#[derive(Debug, Clone)]
pub struct TabBarItem {
    /// Display label (typically the profile name).
    pub label: String,
    /// Optional one-glyph icon shown before the label.
    pub icon: Option<String>,
    /// Whether this tab is the currently-active one.
    pub active: bool,
    /// Background tab has produced new output since the user last
    /// looked at it. The bar renders a small blue accent dot.
    pub unread: bool,
    /// The program in this tab rang the bell (asked for attention) while the
    /// tab was not focused — e.g. Claude Code finished its turn and is waiting
    /// for input. The bar renders a distinct static amber dot, in the opposite
    /// corner from the blue `unread` dot. Only honoured on inactive tabs.
    pub attention: bool,
    /// Context-rule tint colour for the tab chip background (`[R, G, B]`).
    /// `None` = use the default tab colour. Set automatically when a
    /// `[[context_rules]]` entry matches the tab's SSH host or cwd.
    pub color: Option<[u8; 3]>,
    /// Short badge text overlaid on the tab pill (e.g. `"PROD"`).
    /// `None` = no badge. Set automatically when a `[[context_rules]]`
    /// entry with a `badge` field matches the tab.
    pub badge: Option<String>,
    /// When `true` the tab is pinned: it always renders at the leading edge
    /// of the bar with a compact (icon-only) fixed width, and its close-X is
    /// hidden to resist accidental closure.
    pub pinned: bool,
    /// Group accent colour `[R, G, B]` when this tab belongs to a named group.
    /// `None` = ungrouped. Rendered as a distinct accent stripe on the tab pill
    /// so members of the same group share a common visual brand.
    pub group_accent: Option<[u8; 3]>,
    /// Group label text (the group name). Non-`None` only on the **first**
    /// tab of a consecutive run of tabs that belong to the same group — used
    /// to place the group label once, at the group boundary.
    pub group_label: Option<String>,
}

/// State of the top tab bar passed to [`Renderer::set_tab_bar`].
#[derive(Debug, Clone)]
pub struct TabBar {
    /// Tabs from left to right.
    pub items: Vec<TabBarItem>,
    /// Mouse-hovered tab index, if any.
    pub hovered: Option<usize>,
    /// Whether the mouse is hovering the trailing `+` button.
    pub plus_hovered: bool,
    /// Index whose ✕ close button is hovered, if any.
    pub close_hovered: Option<usize>,
    /// Whether the window is currently maximised (controls the max icon).
    pub maximized: bool,
    /// Which window-control button is being hovered, if any.
    pub window_ctrl_hovered: Option<WindowCtrl>,
}

/// Which window-control button is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCtrl {
    /// The minimize ("–") button.
    Minimize,
    /// The maximize / restore ("□") button.
    Maximize,
    /// The close ("✕") button.
    Close,
}

/// Height of the tab bar in logical pixels.
pub const TAB_BAR_HEIGHT: f32 = 36.0;
/// Fixed width of a pinned (compact, icon-only) tab chip in logical pixels.
/// Overridden at runtime by `appearance.pinned_tab_width`.
pub const TAB_PINNED_WIDTH: f32 = 44.0;
/// Height of the per-pane header strip in logical pixels. Physical height
/// is `PANE_HEADER_HEIGHT * scale_factor`. Shown above each pane grid when
/// a tab has more than one leaf and `show_pane_headers` is enabled.
pub const PANE_HEADER_HEIGHT: f32 = 22.0;
/// Width of a single tab (will grow with label length but capped).
pub const TAB_DEFAULT_WIDTH: f32 = 180.0;
/// Lower bound a tab shrinks to when many tabs share the bar.
pub const TAB_MIN_WIDTH: f32 = 90.0;
/// Upper bound a tab grows to for a long label.
pub const TAB_MAX_WIDTH: f32 = 260.0;

/// Result of [`Renderer::tab_hit`]: which tab UI element was clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabHit {
    /// Selecting tab `idx`.
    Tab(usize),
    /// Closing tab `idx` via its ✕ button.
    Close(usize),
    /// The trailing `+` button.
    Plus,
    /// The minimize button (─).
    Minimize,
    /// The maximize / restore button (□ / ❐).
    Maximize,
    /// The close-window button (✕).
    CloseWindow,
    /// The drag handle area (clicking and dragging here moves the window).
    DragHandle,
    /// The group-label pill for the group run starting at `first_idx`.
    GroupLabel(usize),
}

/// Width of each window-control button (min / max / close) in logical px.
pub const WINDOW_CTRL_WIDTH: f32 = 46.0;

/// Absolute floor a tab may shrink to (logical px) when the tab bar is too
/// crowded to honour `tab_min_width` — keeps tabs from spilling under the
/// window-control buttons.
pub const TAB_HARD_MIN_WIDTH: f32 = 36.0;

/// Title-bar window-control button height (logical px). Shared between
/// the main wgpu window and the egui settings window so the icons line
/// up perfectly across them.
pub const WINDOW_CTRL_HEIGHT: f32 = 36.0;

// ── Snap-layout chooser geometry constants (logical pixels) ──────────────────

/// Width of each snap-chooser cell button in logical pixels.
pub const SNAP_CHOOSER_CELL_W: f32 = 72.0;
/// Height of each snap-chooser cell button in logical pixels.
pub const SNAP_CHOOSER_CELL_H: f32 = 48.0;
/// Gap between cells in logical pixels.
pub const SNAP_CHOOSER_GAP: f32 = 6.0;
/// Padding inside the chooser panel in logical pixels.
pub const SNAP_CHOOSER_PAD: f32 = 12.0;
/// Height of the chooser's title header row in logical pixels.
pub const SNAP_CHOOSER_HEADER_H: f32 = 28.0;

/// Compute the chooser panel rectangle `(x, y, w, h)` in **physical** pixels
/// given the window dimensions and scale factor.  The panel is centred
/// horizontally and vertically on the window.
#[must_use]
fn snap_chooser_geometry(win_w: f32, win_h: f32, scale: f32) -> (f32, f32, f32, f32) {
    let cols = 3usize;
    let rows = SNAP_CHOOSER_CELLS.len().div_ceil(cols);
    let panel_w = (SNAP_CHOOSER_PAD * 2.0
        + cols as f32 * SNAP_CHOOSER_CELL_W
        + (cols - 1) as f32 * SNAP_CHOOSER_GAP)
        * scale;
    let panel_h = (SNAP_CHOOSER_HEADER_H
        + SNAP_CHOOSER_PAD
        + rows as f32 * SNAP_CHOOSER_CELL_H
        + (rows - 1) as f32 * SNAP_CHOOSER_GAP
        + SNAP_CHOOSER_PAD)
        * scale;
    let x = (win_w - panel_w) * 0.5;
    let y = (win_h - panel_h) * 0.5;
    (x, y, panel_w, panel_h)
}

/// Overlay menu state passed to the renderer.
#[derive(Debug, Clone)]
pub struct MenuOverlay {
    /// Top-left of the menu in logical pixels.
    pub origin_px: [f32; 2],
    /// Width of the menu in logical pixels.
    pub width_px: f32,
    /// Items, in display order.
    pub items: Vec<MenuItem>,
    /// Currently hovered item index, if any.
    pub hovered: Option<usize>,
}

/// Holds every piece of GPU + text state needed to draw a terminal frame.
pub struct Renderer {
    instance: Arc<Instance>,
    adapter: Arc<wgpu::Adapter>,
    device: Arc<Device>,
    queue: Arc<Queue>,
    surface: Surface<'static>,
    config: SurfaceConfiguration,
    #[allow(dead_code)]
    surface_format: wgpu::TextureFormat,

    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: GlyphonViewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,

    bg: BgPipeline,
    /// Animated background-effect pipeline + its current params and a clock
    /// origin. Drawn under the cell grid when `bg_fx_params.active()`.
    bg_fx: BgFxPipeline,
    bg_fx_params: BgFxParams,
    bg_fx_start: std::time::Instant,
    /// Background image pipeline and its current render params. Drawn under
    /// `bg_fx` and the cell grid when `bg_image_params.active()`.
    bg_image: BgImagePipeline,
    bg_image_params: BgImageParams,
    /// Start instant of the most recent global keystroke pulse (Aurora /
    /// Starfield / PixelCRT global flare). Decays over `BG_FX_PULSE_SECS`.
    bg_fx_pulse_start: Option<std::time::Instant>,
    /// Live per-keystroke emitter bands. Each call to
    /// `spawn_bg_fx_emitter` appends one; entries are pruned once their age
    /// exceeds the configured lifetime. Cap: `MAX_EMITTERS`.
    bg_fx_emitters: Vec<CpuEmitter>,
    /// Monotonic counter for emitter seed diversification.
    bg_fx_spawn_counter: u32,
    /// Whether the Matrix glyph atlas has been rasterized + uploaded. Built
    /// lazily by `set_bg_fx_params` the first time the Matrix mode is enabled
    /// — the effect is off by default and the 60-glyph rasterization doesn't
    /// belong on every window's first-frame path.
    matrix_atlas_ready: bool,
    font_size: f32,
    line_height: f32,
    /// Configured monospace family name (e.g. "JetBrains Mono"). Empty =
    /// fall back to the system default monospace.
    font_family: String,
    /// Optional override family for **bold** text (`font.bold_family`).
    /// `None` = synthesize boldness from `font_family`.
    font_bold_family: Option<String>,
    /// Optional override family for **italic** text (`font.italic_family`).
    /// `None` = synthesize italics from `font_family`.
    font_italic_family: Option<String>,
    /// Optional override family for **bold-italic** text (`font.bold_italic_family`).
    /// Falls back to `font_bold_family`, then `font_italic_family`, then
    /// the synthesized path on `font_family`.
    font_bold_italic_family: Option<String>,
    /// Whether ligatures / contextual alternates are enabled for the
    /// terminal grid text.
    ligatures: bool,
    /// Logical-pixel thickness of SGR underline strokes (single, double,
    /// dotted, dashed, curly). Scaled by `scale_factor` at draw time.
    /// Mirrors `font.underline_thickness_px`. Default `1.0`.
    underline_thickness_px: f32,
    cell_width: f32,
    cell_height: f32,
    scale_factor: f32,

    padding_px: f32,
    selection: Option<CellRect>,
    /// Viewport scroll offset captured when `selection` was last set. The
    /// selection rect is viewport-relative AS OF that moment; together with
    /// `selection_history` the draw pass re-anchors the highlight to the
    /// TEXT as the viewport moves (scrolling) or the text itself moves
    /// (new output growing the scrollback while at the live bottom).
    selection_scroll: usize,
    /// Scrollback history length captured when `selection` was last set.
    selection_history: usize,
    /// History length of the focused pane's emulator as of the last drawn
    /// frame — the freshest value available when `set_selection` is called
    /// between frames (mouse events).
    last_history: usize,
    /// When the scrollback scrollbar is shown. Mirrors `window.scrollbar`.
    scrollbar_mode: ScrollbarMode,
    /// Pointer currently hovering the scrollbar band (set by the app from
    /// `CursorMoved`). Reveals the bar in `Auto` mode and widens it.
    scrollbar_hover: bool,
    /// Thumb currently being dragged (set by the app). Keeps the bar shown
    /// and widened for the duration of the drag.
    scrollbar_active: bool,
    /// Scrollbar geometry as last computed (cached whenever any history
    /// exists, even when the bar isn't visible — `Auto` mode needs the band
    /// for hover-reveal hit-testing). `None` = no scrollback.
    last_scrollbar: Option<ScrollbarGeom>,
    /// Drop-zone highlight rect (physical px, `[x, y, w, h]`) shown while a
    /// dragged tab / pane hovers a pane body — the half of the target pane
    /// it would occupy on release. `None` = no merge drop in flight.
    drop_zone: Option<[f32; 4]>,
    overlay: Option<MenuOverlay>,
    tab_bar: Option<TabBar>,
    /// Lower bound a tab shrinks to (logical px). Mirrors
    /// `appearance.tab_min_width`; defaults to [`TAB_MIN_WIDTH`].
    tab_min_width: f32,
    /// Upper bound a tab grows to (logical px). Mirrors
    /// `appearance.tab_max_width`; defaults to [`TAB_MAX_WIDTH`].
    tab_max_width: f32,
    /// Fixed width for pinned (compact) tabs in logical pixels. Mirrors
    /// `appearance.pinned_tab_width`; defaults to [`TAB_PINNED_WIDTH`].
    tab_pinned_width: f32,
    focused: bool,
    cursor: CursorParams,
    /// Cursor colour from the active theme, used as the fallback when the
    /// user hasn't set an explicit `cursor.color` override. Lets a theme
    /// switch recolour the cursor (e.g. Matrix → green) instead of leaving
    /// it the hard-coded default blue.
    cursor_theme_color: Option<[u8; 3]>,
    cursor_start: std::time::Instant,
    scroll_lines: usize,
    background_rgb: [u8; 3],
    background_alpha: f32,
    selection_rgb: [u8; 3],
    /// Alpha for the selection highlight quad. `1.0` = the theme's selection
    /// colour painted as an opaque cell background (default); see
    /// [`Self::set_selection_opacity`].
    selection_opacity: f32,
    /// Extra underlines (autodetected URLs, search highlights, …) drawn
    /// on top of the normal SGR underline pass. Each entry is the
    /// inclusive cell range `(col_start, col_end, row)` in viewport
    /// coordinates.
    extra_underlines: Vec<(u16, u16, u16)>,
    /// Prompt-status gutter dots set by [`Self::set_prompt_marks`].
    /// Each entry is `(viewport_row, exit_code_opt)` for one visible
    /// OSC 133 prompt-start line. Only drawn when non-empty.
    prompt_marks: Vec<(u16, Option<u32>)>,
    /// `Some(start_instant)` when a visual bell is decaying. Each frame
    /// after the bell is fired we draw a full-window tint whose alpha
    /// fades to zero over [`BELL_DURATION`].
    bell_start: Option<std::time::Instant>,
    /// Bottom-of-window search bar contents — `None` = no bar drawn.
    search_overlay: Option<SearchOverlay>,
    /// Optional URL tooltip drawn near the cursor — used to show the
    /// destination of hyperlinked / autodetected text on hover.
    tooltip: Option<Tooltip>,
    /// Centered fuzzy command-palette modal — `None` when closed.
    command_palette: Option<CommandPalette>,
    /// "Save this SSH host?" toast — `None` when not prompting.
    save_host_prompt: Option<SaveHostPrompt>,
    /// Proactive AI command-suggestion bar — `None` = hidden.
    suggestion_bar: Option<SuggestionBar>,
    /// Second glyphon text renderer used exclusively for the overlay
    /// layer (command palette + floating overlays). Prepared/rendered
    /// *after* the main text pass so palette labels draw on top of the
    /// modal panel rather than bleeding terminal glyphs through it.
    overlay_text_renderer: TextRenderer,
    /// Floating, cursor-following tab pill drawn during a Chrome-style tab
    /// drag. `None` = no drag in progress (or animation disabled). Lives on
    /// the renderer (not [`TabBar`]) so it survives [`Self::set_tab_bar`] /
    /// the App's per-frame tab-bar refresh — those rebuild the bar but must
    /// not wipe the in-flight ghost.
    tab_drag_ghost: Option<TabGhost>,
    /// Vertical insertion bar shown in this window's tab bar at the given
    /// **logical-px** x while a drag would land here. `None` hides it (e.g.
    /// the drag is over another window, or in the detach band).
    tab_drop_indicator: Option<f32>,
    /// Override for the body-rect origin used by the next `render(emu)`
    /// call. Set by [`Self::render_panes`] right before invoking
    /// `render` with the focused pane's emulator, so the focused pane's
    /// grid draws inside its sub-rect. Cleared after each render call.
    pending_body_x: Option<f32>,
    pending_body_y: Option<f32>,
    /// Extra background quads to be appended to the next `render(emu)`
    /// frame, used for drawing non-focused panes' grid cells +
    /// underlines while the focused pane goes through the main render
    /// path. Cleared after each `render_panes` call.
    extra_pane_quads: Vec<Quad>,
    /// Shaped-row text cache for NON-focused panes, keyed by `pane_id`.
    /// Value = (input hash, shaped per-row buffers positioned in physical
    /// px). Same soundness contract as [`Self::cached_focused_text`]: the
    /// hash covers every input the row-building loop reads, so a hit means
    /// the buffers are identical to what a rebuild would produce. Without
    /// this, every non-focused pane re-shaped every visible row on every
    /// frame — the dominant steady-state cost in split layouts.
    extra_pane_text_cache: std::collections::HashMap<u32, (u64, Vec<(Buffer, [f32; 2])>)>,
    /// Pane ids queued by the current `render_panes` call, in draw order.
    /// The text `prepare` pass draws exactly these entries from the cache;
    /// afterwards entries not in this list are evicted (closed panes,
    /// other tabs) so the cache cannot grow unbounded.
    extra_pane_cache_seen: Vec<u32>,
    /// Focus-border quads drawn LAST on the main layer — after all pane
    /// backgrounds AND divider strokes — so neither a neighbour pane's bg
    /// nor an adjacent divider can overpaint any side of the indicator.
    focus_border_quads: Vec<Quad>,
    /// Inactive-pane dim overlay quads queued by `render_panes`. Drained
    /// into the **overlay** layer (after the text pass) so the black tint
    /// sits ABOVE terminal glyphs rather than being painted over by them.
    pane_dim_quads: Vec<Quad>,
    /// Divider-stroke quads queued by `render_panes_with_dividers` for
    /// the splits between panes. Drained into the main layer BEFORE
    /// `focus_border_quads` so the focus border always wins at the edges.
    divider_quads: Vec<Quad>,
    /// Focus-border stroke thickness in **physical** pixels (already
    /// includes `* scale_factor`). Set to `0.0` to disable the border.
    /// Mirrors `appearance.focus_border_thickness_logical * scale`.
    focus_border_thickness_px: f32,
    /// Override RGB for the focus-border stroke. `None` = fallback accent
    /// colour `ACCENT_FOCUS_BORDER`. Mirrors `appearance.focus_border_color`.
    focus_border_color: Option<[u8; 3]>,
    /// Opacity of the focus-border stroke (0.0..=1.0). Mirrors
    /// `appearance.focus_border_opacity`.
    focus_border_alpha: f32,
    /// Pane ids that are currently receiving broadcast input. Set by the app
    /// each frame when broadcast mode is active; empty when broadcast is off.
    /// A distinct tinted border (amber) is drawn around these panes in
    /// addition to the normal focus border, so the user sees broadcast is on.
    broadcast_receiver_ids: Vec<u32>,
    /// Header-strip background quads, one per pane with `header_rect_px`.
    /// Drained into the main layer above pane-bg and focus-border so the
    /// strip visually overlays the top edge of each pane.
    pane_header_quads: Vec<Quad>,
    /// Glyphon text buffers for pane header labels and close-X glyphs.
    /// Positions are in **physical** pixels. Cleared after each frame's
    /// text `prepare`.
    pane_header_text_buffers: Vec<(Buffer, [f32; 2])>,
    /// Frame-level cache of the focused pane's shaped row text. Rebuilding
    /// (and re-shaping) every visible row on every redraw is the dominant
    /// render cost, and most redraws (cursor blink, bg FX, bell, jump
    /// highlight) don't change a single glyph — the cursor is a separate
    /// quad, so a static screen is byte-identical frame over frame. The
    /// cache is keyed by [`Self::focused_text_hash`]; on a hash hit the
    /// shaped [`Buffer`]s are reused as-is.
    cached_focused_text: Vec<(Buffer, [f32; 2])>,
    /// Hash of every input the focused-pane text loop reads: per cell
    /// `{hidden, ch, fg, bold, italic}` (fg is taken after
    /// `apply_sgr_attributes` and `enforce_min_contrast`, so theme/dim/
    /// contrast changes miss) and the globals `{font_size, line_height,
    /// the four family names, ligatures, builtin_box_drawing, cols, cell
    /// sizes, body origins, pad_px, ch_px}`. If the recomputed hash
    /// matches, the cached buffers are provably identical to what a
    /// rebuild would produce.
    focused_text_hash: u64,
    /// Cached `"GPU <name> (<backend>)"` label for the resource strip. The
    /// adapter is fixed for the renderer's lifetime; filled lazily on the
    /// first `build_resource_bar` call.
    gpu_label: Option<String>,
    /// When `true` and a tab has more than one pane, each pane gets a
    /// 22 px header strip. Driven by `config.appearance.show_pane_headers`.
    show_pane_headers: bool,
    /// `Some(pane_id)` while the pointer is hovering a pane header's
    /// close-X glyph. Drives the hover-tint on the ✕ glyph.
    pane_header_close_hovered: Option<u32>,
    /// Label badges for quick-select / pane-select overlay. Set each frame by
    /// [`Self::set_label_overlays`]; cleared to an empty vec when the mode
    /// exits. The `Vec` is empty when no overlay mode is active.
    label_overlays: Vec<LabelBadge>,
    /// Alpha of the full-screen dim tint drawn behind the label badges.
    /// `0.0` = no dim; `1.0` = fully opaque black. Driven by
    /// `quick_select.overlay_dim` from the config.
    label_overlay_dim: f32,
    /// Snap-layout chooser overlay. `None` = not shown. Set by the
    /// `ShowSnapLayouts` action; cleared on a cell click or Esc.
    snap_chooser: Option<SnapChooserOverlay>,
    /// Status-bar content set by the App each refresh. `None` = bar disabled
    /// (the layout reserves no space for it).
    status_bar: Option<StatusBarContent>,
    /// Bottom resource-indicator strip content. `None` = disabled (no space
    /// reserved). Always sits at the very bottom, below a bottom status bar.
    resource_bar: Option<ResourceBarContent>,
    /// Whether the tab bar is shown at all. When `false` the bar is hidden
    /// completely and the space is reclaimed for the terminal grid.
    tab_bar_enabled: bool,
    /// Where the tab bar is positioned relative to the terminal body.
    tab_bar_placement: TabBarPlacement,
    /// When `true`, the tab bar is hidden when exactly one tab is open.
    tab_bar_hide_if_single: bool,
    /// Width (logical px) of the vertical tab strip when placement is
    /// `Left` or `Right`. Mirrors `appearance.vertical_tab_bar_width`.
    vertical_tab_bar_width: f32,
    /// Cell-width multiplier applied on top of the probed monospace advance.
    /// `1.0` = natural advance; `>1.0` widens cells without stretching glyphs.
    cell_width_multiplier: f32,
    /// Visual style for the close-X buttons on tab chips and pane-header
    /// strips. Mirrors `config.appearance.close_button_style`.
    close_button_style: terminale_config::CloseButtonStyle,
    /// Inline-image blit pipeline (OSC 1337 / Sixel / APC graphics). Textures
    /// are uploaded on demand and evicted when the [`ImageStore`] drops them.
    image_blit: image_blit::ImageBlitPipeline,
    /// How strongly SGR 2 (faint/dim) text is blended toward the cell
    /// background. `0.0` = no effect; `1.0` = fully invisible; `0.5` =
    /// halfway (default). Mirrors `appearance.dim_amount`.
    dim_amount: f32,
    /// Minimum WCAG contrast ratio enforced per-cell. `1.0` = disabled.
    /// Mirrors `appearance.minimum_contrast`.
    minimum_contrast: f32,
    /// Alpha of the translucent black overlay drawn over inactive (non-focused)
    /// panes. `0.0` = off (default); up to `0.9` = strong dimming. Mirrors
    /// `appearance.inactive_pane_dim`.
    inactive_pane_dim: f32,
    /// Alpha of the translucent black overlay drawn over the whole grid area
    /// when the window loses OS focus. `0.0` = off (default); up to `0.9`.
    /// Mirrors `appearance.unfocused_window_dim`.
    unfocused_window_dim: f32,
    /// Jump-highlight: the viewport row (0-based) of the tinted band drawn
    /// after a prompt-navigation jump, paired with the alpha for this frame.
    /// `None` when the highlight has fully faded or was never set.
    /// Computed from `jump_highlight_viewport_row` and the elapsed time.
    jump_highlight_band: Option<(u16, f32)>,
    /// When `true` (default), box-drawing (U+2500–U+257F) and block-element
    /// (U+2580–U+259F) characters are rendered as crisp procedural quads
    /// instead of font glyphs. Mirrors `appearance.builtin_box_drawing`.
    builtin_box_drawing: bool,
    /// When `true` (default), the group name label is drawn at the start of
    /// each group's run of tabs. The colour accent is always shown.
    /// Mirrors `appearance.show_tab_group_labels`.
    show_tab_group_labels: bool,
}

/// Height of the status bar in logical pixels (one row of text).
pub const STATUS_BAR_HEIGHT: f32 = 22.0;

/// Runtime content of the status bar — left and right text strings.
#[derive(Debug, Clone)]
pub struct StatusBarContent {
    /// Left-aligned text.
    pub left: String,
    /// Right-aligned text.
    pub right: String,
    /// Position: `true` = bottom, `false` = top.
    pub at_bottom: bool,
}

/// Height of the resource-indicator strip in logical pixels.
pub const RESOURCE_BAR_HEIGHT: f32 = 26.0;

/// Runtime content of the bottom resource-indicator strip: live CPU and memory
/// percentages plus the GPU adapter label. Drawn as pixel-art segmented meters
/// in a reserved strip at the very bottom of the window (the grid shrinks to make
/// room, so it never overlaps terminal content).
#[derive(Debug, Clone, Copy)]
pub struct ResourceBarContent {
    /// Global CPU utilisation, percent `[0, 100]`.
    pub cpu_pct: f32,
    /// Memory used as a percent of total, `[0, 100]`.
    pub mem_pct: f32,
}

/// Description of one pane to render inside the active tab's body. A tab
/// holding a single leaf produces a 1-element slice covering the full body
/// rect; a split tab produces N specs, one per leaf, with the sub-rects
/// computed from the pane tree.
///
/// All measurements are in **physical** pixels — the caller (`main.rs`)
/// is already in physical-px space when it walks the tree, and the
/// renderer's internal math is physical, so this avoids a needless logical
/// hop. `focused` selects the pane that receives cursor / selection /
/// scrollbar / URL-underline chrome; non-focused panes draw only their
/// grid contents + per-cell underlines.
pub struct PaneSpec<'a> {
    /// `(x, y, w, h)` of this pane's render area in physical pixels.
    /// Includes the pane's own padding; the renderer offsets each cell
    /// inside the rect by `padding_px * scale`. When `header_rect_px` is
    /// `Some`, this rect is the **grid-only** area below the header.
    pub rect_px: (f32, f32, f32, f32),
    /// Physical-px rect `(x, y, w, h)` of the 22 px header strip drawn
    /// above this pane's grid, or `None` when headers are disabled or the
    /// tab has only one leaf.
    pub header_rect_px: Option<(f32, f32, f32, f32)>,
    /// Title string shown in the header strip (profile name / cwd / custom).
    /// Only meaningful when `header_rect_px` is `Some`.
    pub title: &'a str,
    /// Stable pane identifier — used to correlate hover state (close-X
    /// highlight) between the App and the renderer without an index.
    pub pane_id: u32,
    /// Borrow of the pane's emulator — the grid + cursor pos come from
    /// here.
    pub emulator: &'a Emulator,
    /// Lines scrolled back into history for *this* pane. `0` = pinned
    /// to live output. Each pane carries its own scroll state.
    pub scroll_lines: usize,
    /// Whether this pane has keyboard focus. Exactly one entry in the
    /// slice should set this; if zero or several do, the renderer
    /// picks the first.
    pub focused: bool,
}

/// A single divider stroke between two adjacent panes. The caller walks
/// the pane tree to place these at split boundaries; the renderer paints
/// one coloured quad per stroke at the supplied physical-px rect. The
/// `rect_px` is the **visible** boundary line, NOT the inflated grab
/// band the App uses for mouse hit-testing.
#[derive(Debug, Clone, Copy)]
pub struct DividerStroke {
    /// `(x, y, w, h)` of the visible stroke in physical pixels.
    pub rect_px: (f32, f32, f32, f32),
    /// Stroke RGB. Resolved by the caller (config override or theme-derived
    /// fallback) so the renderer doesn't need to know about themes.
    pub color: [u8; 3],
}

/// A tab-shaped translucent pill that follows the cursor during a
/// Chrome-style tab drag, drawn in the overlay layer above every tab pill
/// and label. All coordinates are **logical** pixels; the renderer scales
/// them by the DPI factor at draw time, matching the tab-bar geometry.
#[derive(Debug, Clone)]
pub struct TabGhost {
    /// Label to render on the ghost (icon + title, same as a normal tab).
    pub label: String,
    /// Logical-px x of the ghost pill's centre (tracks the cursor).
    pub center_x: f32,
    /// Logical-px y of the ghost pill's centre. Sits in the tab-bar band
    /// while reordering; lifts toward the cursor when detaching.
    pub center_y: f32,
    /// Logical-px width of the ghost pill — the width of the slot the
    /// dragged tab vacated, so the ghost matches the grabbed tab.
    pub width: f32,
}

/// A small floating tooltip drawn near the pointer. Currently used
/// for "URL preview on hover".
#[derive(Debug, Clone)]
pub struct Tooltip {
    /// Text shown in the tooltip.
    pub text: String,
    /// Anchor in **physical** pixels (cursor position).
    pub anchor_px: [f32; 2],
}

/// A single quick-select or pane-select label badge passed to
/// [`Renderer::set_label_overlays`]. Each badge is a small highlighted chip
/// anchored either at a grid cell or at an explicit physical-pixel centre
/// (used for pane-select where the badge floats in the middle of a pane).
#[derive(Debug, Clone)]
pub struct LabelBadge {
    /// Grid column (0-based) in viewport coordinates. Ignored when
    /// `center_px` is `Some`.
    pub col: u16,
    /// Grid row (0-based) in viewport coordinates. Ignored when
    /// `center_px` is `Some`.
    pub row: u16,
    /// Optional physical-pixel `(x, y)` badge centre. When `Some` this
    /// overrides `col`/`row` — used for pane-select where the badge floats
    /// centred in the pane rather than at a grid-aligned match position.
    pub center_px: Option<[f32; 2]>,
    /// The already-typed prefix of this label — drawn in a dimmer colour
    /// to show "already consumed" characters.
    pub typed_prefix: String,
    /// The remaining suffix still to type — drawn full-brightness.
    pub remaining: String,
    /// When `true` this badge is uniquely matched (one candidate left);
    /// render with a stronger accent so the user knows which key to press.
    pub highlighted: bool,
}

/// How long the visual bell tint stays visible before fully decaying.
const BELL_DURATION_MS: u64 = 220;

/// Status displayed in the find-in-buffer bar at the bottom of the
/// terminal. Set via [`Renderer::set_search_overlay`].
#[derive(Debug, Clone)]
pub struct SearchOverlay {
    /// Query the user has typed so far.
    pub query: String,
    /// 1-based index of the focused match.
    pub current: usize,
    /// Total number of matches.
    pub total: usize,
}

/// A non-intrusive "Save this SSH host?" toast drawn near the top of the
/// window when the user types an `ssh …` command for an unsaved host. Set
/// via [`Renderer::set_save_host_prompt`]. The caller owns the host details;
/// the renderer only draws the card + its three hit targets.
#[derive(Debug, Clone)]
pub struct SaveHostPrompt {
    /// `user@host[:port]` rendering shown in the toast body.
    pub endpoint: String,
    /// State of the "don't ask again" checkbox (defaults checked).
    pub dont_ask_again: bool,
}

/// Which interactive part of the [`SaveHostPrompt`] toast a click landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavePromptHit {
    /// The "Save" button.
    Save,
    /// The "Dismiss" button.
    Dismiss,
    /// The "don't ask again" checkbox (toggles it).
    DontAskAgain,
}

/// What the proactive AI command-suggestion bar is currently showing. Set via
/// [`Renderer::set_suggestion_bar`]; the bar is hidden entirely when the field
/// is `None`.
#[derive(Debug, Clone)]
pub enum SuggestionBarState {
    /// A request is in flight. `frame` animates the "scanning" indicator and
    /// is expected to advance a few times per second.
    Loading {
        /// Monotonic animation tick (any wrapping counter).
        frame: u8,
    },
    /// A command is ready to drop onto the prompt. The `[INJECT]` button shows.
    Ready {
        /// The proposed shell command (full text — may be truncated on screen
        /// but injected in full).
        command: String,
    },
    /// The request failed; the message is shown in red. No `[INJECT]` button.
    Error {
        /// Short human-readable failure reason.
        message: String,
    },
    /// Unobtrusive "fix the failed command" offer (amber text). Shows a
    /// `[Fix]` action button in place of `[INJECT]`.
    Hint {
        /// Short human-readable hint (e.g. the failed command + exit code).
        message: String,
    },
}

/// Retained state for the command-suggestion bar overlay.
#[derive(Debug, Clone)]
pub struct SuggestionBar {
    /// Current content / mode of the bar.
    pub state: SuggestionBarState,
}

/// Which interactive part of the [`SuggestionBar`] a click landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionBarHit {
    /// The `[INJECT]` button — drop the proposed command onto the prompt.
    Inject,
    /// The `✕` button — dismiss the suggestion.
    Dismiss,
}

/// Physical-pixel-independent (logical) geometry of the suggestion bar and its
/// click targets. Shared by the draw pass and the hit-test so they never drift.
#[derive(Debug, Clone, Copy)]
struct SuggestionBarLayout {
    /// Full-width bar strip.
    bar: LogicalRect,
    /// `[INJECT]` button (only present in the `Ready` state).
    inject: Option<LogicalRect>,
    /// `✕` dismiss button.
    dismiss: LogicalRect,
}

/// Suggestion bar height, in logical px.
const SUGGESTION_BAR_HEIGHT: f32 = 30.0;

/// Snapshot of the suggestion bar's current content, decoupled from the public
/// [`SuggestionBarState`] so the renderer can build text after releasing its
/// borrow on `self.suggestion_bar`.
enum SuggestionBody {
    /// A request is in flight; the inner value animates the "Thinking…" dots.
    Loading(u8),
    /// A command is ready to inject.
    Ready(String),
    /// The request failed.
    Error(String),
    /// "Fix the failed command" offer (amber, `[Fix]` action button).
    Hint(String),
}

/// Pure layout of the suggestion bar + its buttons from the surface size, DPI
/// `scale`, and the bottom inset already consumed by a bottom status bar. Split
/// out of the renderer so the geometry can be unit-tested without a GPU.
fn suggestion_bar_layout(
    width_px: u32,
    height_px: u32,
    scale: f32,
    bottom_inset_log: f32,
    has_inject: bool,
) -> SuggestionBarLayout {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let w_log = width_px as f32 / scale;
    let h_log = height_px as f32 / scale;
    let bar_y = (h_log - SUGGESTION_BAR_HEIGHT - bottom_inset_log).max(0.0);
    let bar = LogicalRect {
        x: 0.0,
        y: bar_y,
        w: w_log,
        h: SUGGESTION_BAR_HEIGHT,
    };
    let pad = 10.0;
    let btn_h = SUGGESTION_BAR_HEIGHT - 12.0;
    let btn_y = bar_y + 6.0;
    let dismiss = LogicalRect {
        x: (w_log - pad - 22.0).max(0.0),
        y: btn_y,
        w: 22.0,
        h: btn_h,
    };
    let inject = if has_inject {
        Some(LogicalRect {
            x: (dismiss.x - 8.0 - 78.0).max(0.0),
            y: btn_y,
            w: 78.0,
            h: btn_h,
        })
    } else {
        None
    };
    SuggestionBarLayout {
        bar,
        inject,
        dismiss,
    }
}

#[cfg(test)]
mod suggestion_bar_tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn bar_spans_full_width_and_sits_at_bottom() {
        let l = suggestion_bar_layout(1000, 600, 1.0, 0.0, true);
        assert!(approx(l.bar.x, 0.0));
        assert!(approx(l.bar.w, 1000.0));
        assert!(approx(l.bar.h, SUGGESTION_BAR_HEIGHT));
        assert!(approx(l.bar.y, 600.0 - SUGGESTION_BAR_HEIGHT));
    }

    #[test]
    fn bottom_inset_lifts_the_bar_above_a_status_bar() {
        let l = suggestion_bar_layout(1000, 600, 1.0, STATUS_BAR_HEIGHT, true);
        assert!(approx(
            l.bar.y,
            600.0 - SUGGESTION_BAR_HEIGHT - STATUS_BAR_HEIGHT
        ));
    }

    #[test]
    fn inject_present_only_when_requested() {
        assert!(suggestion_bar_layout(1000, 600, 1.0, 0.0, true)
            .inject
            .is_some());
        assert!(suggestion_bar_layout(1000, 600, 1.0, 0.0, false)
            .inject
            .is_none());
    }

    #[test]
    fn inject_sits_left_of_dismiss_and_both_inside_bar() {
        let l = suggestion_bar_layout(1000, 600, 1.0, 0.0, true);
        let inj = l.inject.unwrap();
        assert!(
            inj.x + inj.w <= l.dismiss.x,
            "inject must be left of dismiss"
        );
        // Both buttons fully inside the bar vertically.
        assert!(inj.y >= l.bar.y && inj.y + inj.h <= l.bar.y + l.bar.h);
        assert!(l.dismiss.y >= l.bar.y && l.dismiss.y + l.dismiss.h <= l.bar.y + l.bar.h);
        // Dismiss is flush-ish to the right edge.
        assert!(l.dismiss.x + l.dismiss.w <= 1000.0);
        assert!(l.dismiss.x + l.dismiss.w >= 1000.0 - 40.0);
    }

    #[test]
    fn scale_factor_keeps_logical_geometry_stable() {
        // Logical layout must be DPI-independent: a 2× surface of the same
        // logical size yields identical logical rects.
        let a = suggestion_bar_layout(1000, 600, 1.0, 0.0, true);
        let b = suggestion_bar_layout(2000, 1200, 2.0, 0.0, true);
        assert!((a.bar.w - b.bar.w).abs() < 0.001);
        assert!((a.bar.y - b.bar.y).abs() < 0.001);
        assert!((a.dismiss.x - b.dismiss.x).abs() < 0.001);
    }

    #[test]
    fn zero_scale_does_not_panic() {
        let l = suggestion_bar_layout(800, 600, 0.0, 0.0, true);
        assert!(approx(l.bar.w, 800.0)); // scale clamped to 1.0
    }
}

/// One cell in the snap-layout chooser grid.
///
/// The chooser is a small on-screen grid of clickable layout-preset cells
/// that lets the user snap the window with a single click (or keyboard
/// Esc to dismiss).  Each cell carries a short label and a layout icon
/// (a small Unicode box-drawing character or text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapChooserCell {
    /// Snap to the left half.
    Left,
    /// Snap to the right half.
    Right,
    /// Snap to the top half.
    Top,
    /// Snap to the bottom half.
    Bottom,
    /// Snap to the top-left quarter.
    TopLeft,
    /// Snap to the top-right quarter.
    TopRight,
    /// Snap to the bottom-left quarter.
    BottomLeft,
    /// Snap to the bottom-right quarter.
    BottomRight,
    /// Center the window (keep size).
    Center,
    /// Maximize the window.
    Maximize,
}

impl SnapChooserCell {
    /// Short display label (shown inside the cell button).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::TopLeft => "TL",
            Self::TopRight => "TR",
            Self::BottomLeft => "BL",
            Self::BottomRight => "BR",
            Self::Center => "Center",
            Self::Maximize => "Max",
        }
    }
}

/// State of the snap-layout chooser overlay. Set via
/// [`Renderer::set_snap_chooser`]; `None` hides it.
///
/// The overlay draws a grid of clickable layout buttons.  The caller
/// supplies the hovered cell index so the renderer can highlight it;
/// hit-testing is performed by the caller using
/// [`Renderer::snap_chooser_hit`].
#[derive(Debug, Clone)]
pub struct SnapChooserOverlay {
    /// Currently-hovered cell index into the canonical cell order
    /// (`SNAP_CHOOSER_CELLS`), or `None` if no cell is hovered.
    pub hovered: Option<usize>,
}

/// Canonical order of snap-chooser cells — fixed so hit indices are stable.
pub const SNAP_CHOOSER_CELLS: [SnapChooserCell; 10] = [
    SnapChooserCell::TopLeft,
    SnapChooserCell::Top,
    SnapChooserCell::TopRight,
    SnapChooserCell::Left,
    SnapChooserCell::Center,
    SnapChooserCell::Right,
    SnapChooserCell::BottomLeft,
    SnapChooserCell::Bottom,
    SnapChooserCell::BottomRight,
    SnapChooserCell::Maximize,
];

/// A single row in the [`CommandPalette`]: a human label plus the key
/// binding that triggers it (may be empty when the action is unbound).
#[derive(Debug, Clone)]
pub struct PaletteEntry {
    /// Display text, e.g. "New Tab".
    pub label: String,
    /// Key binding shown right-aligned, e.g. "Ctrl+Shift+T". Empty hides it.
    pub binding: String,
}

/// A centered fuzzy command-palette modal. The caller does the filtering
/// and ranking; the renderer only draws `entries` with `selected`
/// highlighted and `query` echoed in the input row. Drawn as a true
/// overlay (its own quad + text layer) so it cleanly occludes the
/// terminal behind it.
#[derive(Debug, Clone)]
pub struct CommandPalette {
    /// Text the user has typed so far.
    pub query: String,
    /// Already-filtered, already-ranked rows in display order.
    pub entries: Vec<PaletteEntry>,
    /// Index into `entries` of the highlighted row (0 when empty).
    pub selected: usize,
    /// Greyed prompt shown when `query` is empty — mode-specific so the user
    /// knows what the picker does (e.g. "Search SSH hosts…").
    pub placeholder: String,
}

/// Where the tab bar is rendered relative to the terminal body. Mirrors the
/// `TabBarPosition` enum in `terminale-config` but lives here so this crate
/// stays config-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabBarPlacement {
    /// Tab bar is drawn above the terminal grid (default).
    #[default]
    Top,
    /// Tab bar is drawn below the terminal grid.
    Bottom,
    /// Vertical tab strip on the left side of the window.
    Left,
    /// Vertical tab strip on the right side of the window.
    Right,
}

impl TabBarPlacement {
    /// Returns `true` when the tab bar is a side strip (`Left` or `Right`).
    #[must_use]
    pub fn is_vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// Cursor styles understood by the renderer. Mirrors the user-facing enum
/// in `terminale-config` but lives here so this crate stays config-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    /// Solid filled block.
    Block,
    /// Hollow rectangle outline.
    OutlineBlock,
    /// Thin horizontal bar at the bottom of the cell.
    Underline,
    /// Thin vertical bar on the left of the cell.
    Beam,
}

/// Parameters controlling how the cursor is rendered every frame.
#[derive(Debug, Clone, Copy)]
pub struct CursorParams {
    /// Shape (Block / OutlineBlock / Underline / Beam).
    pub style: CursorStyle,
    /// Whether the cursor blinks when focused.
    pub blink: bool,
    /// Half-period of the blink animation, in milliseconds.
    pub blink_rate_ms: u32,
    /// sRGB cursor colour. `None` = use the theme palette's `cursor` entry.
    pub color: Option<[u8; 3]>,
    /// Stroke thickness in logical pixels (Underline / Beam / OutlineBlock).
    pub thickness_px: f32,
    /// Cursor fill alpha (0..1).
    pub opacity: f32,
    /// Faint background tint applied to the cell the cursor is on (0..1).
    pub cell_tint_opacity: f32,
    /// When `true` and `blink` is `true`, the cursor alpha is computed as a
    /// `smoothstep` of the blink phase instead of hard on/off switching.
    pub blink_ease: bool,
    /// Target frame rate for the eased blink animation (frames per second).
    /// Only meaningful when `blink_ease` is `true`. Range: 10..=240.
    pub animation_fps: u32,
}

impl Default for CursorParams {
    fn default() -> Self {
        Self {
            style: CursorStyle::Underline,
            blink: false,
            blink_rate_ms: 530,
            color: None,
            thickness_px: 2.0,
            opacity: 1.0,
            cell_tint_opacity: 0.18,
            blink_ease: false,
            animation_fps: 60,
        }
    }
}

/// A rectangle in logical (DPI-independent) pixels.
#[derive(Debug, Clone, Copy)]
pub struct LogicalRect {
    /// Left edge, in logical pixels from the window's left.
    pub x: f32,
    /// Top edge, in logical pixels from the window's top.
    pub y: f32,
    /// Width in logical pixels.
    pub w: f32,
    /// Height in logical pixels.
    pub h: f32,
}

impl LogicalRect {
    fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// Layout result for one frame of the tab bar.
#[derive(Debug)]
struct TabLayout {
    /// (tab pill rect, close-✕ rect) for each tab.
    tabs: Vec<(LogicalRect, LogicalRect)>,
    /// Trailing `+` button.
    plus: LogicalRect,
    /// Minimize button.
    min_btn: LogicalRect,
    /// Maximize / restore button.
    max_btn: LogicalRect,
    /// Close-window button.
    close_btn: LogicalRect,
    /// Chrome-style group-label pills (horizontal bar only). Each entry is
    /// `(pill_rect, first_tab_idx)` — the pill is drawn in the gap to the
    /// left of the first tab of the group run.
    group_pills: Vec<(LogicalRect, usize)>,
}

/// Offsets passed to [`cells_for`] describing the non-terminal chrome
/// surrounding the grid. All values are in logical pixels.
struct GridOffsets {
    /// Everything above the grid: tab bar height + top padding.
    top: f32,
    /// Everything below the grid: status bar + bottom padding.
    bottom: f32,
    /// Left-side tab strip width (`0.0` for Top/Bottom placement).
    left: f32,
    /// Right-side tab strip width (`0.0` for Top/Bottom placement).
    right: f32,
}

/// Pure cols/rows computation from a logical-pixel content area. Split out
/// of [`Renderer::pixels_to_cells`] so the geometry can be unit-tested
/// without a GPU. Subtracts left+right padding for width and the top offset
/// (tab bar + top padding) plus bottom offset (status bar + bottom padding)
/// for height, then floors by the cell size; always at least 1×1.
fn cells_for(
    logical_w: f32,
    logical_h: f32,
    cell_w: f32,
    cell_h: f32,
    padding: f32,
    offsets: GridOffsets,
) -> (u16, u16) {
    let usable_w = (logical_w - padding * 2.0 - offsets.left - offsets.right).max(cell_w);
    let usable_h = (logical_h - offsets.top - offsets.bottom).max(cell_h);
    let cols = (usable_w / cell_w).floor().max(1.0) as u16;
    let rows = (usable_h / cell_h).floor().max(1.0) as u16;
    (cols, rows)
}

/// Compute the sub-cell vertical remainder that `cells_for` leaves unallocated
/// when the available height is not an exact multiple of the cell height.
///
/// This leftover currently accumulates entirely at the bottom of the grid
/// (on top of `bottom_offset`), making the bottom gap visibly larger than the
/// top gap. Half of this value is shifted downward onto the grid origin by
/// [`Renderer::grid_top_shift_px`] so the grid is visually centred within the
/// chrome budget.
///
/// Arguments are in the **same space** (logical or physical — they must be
/// consistent with each other). Returns a value in [0, cell_h).
/// X offset (physical px) that right-aligns the status-bar right text:
/// the measured logical text width is scaled to physical and subtracted
/// from the surface width together with an 8px (logical) right margin.
/// Pure so the alignment arithmetic is unit-testable without wgpu.
fn status_bar_right_tx(surface_w_px: f32, text_w_logical: f32, scale: f32) -> f32 {
    (surface_w_px - text_w_logical * scale - 8.0 * scale).max(0.0)
}

fn vertical_remainder(logical_h: f32, cell_h: f32, top: f32, bottom: f32) -> f32 {
    let usable = (logical_h - top - bottom).max(cell_h);
    let rows = (usable / cell_h).floor().max(1.0);
    (usable - rows * cell_h).max(0.0)
}

/// Logical-pixel geometry of the "Save this SSH host?" toast: the outer
/// card plus its three click targets (checkbox, Save, Dismiss). Split out so
/// both the renderer's draw pass and its hit-test share one source of truth.
#[derive(Debug, Clone, Copy)]
struct SavePromptLayout {
    /// Outer card rectangle.
    card: LogicalRect,
    /// "don't ask again" checkbox box (the tickable square).
    checkbox: LogicalRect,
    /// "Save" button.
    save: LogicalRect,
    /// "Dismiss" button.
    dismiss: LogicalRect,
}

/// Width of the save-host toast card, in logical px.
const SAVE_PROMPT_WIDTH: f32 = 360.0;
/// Height of the save-host toast card, in logical px.
const SAVE_PROMPT_HEIGHT: f32 = 132.0;
/// Inset of the toast from the top edge, in logical px.
const SAVE_PROMPT_TOP: f32 = 48.0;

/// Pure top-centre placement of the save-host toast + its interactive sub
/// rects, from the surface's physical size and DPI `scale`. Centred
/// horizontally, inset [`SAVE_PROMPT_TOP`] from the top. Split out of the
/// renderer so the geometry can be unit-tested without a GPU.
fn save_prompt_layout(width_px: u32, height_px: u32, scale: f32) -> SavePromptLayout {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let _ = height_px;
    let w_log = width_px as f32 / scale;
    let card_w = SAVE_PROMPT_WIDTH.min(w_log - 24.0).max(220.0);
    let card_x = ((w_log - card_w) * 0.5).max(12.0);
    let card = LogicalRect {
        x: card_x,
        y: SAVE_PROMPT_TOP,
        w: card_w,
        h: SAVE_PROMPT_HEIGHT,
    };

    // Checkbox sits on the row above the buttons.
    let pad = 16.0;
    let checkbox = LogicalRect {
        x: card.x + pad,
        y: card.y + 64.0,
        w: 16.0,
        h: 16.0,
    };

    // Two buttons share the bottom row, right-aligned.
    let btn_w = 88.0;
    let btn_h = 30.0;
    let btn_y = card.y + card.h - btn_h - 14.0;
    let dismiss = LogicalRect {
        x: card.x + card.w - btn_w - pad,
        y: btn_y,
        w: btn_w,
        h: btn_h,
    };
    let save = LogicalRect {
        x: dismiss.x - btn_w - 10.0,
        y: btn_y,
        w: btn_w,
        h: btn_h,
    };

    SavePromptLayout {
        card,
        checkbox,
        save,
        dismiss,
    }
}

/// Gap between the group pill and the first tab of the run (logical px).
const GROUP_PILL_GAP: f32 = 4.0;
/// Horizontal padding inside the group pill on each side (logical px).
const GROUP_PILL_PAD_X: f32 = 8.0;

/// Sanitise a user-supplied `(min, max)` tab-width pair into a valid
/// clamp range: each bound is pinned to `[16, 800]` logical px and `max`
/// is raised to at least `min` so [`Renderer::tab_layout`]'s
/// `raw_w.clamp(min, max)` never panics on an inverted range.
fn sanitize_tab_widths(min: f32, max: f32) -> (f32, f32) {
    let min = min.clamp(16.0, 800.0);
    let max = max.clamp(16.0, 800.0).max(min);
    (min, max)
}

/// Pure drop-slot computation: given the laid-out tab pills as
/// `(left_x, width)` pairs (logical px, left-to-right) and a logical cursor
/// x, return the insertion slot in `0..=tabs.len()` — the index the dragged
/// tab would occupy. A drop lands *before* the first tab whose horizontal
/// midpoint is to the right of the cursor; past the last midpoint it
/// appends. Split out of [`Renderer::drop_slot_at`] so the boundary logic is
/// testable without a GPU device.
fn slot_from_midpoints(tabs: &[(f32, f32)], logical_x: f32) -> usize {
    for (idx, (x, w)) in tabs.iter().enumerate() {
        if logical_x < x + w * 0.5 {
            return idx;
        }
    }
    tabs.len()
}

/// Truncate `text` so it fits within `max_chars` Unicode scalar values,
/// appending a U+2026 HORIZONTAL ELLIPSIS when truncated.  The result is
/// always a single-line string — never contains `\n`.
///
/// `max_chars` is an *approximate* character budget (real layout is font-
/// dependent); use a conservative estimate derived from `available_px /
/// approx_char_w`.  Split out of the tab-bar drawing loop so it is
/// testable without a GPU device.
fn truncate_tab_title(text: &str, max_chars: usize) -> String {
    // Strip any embedded newlines so a multi-line custom title never
    // breaks the single-row tab bar.
    let flat: String = text.chars().filter(|&c| c != '\n' && c != '\r').collect();
    let count = flat.chars().count();
    if count <= max_chars || max_chars == 0 {
        flat
    } else {
        // Leave room for the ellipsis itself (1 char).
        let take = max_chars.saturating_sub(1);
        let truncated: String = flat.chars().take(take).collect();
        format!("{truncated}\u{2026}")
    }
}

/// Derive a "slightly darker" background colour for the tab close-button
/// disc.  Multiplies each channel by `factor` (clamped to `[0, 255]`).
fn darken_tab_bg(bg: [u8; 3], factor: f32) -> [u8; 3] {
    [
        ((bg[0] as f32 * factor).round() as u32).min(255) as u8,
        ((bg[1] as f32 * factor).round() as u32).min(255) as u8,
        ((bg[2] as f32 * factor).round() as u32).min(255) as u8,
    ]
}

/// Pick either near-white or near-black text depending on the luminance of
/// `rgb`, so text on top of a group-pill remains legible regardless of accent
/// colour. Uses the BT.601 luma formula.  Returns a [`GlyphonColor`].
///
/// Threshold: luminance > 140 (on 0–255 scale) → near-black; else → white.
fn contrast_text(rgb: [u8; 3]) -> GlyphonColor {
    let luma = 0.299 * rgb[0] as f32 + 0.587 * rgb[1] as f32 + 0.114 * rgb[2] as f32;
    if luma > 140.0 {
        GlyphonColor::rgb(0x1a, 0x1a, 0x1a) // near-black
    } else {
        GlyphonColor::rgb(0xff, 0xff, 0xff) // white
    }
}

/// Blend `tint` over `base` at the given `alpha` (0 = all base, 1 = all tint).
/// Used to apply context-rule tab colours while keeping the chip readable.
fn blend_tint(base: [u8; 3], tint: [u8; 3], alpha: f32) -> [u8; 3] {
    let a = alpha.clamp(0.0, 1.0);
    let b = 1.0 - a;
    [
        ((base[0] as f32 * b + tint[0] as f32 * a).round() as u32).min(255) as u8,
        ((base[1] as f32 * b + tint[1] as f32 * a).round() as u32).min(255) as u8,
        ((base[2] as f32 * b + tint[2] as f32 * a).round() as u32).min(255) as u8,
    ]
}

/// Compute the cursor eased-blink alpha for `elapsed_ms` within a blink cycle
/// of `period_ms` total duration (= `blink_rate_ms * 2`). Returns a value in
/// `[0.0, 1.0]` using a symmetric smoothstep: fades in for the first half of
/// the period, fades out for the second half.
///
/// Exported as a free function so it can be unit-tested without a GPU device.
pub fn cursor_ease_alpha(elapsed_ms: u64, period_ms: u64) -> f32 {
    if period_ms == 0 {
        return 1.0;
    }
    let phase = (elapsed_ms % period_ms) as f32 / period_ms as f32;
    let tri = if phase < 0.5 {
        phase * 2.0
    } else {
        (1.0 - phase) * 2.0
    };
    // smoothstep(0, 1, tri)
    tri * tri * (3.0 - 2.0 * tri)
}

/// Compute the height (in logical pixels) that the tab bar reserves for
/// layout purposes: `TAB_BAR_HEIGHT` when the bar is shown, `0.0` when
/// disabled or hidden by `hide_if_single`. Pure function so it is testable.
pub fn tab_bar_reservation(enabled: bool, hide_if_single: bool, tab_count: usize) -> f32 {
    if !enabled {
        return 0.0;
    }
    if hide_if_single && tab_count <= 1 {
        return 0.0;
    }
    TAB_BAR_HEIGHT
}

/// Apply a cell-width multiplier to a probed monospace advance, clamping the
/// multiplier to `[0.8, 2.0]`.
pub fn apply_cell_width_multiplier(advance: f32, multiplier: f32) -> f32 {
    advance * multiplier.clamp(0.8, 2.0)
}

fn color_close(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    (i32::from(a[0]) - i32::from(b[0])).abs() <= tol
        && (i32::from(a[1]) - i32::from(b[1])).abs() <= tol
        && (i32::from(a[2]) - i32::from(b[2])).abs() <= tol
}

/// Apply SGR 7 (inverse), SGR 2 (dim), and SGR 8 (hidden) visual transforms to
/// a [`CellSnapshot`] after palette resolution, in the correct order:
///
/// 1. **inverse** — swap fg and bg.
/// 2. **dim** — blend the (post-swap) fg toward the (post-swap) bg by `dim_amount`.
///    `dim_amount = 0.0` → no change; `1.0` → fg == bg (invisible).
/// 3. **hidden** — force fg = bg so the glyph is invisible; bg still draws.
///
/// This is a pure function; it does not mutate the original snapshot.
#[must_use]
fn apply_sgr_attributes(mut snap: CellSnapshot, dim_amount: f32) -> CellSnapshot {
    // 1. Inverse: swap fg ↔ bg.
    if snap.inverse {
        std::mem::swap(&mut snap.fg, &mut snap.bg);
    }
    // 2. Dim: blend fg toward bg.
    if snap.dim {
        let t = dim_amount.clamp(0.0, 1.0);
        snap.fg = [
            lerp_u8(snap.fg[0], snap.bg[0], t),
            lerp_u8(snap.fg[1], snap.bg[1], t),
            lerp_u8(snap.fg[2], snap.bg[2], t),
        ];
    }
    // 3. Hidden: make glyph invisible (fg == bg).
    if snap.hidden {
        snap.fg = snap.bg;
    }
    snap
}

/// Linear interpolation between two `u8` colour channel values.
/// `t = 0.0` → `a`, `t = 1.0` → `b`.
#[must_use]
#[inline]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let result = f32::from(a) + (f32::from(b) - f32::from(a)) * t;
    result.round().clamp(0.0, 255.0) as u8
}

// ── WCAG contrast helpers ─────────────────────────────────────────────────────

/// Convert an 8-bit sRGB channel value to linear light (WCAG relative luminance
/// component). Per IEC 61966-2-1 / WCAG 2.1 §1.4.3.
#[must_use]
#[inline]
fn srgb_to_linear(c: u8) -> f32 {
    let s = f32::from(c) / 255.0;
    if s <= 0.040_45 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// Compute WCAG relative luminance for an sRGB colour.
/// Returns a value in `0.0..=1.0`.
#[must_use]
#[inline]
fn relative_luminance(rgb: [u8; 3]) -> f32 {
    0.2126 * srgb_to_linear(rgb[0])
        + 0.7152 * srgb_to_linear(rgb[1])
        + 0.0722 * srgb_to_linear(rgb[2])
}

/// Compute WCAG contrast ratio between two colours.
/// The result is in `1.0..=21.0`; the formula is
/// `(L_lighter + 0.05) / (L_darker + 0.05)`.
#[must_use]
#[inline]
fn wcag_contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    let (lighter, darker) = if la > lb { (la, lb) } else { (lb, la) };
    (lighter + 0.05) / (darker + 0.05)
}

/// Nudge `fg` away from `bg` until their WCAG contrast ratio is `>= target_ratio`
/// or `fg` hits black / white.
///
/// If `target_ratio <= 1.0` or `fg == bg` (concealed text), returns `fg`
/// unchanged. Lightens on a dark background, darkens on a light background,
/// using binary search (16 steps) to find the smallest nudge that satisfies
/// the ratio. Pure function.
#[must_use]
pub fn enforce_min_contrast(fg: [u8; 3], bg: [u8; 3], target_ratio: f32) -> [u8; 3] {
    // Feature disabled or impossible target.
    if target_ratio <= 1.0 {
        return fg;
    }
    // Concealed text (fg == bg intentionally) — skip.
    if fg == bg {
        return fg;
    }
    // Already meets the ratio.
    if wcag_contrast(fg, bg) >= target_ratio {
        return fg;
    }
    // Decide whether to push fg toward white (dark bg) or toward black
    // (light bg), based on bg's luminance.
    let bg_lum = relative_luminance(bg);
    let push_toward: [u8; 3] = if bg_lum < 0.5 {
        [255, 255, 255]
    } else {
        [0, 0, 0]
    };

    // Binary search: find the smallest t in [0,1] such that
    // lerp(fg, push_toward, t) meets the target ratio.
    let mut lo: f32 = 0.0;
    let mut hi: f32 = 1.0;
    let mut best = push_toward; // fallback: fully black/white always meets ratio
    for _ in 0..16 {
        let mid = (lo + hi) * 0.5;
        let candidate = [
            lerp_u8(fg[0], push_toward[0], mid),
            lerp_u8(fg[1], push_toward[1], mid),
            lerp_u8(fg[2], push_toward[2], mid),
        ];
        if wcag_contrast(candidate, bg) >= target_ratio {
            best = candidate;
            hi = mid;
        } else {
            lo = mid;
        }
    }
    best
}

impl Renderer {
    /// Construct a renderer bound to the given window.
    ///
    /// `gpu` selects the graphics backend and power preference and can
    /// request a software (CPU fallback) adapter so users may disable
    /// hardware acceleration. If adapter selection fails with the
    /// requested options, it is retried once with wgpu's defaults (all
    /// backends, no power hint, hardware adapter) and a warning is logged,
    /// so a bad backend choice degrades gracefully instead of failing to
    /// launch.
    ///
    /// # Errors
    ///
    /// Bubbles up failures from wgpu adapter selection, device creation, or
    /// surface configuration.
    pub fn new<W>(
        window: Arc<W>,
        physical_width: u32,
        physical_height: u32,
        scale_factor: f32,
        gpu: GpuOptions,
    ) -> Result<Self, RenderError>
    where
        W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
    {
        let instance = Instance::new(InstanceDescriptor {
            backends: gpu.backends,
            ..Default::default()
        });

        let mut surface = instance.create_surface(Arc::clone(&window))?;

        let mut adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: gpu.power_preference,
            force_fallback_adapter: gpu.force_fallback_adapter,
            compatible_surface: Some(&surface),
        }));

        // Graceful fallback: a forced backend that isn't available on this
        // host, or a CPU-fallback request with no software adapter present,
        // would otherwise fail to launch. Rebuild the instance with every
        // backend and no special hints, recreate the surface against it, and
        // try once more before giving up.
        let instance = if adapter.is_none() {
            tracing::warn!(
                backends = ?gpu.backends,
                power_preference = ?gpu.power_preference,
                force_fallback_adapter = gpu.force_fallback_adapter,
                "no GPU adapter matched the requested [gpu] options; retrying with defaults"
            );
            let fallback = Instance::new(InstanceDescriptor::default());
            surface = fallback.create_surface(window)?;
            adapter = pollster::block_on(fallback.request_adapter(&RequestAdapterOptions {
                power_preference: PowerPreference::default(),
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            }));
            fallback
        } else {
            instance
        };

        let adapter = adapter.ok_or(RenderError::NoAdapter)?;

        // Record which GPU we actually landed on. A weak/software adapter (or an
        // unexpected fallback) is a prime suspect for intermittent freeze-and-
        // recover stalls (GPU TDR / device-lost), so make the choice visible in
        // the log file at startup.
        let adapter_info = adapter.get_info();
        tracing::info!(
            name = %adapter_info.name,
            backend = ?adapter_info.backend,
            device_type = ?adapter_info.device_type,
            driver = %adapter_info.driver,
            driver_info = %adapter_info.driver_info,
            "GPU adapter selected"
        );

        let (device, queue) = pollster::block_on(adapter.request_device(
            &DeviceDescriptor {
                label: Some("terminale device"),
                required_features: wgpu::Features::empty(),
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))?;

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| surface_caps.formats.first().copied())
            .ok_or(RenderError::EmptySurfaceCaps)?;
        let alpha_mode = surface_caps
            .alpha_modes
            .iter()
            .copied()
            .find(|m| *m == CompositeAlphaMode::Opaque)
            .or_else(|| surface_caps.alpha_modes.first().copied())
            .ok_or(RenderError::EmptySurfaceCaps)?;
        let present_mode = surface_caps
            .present_modes
            .iter()
            .copied()
            .find(|m| *m == PresentMode::Mailbox)
            .unwrap_or(PresentMode::AutoVsync);

        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: physical_width.max(1),
            height: physical_height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let mut font_system = FontSystem::new();
        // Make the bundled symbol/emoji fonts available for per-glyph
        // fallback so tab-bar and overlay icons never render as tofu.
        load_symbol_fonts(&mut font_system);
        // Register the curated set of embedded monospace typefaces so they
        // are always selectable in the font picker on any machine.
        bundled_fonts::load_bundled_fonts(&mut font_system);
        let swash_cache = SwashCache::new();
        let glyphon_cache = GlyphonCache::new(&device);
        let viewport = GlyphonViewport::new(&device, &glyphon_cache);
        let mut atlas = TextAtlas::with_color_mode(
            &device,
            &queue,
            &glyphon_cache,
            format,
            ColorMode::Accurate,
        );
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);
        let overlay_text_renderer =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);

        let (cell_width, cell_height) = monospace_cell_size(&mut font_system, DEFAULT_FONT_SIZE);

        let bg = BgPipeline::new(&device, format);
        // The Matrix glyph atlas is built lazily on first use (default-off
        // effect; rasterizing 60 glyphs would tax every window's first frame).
        let bg_fx = BgFxPipeline::new(&device, format);
        let bg_image = BgImagePipeline::new(&device, format);
        let image_blit = image_blit::ImageBlitPipeline::new(&device, format);
        Ok(Self {
            instance: Arc::new(instance),
            adapter: Arc::new(adapter),
            device: Arc::new(device),
            queue: Arc::new(queue),
            surface,
            config,
            surface_format: format,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            bg,
            bg_fx,
            bg_fx_params: BgFxParams::default(),
            bg_fx_start: std::time::Instant::now(),
            bg_fx_pulse_start: None,
            bg_fx_emitters: Vec::new(),
            bg_fx_spawn_counter: 0,
            matrix_atlas_ready: false,
            bg_image,
            bg_image_params: BgImageParams::default(),
            font_size: DEFAULT_FONT_SIZE,
            line_height: DEFAULT_LINE_HEIGHT,
            font_family: String::new(),
            font_bold_family: None,
            font_italic_family: None,
            font_bold_italic_family: None,
            ligatures: true,
            underline_thickness_px: 1.0,
            cell_width,
            cell_height: cell_height * DEFAULT_LINE_HEIGHT,
            scale_factor,
            padding_px: PADDING_PX,
            selection: None,
            selection_scroll: 0,
            selection_history: 0,
            last_history: 0,
            scrollbar_mode: ScrollbarMode::default(),
            scrollbar_hover: false,
            scrollbar_active: false,
            last_scrollbar: None,
            drop_zone: None,
            overlay: None,
            tab_bar: None,
            tab_min_width: TAB_MIN_WIDTH,
            tab_max_width: TAB_MAX_WIDTH,
            tab_pinned_width: TAB_PINNED_WIDTH,
            focused: true,
            cursor: CursorParams::default(),
            cursor_theme_color: None,
            cursor_start: std::time::Instant::now(),
            scroll_lines: 0,
            background_rgb: BACKGROUND_RGB,
            background_alpha: 1.0,
            selection_rgb: [0x33, 0x46, 0x7c],
            selection_opacity: 1.0,
            extra_underlines: Vec::new(),
            prompt_marks: Vec::new(),
            bell_start: None,
            search_overlay: None,
            tooltip: None,
            command_palette: None,
            save_host_prompt: None,
            suggestion_bar: None,
            overlay_text_renderer,
            tab_drag_ghost: None,
            tab_drop_indicator: None,
            pending_body_x: None,
            pending_body_y: None,
            extra_pane_quads: Vec::new(),
            extra_pane_text_cache: std::collections::HashMap::new(),
            extra_pane_cache_seen: Vec::new(),
            focus_border_quads: Vec::new(),
            pane_dim_quads: Vec::new(),
            divider_quads: Vec::new(),
            focus_border_thickness_px: 2.0 * scale_factor,
            focus_border_color: None,
            focus_border_alpha: 0.35,
            broadcast_receiver_ids: Vec::new(),
            pane_header_quads: Vec::new(),
            pane_header_text_buffers: Vec::new(),
            cached_focused_text: Vec::new(),
            focused_text_hash: 0,
            gpu_label: None,
            show_pane_headers: true,
            pane_header_close_hovered: None,
            label_overlays: Vec::new(),
            label_overlay_dim: 0.45,
            snap_chooser: None,
            status_bar: None,
            resource_bar: None,
            tab_bar_enabled: true,
            tab_bar_placement: TabBarPlacement::Top,
            tab_bar_hide_if_single: false,
            vertical_tab_bar_width: 180.0,
            cell_width_multiplier: 1.0,
            close_button_style: terminale_config::CloseButtonStyle::default(),
            image_blit,
            dim_amount: 0.5,
            minimum_contrast: 1.0,
            inactive_pane_dim: 0.0,
            unfocused_window_dim: 0.0,
            jump_highlight_band: None,
            builtin_box_drawing: true,
            show_tab_group_labels: true,
        })
    }

    /// Construct a renderer for an additional native window that **shares**
    /// the wgpu `Instance` / `Adapter` / `Device` / `Queue` of an existing
    /// renderer, but builds its own per-window surface, `FontSystem`,
    /// `SwashCache`, glyphon cache / atlas / text renderers, and background
    /// pipeline. Used for tab tear-out: a torn-off tab opens a brand-new
    /// top-level window whose renderer reuses the single shared GPU device
    /// (no second `Instance::new` / `request_adapter` / `request_device`),
    /// mirroring how the settings / AI egui windows already share the device.
    ///
    /// # Errors
    ///
    /// Bubbles up failures from surface creation or surface configuration.
    pub fn new_shared<W>(
        instance: Arc<Instance>,
        adapter: Arc<wgpu::Adapter>,
        device: Arc<Device>,
        queue: Arc<Queue>,
        window: Arc<W>,
        physical_width: u32,
        physical_height: u32,
        scale_factor: f32,
    ) -> Result<Self, RenderError>
    where
        W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
    {
        let surface = instance.create_surface(window)?;

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| surface_caps.formats.first().copied())
            .ok_or(RenderError::EmptySurfaceCaps)?;
        let alpha_mode = surface_caps
            .alpha_modes
            .iter()
            .copied()
            .find(|m| *m == CompositeAlphaMode::Opaque)
            .or_else(|| surface_caps.alpha_modes.first().copied())
            .ok_or(RenderError::EmptySurfaceCaps)?;
        let present_mode = surface_caps
            .present_modes
            .iter()
            .copied()
            .find(|m| *m == PresentMode::Mailbox)
            .unwrap_or(PresentMode::AutoVsync);

        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: physical_width.max(1),
            height: physical_height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let mut font_system = FontSystem::new();
        // new_shared windows also need symbol icons and bundled monospace
        // typefaces — torn-off / shared windows are first-class, not stripped.
        load_symbol_fonts(&mut font_system);
        bundled_fonts::load_bundled_fonts(&mut font_system);
        let swash_cache = SwashCache::new();
        let glyphon_cache = GlyphonCache::new(&device);
        let viewport = GlyphonViewport::new(&device, &glyphon_cache);
        let mut atlas = TextAtlas::with_color_mode(
            &device,
            &queue,
            &glyphon_cache,
            format,
            ColorMode::Accurate,
        );
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);
        let overlay_text_renderer =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);

        let (cell_width, cell_height) = monospace_cell_size(&mut font_system, DEFAULT_FONT_SIZE);

        let bg = BgPipeline::new(&device, format);
        // The Matrix glyph atlas is built lazily on first use (default-off
        // effect; rasterizing 60 glyphs would tax every window's first frame).
        let bg_fx = BgFxPipeline::new(&device, format);
        let bg_image = BgImagePipeline::new(&device, format);
        let image_blit = image_blit::ImageBlitPipeline::new(&device, format);
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            surface,
            config,
            surface_format: format,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            bg,
            bg_fx,
            bg_fx_params: BgFxParams::default(),
            bg_fx_start: std::time::Instant::now(),
            bg_fx_pulse_start: None,
            bg_fx_emitters: Vec::new(),
            bg_fx_spawn_counter: 0,
            matrix_atlas_ready: false,
            bg_image,
            bg_image_params: BgImageParams::default(),
            font_size: DEFAULT_FONT_SIZE,
            line_height: DEFAULT_LINE_HEIGHT,
            font_family: String::new(),
            font_bold_family: None,
            font_italic_family: None,
            font_bold_italic_family: None,
            ligatures: true,
            underline_thickness_px: 1.0,
            cell_width,
            cell_height: cell_height * DEFAULT_LINE_HEIGHT,
            scale_factor,
            padding_px: PADDING_PX,
            selection: None,
            selection_scroll: 0,
            selection_history: 0,
            last_history: 0,
            scrollbar_mode: ScrollbarMode::default(),
            scrollbar_hover: false,
            scrollbar_active: false,
            last_scrollbar: None,
            drop_zone: None,
            overlay: None,
            tab_bar: None,
            tab_min_width: TAB_MIN_WIDTH,
            tab_max_width: TAB_MAX_WIDTH,
            tab_pinned_width: TAB_PINNED_WIDTH,
            focused: true,
            cursor: CursorParams::default(),
            cursor_theme_color: None,
            cursor_start: std::time::Instant::now(),
            scroll_lines: 0,
            background_rgb: BACKGROUND_RGB,
            background_alpha: 1.0,
            selection_rgb: [0x33, 0x46, 0x7c],
            selection_opacity: 1.0,
            extra_underlines: Vec::new(),
            prompt_marks: Vec::new(),
            bell_start: None,
            search_overlay: None,
            tooltip: None,
            command_palette: None,
            save_host_prompt: None,
            suggestion_bar: None,
            overlay_text_renderer,
            tab_drag_ghost: None,
            tab_drop_indicator: None,
            pending_body_x: None,
            pending_body_y: None,
            extra_pane_quads: Vec::new(),
            extra_pane_text_cache: std::collections::HashMap::new(),
            extra_pane_cache_seen: Vec::new(),
            focus_border_quads: Vec::new(),
            pane_dim_quads: Vec::new(),
            divider_quads: Vec::new(),
            focus_border_thickness_px: 2.0 * scale_factor,
            focus_border_color: None,
            focus_border_alpha: 0.35,
            broadcast_receiver_ids: Vec::new(),
            pane_header_quads: Vec::new(),
            pane_header_text_buffers: Vec::new(),
            cached_focused_text: Vec::new(),
            focused_text_hash: 0,
            gpu_label: None,
            show_pane_headers: true,
            pane_header_close_hovered: None,
            label_overlays: Vec::new(),
            label_overlay_dim: 0.45,
            snap_chooser: None,
            status_bar: None,
            resource_bar: None,
            tab_bar_enabled: true,
            tab_bar_placement: TabBarPlacement::Top,
            tab_bar_hide_if_single: false,
            vertical_tab_bar_width: 180.0,
            cell_width_multiplier: 1.0,
            close_button_style: terminale_config::CloseButtonStyle::default(),
            image_blit,
            dim_amount: 0.5,
            minimum_contrast: 1.0,
            inactive_pane_dim: 0.0,
            unfocused_window_dim: 0.0,
            jump_highlight_band: None,
            builtin_box_drawing: true,
            show_tab_group_labels: true,
        })
    }

    /// Like [`Self::new_shared`] but configures the surface for **transparent
    /// compositing** — prefers a premultiplied/postmultiplied alpha mode so a
    /// borderless top-level window can paint a translucent ghost over the
    /// desktop without an opaque background. Used by the floating tab-drag
    /// ghost window so the pill stays visible while the cursor is dragged
    /// outside any terminal window.
    ///
    /// # Errors
    ///
    /// Bubbles up failures from surface creation or surface configuration.
    pub fn new_shared_transparent<W>(
        instance: Arc<Instance>,
        adapter: Arc<wgpu::Adapter>,
        device: Arc<Device>,
        queue: Arc<Queue>,
        window: Arc<W>,
        physical_width: u32,
        physical_height: u32,
        scale_factor: f32,
    ) -> Result<Self, RenderError>
    where
        W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
    {
        let surface = instance.create_surface(window)?;

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| surface_caps.formats.first().copied())
            .ok_or(RenderError::EmptySurfaceCaps)?;
        // For a transparent ghost window we want compositor-driven alpha
        // blending. Prefer premultiplied alpha (the BgPipeline already emits
        // premultiplied colours, matching `clear_color`), then postmultiplied,
        // and only fall back to `Opaque` if neither is supported — in that
        // case the ghost will appear with its background colour, which is
        // ugly but harmless.
        let alpha_mode = surface_caps
            .alpha_modes
            .iter()
            .copied()
            .find(|m| *m == CompositeAlphaMode::PreMultiplied)
            .or_else(|| {
                surface_caps
                    .alpha_modes
                    .iter()
                    .copied()
                    .find(|m| *m == CompositeAlphaMode::PostMultiplied)
            })
            .or_else(|| surface_caps.alpha_modes.first().copied())
            .ok_or(RenderError::EmptySurfaceCaps)?;
        let present_mode = surface_caps
            .present_modes
            .iter()
            .copied()
            .find(|m| *m == PresentMode::Mailbox)
            .unwrap_or(PresentMode::AutoVsync);

        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: physical_width.max(1),
            height: physical_height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let mut font_system = FontSystem::new();
        load_symbol_fonts(&mut font_system);
        bundled_fonts::load_bundled_fonts(&mut font_system);
        let swash_cache = SwashCache::new();
        let glyphon_cache = GlyphonCache::new(&device);
        let viewport = GlyphonViewport::new(&device, &glyphon_cache);
        let mut atlas = TextAtlas::with_color_mode(
            &device,
            &queue,
            &glyphon_cache,
            format,
            ColorMode::Accurate,
        );
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);
        let overlay_text_renderer =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);

        let (cell_width, cell_height) = monospace_cell_size(&mut font_system, DEFAULT_FONT_SIZE);

        let bg = BgPipeline::new(&device, format);
        // The Matrix glyph atlas is built lazily on first use (default-off
        // effect; rasterizing 60 glyphs would tax every window's first frame).
        let bg_fx = BgFxPipeline::new(&device, format);
        let bg_image = BgImagePipeline::new(&device, format);
        let image_blit = image_blit::ImageBlitPipeline::new(&device, format);
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            surface,
            config,
            surface_format: format,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            bg,
            bg_fx,
            bg_fx_params: BgFxParams::default(),
            bg_fx_start: std::time::Instant::now(),
            bg_fx_pulse_start: None,
            bg_fx_emitters: Vec::new(),
            bg_fx_spawn_counter: 0,
            matrix_atlas_ready: false,
            bg_image,
            bg_image_params: BgImageParams::default(),
            font_size: DEFAULT_FONT_SIZE,
            line_height: DEFAULT_LINE_HEIGHT,
            font_family: String::new(),
            font_bold_family: None,
            font_italic_family: None,
            font_bold_italic_family: None,
            ligatures: true,
            underline_thickness_px: 1.0,
            cell_width,
            cell_height: cell_height * DEFAULT_LINE_HEIGHT,
            scale_factor,
            padding_px: PADDING_PX,
            selection: None,
            selection_scroll: 0,
            selection_history: 0,
            last_history: 0,
            scrollbar_mode: ScrollbarMode::default(),
            scrollbar_hover: false,
            scrollbar_active: false,
            last_scrollbar: None,
            drop_zone: None,
            overlay: None,
            tab_bar: None,
            tab_min_width: TAB_MIN_WIDTH,
            tab_max_width: TAB_MAX_WIDTH,
            tab_pinned_width: TAB_PINNED_WIDTH,
            focused: true,
            cursor: CursorParams::default(),
            cursor_theme_color: None,
            cursor_start: std::time::Instant::now(),
            scroll_lines: 0,
            // Transparent: zero alpha clear so the ghost overlay is the only
            // thing the compositor blends over the desktop.
            background_rgb: [0, 0, 0],
            background_alpha: 0.0,
            selection_rgb: [0x33, 0x46, 0x7c],
            selection_opacity: 1.0,
            extra_underlines: Vec::new(),
            prompt_marks: Vec::new(),
            bell_start: None,
            search_overlay: None,
            tooltip: None,
            command_palette: None,
            save_host_prompt: None,
            suggestion_bar: None,
            overlay_text_renderer,
            tab_drag_ghost: None,
            tab_drop_indicator: None,
            pending_body_x: None,
            pending_body_y: None,
            extra_pane_quads: Vec::new(),
            extra_pane_text_cache: std::collections::HashMap::new(),
            extra_pane_cache_seen: Vec::new(),
            focus_border_quads: Vec::new(),
            pane_dim_quads: Vec::new(),
            divider_quads: Vec::new(),
            focus_border_thickness_px: 2.0 * scale_factor,
            focus_border_color: None,
            focus_border_alpha: 0.35,
            broadcast_receiver_ids: Vec::new(),
            pane_header_quads: Vec::new(),
            pane_header_text_buffers: Vec::new(),
            cached_focused_text: Vec::new(),
            focused_text_hash: 0,
            gpu_label: None,
            show_pane_headers: true,
            pane_header_close_hovered: None,
            label_overlays: Vec::new(),
            label_overlay_dim: 0.45,
            snap_chooser: None,
            status_bar: None,
            resource_bar: None,
            tab_bar_enabled: true,
            tab_bar_placement: TabBarPlacement::Top,
            tab_bar_hide_if_single: false,
            vertical_tab_bar_width: 180.0,
            cell_width_multiplier: 1.0,
            close_button_style: terminale_config::CloseButtonStyle::default(),
            image_blit,
            dim_amount: 0.5,
            minimum_contrast: 1.0,
            inactive_pane_dim: 0.0,
            unfocused_window_dim: 0.0,
            jump_highlight_band: None,
            builtin_box_drawing: true,
            show_tab_group_labels: true,
        })
    }

    /// Render path for the floating tab-drag ghost window: clears the surface
    /// fully transparent and draws **only** the ghost pill (shadow + body +
    /// accent + label) centered in the surface. Called every frame the ghost
    /// window redraws — never reads an [`Emulator`] and never paints a
    /// terminal grid, tab bar, scrollbar or any other chrome.
    ///
    /// No-op (still presents an empty transparent frame) when no
    /// [`TabGhost`] is set, so the surface is correctly drained.
    ///
    /// # Errors
    ///
    /// Bubbles up failures from surface acquisition or text preparation.
    pub fn render_ghost_only(&mut self) -> Result<(), RenderError> {
        let frame = self.acquire_frame()?;
        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("terminale ghost-only frame encoder"),
            });

        let scale = self.scale_factor;
        let w_log = self.config.width as f32 / scale;
        let h_log = self.config.height as f32 / scale;

        let mut quads: Vec<Quad> = Vec::new();
        let mut ghost_text_areas: Vec<(Buffer, [f32; 2])> = Vec::new();

        if let Some(ghost) = self.tab_drag_ghost.clone() {
            let pill_h = TAB_BAR_HEIGHT - 8.0;
            let pill_w = ghost.width;
            // Pill centered in the surface — the App sizes/positions the
            // ghost window so the cursor lands at the same relative spot the
            // user grabbed.
            let cx = w_log * 0.5;
            let cy = h_log * 0.5;
            let x = cx - pill_w * 0.5;
            let y = cy - pill_h * 0.5;

            // Shadow.
            quads.push(Quad::new(
                [(x + 3.0) * scale, (y + 5.0) * scale],
                [pill_w * scale, pill_h * scale],
                [0x00, 0x00, 0x00],
                0.40,
            ));
            // Body.
            quads.push(Quad::new(
                [x * scale, y * scale],
                [pill_w * scale, pill_h * scale],
                [0x1a, 0x20, 0x33],
                0.95,
            ));
            // Accent bar.
            quads.push(Quad::new(
                [x * scale, (y + pill_h - 2.0) * scale],
                [pill_w * scale, 2.0 * scale],
                [0x7d, 0xa6, 0xff],
                1.0,
            ));

            // Label.
            let mut buf = Buffer::new(
                &mut self.font_system,
                Metrics::new(self.font_size * 0.92, self.font_size * 1.1),
            );
            buf.set_size(
                &mut self.font_system,
                Some((pill_w - 24.0).max(0.0)),
                Some(pill_h),
            );
            buf.set_text(
                &mut self.font_system,
                &ghost.label,
                Attrs::new()
                    .family(Family::Monospace)
                    .color(GlyphonColor::rgb(0xe6, 0xea, 0xf8)),
                Shaping::Advanced,
            );
            let text_x = x + 12.0;
            let text_y = y + (pill_h - self.font_size * 1.1) * 0.5;
            ghost_text_areas.push((buf, [text_x, text_y]));
        }

        let viewport_px = [self.config.width as f32, self.config.height as f32];
        self.bg
            .upload(&self.device, &self.queue, viewport_px, &quads);

        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );

        let text_areas_iter = ghost_text_areas.iter().map(|(buf, pos)| TextArea {
            buffer: buf,
            left: pos[0] * scale,
            top: pos[1] * scale,
            scale: self.scale_factor,
            bounds: TextBounds::default(),
            default_color: GlyphonColor::rgb(0xe6, 0xea, 0xf8),
            custom_glyphs: &[],
        });

        self.overlay_text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            text_areas_iter,
            &mut self.swash_cache,
        )?;

        let quad_count = quads.len() as u32;
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("terminale ghost-only pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: Operations {
                        // Fully transparent — the compositor blends only the
                        // ghost overlay over whatever sits behind the window.
                        load: LoadOp::Clear(Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.bg.draw_range(&mut pass, 0..quad_count);
            self.overlay_text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)?;
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        self.atlas.trim();
        Ok(())
    }

    /// Set or clear the centered command-palette modal.
    pub fn set_command_palette(&mut self, palette: Option<CommandPalette>) {
        self.command_palette = palette;
    }

    /// Whether the command palette is currently shown.
    #[must_use]
    pub fn command_palette_open(&self) -> bool {
        self.command_palette.is_some()
    }

    /// Map a physical-pixel click to a command-palette result-row index, if the
    /// click landed on a row. Geometry mirrors `Self::build_command_palette`.
    /// Returns `None` when no palette is open or the click missed the list.
    #[must_use]
    pub fn command_palette_row_at(&self, x_px: f32, y_px: f32) -> Option<usize> {
        let p = self.command_palette.as_ref()?;
        let scale = self.scale_factor;
        let w_log = self.config.width as f32 / scale;
        let h_log = self.config.height as f32 / scale;
        let box_w = (w_log * 0.6).clamp(380.0, 720.0);
        let box_x = (w_log - box_w) * 0.5;
        let box_y = (h_log * 0.12).max(36.0);
        let input_h = 46.0;
        let row_h = 30.0;
        let max_rows = self.palette_visible_rows();
        let total = p.entries.len();
        let first = if p.selected >= max_rows {
            p.selected + 1 - max_rows
        } else {
            0
        };
        let visible = total.min(max_rows);
        let xl = x_px / scale;
        let yl = y_px / scale;
        if xl < box_x || xl > box_x + box_w {
            return None;
        }
        let list_top = box_y + input_h;
        if yl < list_top {
            return None;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let j = ((yl - list_top) / row_h) as usize;
        if j >= visible {
            return None;
        }
        let idx = first + j;
        (idx < total).then_some(idx)
    }

    /// How many list rows the palette shows at once (for page-scroll math
    /// on the caller side).
    #[must_use]
    pub fn palette_visible_rows(&self) -> usize {
        10
    }

    /// Show or hide the "Save this SSH host?" toast.
    pub fn set_save_host_prompt(&mut self, prompt: Option<SaveHostPrompt>) {
        self.save_host_prompt = prompt;
    }

    /// Whether the save-host toast is currently shown.
    #[must_use]
    pub fn save_host_prompt_open(&self) -> bool {
        self.save_host_prompt.is_some()
    }

    /// Hit-test a physical-pixel click against the save-host toast's three
    /// targets (checkbox / Save / Dismiss). `None` when the toast is hidden
    /// or the click missed every target. Mirrors [`Self::tab_hit`].
    #[must_use]
    pub fn save_prompt_hit(&self, x_px: f32, y_px: f32) -> Option<SavePromptHit> {
        self.save_host_prompt.as_ref()?;
        let lx = x_px / self.scale_factor;
        let ly = y_px / self.scale_factor;
        let layout = save_prompt_layout(self.config.width, self.config.height, self.scale_factor);
        if layout.checkbox.contains(lx, ly) {
            Some(SavePromptHit::DontAskAgain)
        } else if layout.save.contains(lx, ly) {
            Some(SavePromptHit::Save)
        } else if layout.dismiss.contains(lx, ly) {
            Some(SavePromptHit::Dismiss)
        } else {
            None
        }
    }

    /// Show or hide the proactive AI command-suggestion bar.
    pub fn set_suggestion_bar(&mut self, bar: Option<SuggestionBar>) {
        self.suggestion_bar = bar;
    }

    /// Whether the suggestion bar is currently shown.
    #[must_use]
    pub fn suggestion_bar_open(&self) -> bool {
        self.suggestion_bar.is_some()
    }

    /// Bottom inset (logical px) consumed by a bottom-anchored status bar and
    /// the resource-indicator strip, so the suggestion bar sits just above them
    /// rather than on top.
    fn suggestion_bottom_inset(&self) -> f32 {
        let sb = match &self.status_bar {
            Some(sb) if sb.at_bottom => STATUS_BAR_HEIGHT,
            _ => 0.0,
        };
        let res = if self.resource_bar.is_some() {
            RESOURCE_BAR_HEIGHT
        } else {
            0.0
        };
        sb + res
    }

    /// Hit-test a physical-pixel click against the suggestion bar's buttons.
    /// `None` when the bar is hidden, the search bar is up (it owns the
    /// bottom), or the click missed every target.
    #[must_use]
    pub fn suggestion_bar_hit(&self, x_px: f32, y_px: f32) -> Option<SuggestionBarHit> {
        let bar = self.suggestion_bar.as_ref()?;
        if self.search_overlay.is_some() {
            return None;
        }
        let has_inject = matches!(
            bar.state,
            SuggestionBarState::Ready { .. } | SuggestionBarState::Hint { .. }
        );
        let scale = if self.scale_factor > 0.0 {
            self.scale_factor
        } else {
            1.0
        };
        let lx = x_px / scale;
        let ly = y_px / scale;
        let layout = suggestion_bar_layout(
            self.config.width,
            self.config.height,
            scale,
            self.suggestion_bottom_inset(),
            has_inject,
        );
        if let Some(inj) = layout.inject {
            if inj.contains(lx, ly) {
                return Some(SuggestionBarHit::Inject);
            }
        }
        if layout.dismiss.contains(lx, ly) {
            return Some(SuggestionBarHit::Dismiss);
        }
        None
    }

    /// Build the proactive AI command-suggestion bar — a clean, flat strip
    /// pinned to the bottom of the window, styled to match the search bar.
    /// Pushes the panel + button background quads into `quads` and returns the
    /// glyphon text areas (label, command, button captions, all positioned in
    /// physical px) for the caller to chain into the text-prepare pass. Returns
    /// empty when the bar is hidden or the search bar is up (they share the
    /// bottom edge).
    fn build_suggestion_bar(
        &mut self,
        scale: f32,
        quads: &mut Vec<Quad>,
    ) -> Vec<(Buffer, [f32; 2])> {
        let mut texts: Vec<(Buffer, [f32; 2])> = Vec::new();
        if self.search_overlay.is_some() {
            return texts;
        }
        // Snapshot the bar state into owned values so `font_system` can be
        // borrowed mutably below without aliasing `self.suggestion_bar`.
        let (has_inject, body) = match self.suggestion_bar.as_ref() {
            None => return texts,
            Some(bar) => match &bar.state {
                SuggestionBarState::Loading { frame } => (false, SuggestionBody::Loading(*frame)),
                SuggestionBarState::Ready { command } => {
                    (true, SuggestionBody::Ready(command.clone()))
                }
                SuggestionBarState::Error { message } => {
                    (false, SuggestionBody::Error(message.clone()))
                }
                SuggestionBarState::Hint { message } => {
                    (true, SuggestionBody::Hint(message.clone()))
                }
            },
        };
        let layout = suggestion_bar_layout(
            self.config.width,
            self.config.height,
            scale,
            self.suggestion_bottom_inset(),
            has_inject,
        );

        // ── Flat panel + single accent line (matches the search bar) ────────
        let bx = layout.bar.x * scale;
        let by = layout.bar.y * scale;
        let bw = layout.bar.w * scale;
        let bh = layout.bar.h * scale;
        let hairline = (1.0 * scale).max(1.0);
        quads.push(Quad::new([bx, by], [bw, bh], [0x18, 0x1c, 0x2c], 0.97));
        quads.push(Quad::new(
            [bx, by],
            [bw, hairline],
            [0x7d, 0xa6, 0xff],
            0.85,
        ));

        // Inject button: flat fill with a subtle accent top hairline.
        if let Some(r) = layout.inject {
            let p = [r.x * scale, r.y * scale];
            let s = [r.w * scale, r.h * scale];
            quads.push(Quad::new(p, s, [0x2d, 0x3b, 0x63], 1.0));
            quads.push(Quad::new(p, [s[0], hairline], [0x7d, 0xa6, 0xff], 0.6));
        }

        // ── Text (normal monospace via glyphon) ─────────────────────────────
        let fs = self.font_size;
        let metrics = Metrics::new(fs, fs * 1.2);
        let ty = by + (bh - fs * 1.1) * 0.5; // vertical centre (mirrors search bar)
        let char_w = (fs * 0.6).max(1.0); // monospace advance estimate (logical)

        let accent = GlyphonColor::rgb(0x7d, 0xa6, 0xff);
        let dim = GlyphonColor::rgb(0x8a, 0x97, 0xbf);
        let bright = GlyphonColor::rgb(0xe6, 0xea, 0xf8);
        let err = GlyphonColor::rgb(0xff, 0x9b, 0x9b);
        let warn = GlyphonColor::rgb(0xf0, 0xc6, 0x74);

        // "AI" label (accent).
        {
            let mut buf = Buffer::new(&mut self.font_system, metrics);
            buf.set_wrap(&mut self.font_system, Wrap::None);
            buf.set_size(&mut self.font_system, Some(bw), Some(bh));
            buf.set_text(
                &mut self.font_system,
                "AI",
                Attrs::new().family(Family::Monospace).color(accent),
                Shaping::Basic,
            );
            texts.push((buf, [12.0 * scale, ty]));
        }

        // Command / status text, truncated to its column.
        let cmd_x = 12.0 + 2.0 * char_w + 12.0;
        let cmd_right = layout.inject.map_or(layout.dismiss.x, |r| r.x) - 10.0;
        let cmd_w = (cmd_right - cmd_x).max(char_w * 2.0);
        let max_chars = (cmd_w / char_w).floor().max(1.0) as usize;
        let (raw, cmd_color) = match &body {
            SuggestionBody::Loading(frame) => {
                (format!("Thinking{}", ".".repeat((frame % 4) as usize)), dim)
            }
            SuggestionBody::Ready(c) => (c.clone(), bright),
            SuggestionBody::Error(m) => (format!("error: {m}"), err),
            SuggestionBody::Hint(m) => (m.clone(), warn),
        };
        let cmd_str = if raw.chars().count() > max_chars && max_chars > 1 {
            let kept: String = raw.chars().take(max_chars - 1).collect();
            format!("{kept}…")
        } else {
            raw
        };
        {
            let mut buf = Buffer::new(&mut self.font_system, metrics);
            buf.set_wrap(&mut self.font_system, Wrap::None);
            buf.set_size(&mut self.font_system, Some(bw), Some(bh));
            buf.set_text(
                &mut self.font_system,
                &cmd_str,
                Attrs::new().family(Family::Monospace).color(cmd_color),
                Shaping::Advanced,
            );
            texts.push((buf, [cmd_x * scale, ty]));
        }

        // Action caption ([Inject] / [Fix]), centred in the button.
        if let Some(r) = layout.inject {
            let cap = if matches!(body, SuggestionBody::Hint(_)) {
                "Fix"
            } else {
                "Inject"
            };
            let cap_w = cap.chars().count() as f32 * char_w;
            let cx = r.x + (r.w - cap_w) * 0.5;
            let mut buf = Buffer::new(&mut self.font_system, metrics);
            buf.set_wrap(&mut self.font_system, Wrap::None);
            buf.set_size(&mut self.font_system, Some(r.w * scale), Some(bh));
            buf.set_text(
                &mut self.font_system,
                cap,
                Attrs::new().family(Family::Monospace).color(bright),
                Shaping::Basic,
            );
            texts.push((buf, [cx * scale, ty]));
        }

        // Dismiss ✕, centred in its button.
        {
            let cx = layout.dismiss.x + (layout.dismiss.w - char_w) * 0.5;
            let mut buf = Buffer::new(&mut self.font_system, metrics);
            buf.set_wrap(&mut self.font_system, Wrap::None);
            buf.set_size(
                &mut self.font_system,
                Some(layout.dismiss.w * scale),
                Some(bh),
            );
            buf.set_text(
                &mut self.font_system,
                "\u{00d7}", // MULTIPLICATION SIGN — universally available, cleaner than ✕
                Attrs::new().family(Family::Monospace).color(dim),
                Shaping::Basic,
            );
            texts.push((buf, [cx * scale, ty]));
        }

        texts
    }

    /// Build the bottom resource-indicator strip: a flat panel with pixel-art
    /// segmented meters for CPU and memory, plus the GPU adapter label. Pushes
    /// the panel + meter quads here and returns the label/percentage text
    /// buffers for the shared text pass. No-op while the strip is disabled.
    fn build_resource_bar(&mut self, scale: f32, quads: &mut Vec<Quad>) -> Vec<(Buffer, [f32; 2])> {
        let mut texts: Vec<(Buffer, [f32; 2])> = Vec::new();
        let Some(content) = self.resource_bar else {
            return texts;
        };

        let h = RESOURCE_BAR_HEIGHT * scale;
        let w = self.config.width as f32;
        let top = self.config.height as f32 - h;
        let hairline = (1.0 * scale).max(1.0);
        // Flat panel + top accent hairline (matches the suggestion bar look).
        quads.push(Quad::new([0.0, top], [w, h], [0x12, 0x15, 0x20], 0.97));
        quads.push(Quad::new(
            [0.0, top],
            [w, hairline],
            [0x2d, 0x3b, 0x63],
            0.9,
        ));

        // Font size is LOGICAL: glyphon's `TextArea.scale` (= scale_factor)
        // multiplies it up to physical pixels at render time. Passing a
        // pre-scaled size here double-applied the scale, so on HiDPI/Retina
        // (scale 2) the labels rendered ~2x too big and overflowed the strip
        // (it looked fine on Windows/Linux at scale 1). Keep the *geometry*
        // below in physical px, but derive the on-screen text metrics from the
        // physical rendered height `fs_px`.
        let fs_logical = 11.0;
        let metrics = Metrics::new(fs_logical, fs_logical * 1.2);
        let fs_px = fs_logical * scale;
        let ty = top + (h - fs_px * 1.15) * 0.5;
        let char_w = (fs_px * 0.6).max(1.0);
        let dim = GlyphonColor::rgb(0x8a, 0x97, 0xbf);
        let bright = GlyphonColor::rgb(0xe6, 0xea, 0xf8);

        // Pixel-art segmented meter geometry.
        const SEGMENTS: usize = 12;
        let seg_w = 5.0 * scale;
        let seg_gap = 2.0 * scale;
        let seg_h = 10.0 * scale;
        let seg_y = top + (h - seg_h) * 0.5;

        let mut x = 12.0 * scale;
        for (label, pct) in [("CPU", content.cpu_pct), ("RAM", content.mem_pct)] {
            let pct = pct.clamp(0.0, 100.0);
            // Label.
            let mut buf = Buffer::new(&mut self.font_system, metrics);
            buf.set_wrap(&mut self.font_system, Wrap::None);
            buf.set_size(&mut self.font_system, Some(w), Some(h));
            buf.set_text(
                &mut self.font_system,
                label,
                Attrs::new().family(Family::Monospace).color(dim),
                Shaping::Basic,
            );
            texts.push((buf, [x, ty]));
            x += 3.0 * char_w + 6.0 * scale;

            // Segmented bar: filled segments coloured by load level.
            let filled = ((pct / 100.0) * SEGMENTS as f32)
                .round()
                .clamp(0.0, SEGMENTS as f32) as usize;
            let fill_col = if pct >= 85.0 {
                [0xff, 0x6b, 0x6b]
            } else if pct >= 60.0 {
                [0xff, 0xcc, 0x66]
            } else {
                [0x7c, 0xe0, 0x9c]
            };
            for i in 0..SEGMENTS {
                let sx = x + i as f32 * (seg_w + seg_gap);
                let col = if i < filled {
                    fill_col
                } else {
                    [0x30, 0x37, 0x4c]
                };
                quads.push(Quad::new([sx, seg_y], [seg_w, seg_h], col, 1.0));
            }
            x += SEGMENTS as f32 * (seg_w + seg_gap) + 6.0 * scale;

            // Percentage.
            let pct_str = format!("{pct:>3.0}%");
            let mut buf = Buffer::new(&mut self.font_system, metrics);
            buf.set_wrap(&mut self.font_system, Wrap::None);
            buf.set_size(&mut self.font_system, Some(w), Some(h));
            buf.set_text(
                &mut self.font_system,
                &pct_str,
                Attrs::new().family(Family::Monospace).color(bright),
                Shaping::Basic,
            );
            texts.push((buf, [x, ty]));
            x += 4.0 * char_w + 18.0 * scale;
        }

        // GPU adapter label (no live meter — utilisation isn't cross-platform).
        // The adapter is fixed for the renderer's lifetime, so resolve the
        // label once and cache it — `get_info()` + `format!` were running on
        // every frame the strip is enabled, for a constant string.
        {
            if self.gpu_label.is_none() {
                let info = self.adapter.get_info();
                self.gpu_label = Some(format!("GPU {} ({:?})", info.name, info.backend));
            }
            let gpu = self.gpu_label.clone().unwrap_or_default();
            let mut buf = Buffer::new(&mut self.font_system, metrics);
            buf.set_wrap(&mut self.font_system, Wrap::None);
            buf.set_size(
                &mut self.font_system,
                Some((w - x - 12.0 * scale).max(char_w)),
                Some(h),
            );
            buf.set_text(
                &mut self.font_system,
                &gpu,
                Attrs::new().family(Family::Monospace).color(dim),
                Shaping::Basic,
            );
            texts.push((buf, [x, ty]));
        }

        texts
    }

    /// Build the quick-select / pane-select label overlay. For each badge in
    /// `self.label_overlays`:
    ///   1. A full-screen dim quad (drawn once, only when `label_overlay_dim >
    ///      0.01`) so the terminal content reads as "inactive".
    ///   2. A small highlight quad at the badge's grid cell.
    ///   3. A glyphon text buffer with the label text (typed prefix dimmed,
    ///      remaining suffix bright).
    ///
    /// All geometry uses the current `body_x_origin` / `body_y_origin` so the
    /// badges anchor correctly inside split-pane sub-rects.
    /// Appends to `quads` (which land in the *overlay* range — drawn after the
    /// main text pass) and to `text_areas` (overlay text renderer pass).
    fn build_label_overlays(
        &mut self,
        scale: f32,
        body_x_origin: f32,
        body_y_origin: f32,
        quads: &mut Vec<Quad>,
        text_areas: &mut Vec<(Buffer, [f32; 2])>,
    ) {
        if self.label_overlays.is_empty() {
            return;
        }

        let w_px = self.config.width as f32;
        let h_px = self.config.height as f32;
        let cw_px = self.cell_width * scale;
        let ch_px = self.cell_height * scale;
        let pad_px = self.padding_px * scale;

        // Full-screen dim behind the badges.
        if self.label_overlay_dim > 0.01 {
            quads.push(Quad::new(
                [0.0, 0.0],
                [w_px, h_px],
                [0x04, 0x06, 0x0b],
                self.label_overlay_dim,
            ));
        }

        // Badge colours — amber palette: yellow/amber chip, white text.
        let badge_bg: [u8; 3] = [0xd4, 0xa0, 0x17]; // amber fill
        let badge_bg_hi: [u8; 3] = [0xff, 0xd7, 0x00]; // gold for the single highlighted match
        let badge_border: [u8; 3] = [0x1a, 0x16, 0x00]; // dark border
        let text_remaining = GlyphonColor::rgb(0x10, 0x10, 0x10); // near-black on amber
        let text_typed = GlyphonColor::rgb(0x50, 0x40, 0x00); // dim brown — already typed

        let bt = (1.0 * scale).max(1.0); // border thickness

        let badges = std::mem::take(&mut self.label_overlays);
        for badge in &badges {
            // Badge rectangle: spans the label characters.
            let label_len = (badge.typed_prefix.chars().count() + badge.remaining.chars().count())
                .max(1) as f32;
            let bw = cw_px * label_len + pad_px * 0.5; // a little horizontal padding
            let bh = ch_px * 0.88;
            // Grid-cell anchor OR physical-pixel centre override (pane-select).
            let (bx, by) = if let Some([cx, cy]) = badge.center_px {
                // `center_px` is a physical-pixel centre; derive top-left corner.
                (cx - bw * 0.5, cy - bh * 0.5)
            } else {
                let col = badge.col as f32;
                let row = badge.row as f32;
                (
                    body_x_origin + pad_px + col * cw_px,
                    body_y_origin + row * ch_px + (ch_px - bh) * 0.5,
                )
            };

            let fill = if badge.highlighted {
                badge_bg_hi
            } else {
                badge_bg
            };

            // Thin dark border, then fill.
            quads.push(Quad::new(
                [bx - bt, by - bt],
                [bw + bt * 2.0, bh + bt * 2.0],
                badge_border,
                0.85,
            ));
            quads.push(Quad::new([bx, by], [bw, bh], fill, 0.95));

            // Build a two-span text buffer: typed prefix (dim) + remaining suffix (bright).
            let label_h = bh / scale;
            let fs_badge = (self.font_size * 0.8).max(8.0);
            let mut buf = Buffer::new(&mut self.font_system, Metrics::new(fs_badge, bh / scale));
            buf.set_size(
                &mut self.font_system,
                Some((bw / scale).max(4.0)),
                Some(label_h),
            );

            let combined = format!("{}{}", badge.typed_prefix, badge.remaining);
            if !badge.typed_prefix.is_empty() && !badge.remaining.is_empty() {
                let spans: &[(&str, Attrs<'_>)] = &[
                    (
                        badge.typed_prefix.as_str(),
                        Attrs::new().family(Family::Monospace).color(text_typed),
                    ),
                    (
                        badge.remaining.as_str(),
                        Attrs::new().family(Family::Monospace).color(text_remaining),
                    ),
                ];
                buf.set_rich_text(
                    &mut self.font_system,
                    spans.iter().copied(),
                    Attrs::new().family(Family::Monospace),
                    Shaping::Basic,
                );
            } else {
                // Single span (either typed prefix only, or remaining only).
                let color = if badge.remaining.is_empty() {
                    text_typed
                } else {
                    text_remaining
                };
                buf.set_text(
                    &mut self.font_system,
                    &combined,
                    Attrs::new().family(Family::Monospace).color(color),
                    Shaping::Basic,
                );
            }

            // Text is in LOGICAL pixels; the overlay text pass scales them.
            let tx = (bx / scale) + 1.0;
            let ty = (by / scale) + (label_h - fs_badge * 0.92) * 0.5;
            text_areas.push((buf, [tx, ty]));
        }
        // Restore the overlays (they were taken; put them back so they stay
        // alive for the next frame if the mode is still active).
        self.label_overlays = badges;
    }

    /// Build the command-palette modal: append its background quads to
    /// `quads` and its text buffers (logical-px positions, scaled at
    /// prepare time) to `text_areas`. Factored out of [`Self::render`] to
    /// keep the hot path readable.
    fn build_command_palette(
        &mut self,
        p: &CommandPalette,
        scale: f32,
        quads: &mut Vec<Quad>,
        text_areas: &mut Vec<(Buffer, [f32; 2])>,
    ) {
        let w_px = self.config.width as f32;
        let h_px = self.config.height as f32;
        let w_log = w_px / scale;
        let h_log = h_px / scale;
        let px = |v: f32| v * scale;

        // Dim the rest of the window so the modal reads as focused.
        quads.push(Quad::new(
            [0.0, 0.0],
            [w_px, h_px],
            [0x04, 0x06, 0x0b],
            0.55,
        ));

        // Panel geometry in logical px.
        let box_w = (w_log * 0.6).clamp(380.0, 720.0);
        let box_x = (w_log - box_w) * 0.5;
        let box_y = (h_log * 0.12).max(36.0);
        let input_h = 46.0;
        let row_h = 30.0;

        let max_rows = self.palette_visible_rows();
        let total = p.entries.len();
        let visible = total.min(max_rows);
        // Scroll the window so the selected row stays on screen.
        let first = if p.selected >= max_rows {
            p.selected + 1 - max_rows
        } else {
            0
        };
        let list_h = visible as f32 * row_h;
        let box_h = input_h + list_h + 10.0;

        let bx = px(box_x);
        let by = px(box_y);
        let bw = px(box_w);
        let bh = px(box_h);
        let bt = (1.0 * scale).max(1.0);

        // Drop shadow, border, panel, accent line, input separator.
        quads.push(Quad::new(
            [bx + px(8.0), by + px(12.0)],
            [bw, bh],
            [0x00, 0x00, 0x00],
            0.40,
        ));
        quads.push(Quad::new(
            [bx - bt, by - bt],
            [bw + bt * 2.0, bh + bt * 2.0],
            [0x3b, 0x42, 0x5a],
            1.0,
        ));
        quads.push(Quad::new([bx, by], [bw, bh], [0x16, 0x19, 0x24], 0.99));
        quads.push(Quad::new(
            [bx, by],
            [bw, (2.0 * scale).max(1.0)],
            [0x7d, 0xa6, 0xff],
            0.9,
        ));
        quads.push(Quad::new(
            [bx, by + px(input_h)],
            [bw, bt],
            [0x2f, 0x35, 0x47],
            1.0,
        ));

        // Input row: prompt + query (or placeholder).
        let prompt = if p.query.is_empty() {
            format!("›  {}", p.placeholder)
        } else {
            format!("›  {}", p.query)
        };
        let query_color = if p.query.is_empty() {
            GlyphonColor::rgb(0x5a, 0x61, 0x78)
        } else {
            GlyphonColor::rgb(0xe6, 0xea, 0xf8)
        };
        let mut qbuf = Buffer::new(
            &mut self.font_system,
            Metrics::new(
                self.font_size * 1.1,
                self.font_size * 1.1 * self.line_height,
            ),
        );
        qbuf.set_size(&mut self.font_system, Some(box_w - 28.0), Some(input_h));
        qbuf.set_text(
            &mut self.font_system,
            &prompt,
            Attrs::new().family(Family::Monospace).color(query_color),
            Shaping::Advanced,
        );
        text_areas.push((
            qbuf,
            [box_x + 16.0, box_y + (input_h - self.font_size * 1.3) * 0.5],
        ));

        // Result count, right-aligned in the input row.
        let count = format!("{} result{}", total, if total == 1 { "" } else { "s" });
        let mut cbuf = Buffer::new(
            &mut self.font_system,
            Metrics::new(
                self.font_size * 0.8,
                self.font_size * 0.8 * self.line_height,
            ),
        );
        cbuf.set_size(&mut self.font_system, Some(180.0), Some(input_h));
        cbuf.set_text(
            &mut self.font_system,
            &count,
            Attrs::new()
                .family(Family::Monospace)
                .color(GlyphonColor::rgb(0x5a, 0x61, 0x78)),
            Shaping::Advanced,
        );
        let count_est = count.chars().count() as f32 * self.cell_width * 0.6;
        text_areas.push((
            cbuf,
            [
                box_x + box_w - count_est - 16.0,
                box_y + (input_h - self.font_size) * 0.5,
            ],
        ));

        // List rows.
        for (vis_idx, entry) in p.entries.iter().skip(first).take(visible).enumerate() {
            let idx = first + vis_idx;
            let row_y = box_y + input_h + vis_idx as f32 * row_h;
            let row_y_px = px(row_y);
            if idx == p.selected {
                quads.push(Quad::new(
                    [bx, row_y_px],
                    [bw, px(row_h)],
                    [0x2c, 0x3c, 0x6a],
                    0.9,
                ));
                quads.push(Quad::new(
                    [bx, row_y_px],
                    [px(3.0), px(row_h)],
                    [0x7d, 0xa6, 0xff],
                    1.0,
                ));
            }
            let label_color = if idx == p.selected {
                GlyphonColor::rgb(0xff, 0xff, 0xff)
            } else {
                GlyphonColor::rgb(0xcf, 0xd6, 0xea)
            };
            let text_y = row_y + (row_h - self.font_size * self.line_height) * 0.5;
            let mut lbuf = Buffer::new(
                &mut self.font_system,
                Metrics::new(self.font_size, self.font_size * self.line_height),
            );
            lbuf.set_size(&mut self.font_system, Some(box_w - 130.0), Some(row_h));
            lbuf.set_text(
                &mut self.font_system,
                &entry.label,
                Attrs::new().family(Family::Monospace).color(label_color),
                Shaping::Advanced,
            );
            text_areas.push((lbuf, [box_x + 18.0, text_y]));

            if !entry.binding.is_empty() {
                let mut bbuf = Buffer::new(
                    &mut self.font_system,
                    Metrics::new(
                        self.font_size * 0.82,
                        self.font_size * 0.82 * self.line_height,
                    ),
                );
                bbuf.set_size(&mut self.font_system, Some(box_w - 24.0), Some(row_h));
                bbuf.set_text(
                    &mut self.font_system,
                    &entry.binding,
                    Attrs::new()
                        .family(Family::Monospace)
                        .color(GlyphonColor::rgb(0x80, 0x88, 0x9e)),
                    Shaping::Advanced,
                );
                let est = entry.binding.chars().count() as f32 * self.cell_width * 0.62;
                let hot_x = box_x + box_w - est - 16.0;
                text_areas.push((bbuf, [hot_x, text_y + 1.0]));
            }
        }
    }

    /// Draw the snap-layout chooser overlay into the quad + text lists.
    ///
    /// The chooser is a centred panel showing a 3×4 grid of layout cells
    /// (left, right, top, bottom, four quarters, center, maximize). Each cell
    /// has a small preview icon drawn with box-drawing characters and a short
    /// text label below it.  The hovered cell (from mouse motion) is
    /// highlighted in the accent colour.
    fn build_snap_chooser(
        &mut self,
        chooser: &SnapChooserOverlay,
        scale: f32,
        quads: &mut Vec<Quad>,
        text_areas: &mut Vec<(Buffer, [f32; 2])>,
    ) {
        let win_w = self.config.width as f32;
        let win_h = self.config.height as f32;

        // Full-screen dim scrim.
        quads.push(Quad::new(
            [0.0, 0.0],
            [win_w, win_h],
            [0x04, 0x06, 0x0b],
            0.60,
        ));

        let (px, py, pw, ph) = snap_chooser_geometry(win_w, win_h, scale);

        // Panel shadow.
        let shadow_off = 6.0 * scale;
        quads.push(Quad::new(
            [px + shadow_off, py + shadow_off],
            [pw, ph],
            [0x00, 0x00, 0x00],
            0.45,
        ));

        // Panel background.
        quads.push(Quad::new([px, py], [pw, ph], [0x11, 0x16, 0x22], 0.97));

        // Panel border.
        let bt = (1.5 * scale).max(1.0);
        quads.push(Quad::new(
            [px - bt, py - bt],
            [pw + bt * 2.0, ph + bt * 2.0],
            [0x3a, 0x4a, 0x70],
            0.80,
        ));

        // Title text: "Snap Layout"
        let title_fs = (self.font_size * 0.9).max(9.0);
        let mut title_buf = Buffer::new(
            &mut self.font_system,
            Metrics::new(title_fs, SNAP_CHOOSER_HEADER_H),
        );
        title_buf.set_size(
            &mut self.font_system,
            Some(pw / scale),
            Some(SNAP_CHOOSER_HEADER_H),
        );
        title_buf.set_text(
            &mut self.font_system,
            "Snap Layout",
            Attrs::new()
                .family(Family::Monospace)
                .color(GlyphonColor::rgb(0xb0, 0xc0, 0xe8)),
            Shaping::Advanced,
        );
        let title_x = px / scale + SNAP_CHOOSER_PAD;
        let title_y = py / scale + (SNAP_CHOOSER_HEADER_H - title_fs) * 0.5;
        text_areas.push((title_buf, [title_x, title_y]));

        // Divider below title.
        quads.push(Quad::new(
            [px, py + SNAP_CHOOSER_HEADER_H * scale],
            [pw, bt],
            [0x3a, 0x4a, 0x70],
            0.50,
        ));

        let cols = 3usize;
        let cell_w = SNAP_CHOOSER_CELL_W * scale;
        let cell_h = SNAP_CHOOSER_CELL_H * scale;
        let gap = SNAP_CHOOSER_GAP * scale;
        let rows = SNAP_CHOOSER_CELLS.len().div_ceil(cols);

        for row in 0..rows {
            for col in 0..cols {
                let idx = row * cols + col;
                if idx >= SNAP_CHOOSER_CELLS.len() {
                    break;
                }
                let cell = SNAP_CHOOSER_CELLS[idx];
                let cx = px + SNAP_CHOOSER_PAD * scale + col as f32 * (cell_w + gap);
                let cy = py + SNAP_CHOOSER_HEADER_H * scale + row as f32 * (cell_h + gap);

                let is_hovered = chooser.hovered == Some(idx);

                // Cell background.
                let (cell_bg, cell_alpha): ([u8; 3], f32) = if is_hovered {
                    ([0x7d, 0xa6, 0xff], 0.25)
                } else {
                    ([0x1e, 0x26, 0x3a], 0.90)
                };
                // Cell border.
                let border_col: [u8; 3] = if is_hovered {
                    [0x7d, 0xa6, 0xff]
                } else {
                    [0x2e, 0x3c, 0x5a]
                };
                let bbt = (1.0 * scale).max(1.0);
                quads.push(Quad::new(
                    [cx - bbt, cy - bbt],
                    [cell_w + bbt * 2.0, cell_h + bbt * 2.0],
                    border_col,
                    0.80,
                ));
                quads.push(Quad::new([cx, cy], [cell_w, cell_h], cell_bg, cell_alpha));

                // Draw a small preview diagram inside the cell using quads.
                // The diagram is a tiny rectangle showing where the window
                // would sit on a miniature monitor outline.
                let diag_margin = 6.0 * scale;
                let diag_w = cell_w - diag_margin * 2.0;
                let diag_h = (cell_h * 0.48).max(4.0);
                let diag_x = cx + diag_margin;
                let diag_y = cy + (cell_h * 0.5 - diag_h) * 0.5;

                // Monitor outline (dim grey rectangle).
                quads.push(Quad::new(
                    [diag_x, diag_y],
                    [diag_w, diag_h],
                    [0x30, 0x38, 0x50],
                    0.80,
                ));

                // Window-placement preview quad (accent colour, portion of the monitor).
                let (wx, wy, ww, wh): (f32, f32, f32, f32) = match cell {
                    SnapChooserCell::Left => (diag_x, diag_y, diag_w * 0.5, diag_h),
                    SnapChooserCell::Right => (diag_x + diag_w * 0.5, diag_y, diag_w * 0.5, diag_h),
                    SnapChooserCell::Top => (diag_x, diag_y, diag_w, diag_h * 0.5),
                    SnapChooserCell::Bottom => {
                        (diag_x, diag_y + diag_h * 0.5, diag_w, diag_h * 0.5)
                    }
                    SnapChooserCell::TopLeft => (diag_x, diag_y, diag_w * 0.5, diag_h * 0.5),
                    SnapChooserCell::TopRight => {
                        (diag_x + diag_w * 0.5, diag_y, diag_w * 0.5, diag_h * 0.5)
                    }
                    SnapChooserCell::BottomLeft => {
                        (diag_x, diag_y + diag_h * 0.5, diag_w * 0.5, diag_h * 0.5)
                    }
                    SnapChooserCell::BottomRight => (
                        diag_x + diag_w * 0.5,
                        diag_y + diag_h * 0.5,
                        diag_w * 0.5,
                        diag_h * 0.5,
                    ),
                    SnapChooserCell::Center => {
                        let sw = diag_w * 0.6;
                        let sh = diag_h * 0.6;
                        (
                            diag_x + (diag_w - sw) * 0.5,
                            diag_y + (diag_h - sh) * 0.5,
                            sw,
                            sh,
                        )
                    }
                    SnapChooserCell::Maximize => (diag_x, diag_y, diag_w, diag_h),
                };
                let preview_col: [u8; 3] = if is_hovered {
                    [0x7d, 0xa6, 0xff]
                } else {
                    [0x50, 0x70, 0xb0]
                };
                quads.push(Quad::new(
                    [wx, wy],
                    [ww.max(1.0), wh.max(1.0)],
                    preview_col,
                    0.90,
                ));

                // Label text below the diagram.
                let label_fs = (self.font_size * 0.72).max(7.0);
                let label_h_log = cell_h / scale * 0.40;
                let mut lbuf =
                    Buffer::new(&mut self.font_system, Metrics::new(label_fs, label_h_log));
                lbuf.set_size(
                    &mut self.font_system,
                    Some(cell_w / scale),
                    Some(label_h_log),
                );
                let label_col = if is_hovered {
                    GlyphonColor::rgb(0xe0, 0xec, 0xff)
                } else {
                    GlyphonColor::rgb(0x80, 0x98, 0xc8)
                };
                lbuf.set_text(
                    &mut self.font_system,
                    cell.label(),
                    Attrs::new().family(Family::Monospace).color(label_col),
                    Shaping::Advanced,
                );
                let lx = cx / scale;
                let ly = (cy + cell_h * 0.60) / scale;
                text_areas.push((lbuf, [lx, ly]));
            }
        }

        // "Press Esc to close" hint at the bottom.
        let hint_fs = (self.font_size * 0.72).max(7.0);
        let hint_h_log = SNAP_CHOOSER_PAD;
        let mut hint_buf = Buffer::new(&mut self.font_system, Metrics::new(hint_fs, hint_h_log));
        hint_buf.set_size(&mut self.font_system, Some(pw / scale), Some(hint_h_log));
        hint_buf.set_text(
            &mut self.font_system,
            "Click a layout to apply  \u{00b7}  Esc to close",
            Attrs::new()
                .family(Family::Monospace)
                .color(GlyphonColor::rgb(0x50, 0x60, 0x88)),
            Shaping::Advanced,
        );
        let hint_x = px / scale + SNAP_CHOOSER_PAD;
        let hints_row_y = rows as f32 * (SNAP_CHOOSER_CELL_H + SNAP_CHOOSER_GAP) - SNAP_CHOOSER_GAP;
        let hint_y = py / scale + SNAP_CHOOSER_HEADER_H + hints_row_y + SNAP_CHOOSER_PAD * 0.4;
        text_areas.push((hint_buf, [hint_x, hint_y]));
    }

    /// Draw the bottom-right SSH quick-connect button into the overlay layer:
    /// Draw the top-centre "Save this SSH host?" toast into the overlay
    /// layer: a bordered card with a title, the endpoint, a "don't ask
    /// again" checkbox, and Save / Dismiss buttons. Background quads go to
    /// `quads`; labels (logical-px positions) go to `text_areas`.
    fn build_save_host_prompt(
        &mut self,
        prompt: &SaveHostPrompt,
        scale: f32,
        quads: &mut Vec<Quad>,
        text_areas: &mut Vec<(Buffer, [f32; 2])>,
    ) {
        let layout = save_prompt_layout(self.config.width, self.config.height, scale);
        let px = |v: f32| v * scale;
        let bt = (1.0 * scale).max(1.0);
        let c = layout.card;

        // Drop shadow → accent border → panel.
        quads.push(Quad::new(
            [px(c.x) + px(6.0), px(c.y) + px(8.0)],
            [px(c.w), px(c.h)],
            [0x00, 0x00, 0x00],
            0.40,
        ));
        quads.push(Quad::new(
            [px(c.x) - bt, px(c.y) - bt],
            [px(c.w) + bt * 2.0, px(c.h) + bt * 2.0],
            [0x3b, 0x42, 0x5a],
            1.0,
        ));
        quads.push(Quad::new(
            [px(c.x), px(c.y)],
            [px(c.w), px(c.h)],
            [0x16, 0x19, 0x24],
            0.99,
        ));
        // Top accent line.
        quads.push(Quad::new(
            [px(c.x), px(c.y)],
            [px(c.w), (2.0 * scale).max(1.0)],
            [0x7d, 0xa6, 0xff],
            0.9,
        ));

        // Title + endpoint text.
        let mut title = Buffer::new(
            &mut self.font_system,
            Metrics::new(
                self.font_size * 1.05,
                self.font_size * 1.05 * self.line_height,
            ),
        );
        title.set_size(&mut self.font_system, Some(c.w - 24.0), Some(28.0));
        title.set_text(
            &mut self.font_system,
            "Save this SSH host?",
            Attrs::new()
                .family(Family::SansSerif)
                .color(GlyphonColor::rgb(0xe6, 0xea, 0xf8)),
            Shaping::Advanced,
        );
        text_areas.push((title, [c.x + 16.0, c.y + 12.0]));

        let mut ep = Buffer::new(
            &mut self.font_system,
            Metrics::new(self.font_size, self.font_size * self.line_height),
        );
        ep.set_size(&mut self.font_system, Some(c.w - 24.0), Some(24.0));
        ep.set_text(
            &mut self.font_system,
            &prompt.endpoint,
            Attrs::new()
                .family(Family::Monospace)
                .color(GlyphonColor::rgb(0x9d, 0xb0, 0xd8)),
            Shaping::Advanced,
        );
        text_areas.push((ep, [c.x + 16.0, c.y + 36.0]));

        // "don't ask again" checkbox: a 16px box, ticked with a filled inner
        // square when checked, plus its label.
        let cb = layout.checkbox;
        quads.push(Quad::new(
            [px(cb.x) - bt, px(cb.y) - bt],
            [px(cb.w) + bt * 2.0, px(cb.h) + bt * 2.0],
            [0x4a, 0x53, 0x70],
            1.0,
        ));
        quads.push(Quad::new(
            [px(cb.x), px(cb.y)],
            [px(cb.w), px(cb.h)],
            [0x0d, 0x10, 0x18],
            1.0,
        ));
        if prompt.dont_ask_again {
            quads.push(Quad::new(
                [px(cb.x + 3.0), px(cb.y + 3.0)],
                [px(cb.w - 6.0), px(cb.h - 6.0)],
                [0x7d, 0xa6, 0xff],
                1.0,
            ));
        }
        let mut cb_label = Buffer::new(
            &mut self.font_system,
            Metrics::new(
                self.font_size * 0.9,
                self.font_size * 0.9 * self.line_height,
            ),
        );
        cb_label.set_size(&mut self.font_system, Some(c.w - 24.0), Some(20.0));
        cb_label.set_text(
            &mut self.font_system,
            "Don't ask again",
            Attrs::new()
                .family(Family::SansSerif)
                .color(GlyphonColor::rgb(0xb6, 0xbf, 0xd6)),
            Shaping::Advanced,
        );
        text_areas.push((cb_label, [cb.x + cb.w + 8.0, cb.y - 1.0]));

        // Save (accent) + Dismiss (muted) buttons.
        for (rect, label, bg, fg) in [
            (
                layout.save,
                "Save",
                [0x2c, 0x3c, 0x6a],
                GlyphonColor::rgb(0xff, 0xff, 0xff),
            ),
            (
                layout.dismiss,
                "Dismiss",
                [0x20, 0x25, 0x33],
                GlyphonColor::rgb(0xc0, 0xc8, 0xdc),
            ),
        ] {
            quads.push(Quad::new(
                [px(rect.x) - bt, px(rect.y) - bt],
                [px(rect.w) + bt * 2.0, px(rect.h) + bt * 2.0],
                [0x3b, 0x42, 0x5a],
                1.0,
            ));
            quads.push(Quad::new(
                [px(rect.x), px(rect.y)],
                [px(rect.w), px(rect.h)],
                bg,
                1.0,
            ));
            let mut lbl = Buffer::new(
                &mut self.font_system,
                Metrics::new(self.font_size, self.font_size * self.line_height),
            );
            lbl.set_size(&mut self.font_system, Some(rect.w), Some(rect.h));
            lbl.set_text(
                &mut self.font_system,
                label,
                Attrs::new().family(Family::SansSerif).color(fg),
                Shaping::Advanced,
            );
            // Roughly centre the label in the button.
            let est = label.chars().count() as f32 * self.font_size * 0.5;
            let tx = rect.x + (rect.w - est) * 0.5;
            text_areas.push((
                lbl,
                [
                    tx.max(rect.x + 6.0),
                    rect.y + (rect.h - self.font_size * self.line_height) * 0.5,
                ],
            ));
        }
    }

    /// Set or clear the bottom-of-window search bar.
    pub fn set_search_overlay(&mut self, overlay: Option<SearchOverlay>) {
        self.search_overlay = overlay;
    }

    /// Set or clear the floating tooltip drawn near the cursor.
    pub fn set_tooltip(&mut self, tooltip: Option<Tooltip>) {
        self.tooltip = tooltip;
    }

    /// Fire the visual bell. Decays over a couple of hundred ms.
    pub fn trigger_visual_bell(&mut self) {
        self.bell_start = Some(std::time::Instant::now());
    }

    /// True while the visual bell is still animating — callers can use
    /// this to know they need to schedule another redraw.
    #[must_use]
    pub fn bell_active(&self) -> bool {
        match self.bell_start {
            Some(t) => t.elapsed().as_millis() < u128::from(BELL_DURATION_MS),
            None => false,
        }
    }

    /// Replace the list of "extra" underlined cell ranges. Used to
    /// underline autodetected URLs and search-match highlights without
    /// going through the per-cell `has_link` mechanism.
    pub fn set_extra_underlines(&mut self, ranges: Vec<(u16, u16, u16)>) {
        self.extra_underlines = ranges;
    }

    /// Replace the list of prompt-status gutter marks. Each entry is
    /// `(viewport_row, exit_code)` where `exit_code` is `None` when the
    /// exit status is unknown, `Some(0)` for success, or `Some(n)` for
    /// failure. Pass an empty slice to clear all marks (e.g. when
    /// `show_prompt_marks` is disabled or the emulator has no marks).
    pub fn set_prompt_marks(&mut self, marks: Vec<(u16, Option<u32>)>) {
        self.prompt_marks = marks;
    }

    /// Replace the quick-select / pane-select label badges drawn on top of the
    /// terminal this frame. Pass an empty `Vec` (or call with `&[]`) to clear
    /// the overlay when the mode exits. The `dim` value is the opacity of the
    /// full-screen tint layer drawn behind the badges — `0.0` = none, `1.0` =
    /// fully opaque. Mirrors `quick_select.overlay_dim` from the config.
    pub fn set_label_overlays(&mut self, badges: Vec<LabelBadge>, dim: f32) {
        self.label_overlays = badges;
        self.label_overlay_dim = dim.clamp(0.0, 1.0);
    }

    /// Set the jump-highlight band for the current frame.
    ///
    /// `row` is the **viewport** row (0-based) to highlight; `alpha` is the
    /// current opacity (0.0 = transparent, fully faded; 1.0 = peak brightness).
    /// Pass `None` to clear the highlight (it has expired or is disabled).
    pub fn set_jump_highlight_band(&mut self, band: Option<(u16, f32)>) {
        self.jump_highlight_band = band;
    }

    /// Returns `true` while the jump highlight is still animating. Used by
    /// `about_to_wait` to know whether a frame should be scheduled.
    #[must_use]
    pub fn jump_highlight_active(&self) -> bool {
        self.jump_highlight_band
            .is_some_and(|(_, alpha)| alpha > 0.0)
    }

    /// Show or hide the snap-layout chooser overlay.  Pass `Some` to open it
    /// (with the initial hovered cell unset); `None` to close it.
    pub fn set_snap_chooser(&mut self, state: Option<SnapChooserOverlay>) {
        self.snap_chooser = state;
    }

    /// Update the hovered cell index inside the currently-open snap-layout
    /// chooser (no-op when the chooser is closed).
    pub fn set_snap_chooser_hovered(&mut self, hovered: Option<usize>) {
        if let Some(c) = self.snap_chooser.as_mut() {
            c.hovered = hovered;
        }
    }

    /// Hit-test a physical-pixel pointer position against the snap-layout
    /// chooser cells.  Returns the index into [`SNAP_CHOOSER_CELLS`] when the
    /// point is inside a cell, or `None` when it misses.
    ///
    /// Geometry is computed to match `Self::build_snap_chooser` exactly.
    #[must_use]
    pub fn snap_chooser_hit(&self, px: f32, py: f32) -> Option<usize> {
        let chooser = self.snap_chooser.as_ref()?;
        let _ = chooser; // existence check
        let scale = self.scale_factor;
        let (overlay_x, overlay_y, _overlay_w, _overlay_h) =
            snap_chooser_geometry(self.config.width as f32, self.config.height as f32, scale);
        let cols: usize = 3;
        let cell_w = SNAP_CHOOSER_CELL_W * scale;
        let cell_h = SNAP_CHOOSER_CELL_H * scale;
        let gap = SNAP_CHOOSER_GAP * scale;
        let row_count = SNAP_CHOOSER_CELLS.len().div_ceil(cols);
        for row in 0..row_count {
            for col in 0..cols {
                let idx = row * cols + col;
                if idx >= SNAP_CHOOSER_CELLS.len() {
                    break;
                }
                let cx = overlay_x + SNAP_CHOOSER_PAD * scale + col as f32 * (cell_w + gap);
                let cy = overlay_y + SNAP_CHOOSER_HEADER_H * scale + row as f32 * (cell_h + gap);
                if px >= cx && px < cx + cell_w && py >= cy && py < cy + cell_h {
                    return Some(idx);
                }
            }
        }
        None
    }

    /// Returns `true` when the snap-layout chooser overlay is currently open.
    #[must_use]
    pub fn snap_chooser_open(&self) -> bool {
        self.snap_chooser.is_some()
    }

    /// Window clear colour (also used as the "neutral" background for ANSI
    /// cells whose bg matches it — those get skipped to keep overdraw low).
    pub fn set_background_color(&mut self, rgb: [u8; 3]) {
        self.background_rgb = rgb;
    }

    /// Background alpha in `[0,1]`. `1.0` = fully opaque, anything less
    /// lets the desktop wallpaper show through (assumes the OS window
    /// supports translucent compositing).
    pub fn set_background_alpha(&mut self, alpha: f32) {
        self.background_alpha = alpha.clamp(0.0, 1.0);
    }

    /// Update the animated background-effect parameters (style, intensity,
    /// speed, tints). Live-applied from the settings window / config reload.
    ///
    /// Shader mode `3` is the Matrix "digital rain", the only mode that
    /// samples the glyph atlas — which is rasterized + uploaded here on first
    /// use (the pipeline ships with a placeholder texture until then).
    pub fn set_bg_fx_params(&mut self, params: BgFxParams) {
        if params.enabled && params.mode == 3 && !self.matrix_atlas_ready {
            let a = build_matrix_atlas(&mut self.font_system, &mut self.swash_cache);
            self.bg_fx.set_glyph_atlas(
                &self.device,
                &self.queue,
                &a.data,
                a.width,
                a.height,
                a.cols,
                a.rows,
                a.count,
            );
            self.matrix_atlas_ready = true;
        }
        self.bg_fx_params = params;
    }

    /// Set (or clear) the background image and its render params. When
    /// `params.path` is `None` or `opacity == 0`, no quad is drawn. The
    /// texture is reloaded only when the path changes; params (opacity, fit,
    /// HSB) can be updated without re-decoding the file.
    pub fn set_background_image(&mut self, params: BgImageParams) {
        self.bg_image_params = params;
    }

    /// Spawn a new per-keystroke emitter band at the given normalised column
    /// (`0.0..=1.0`). Each call pushes a new independent band that travels and
    /// decays on its own — concurrent bands accumulate visually. The ring is
    /// capped at [`MAX_EMITTERS`]; if full the oldest entry is evicted first.
    ///
    /// Also resets the global pulse used by Aurora / Starfield / PixelCRT for
    /// their background-wide flare so those modes still react to keystrokes.
    pub fn spawn_bg_fx_emitter(&mut self, col: f32) {
        // Global pulse for non-Matrix modes.
        self.bg_fx_pulse_start = Some(std::time::Instant::now());

        let t = self.bg_fx_start.elapsed().as_secs_f32() * self.bg_fx_params.speed;
        let counter = self.bg_fx_spawn_counter;
        self.bg_fx_spawn_counter = counter.wrapping_add(1);

        // Simple integer hash for the seed to diversify concurrent bands.
        let h = counter.wrapping_mul(0x9e37_9e37).wrapping_add(0x6c62_272e) ^ (counter >> 16);
        #[allow(clippy::cast_precision_loss)]
        let seed = (h as f32) / (u32::MAX as f32);

        let emitter = CpuEmitter {
            birth: t,
            col: col.clamp(0.0, 1.0),
            seed,
            kind: self.bg_fx_params.mode as f32,
        };

        let cap = (self.bg_fx_params.max_emitters as usize).clamp(1, MAX_EMITTERS);
        if self.bg_fx_emitters.len() >= cap {
            self.bg_fx_emitters.remove(0); // evict oldest to stay within cap
        }
        self.bg_fx_emitters.push(emitter);
    }

    /// Seed multiple emitters with explicit column positions and age offsets for
    /// deterministic demo / screenshot scenarios. Each `(col, age_secs)` pair
    /// places a band at column `col` with an apparent age of `age_secs` so the
    /// very first frame shows bands already mid-fall at staggered heights.
    pub fn seed_bg_fx_demo(&mut self, entries: &[(f32, f32)]) {
        let t_now = self.bg_fx_start.elapsed().as_secs_f32() * self.bg_fx_params.speed;
        for (i, (col, age)) in entries.iter().enumerate() {
            let counter = self.bg_fx_spawn_counter;
            self.bg_fx_spawn_counter = counter.wrapping_add(1);
            let h = counter.wrapping_mul(0x9e37_9e37).wrapping_add(0x6c62_272e) ^ (counter >> 16);
            #[allow(clippy::cast_precision_loss)]
            let seed = (h as f32) / (u32::MAX as f32);
            let _ = i;
            let emitter = CpuEmitter {
                birth: t_now - age.max(0.0),
                col: col.clamp(0.0, 1.0),
                seed,
                kind: self.bg_fx_params.mode as f32,
            };
            let cap = (self.bg_fx_params.max_emitters as usize).clamp(1, MAX_EMITTERS);
            if self.bg_fx_emitters.len() >= cap {
                self.bg_fx_emitters.remove(0);
            }
            self.bg_fx_emitters.push(emitter);
        }
    }

    /// Back-compat wrapper — spawns an emitter at a column derived from the
    /// current spawn counter (evenly spread). Existing callers that do not
    /// pass a cursor column continue to work.
    pub fn pulse_bg_fx(&mut self) {
        #[allow(clippy::cast_precision_loss)]
        let col = {
            let c = self.bg_fx_spawn_counter;
            // Spread columns across the screen using the golden ratio.
            (c as f32 * 0.618_033_9) % 1.0
        };
        self.spawn_bg_fx_emitter(col);
    }

    /// Remove emitters whose age in scaled time exceeds `lifetime_secs * 1.1`
    /// (a small grace window so the shader's own fade completes). Called each
    /// frame so the emitter list stays bounded and `bg_fx_active` returns
    /// `false` once all bands have finished.
    pub fn prune_bg_fx_emitters(&mut self, lifetime_secs: f32) {
        let now = self.bg_fx_start.elapsed().as_secs_f32() * self.bg_fx_params.speed;
        self.bg_fx_emitters
            .retain(|e| (now - e.birth) < lifetime_secs * 1.1);
    }

    /// Current decayed global pulse value in `0..=1`.
    fn bg_fx_pulse(&self) -> f32 {
        match self.bg_fx_pulse_start {
            Some(t) => {
                let e = t.elapsed().as_secs_f32();
                (1.0 - e / BG_FX_PULSE_SECS).clamp(0.0, 1.0)
            }
            None => 0.0,
        }
    }

    /// Whether the animated background is currently drawing — the host uses
    /// this to keep requesting redraws so the effect actually animates.
    ///
    /// For Matrix (mode 3) the effect only draws while there are live emitters,
    /// so it costs no CPU/GPU cycles between typing sessions. For other modes
    /// it animates continuously while enabled.
    #[must_use]
    pub fn bg_fx_active(&self) -> bool {
        if !self.bg_fx_params.active() {
            return false;
        }
        // Matrix only rains while at least one emitter band is alive.
        if self.bg_fx_params.mode == 3 {
            return !self.bg_fx_emitters.is_empty();
        }
        true
    }

    /// sRGB selection-highlight colour. Driven by the active theme.
    pub fn set_selection_color(&mut self, rgb: [u8; 3]) {
        self.selection_rgb = rgb;
    }

    /// Opacity of the selection highlight quad. `1.0` paints the theme's
    /// selection colour as an opaque cell background (the way theme authors
    /// design it); lower values blend over the cell background. Clamped to
    /// `0.2..=1.0` — fully transparent selections are never allowed (a
    /// hardcoded `0.55` blend used to render dark-theme selections nearly
    /// invisible). Mirrors `appearance.selection_opacity`.
    pub fn set_selection_opacity(&mut self, alpha: f32) {
        self.selection_opacity = alpha.clamp(0.2, 1.0);
    }

    /// The active theme's cursor colour, used as the cursor tint whenever
    /// the user hasn't set an explicit `cursor.color` override. Driven by
    /// the active theme so a theme switch recolours the cursor too.
    pub fn set_cursor_theme_color(&mut self, rgb: Option<[u8; 3]>) {
        self.cursor_theme_color = rgb;
    }

    /// Current window background colour.
    #[must_use]
    pub fn background_color(&self) -> [u8; 3] {
        self.background_rgb
    }

    /// Number of lines the viewport is scrolled up into history. `0` =
    /// pinned to the bottom (live output).
    #[must_use]
    pub fn scroll_lines(&self) -> usize {
        self.scroll_lines
    }

    /// Pan the viewport into the scrollback buffer. `0` = stick to the
    /// bottom (default).
    pub fn set_scroll_lines(&mut self, lines: usize) {
        self.scroll_lines = lines;
    }

    /// Replace the cursor rendering parameters. Cheap — just stores the
    /// struct; takes effect on the next frame.
    pub fn set_cursor(&mut self, cursor: CursorParams) {
        self.cursor = cursor;
        self.cursor_start = std::time::Instant::now();
    }

    /// Current cursor configuration.
    #[must_use]
    pub fn cursor(&self) -> CursorParams {
        self.cursor
    }

    /// True if the cursor is currently in the "off" phase of a blink
    /// cycle. Callers use this to know whether they need to schedule
    /// another redraw to animate the blink.
    #[must_use]
    pub fn cursor_blinking(&self) -> bool {
        self.cursor.blink && self.focused
    }

    /// Shared wgpu instance — reuse this when creating sub-windows
    /// (settings, popup menus) so they skip the cost of `wgpu::Instance::new`.
    #[must_use]
    pub fn instance(&self) -> Arc<Instance> {
        Arc::clone(&self.instance)
    }

    /// Shared wgpu adapter — reuse to avoid `request_adapter`.
    #[must_use]
    pub fn adapter(&self) -> Arc<wgpu::Adapter> {
        Arc::clone(&self.adapter)
    }

    /// Shared wgpu device.
    #[must_use]
    pub fn device(&self) -> Arc<Device> {
        Arc::clone(&self.device)
    }

    /// Shared wgpu queue.
    #[must_use]
    pub fn queue(&self) -> Arc<Queue> {
        Arc::clone(&self.queue)
    }

    fn top_offset_logical(&self) -> f32 {
        let tab_h = if self.tab_bar_visible_logical() > 0.0
            && self.tab_bar_placement == TabBarPlacement::Top
        {
            TAB_BAR_HEIGHT
        } else {
            0.0
        };
        let sb_top = match &self.status_bar {
            Some(sb) if !sb.at_bottom => STATUS_BAR_HEIGHT,
            _ => 0.0,
        };
        // When the tab bar is vertical, the window-control buttons live in a
        // slim top strip above the terminal body (not inside the side strip).
        let vert_ctrl_top =
            if self.tab_bar_visible_logical() > 0.0 && self.tab_bar_placement.is_vertical() {
                TAB_BAR_HEIGHT // reuse the horizontal bar height for the ctrl row
            } else {
                0.0
            };
        self.padding_px + tab_h + sb_top + vert_ctrl_top
    }

    /// Extra logical pixels reserved on the **left** for a vertical tab strip.
    fn left_offset_logical(&self) -> f32 {
        if self.tab_bar_visible_logical() > 0.0 && self.tab_bar_placement == TabBarPlacement::Left {
            self.vertical_tab_bar_width
        } else {
            0.0
        }
    }

    /// Extra logical pixels reserved on the **right** for a vertical tab strip.
    fn right_offset_logical(&self) -> f32 {
        if self.tab_bar_visible_logical() > 0.0 && self.tab_bar_placement == TabBarPlacement::Right
        {
            self.vertical_tab_bar_width
        } else {
            0.0
        }
    }

    /// Returns the tab bar height to reserve in logical pixels — `TAB_BAR_HEIGHT`
    /// when the tab bar should be shown, `0.0` when it is hidden (disabled or
    /// hidden-if-single with only one tab).
    fn tab_bar_visible_logical(&self) -> f32 {
        if !self.tab_bar_enabled {
            return 0.0;
        }
        if self.tab_bar_hide_if_single {
            // Count the number of tabs; hide when there is exactly one.
            if let Some(bar) = &self.tab_bar {
                if bar.items.len() <= 1 {
                    return 0.0;
                }
            } else {
                return 0.0;
            }
        } else if self.tab_bar.is_none() {
            return 0.0;
        }
        TAB_BAR_HEIGHT
    }

    /// Extra logical pixels reserved at the **bottom** of the viewport for
    /// the status bar (when at bottom) and/or the tab bar (when at bottom).
    /// Returns `padding_px` when neither is at the bottom.
    fn bottom_offset_logical(&self) -> f32 {
        let sb_h = match &self.status_bar {
            Some(sb) if sb.at_bottom => STATUS_BAR_HEIGHT,
            _ => 0.0,
        };
        let tab_h = if self.tab_bar_visible_logical() > 0.0
            && self.tab_bar_placement == TabBarPlacement::Bottom
        {
            TAB_BAR_HEIGHT
        } else {
            0.0
        };
        let res_h = if self.resource_bar.is_some() {
            RESOURCE_BAR_HEIGHT
        } else {
            0.0
        };
        // The AI suggestion bar floats directly above the resource strip;
        // reserve its band while it is open so the bottom grid rows never
        // render (or stay hidden) under it. The app reflows the PTY on the
        // open/close transition (see the suggestion sync in about_to_wait).
        let sug_h = if self.suggestion_bar.is_some() {
            SUGGESTION_BAR_HEIGHT
        } else {
            0.0
        };
        self.padding_px + sb_h + tab_h + res_h + sug_h
    }

    /// Convert a window pixel area to terminal cell dimensions.
    ///
    /// Expects the **full window** size: the chrome offsets (tab bar,
    /// status bar, resource strip, padding) are subtracted internally.
    /// For a body/pane sub-rect that already had the chrome stripped use
    /// [`Self::rect_to_cells`] instead — feeding a body rect here
    /// double-subtracts the chrome and loses several rows.
    #[must_use]
    pub fn pixels_to_cells(&self, width_px: u32, height_px: u32) -> (u16, u16) {
        cells_for(
            width_px as f32 / self.scale_factor,
            height_px as f32 / self.scale_factor,
            self.cell_width,
            self.cell_height,
            self.padding_px,
            GridOffsets {
                top: self.top_offset_logical(),
                bottom: self.bottom_offset_logical(),
                left: self.left_offset_logical(),
                right: self.right_offset_logical(),
            },
        )
    }

    /// Convert a body/pane sub-rect (physical px) to cell dimensions.
    ///
    /// Counterpart of [`Self::pixels_to_cells`] for rects that ALREADY had
    /// the window chrome removed — the `body_*_px()` body area or a pane
    /// sub-rect from `walk_pane_tree`. No offsets are subtracted here:
    /// `body_left/right_px` strip the horizontal padding and any vertical
    /// tab strip, and `body_top/bottom_px` strip the vertical chrome
    /// (including padding, which lives inside the top/bottom offsets), so
    /// for a single-pane body this returns exactly what `pixels_to_cells`
    /// returns for the full window. (`body_top_px` also includes the
    /// sub-cell centering shift, which is < half a cell and therefore
    /// never changes the floored row count.)
    #[must_use]
    pub fn rect_to_cells(&self, w_px: u32, h_px: u32) -> (u16, u16) {
        let s = self.scale_factor;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let cols = (((w_px as f32 / s).max(self.cell_width)) / self.cell_width)
            .floor()
            .max(1.0) as u16;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let rows = (((h_px as f32 / s).max(self.cell_height)) / self.cell_height)
            .floor()
            .max(1.0) as u16;
        (cols, rows)
    }

    /// Translate a window pixel position to a (col, row) cell index.
    #[must_use]
    pub fn cell_at_pixel(&self, x_px: f32, y_px: f32) -> Option<(u16, u16)> {
        let logical_x = x_px / self.scale_factor;
        let logical_y = y_px / self.scale_factor;
        // Account for a left-side vertical tab strip shifting the grid origin.
        let usable_x = logical_x - self.padding_px - self.left_offset_logical();
        // Subtract the centering shift (converted to logical px) so click
        // coordinates stay aligned with the shifted grid origin.
        let shift_logical = self.grid_top_shift_px(self.config.height) / self.scale_factor;
        let usable_y = logical_y - self.top_offset_logical() - shift_logical;
        if usable_x < 0.0 || usable_y < 0.0 {
            return None;
        }
        let col = (usable_x / self.cell_width).floor() as i32;
        let row = (usable_y / self.cell_height).floor() as i32;
        if col < 0 || row < 0 {
            return None;
        }
        // Clamp into the grid so clicks in the right / bottom padding map to
        // the nearest edge cell (xterm-style) instead of producing
        // out-of-grid indices that reach mouse reporting / selection.
        let (cols, rows) = self.pixels_to_cells(self.config.width, self.config.height);
        let col = col.min(i32::from(cols) - 1).max(0) as u16;
        let row = row.min(i32::from(rows) - 1).max(0) as u16;
        Some((col, row))
    }

    /// Replace the current tab bar (or clear it). Pass `None` for a single
    /// tab UX (the bar is hidden).
    pub fn set_tab_bar(&mut self, bar: Option<TabBar>) {
        self.tab_bar = bar;
    }

    /// Set the min / max width (logical px) a tab clamps to in the tab bar.
    /// Mirrors `appearance.tab_min_width` / `appearance.tab_max_width`.
    /// Values are sanitised: each is clamped to a sane range and `max` is
    /// raised to at least `min` so the tab layout clamp never inverts.
    pub fn set_tab_widths(&mut self, min: f32, max: f32) {
        let (min, max) = sanitize_tab_widths(min, max);
        self.tab_min_width = min;
        self.tab_max_width = max;
    }

    /// Set the fixed width (logical px) rendered for pinned (compact) tab chips.
    /// Mirrors `appearance.pinned_tab_width`. Clamped to `[24, 120]`.
    pub fn set_tab_pinned_width(&mut self, width: f32) {
        self.tab_pinned_width = width.clamp(24.0, 120.0);
    }

    /// Mutable reference to the tab bar (used by the App to update hover
    /// state between frames).
    pub fn tab_bar_mut(&mut self) -> Option<&mut TabBar> {
        self.tab_bar.as_mut()
    }

    /// Set or clear the floating drag ghost — the cursor-following tab pill
    /// drawn in the overlay layer during a Chrome-style tab drag. `None`
    /// removes it. Independent of [`Self::set_tab_bar`] so the ghost
    /// survives a per-frame tab-bar refresh.
    pub fn set_tab_drag_ghost(&mut self, ghost: Option<TabGhost>) {
        self.tab_drag_ghost = ghost;
    }

    /// Set or clear the drop indicator — a vertical insertion bar drawn in
    /// this window's tab bar at the given **logical-px** x. `None` hides it.
    pub fn set_tab_drop_indicator(&mut self, x: Option<f32>) {
        self.tab_drop_indicator = x;
    }

    /// Hit-test the tab bar at a window pixel coordinate.
    #[must_use]
    pub fn tab_hit(&self, x_px: f32, y_px: f32) -> Option<TabHit> {
        let tab_bar = self.tab_bar.as_ref()?;
        // Return `None` immediately when the tab bar is not visible.
        if self.tab_bar_visible_logical() == 0.0 {
            return None;
        }
        let logical_x = x_px / self.scale_factor;
        let logical_y = y_px / self.scale_factor;

        // ── Vertical strip hit-test ──────────────────────────────────────────
        if self.tab_bar_placement.is_vertical() {
            return self.tab_hit_vertical(tab_bar, logical_x, logical_y);
        }

        // ── Horizontal (Top / Bottom) hit-test ───────────────────────────────
        // Check the y range: the bar occupies `[tab_y_log, tab_y_log + TAB_BAR_HEIGHT]`.
        let tab_y_log = self.tab_bar_y_logical(self.config.height as f32);
        if logical_y < tab_y_log || logical_y > tab_y_log + TAB_BAR_HEIGHT {
            return None;
        }
        // Translate to bar-local y for containment tests against the layout.
        let local_y = logical_y - tab_y_log;
        let layout = self.tab_layout(tab_bar);

        // Window controls win first — they must be reachable even when the
        // tab bar is crowded. Layout rects are bar-local (y=0 = top of bar).
        if layout.close_btn.contains(logical_x, local_y) {
            return Some(TabHit::CloseWindow);
        }
        if layout.max_btn.contains(logical_x, local_y) {
            return Some(TabHit::Maximize);
        }
        if layout.min_btn.contains(logical_x, local_y) {
            return Some(TabHit::Minimize);
        }

        // Group-label pills — check before per-tab loop so a pill click
        // takes priority over any tab underneath it.
        for &(pill_rect, first_idx) in &layout.group_pills {
            if pill_rect.contains(logical_x, local_y) {
                return Some(TabHit::GroupLabel(first_idx));
            }
        }

        for (idx, (tab_rect, close_rect)) in layout.tabs.iter().enumerate() {
            // Pinned tabs have no close-X; clicks in that area fall through to Tab.
            let is_pinned = tab_bar.items.get(idx).is_some_and(|i| i.pinned);
            if !is_pinned && close_rect.contains(logical_x, local_y) {
                return Some(TabHit::Close(idx));
            }
            if tab_rect.contains(logical_x, local_y) {
                return Some(TabHit::Tab(idx));
            }
        }
        if layout.plus.contains(logical_x, local_y) {
            return Some(TabHit::Plus);
        }

        // Any unclaimed space in the title bar acts as a drag handle.
        Some(TabHit::DragHandle)
    }

    /// Hit-test the vertical tab strip at logical coordinates.
    fn tab_hit_vertical(&self, bar: &TabBar, logical_x: f32, logical_y: f32) -> Option<TabHit> {
        let strip_x = self.tab_strip_x_logical();
        let strip_w = self.vertical_tab_bar_width;
        let viewport_h = self.config.height as f32 / self.scale_factor;

        // Confirm the cursor is within the strip's x range.
        if logical_x < strip_x || logical_x > strip_x + strip_w {
            return None;
        }

        // The window-control buttons live in the top TAB_BAR_HEIGHT-high strip
        // ABOVE the side strip's tab list.
        let ctrl_top = 0.0_f32;
        let ctrl_bot = TAB_BAR_HEIGHT;
        if logical_y >= ctrl_top && logical_y <= ctrl_bot {
            let layout = self.vert_ctrl_layout(strip_x, strip_w);
            if layout.close_btn.contains(logical_x, logical_y) {
                return Some(TabHit::CloseWindow);
            }
            if layout.max_btn.contains(logical_x, logical_y) {
                return Some(TabHit::Maximize);
            }
            if layout.min_btn.contains(logical_x, logical_y) {
                return Some(TabHit::Minimize);
            }
            // Unmatched area inside the ctrl row is the drag handle.
            return Some(TabHit::DragHandle);
        }

        // Each tab row is TAB_BAR_HEIGHT tall, stacked below the ctrl row.
        let tab_area_top = TAB_BAR_HEIGHT;
        let row_h = TAB_BAR_HEIGHT;

        for (idx, item) in bar.items.iter().enumerate() {
            let row_y = tab_area_top + row_h * idx as f32;
            let tab_rect = LogicalRect {
                x: strip_x,
                y: row_y,
                w: strip_w,
                h: row_h,
            };
            if !tab_rect.contains(logical_x, logical_y) {
                continue;
            }
            // Pinned tabs have no close-X; fall through to Tab hit.
            if !item.pinned {
                // Close-X hit area — right 32 px of each row.
                let close_rect = LogicalRect {
                    x: strip_x + strip_w - 32.0,
                    y: row_y + (row_h - 22.0) * 0.5,
                    w: 22.0,
                    h: 22.0,
                };
                if close_rect.contains(logical_x, logical_y) {
                    return Some(TabHit::Close(idx));
                }
            }
            return Some(TabHit::Tab(idx));
        }

        // Plus button below the last tab.
        let plus_y = tab_area_top + row_h * bar.items.len() as f32;
        let plus_rect = LogicalRect {
            x: strip_x,
            y: plus_y,
            w: strip_w,
            h: row_h,
        };
        if plus_rect.contains(logical_x, logical_y) && plus_y + row_h <= viewport_h {
            return Some(TabHit::Plus);
        }

        // Any remaining strip area is a drag handle.
        Some(TabHit::DragHandle)
    }

    /// Layout of the three window-control buttons inside the vertical strip's
    /// top control row (`y=0..TAB_BAR_HEIGHT`). Buttons are on the right.
    fn vert_ctrl_layout(&self, strip_x: f32, strip_w: f32) -> TabLayout {
        let cw = WINDOW_CTRL_WIDTH.min(strip_w / 3.0);
        let ch = TAB_BAR_HEIGHT;
        let close_btn = LogicalRect {
            x: strip_x + strip_w - cw,
            y: 0.0,
            w: cw,
            h: ch,
        };
        let max_btn = LogicalRect {
            x: strip_x + strip_w - cw * 2.0,
            y: 0.0,
            w: cw,
            h: ch,
        };
        let min_btn = LogicalRect {
            x: strip_x + strip_w - cw * 3.0,
            y: 0.0,
            w: cw,
            h: ch,
        };
        TabLayout {
            tabs: Vec::new(),
            plus: LogicalRect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
            min_btn,
            max_btn,
            close_btn,
            group_pills: Vec::new(), // vertical strip has no pills
        }
    }

    /// Render the vertical tab strip (Left or Right placement) into `quads`
    /// and `tab_text_areas`. The strip layout is:
    /// - Row 0 (`y=0..TAB_BAR_HEIGHT`): window-control buttons (min/max/close).
    /// - Rows 1..N (`y=TAB_BAR_HEIGHT + i*TAB_BAR_HEIGHT`): one per tab.
    /// - Row N+1: the `+` new-tab button.
    #[allow(clippy::too_many_lines)]
    fn render_vertical_tab_strip(
        &mut self,
        bar: &TabBar,
        scale: f32,
        quads: &mut Vec<Quad>,
        tab_text_areas: &mut Vec<(Buffer, [f32; 2])>,
    ) {
        use glyphon::{Attrs, Buffer, Family, Metrics, Shaping, Wrap};
        use system_icons::{
            icon_lines, SystemIcon, BG_CLOSE_HOVER, BG_HOVER, BG_IDLE, STROKE_CLOSE_HOVER,
            STROKE_DEFAULT, STROKE_PX,
        };

        let strip_x_log = self.tab_strip_x_logical();
        let strip_w_log = self.vertical_tab_bar_width;
        let strip_h_log = self.config.height as f32 / self.scale_factor;
        let row_h = TAB_BAR_HEIGHT;

        // ── Background strip ─────────────────────────────────────────────────
        quads.push(Quad::new(
            [strip_x_log * scale, 0.0],
            [strip_w_log * scale, strip_h_log * scale],
            [0x07, 0x09, 0x0e],
            1.0,
        ));
        // Separator line on the inner edge.
        let sep_x = if self.tab_bar_placement == TabBarPlacement::Left {
            (strip_x_log + strip_w_log - 1.0) * scale
        } else {
            strip_x_log * scale
        };
        quads.push(Quad::new(
            [sep_x, 0.0],
            [1.0 * scale, strip_h_log * scale],
            [0x22, 0x28, 0x3a],
            1.0,
        ));

        // ── Window-control row (top TAB_BAR_HEIGHT of strip) ─────────────────
        let ctrl_layout = self.vert_ctrl_layout(strip_x_log, strip_w_log);
        let ctrl_thickness = STROKE_PX * scale;
        for (which, rect) in [
            (WindowCtrl::Minimize, &ctrl_layout.min_btn),
            (WindowCtrl::Maximize, &ctrl_layout.max_btn),
            (WindowCtrl::Close, &ctrl_layout.close_btn),
        ] {
            let hovered = bar.window_ctrl_hovered == Some(which);
            let bg = match (which, hovered) {
                (WindowCtrl::Close, true) => BG_CLOSE_HOVER,
                (_, true) => BG_HOVER,
                _ => BG_IDLE,
            };
            quads.push(Quad::new(
                [rect.x * scale, rect.y * scale],
                [rect.w * scale, rect.h * scale],
                bg,
                1.0,
            ));
            let stroke = if hovered && matches!(which, WindowCtrl::Close) {
                STROKE_CLOSE_HOVER
            } else {
                STROKE_DEFAULT
            };
            let kind = match which {
                WindowCtrl::Minimize => SystemIcon::Minimize,
                WindowCtrl::Maximize => {
                    if bar.maximized {
                        SystemIcon::Restore
                    } else {
                        SystemIcon::Maximize
                    }
                }
                WindowCtrl::Close => SystemIcon::Close,
            };
            let cx_log = rect.x + rect.w * 0.5;
            let cy_log = rect.y + rect.h * 0.5;
            for line in icon_lines(kind, cx_log, cy_log) {
                quads.push(Quad::line(
                    [line.from.0 * scale, line.from.1 * scale],
                    [line.to.0 * scale, line.to.1 * scale],
                    ctrl_thickness,
                    stroke,
                    1.0,
                ));
            }
        }

        // Separator line below the control row.
        quads.push(Quad::new(
            [strip_x_log * scale, (row_h - 1.0) * scale],
            [strip_w_log * scale, 1.0 * scale],
            [0x22, 0x28, 0x3a],
            1.0,
        ));

        // ── Tab rows ─────────────────────────────────────────────────────────
        let tab_area_top = row_h; // below the ctrl row
        let close_size = 22.0_f32;
        let close_col_w = 32.0_f32;

        for (idx, item) in bar.items.iter().enumerate() {
            let row_y_log = tab_area_top + row_h * idx as f32;
            let active = item.active;
            let hovered = bar.hovered == Some(idx);

            let base_bg: [u8; 3] = if active {
                [0x1a, 0x20, 0x33]
            } else if hovered {
                [0x18, 0x1f, 0x33]
            } else {
                [0x0a, 0x0c, 0x14]
            };
            let bg_color = if let Some(tint) = item.color {
                blend_tint(base_bg, tint, 0.40)
            } else {
                base_bg
            };
            let bg_alpha = 1.0_f32;

            // Row background.
            quads.push(Quad::new(
                [strip_x_log * scale, row_y_log * scale],
                [strip_w_log * scale, row_h * scale],
                bg_color,
                bg_alpha,
            ));

            // Active indicator or context-rule accent bar on the inner edge.
            let accent_color = item.color.unwrap_or([0x7d, 0xa6, 0xff]);
            if active || item.color.is_some() {
                let accent_x = if self.tab_bar_placement == TabBarPlacement::Left {
                    strip_x_log + strip_w_log - 3.0
                } else {
                    strip_x_log
                };
                quads.push(Quad::new(
                    [accent_x * scale, row_y_log * scale],
                    [3.0 * scale, row_h * scale],
                    accent_color,
                    1.0,
                ));
            } else if item.unread {
                // Small accent dot in the top-left of inactive unread tabs.
                let dot = 6.0 * scale;
                quads.push(Quad::new(
                    [(strip_x_log + 8.0) * scale, (row_y_log + 6.0) * scale],
                    [dot, dot],
                    [0x7d, 0xa6, 0xff],
                    1.0,
                ));
            }

            // Static "waiting for input" dot — bottom-left of tabs whose
            // program rang the bell (e.g. Claude Code awaiting input). Amber
            // and in the opposite corner from the blue unread dot so the two
            // never collide. Independent `if` (not part of the
            // active/accent/unread chain) so it also shows on tinted tabs. The
            // build side already decides visibility (non-active tabs, or the
            // active tab while the window is unfocused), so no `!active` here.
            if item.attention {
                let dot = 6.0 * scale;
                quads.push(Quad::new(
                    [
                        (strip_x_log + 8.0) * scale,
                        (row_y_log + row_h - 12.0) * scale,
                    ],
                    [dot, dot],
                    [0xe0, 0x90, 0x30],
                    1.0,
                ));
            }

            // ── Group accent: 3-px stripe on the leading inner edge ───────────
            // Vertical tab bars show the accent on the opposite inner edge so
            // it is distinct from the active-tab indicator.
            if let Some(ga) = item.group_accent {
                let ga_x = if self.tab_bar_placement == TabBarPlacement::Left {
                    strip_x_log
                } else {
                    strip_x_log + strip_w_log - 3.0
                };
                quads.push(Quad::new(
                    [ga_x * scale, row_y_log * scale],
                    [3.0 * scale, row_h * scale],
                    ga,
                    0.85,
                ));
            }
            // ── Group boundary separator (horizontal line above first tab in
            // a group run).
            if item.group_label.is_some() && idx > 0 {
                let sep_color = item.group_accent.unwrap_or([0x40, 0x50, 0x70]);
                quads.push(Quad::new(
                    [strip_x_log * scale, (row_y_log - 1.0) * scale],
                    [strip_w_log * scale, 1.0 * scale],
                    sep_color,
                    1.0,
                ));
            }

            // Close-button chip (optional) + vector X — hidden for pinned tabs.
            if item.pinned {
                // Pinned indicator: small corner square in the accent colour.
                let tri_size = 5.0 * scale;
                quads.push(Quad::new(
                    [strip_x_log * scale, row_y_log * scale],
                    [tri_size, tri_size],
                    item.color.unwrap_or([0x7d, 0xa6, 0xff]),
                    1.0,
                ));
            } else {
                let disc_cx = (strip_x_log + strip_w_log - close_col_w * 0.5) * scale;
                let disc_cy = (row_y_log + row_h * 0.5) * scale;
                let disc_d = 18.0 * scale;
                let disc_color = darken_tab_bg(bg_color, 0.72);
                let show_chip = matches!(
                    self.close_button_style,
                    terminale_config::CloseButtonStyle::Chip
                );
                if show_chip {
                    quads.push(Quad::new(
                        [disc_cx - disc_d * 0.5, disc_cy - disc_d * 0.5],
                        [disc_d, disc_d],
                        disc_color,
                        1.0,
                    ));
                }
                let x_half = 3.0 * scale;
                let x_thickness = STROKE_PX * scale;
                let x_color = [0xb8_u8, 0xc0, 0xd0];
                for (from, to) in [
                    (
                        [disc_cx - x_half, disc_cy - x_half],
                        [disc_cx + x_half, disc_cy + x_half],
                    ),
                    (
                        [disc_cx - x_half, disc_cy + x_half],
                        [disc_cx + x_half, disc_cy - x_half],
                    ),
                ] {
                    quads.push(Quad::line(from, to, x_thickness, x_color, 1.0));
                }
            }

            // Tab label — ellipsised to fit the strip width minus close column.
            let avail_w = (strip_w_log - close_col_w - 16.0).max(0.0);
            let approx_char_w = self.font_size * 0.6;
            let max_chars = (avail_w / approx_char_w).floor() as usize;
            let raw_label = match &item.icon {
                Some(icon) => format!("{}  {}", icon, item.label),
                None => item.label.clone(),
            };
            let label_text = truncate_tab_title(&raw_label, max_chars);

            let mut buf = Buffer::new(
                &mut self.font_system,
                Metrics::new(self.font_size * 0.92, self.font_size * 1.1),
            );
            buf.set_wrap(&mut self.font_system, Wrap::None);
            buf.set_size(
                &mut self.font_system,
                Some(avail_w),
                Some(self.font_size * 1.1),
            );
            let text_color = if active {
                glyphon::Color::rgb(0xe6, 0xea, 0xf8)
            } else {
                glyphon::Color::rgb(0xa8, 0xb1, 0xc4)
            };
            buf.set_text(
                &mut self.font_system,
                &label_text,
                Attrs::new().family(Family::Monospace).color(text_color),
                Shaping::Advanced,
            );
            let text_x = strip_x_log + 12.0;
            let text_y = row_y_log + (row_h - self.font_size * 1.1) * 0.5;
            tab_text_areas.push((buf, [text_x, text_y]));

            // ── Context-rule badge (top-left of the row) ─────────────────────
            if let Some(badge_text) = &item.badge {
                if !badge_text.is_empty() {
                    let badge_tint = item.color.unwrap_or([0xc0, 0x50, 0x50]);
                    let badge_font_size = self.font_size * 0.60;
                    let badge_h = badge_font_size * 1.3;
                    let badge_w = badge_font_size * badge_text.chars().count() as f32 * 0.72 + 6.0;
                    let badge_x = strip_x_log + 4.0;
                    let badge_y = row_y_log + 2.0;
                    quads.push(Quad::new(
                        [badge_x * scale, badge_y * scale],
                        [badge_w * scale, badge_h * scale],
                        badge_tint,
                        0.90,
                    ));
                    let mut badge_buf = Buffer::new(
                        &mut self.font_system,
                        Metrics::new(badge_font_size, badge_font_size * 1.3),
                    );
                    badge_buf.set_wrap(&mut self.font_system, glyphon::Wrap::None);
                    badge_buf.set_size(&mut self.font_system, Some(badge_w), Some(badge_h));
                    badge_buf.set_text(
                        &mut self.font_system,
                        badge_text,
                        Attrs::new()
                            .family(Family::SansSerif)
                            .color(glyphon::Color::rgb(0xff, 0xff, 0xff)),
                        Shaping::Advanced,
                    );
                    tab_text_areas.push((badge_buf, [badge_x + 3.0, badge_y + 1.0]));
                }
            }

            // Row separator.
            quads.push(Quad::new(
                [strip_x_log * scale, (row_y_log + row_h - 1.0) * scale],
                [strip_w_log * scale, 1.0 * scale],
                [0x12, 0x18, 0x28],
                1.0,
            ));
        }

        // ── Plus button ──────────────────────────────────────────────────────
        let plus_y_log = tab_area_top + row_h * bar.items.len() as f32;
        if plus_y_log + row_h <= strip_h_log {
            let plus_bg = if bar.plus_hovered {
                [0x1d, 0x26, 0x40]
            } else {
                [0x07, 0x09, 0x0e]
            };
            quads.push(Quad::new(
                [strip_x_log * scale, plus_y_log * scale],
                [strip_w_log * scale, row_h * scale],
                plus_bg,
                1.0,
            ));
            let mut plus_buf = Buffer::new(
                &mut self.font_system,
                Metrics::new(self.font_size * 1.0, self.font_size * 1.1),
            );
            plus_buf.set_size(&mut self.font_system, Some(strip_w_log), Some(row_h));
            plus_buf.set_text(
                &mut self.font_system,
                "+",
                Attrs::new()
                    .family(Family::SansSerif)
                    .color(glyphon::Color::rgb(0xc0, 0xc8, 0xdc)),
                Shaping::Advanced,
            );
            tab_text_areas.push((plus_buf, [strip_x_log + 12.0, plus_y_log + 6.0]));
        }

        // Suppress the unused-variable warning for `close_size` on paths
        // where the chip is disabled (the disc_d var uses close_size indirectly
        // but only in vertical mode). This is a pure layout constant.
        let _ = close_size;
    }

    /// Insertion slot (`0..=items.len()`) for a tab dropped at window pixel
    /// x-coordinate `x_px` — the index the dragged tab would occupy. The
    /// boundary is chosen by tab midpoints: a drop lands *before* the first
    /// tab whose horizontal centre is to the right of the cursor.
    ///
    /// Returns `0` when there is no tab bar. Takes **physical** px and
    /// divides by the scale factor internally, mirroring [`Self::tab_hit`]
    /// so callers can pass the same coordinates to both.
    #[must_use]
    pub fn drop_slot_at(&self, x_px: f32) -> usize {
        let Some(bar) = self.tab_bar.as_ref() else {
            return 0;
        };
        let logical_x = x_px / self.scale_factor;
        let layout = self.tab_layout(bar);
        let pills: Vec<(f32, f32)> = layout.tabs.iter().map(|(r, _)| (r.x, r.w)).collect();
        slot_from_midpoints(&pills, logical_x)
    }

    /// Logical-px `(x, width)` of the pill for tab `idx`, or `None` when the
    /// index is out of range / there is no tab bar. Lets the App compute a
    /// drag's grab offset and the ghost's width without duplicating layout.
    #[must_use]
    pub fn tab_slot_rect(&self, idx: usize) -> Option<(f32, f32)> {
        let bar = self.tab_bar.as_ref()?;
        let layout = self.tab_layout(bar);
        layout.tabs.get(idx).map(|(rect, _)| (rect.x, rect.w))
    }

    /// Insertion slot (`0..=items.len()`) for a tab dropped at window pixel
    /// y-coordinate `y_px` — the vertical analogue of [`Self::drop_slot_at`],
    /// used when the tab bar is a left/right side strip. The boundary is
    /// chosen by tab row midpoints: a drop lands *before* the first row whose
    /// vertical centre is below the cursor.
    ///
    /// Returns `0` when there is no tab bar. Takes **physical** px and divides
    /// by the scale factor internally.
    #[must_use]
    pub fn drop_slot_at_y(&self, y_px: f32) -> usize {
        let Some(bar) = self.tab_bar.as_ref() else {
            return 0;
        };
        if !self.tab_bar_placement.is_vertical() {
            return 0;
        }
        let logical_y = y_px / self.scale_factor;
        // Tab rows start below the control row (TAB_BAR_HEIGHT) and each row
        // is TAB_BAR_HEIGHT tall — mirrors `tab_hit_vertical`.
        let tab_area_top = TAB_BAR_HEIGHT;
        let row_h = TAB_BAR_HEIGHT;
        let rows: Vec<(f32, f32)> = bar
            .items
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let y = tab_area_top + row_h * i as f32;
                (y, row_h)
            })
            .collect();
        slot_from_midpoints(&rows, logical_y)
    }

    /// Returns the current [`TabBarPlacement`].
    #[must_use]
    pub fn tab_placement(&self) -> TabBarPlacement {
        self.tab_bar_placement
    }

    /// When the tab bar is a vertical side strip, returns
    /// `Some((strip_x_logical, strip_w_logical, inner_edge_logical))` where
    /// `inner_edge_logical` is the x coordinate of the edge that faces the
    /// terminal grid (strip right edge for `Left`, strip left edge for
    /// `Right`). Returns `None` when the placement is horizontal.
    #[must_use]
    pub fn vertical_strip_inner_edge(&self) -> Option<(f32, f32, f32)> {
        if !self.tab_bar_placement.is_vertical() {
            return None;
        }
        let strip_x = self.tab_strip_x_logical();
        let strip_w = self.vertical_tab_bar_width;
        let inner_edge = match self.tab_bar_placement {
            TabBarPlacement::Left => strip_x + strip_w,
            TabBarPlacement::Right => strip_x,
            _ => return None,
        };
        Some((strip_x, strip_w, inner_edge))
    }

    /// Logical-px x of the insertion-bar boundary for drop slot `slot`
    /// (`0..=items.len()`): the left edge of the pill at `slot`, or the
    /// right edge of the last pill when appending. `None` when there is no
    /// tab bar. Used to position the drop indicator precisely on a boundary.
    #[must_use]
    pub fn drop_boundary_x(&self, slot: usize) -> Option<f32> {
        let bar = self.tab_bar.as_ref()?;
        let layout = self.tab_layout(bar);
        if layout.tabs.is_empty() {
            return Some(8.0);
        }
        if let Some((rect, _)) = layout.tabs.get(slot) {
            Some(rect.x)
        } else {
            let (rect, _) = layout.tabs.last()?;
            Some(rect.x + rect.w)
        }
    }

    fn tab_layout(&self, bar: &TabBar) -> TabLayout {
        let viewport_w = self.config.width as f32 / self.scale_factor;
        let y = 4.0;
        let tab_h = TAB_BAR_HEIGHT - 8.0;
        let plus_w = 32.0;

        // Reserve space on the right for the three window-control buttons.
        let ctrls_total = WINDOW_CTRL_WIDTH * 3.0;
        let close_btn = LogicalRect {
            x: viewport_w - WINDOW_CTRL_WIDTH,
            y: 0.0,
            w: WINDOW_CTRL_WIDTH,
            h: TAB_BAR_HEIGHT,
        };
        let max_btn = LogicalRect {
            x: viewport_w - WINDOW_CTRL_WIDTH * 2.0,
            y: 0.0,
            w: WINDOW_CTRL_WIDTH,
            h: TAB_BAR_HEIGHT,
        };
        let min_btn = LogicalRect {
            x: viewport_w - WINDOW_CTRL_WIDTH * 3.0,
            y: 0.0,
            w: WINDOW_CTRL_WIDTH,
            h: TAB_BAR_HEIGHT,
        };

        let tab_area_w = (viewport_w - 16.0 - plus_w - 8.0 - ctrls_total).max(0.0);

        // Pinned tabs always get a fixed compact width; the remaining space is
        // divided among unpinned tabs.
        let pinned_count = bar.items.iter().filter(|i| i.pinned).count();
        let unpinned_count = bar.items.len().saturating_sub(pinned_count);
        let pinned_w = self.tab_pinned_width;
        let pinned_total = pinned_w * pinned_count as f32;

        // Tabs shrink to fit the available area so they never spill under the
        // window-control buttons (minimize/maximize/close). We honour
        // `tab_min_width` while there's room, but allow shrinking below it
        // (down to a small hard floor) when crowded — browser-style — instead
        // of overflowing. (A scrollable tab strip is a future refinement for
        // truly extreme tab counts.)
        let hard_min = self.tab_min_width.min(TAB_HARD_MIN_WIDTH);

        // Pre-pass: sum total pill + gap widths so we can account for them in
        // the tab-area budget and avoid tabs spilling under window controls.
        let pill_total: f32 = bar
            .items
            .iter()
            .map(|item| {
                if let Some(ref glabel) = item.group_label {
                    let chars = glabel.chars().count();
                    let pill_w = (GROUP_PILL_PAD_X * 2.0
                        + self.font_size * 0.62 * 0.72 * chars as f32)
                        .clamp(24.0, 140.0);
                    pill_w + GROUP_PILL_GAP * 2.0
                } else {
                    0.0
                }
            })
            .sum();

        // Tab width accounts for both pinned-tab fixed widths and pill overhead.
        let tab_w = {
            let area_net = (tab_area_w - pinned_total - pill_total).max(0.0);
            let raw = if unpinned_count == 0 {
                TAB_DEFAULT_WIDTH
            } else {
                area_net / unpinned_count as f32
            };
            raw.clamp(hard_min, self.tab_max_width)
        };

        let mut x = 8.0;
        let mut tabs = Vec::with_capacity(bar.items.len());
        let mut group_pills: Vec<(LogicalRect, usize)> = Vec::new();

        for (idx, item) in bar.items.iter().enumerate() {
            // If this tab is the start of a group run (has a group_label),
            // insert the pill gap BEFORE placing the tab.
            if let Some(ref glabel) = item.group_label {
                let chars = glabel.chars().count();
                let pill_w = (GROUP_PILL_PAD_X * 2.0 + self.font_size * 0.62 * 0.72 * chars as f32)
                    .clamp(24.0, 140.0);
                // Pill rect sits in the gap to the left of the tab.
                let pill_x = x + GROUP_PILL_GAP;
                let pill_rect = LogicalRect {
                    x: pill_x,
                    y,
                    w: pill_w,
                    h: tab_h,
                };
                group_pills.push((pill_rect, idx));
                // Advance x past the pill + both gaps.
                x += pill_w + GROUP_PILL_GAP * 2.0;
            }

            let this_w = if item.pinned { pinned_w } else { tab_w };
            let tab_rect = LogicalRect {
                x,
                y,
                w: this_w - 2.0,
                h: tab_h,
            };
            let close_w = 22.0;
            let close_rect = LogicalRect {
                x: x + this_w - close_w - 6.0,
                y: y + (tab_h - close_w) * 0.5,
                w: close_w,
                h: close_w,
            };
            tabs.push((tab_rect, close_rect));
            x += this_w;
        }
        let plus = LogicalRect {
            x: x + 4.0,
            y,
            w: plus_w,
            h: tab_h,
        };

        TabLayout {
            tabs,
            plus,
            min_btn,
            max_btn,
            close_btn,
            group_pills,
        }
    }

    /// Resize the surface and the internal text buffer.
    pub fn resize(&mut self, physical_width: u32, physical_height: u32) {
        self.config.width = physical_width.max(1);
        self.config.height = physical_height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// Update the scale factor reported by winit (HiDPI changes, monitor move).
    ///
    /// Floored at `0.1`: winit promises a positive value, but monitor-unplug /
    /// remote-session edge cases have produced `0` in the wild, and the scale
    /// is used as a divisor in cell hit-testing (`inf`/`NaN` coordinates).
    pub fn set_scale_factor(&mut self, scale_factor: f32) {
        self.scale_factor = scale_factor.max(0.1);
    }

    /// Current DPI scale factor (physical pixels per logical pixel).
    #[must_use]
    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// Downward shift (physical px) applied to the grid origin so that the
    /// sub-cell vertical remainder is distributed symmetrically: half above the
    /// first row and half below the last row.  This eliminates the asymmetric
    /// "big gap at the bottom" that arises because `cells_for` floors the row
    /// count and leaves the entire remainder at the bottom.
    ///
    /// **Unit note**: `self.cell_height` is in logical pixels; it is scaled to
    /// physical here to avoid HiDPI rounding errors.
    ///
    /// Returns 0.0 for multi-pane layouts — panes manage their own origins via
    /// `pending_body_y` and must not receive this shift.
    #[must_use]
    fn grid_top_shift_px(&self, surface_h_px: u32) -> f32 {
        let scale = self.scale_factor;
        let cell_h_px = self.cell_height * scale;
        let top_px = self.top_offset_logical() * scale;
        let bottom_px = self.bottom_offset_logical() * scale;
        let leftover_px = vertical_remainder(surface_h_px as f32, cell_h_px, top_px, bottom_px);
        leftover_px * 0.5
    }

    /// Top of the terminal body area in **physical** pixels (below the tab
    /// bar and any top-positioned status bar), including the half-remainder
    /// centering shift so the grid is visually symmetric.
    #[must_use]
    pub fn body_top_px(&self) -> f32 {
        self.top_offset_logical() * self.scale_factor + self.grid_top_shift_px(self.config.height)
    }

    /// Set or clear the status-bar content. Pass `None` to hide the bar and
    /// reclaim its height for the terminal grid. When `Some`, the layout
    /// reserves [`STATUS_BAR_HEIGHT`] logical pixels at the configured edge.
    pub fn set_status_bar(&mut self, content: Option<StatusBarContent>) {
        self.status_bar = content;
    }

    /// Set or clear the bottom resource-indicator strip. Pass `None` to hide it
    /// and reclaim [`RESOURCE_BAR_HEIGHT`] logical pixels for the grid.
    pub fn set_resource_bar(&mut self, content: Option<ResourceBarContent>) {
        self.resource_bar = content;
    }

    /// Whether the resource-indicator strip is currently shown (and thus
    /// reserving bottom space). Used to detect enable/disable transitions that
    /// require a grid reflow.
    #[must_use]
    pub fn resource_bar_enabled(&self) -> bool {
        self.resource_bar.is_some()
    }

    /// Bottom of the terminal body area in **physical** pixels.
    ///
    /// Accounts for the status bar (when at bottom), the tab bar (when at
    /// bottom), and the bottom padding.
    #[must_use]
    pub fn body_bottom_px(&self, surface_height_px: u32) -> f32 {
        let h = surface_height_px as f32;
        h - self.bottom_offset_logical() * self.scale_factor
    }

    /// Left edge of the terminal body area in **physical** pixels.
    /// When a left-side vertical tab strip is active, this includes the strip width.
    #[must_use]
    pub fn body_left_px(&self) -> f32 {
        (self.padding_px + self.left_offset_logical()) * self.scale_factor
    }

    /// Right edge of the terminal body area in **physical** pixels.
    /// When a right-side vertical tab strip is active, this excludes the strip width.
    #[must_use]
    pub fn body_right_px(&self, surface_width_px: u32) -> f32 {
        let w = surface_width_px as f32;
        w - (self.padding_px + self.right_offset_logical()) * self.scale_factor
    }

    /// Width of one terminal cell in **physical** pixels.
    #[must_use]
    pub fn cell_width(&self) -> f32 {
        self.cell_width
    }

    /// Height of one terminal cell in **physical** pixels.
    #[must_use]
    pub fn cell_height(&self) -> f32 {
        self.cell_height
    }

    /// Viewport width in **logical** pixels.
    #[must_use]
    pub fn viewport_logical_width(&self) -> f32 {
        self.config.width as f32 / self.scale_factor
    }

    /// Adjust the font size (zoom). Recomputes cell metrics.
    pub fn set_font_size(&mut self, new_size: f32) {
        self.font_size = new_size.clamp(6.0, 96.0);
        self.remeasure_cell();
    }

    /// Current font size.
    #[must_use]
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Set the monospace family used for the terminal grid. Pass the
    /// family name exactly as installed (e.g. "JetBrains Mono"); an
    /// empty string falls back to the system default monospace.
    /// Recomputes cell metrics since a new font changes the advance.
    ///
    /// If the requested family is not present in the font database we
    /// fall back to the system monospace — critical, because a missing
    /// family would otherwise resolve to an arbitrary (often
    /// proportional) font, breaking the fixed-cell grid alignment and
    /// making the cursor drift from the glyphs.
    pub fn set_font_family(&mut self, family: &str) {
        let requested = family.trim();
        if requested.is_empty() {
            self.font_family.clear();
        } else if family_is_available(&self.font_system, requested) {
            self.font_family = requested.to_string();
        } else {
            tracing::warn!(
                family = requested,
                "configured font not found; falling back to system monospace"
            );
            self.font_family.clear();
        }
        self.remeasure_cell();
    }

    /// Currently-resolved grid font family name (empty = system monospace).
    #[must_use]
    pub fn font_family(&self) -> &str {
        &self.font_family
    }

    /// Enumerate the **monospaced** font families actually present in the font
    /// database (system fonts + the bundled faces), sorted and de-duplicated.
    /// The Settings font pickers list these so every choice resolves via
    /// [`Self::set_font_family`] instead of warning and falling back — only
    /// fonts that are genuinely installed are offered.
    #[must_use]
    pub fn available_monospace_families(&self) -> Vec<String> {
        monospace_families_in(self.font_system.db())
    }

    /// Set (or clear) the per-style font family overrides.
    ///
    /// - `bold`      → `font.bold_family`
    /// - `italic`    → `font.italic_family`
    /// - `bold_italic` → `font.bold_italic_family`
    ///
    /// `None` means "derive from the main family" (synthesized weight/style).
    /// A non-empty `Some` is validated against the font database; if the
    /// family is not found the override is cleared with a warning so rendering
    /// continues to work correctly.
    pub fn set_font_style_overrides(
        &mut self,
        bold: Option<&str>,
        italic: Option<&str>,
        bold_italic: Option<&str>,
    ) {
        self.font_bold_family =
            resolve_override_family(&self.font_system, bold, "font.bold_family");
        self.font_italic_family =
            resolve_override_family(&self.font_system, italic, "font.italic_family");
        self.font_bold_italic_family =
            resolve_override_family(&self.font_system, bold_italic, "font.bold_italic_family");
    }

    /// Set the line-height multiplier (clamped to `0.8..=3.0`) and
    /// recompute the cell height.
    pub fn set_line_height(&mut self, multiplier: f32) {
        self.line_height = multiplier.clamp(0.8, 3.0);
        self.remeasure_cell();
    }

    /// Enable or disable ligatures / contextual shaping for grid text.
    pub fn set_ligatures(&mut self, enabled: bool) {
        self.ligatures = enabled;
    }

    /// Set the logical-pixel thickness of SGR underline strokes (all styles).
    /// Clamped to `[0.5, 4.0]`. At draw time this is multiplied by the
    /// current DPI scale factor.
    pub fn set_underline_thickness(&mut self, thickness_px: f32) {
        self.underline_thickness_px = thickness_px.clamp(0.5, 4.0);
    }

    /// Re-probe a representative cell width/height using the current font
    /// size + family, then apply the line-height multiplier and the
    /// cell-width multiplier.
    fn remeasure_cell(&mut self) {
        let family = if self.font_family.is_empty() {
            None
        } else {
            Some(self.font_family.clone())
        };
        let (w, h) = cell_size_for(&mut self.font_system, self.font_size, family.as_deref());
        self.cell_width = w * self.cell_width_multiplier.clamp(0.8, 2.0);
        self.cell_height = h * self.line_height;
    }

    /// Replace (or clear) the current text selection rectangle.
    ///
    /// The rect is interpreted as viewport-relative **at this moment**: the
    /// current scroll offset and scrollback length are snapshotted so the
    /// draw pass (and [`Self::selection_scroll`] /
    /// [`Self::selection_history`] consumers) can keep the highlight glued
    /// to the text as the viewport scrolls or new output arrives.
    pub fn set_selection(&mut self, selection: Option<CellRect>) {
        self.selection = selection;
        self.selection_scroll = self.scroll_lines;
        self.selection_history = self.last_history;
    }

    /// Scroll offset snapshotted when the selection was last set. Pair with
    /// [`Self::selection_history`] to map the viewport-relative selection
    /// rect back to the text it covered (see `selection_text`).
    #[must_use]
    pub fn selection_scroll(&self) -> usize {
        self.selection_scroll
    }

    /// Scrollback length snapshotted when the selection was last set.
    #[must_use]
    pub fn selection_history(&self) -> usize {
        self.selection_history
    }

    /// Set when the scrollback scrollbar is shown. Mirrors `window.scrollbar`.
    pub fn set_scrollbar_mode(&mut self, mode: ScrollbarMode) {
        self.scrollbar_mode = mode;
    }

    /// Update the pointer-over-scrollbar state (from `CursorMoved`). Returns
    /// `true` when the value changed, so the caller knows to repaint.
    pub fn set_scrollbar_hover(&mut self, hover: bool) -> bool {
        let changed = self.scrollbar_hover != hover;
        self.scrollbar_hover = hover;
        changed
    }

    /// Mark the scrollbar thumb as being dragged — keeps the bar visible and
    /// widened for the duration of the drag regardless of pointer position.
    pub fn set_scrollbar_active(&mut self, active: bool) {
        self.scrollbar_active = active;
    }

    /// Scrollbar geometry as last computed by the draw pass. `Some` whenever
    /// any scrollback history exists (even while the bar is hidden in `Auto`
    /// mode — the band is needed for hover-reveal); `None` with no history
    /// or in `Never` mode.
    #[must_use]
    pub fn scrollbar_geometry(&self) -> Option<ScrollbarGeom> {
        self.last_scrollbar
    }

    /// Replace (or clear) the merge drop-zone highlight: the half of a
    /// target pane (physical px) a dragged tab / pane would occupy when
    /// released. Drawn in the overlay layer as an accent-tinted rect with a
    /// brighter frame.
    pub fn set_drop_zone(&mut self, zone: Option<[f32; 4]>) {
        self.drop_zone = zone;
    }

    /// `true` when the scrollbar is currently visible to the user (geometry
    /// cached AND the mode + hover/drag/scroll state says it's drawn). The
    /// mouse handler gates grabs on this so an invisible bar never swallows
    /// right-edge clicks.
    #[must_use]
    pub fn scrollbar_visible(&self) -> bool {
        self.last_scrollbar.is_some()
            && match self.scrollbar_mode {
                ScrollbarMode::Never => false,
                ScrollbarMode::Always => true,
                ScrollbarMode::Auto => {
                    self.scroll_lines > 0 || self.scrollbar_hover || self.scrollbar_active
                }
            }
    }

    /// Update window-focus state. Controls cursor style (solid bar when
    /// focused, hollow rectangle when not).
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Set the focus-border thickness from a **logical-px** value.
    /// Converts to physical pixels using the current scale factor.
    /// Set to `0.0` to disable the border entirely.
    pub fn set_focus_border_thickness_logical(&mut self, logical: f32) {
        self.focus_border_thickness_px = logical * self.scale_factor;
    }

    /// Override the focus-border stroke colour.  `None` = use the built-in
    /// accent fallback (`[0x7d, 0xa6, 0xff]`).
    pub fn set_focus_border_color(&mut self, color: Option<[u8; 3]>) {
        self.focus_border_color = color;
    }

    /// Set the focus-border stroke opacity (clamped to `0.0..=1.0`).
    pub fn set_focus_border_alpha(&mut self, alpha: f32) {
        self.focus_border_alpha = alpha.clamp(0.0, 1.0);
    }

    /// Set the ids of panes currently receiving broadcast input. While this
    /// list is non-empty a distinct amber tinted border is drawn around those
    /// panes so the user always sees broadcast mode is active. Pass an empty
    /// slice to clear the indicator (broadcast off).
    pub fn set_broadcast_receiver_ids(&mut self, ids: &[u32]) {
        self.broadcast_receiver_ids.clear();
        self.broadcast_receiver_ids.extend_from_slice(ids);
    }

    /// Currently-active selection, if any.
    #[must_use]
    pub fn selection(&self) -> Option<CellRect> {
        self.selection
    }

    /// Replace (or clear) the overlay menu drawn this frame.
    pub fn set_overlay(&mut self, overlay: Option<MenuOverlay>) {
        self.overlay = overlay;
    }

    /// Cell dimensions in logical pixels.
    #[must_use]
    pub fn cell_size(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }

    /// Logical padding around the grid.
    #[must_use]
    pub fn padding(&self) -> f32 {
        self.padding_px
    }

    /// Update the logical padding around the terminal grid. Caller
    /// should then resize the active tab so the cell count reflects
    /// the new available area.
    pub fn set_padding(&mut self, padding: f32) {
        self.padding_px = padding.clamp(0.0, 128.0);
    }

    /// Enable or disable the 22 px per-pane header strip. When disabled
    /// no headers are queued during [`Self::render_panes`], and pane grids
    /// reclaim the full rect.
    pub fn set_show_pane_headers(&mut self, on: bool) {
        self.show_pane_headers = on;
    }

    /// Set the visual style for close-X buttons on tab chips and pane headers.
    pub fn set_close_button_style(&mut self, style: terminale_config::CloseButtonStyle) {
        self.close_button_style = style;
    }

    /// Set the SGR 2 (faint/dim) blend factor. `0.0` = no dimming, `1.0` =
    /// fully blended into the background. Live-applied from config.
    pub fn set_dim_amount(&mut self, amount: f32) {
        self.dim_amount = amount.clamp(0.0, 1.0);
    }

    /// Set the alpha of the overlay drawn over inactive (non-focused) panes.
    /// `0.0` = off; `0.9` = maximum. Live-applied from config.
    pub fn set_inactive_pane_dim(&mut self, alpha: f32) {
        self.inactive_pane_dim = alpha.clamp(0.0, 0.9);
    }

    /// Set the alpha of the overlay drawn over the full grid when the window
    /// loses OS focus. `0.0` = off; `0.9` = maximum. Live-applied from config.
    pub fn set_unfocused_window_dim(&mut self, alpha: f32) {
        self.unfocused_window_dim = alpha.clamp(0.0, 0.9);
    }

    /// Set the minimum WCAG contrast ratio enforced per cell.
    /// `1.0` disables the feature; `4.5` = WCAG AA; `7.0` = WCAG AAA.
    /// Clamped to `1.0..=21.0`. Live-applied from config.
    pub fn set_minimum_contrast(&mut self, ratio: f32) {
        self.minimum_contrast = ratio.clamp(1.0, 21.0);
    }

    /// Enable or disable the built-in procedural rendering for box-drawing
    /// (U+2500–U+257F) and block-element (U+2580–U+259F) characters.
    /// When `true` (default), these characters are drawn as crisp pixel-aligned
    /// quads instead of font glyphs, eliminating seams between adjacent cells.
    /// Live-applied from `appearance.builtin_box_drawing`.
    pub fn set_builtin_box_drawing(&mut self, enabled: bool) {
        self.builtin_box_drawing = enabled;
    }

    /// Show or hide the group-name label text at the leading edge of each
    /// consecutive group run in the tab bar. The colour accent is always
    /// shown regardless of this setting.
    /// Live-applied from `appearance.show_tab_group_labels`.
    pub fn set_show_tab_group_labels(&mut self, enabled: bool) {
        self.show_tab_group_labels = enabled;
    }

    /// Enable or disable the tab bar. When disabled the bar is hidden
    /// completely and its space is reclaimed for the terminal grid.
    pub fn set_tab_bar_enabled(&mut self, enabled: bool) {
        self.tab_bar_enabled = enabled;
    }

    /// Set the tab bar position (`Top` or `Bottom` relative to the grid).
    pub fn set_tab_bar_placement(&mut self, placement: TabBarPlacement) {
        self.tab_bar_placement = placement;
    }

    /// When `true`, the tab bar is automatically hidden while only one tab
    /// is open, and shown again once a second tab appears.
    pub fn set_tab_bar_hide_if_single(&mut self, hide: bool) {
        self.tab_bar_hide_if_single = hide;
    }

    /// Set the width of the vertical tab strip (logical px). Clamped to
    /// `120..=360`. Only affects layout when placement is `Left` or `Right`.
    pub fn set_vertical_tab_bar_width(&mut self, width: f32) {
        self.vertical_tab_bar_width = width.clamp(120.0, 360.0);
    }

    /// Set the cell-width multiplier (clamped to `0.8..=2.0`). Values
    /// above `1.0` widen each cell; below `1.0` narrow it. Triggers a cell
    /// remeasure.
    pub fn set_cell_width_multiplier(&mut self, multiplier: f32) {
        self.cell_width_multiplier = multiplier.clamp(0.8, 2.0);
        self.remeasure_cell();
    }

    /// Logical-px y coordinate of the tab bar top, given the current surface
    /// height. Returns `0.0` for top placement or
    /// `surface_h_logical - TAB_BAR_HEIGHT` for bottom placement.
    /// For vertical placements (`Left`/`Right`) the strip does not have a
    /// y-origin in this sense — returns `0.0` (unused by vertical rendering).
    fn tab_bar_y_logical(&self, surface_h_physical: f32) -> f32 {
        match self.tab_bar_placement {
            TabBarPlacement::Top | TabBarPlacement::Left | TabBarPlacement::Right => 0.0,
            TabBarPlacement::Bottom => {
                let h_log = surface_h_physical / self.scale_factor;
                (h_log - TAB_BAR_HEIGHT).max(0.0)
            }
        }
    }

    /// Logical-px x coordinate of the vertical tab strip left edge.
    /// Returns `0.0` for `Left` placement, `viewport_w - strip_w` for `Right`.
    /// Only meaningful when placement is `Left` or `Right`.
    fn tab_strip_x_logical(&self) -> f32 {
        let viewport_w = self.config.width as f32 / self.scale_factor;
        match self.tab_bar_placement {
            TabBarPlacement::Left => 0.0,
            TabBarPlacement::Right => (viewport_w - self.vertical_tab_bar_width).max(0.0),
            _ => 0.0,
        }
    }

    /// Notify the renderer that the pointer is hovering the close-X of
    /// `pane_id`'s header, or `None` when not hovering any close-X. The
    /// ✕ glyph is tinted red while hovered.
    pub fn set_pane_header_close_hovered(&mut self, id: Option<u32>) {
        self.pane_header_close_hovered = id;
    }

    /// Draw one frame from the emulator state.
    ///
    /// Like [`Self::render_panes`] but also paints divider strokes
    /// between split-pane neighbours. `dividers` is the flat list of
    /// strokes the caller produced from a tree-walk; each entry is
    /// drawn as a single coloured quad at its `rect_px`.
    ///
    /// # Errors
    ///
    /// Bubbles failures from surface acquisition or text-renderer
    /// preparation. Returns `Ok(())` without drawing when `panes` is
    /// empty.
    pub fn render_panes_with_dividers(
        &mut self,
        panes: &[PaneSpec<'_>],
        dividers: &[DividerStroke],
    ) -> Result<(), RenderError> {
        for d in dividers {
            let (x, y, w, h) = d.rect_px;
            self.divider_quads
                .push(Quad::new([x, y], [w, h], d.color, 1.0));
        }
        self.render_panes(panes)
    }

    /// Render one or more panes inside the active tab. The slice is
    /// the active tab's pane-tree flattened to leaves with their
    /// computed sub-rects; for a single-leaf tab it's a 1-element
    /// slice covering the full body area.
    ///
    /// Implementation: the focused pane drives the main `render`
    /// pass with its `body_origin` overridden to its sub-rect's
    /// corner — so its grid, cursor, selection and scrollback chrome
    /// all draw inside that sub-rect. Every NON-focused pane has its
    /// grid + underlines + text snapshot pushed into
    /// `self.extra_pane_*` before that call, so they ride along in
    /// the same frame submission. A 1-px focus border around the
    /// focused pane is queued via `self.focus_border_quads` (only
    /// when there's more than one pane).
    ///
    /// # Errors
    ///
    /// Bubbles failures from surface acquisition or text-renderer
    /// preparation. Returns `Ok(())` without drawing when `panes` is
    /// empty.
    pub fn render_panes(&mut self, panes: &[PaneSpec<'_>]) -> Result<(), RenderError> {
        if panes.is_empty() {
            return Ok(());
        }
        let focused_idx = panes.iter().position(|p| p.focused).unwrap_or(0);
        let scale = self.scale_factor;
        let cw_px = self.cell_width * scale;
        let ch_px = self.cell_height * scale;
        let pad_px = self.padding_px * scale;

        // Build the non-focused panes' grids into `self.extra_pane_*`
        // so the main `render` call sweeps them into the same frame.
        // (Drained inside the render path — no need to clear here.)
        for (idx, spec) in panes.iter().enumerate() {
            if idx == focused_idx {
                continue;
            }
            self.queue_extra_pane(spec, scale, cw_px, ch_px, pad_px);
        }

        // Dim overlay for inactive panes — a translucent black quad drawn
        // over each non-focused pane's rect_px (grid area only; the header
        // strip is excluded by using the grid rect).  Skipped when there is
        // only one pane or when the configured alpha is effectively zero.
        if panes.len() > 1 && self.inactive_pane_dim > 0.01 {
            let alpha = self.inactive_pane_dim;
            for (idx, spec) in panes.iter().enumerate() {
                if idx == focused_idx {
                    continue;
                }
                let (rx, ry, rw, rh) = spec.rect_px;
                // Push into `pane_dim_quads` — drained into the overlay
                // layer (after the text pass) so the tint sits ABOVE all
                // terminal glyphs rather than being overdrawn by them.
                self.pane_dim_quads
                    .push(Quad::new([rx, ry], [rw, rh], [0x00, 0x00, 0x00], alpha));
            }
        }

        // Queue a focus border around the focused pane when there's
        // more than one pane — tells the user where their keystrokes
        // are going. The border is drawn LAST on the main layer (after
        // all pane backgrounds AND divider strokes) so neither a
        // neighbour's bg nor an adjacent divider can overpaint it.
        if panes.len() > 1 {
            let t = self.focus_border_thickness_px;
            // Allow the user to disable the border by setting thickness to 0.
            if t > 0.0 {
                // Snap pane rect to integer physical pixels — defensive
                // against sub-pixel rects from walk_pane_tree floor math.
                let (rx, ry, rw, rh) = panes[focused_idx].rect_px;
                let (fx, fy, fw, fh) = (rx.round(), ry.round(), rw.round(), rh.round());
                // Straddle the pane boundary: each stroke is centred ON the
                // rect edge (half outside, half inside) instead of inset
                // INSIDE the pane — an inset stroke landed right under the
                // first/last text row and column, tinting the glyphs. On
                // internal edges the stroke now recolours the divider band
                // (dead space, iTerm2-style); on window edges it sits in the
                // outer padding. The at-most-t/2 px that still touch the
                // cell area are the outermost edge pixels, where glyph ink
                // doesn't reach — and the glyph pass paints over the stroke
                // anyway (it lives on the main layer, behind text).
                let h = t / 2.0;
                let accent = self.focus_border_color.unwrap_or(ACCENT_FOCUS_BORDER);
                // Translucent stroke: drawn on the main layer (behind the
                // glyph pass) at the configured opacity, so it reads as a
                // background hint instead of a hard frame against the text.
                let a = self.focus_border_alpha.clamp(0.0, 1.0);
                let outer_w = fw + t;
                let outer_h = fh + t;
                // Top
                self.focus_border_quads
                    .push(Quad::new([fx - h, fy - h], [outer_w, t], accent, a));
                // Bottom
                self.focus_border_quads.push(Quad::new(
                    [fx - h, fy + fh - h],
                    [outer_w, t],
                    accent,
                    a,
                ));
                // Left (full height; corners filled by the horizontals)
                self.focus_border_quads
                    .push(Quad::new([fx - h, fy - h], [t, outer_h], accent, a));
                // Right
                self.focus_border_quads.push(Quad::new(
                    [fx + fw - h, fy - h],
                    [t, outer_h],
                    accent,
                    a,
                ));
            }
        }

        // Queue broadcast-indicator borders: an amber tinted rectangle drawn
        // around each pane that is currently receiving mirrored keystrokes.
        // Drawn after the focus border so broadcast always wins on top.
        if !self.broadcast_receiver_ids.is_empty() {
            let t = self.focus_border_thickness_px.max(2.0);
            // Amber colour: distinct from the focus-border blue and visible on
            // both dark and light backgrounds.
            const BROADCAST_ACCENT: [u8; 3] = [0xff, 0xb0, 0x40];
            for spec in panes {
                if !self.broadcast_receiver_ids.contains(&spec.pane_id) {
                    continue;
                }
                let (rx, ry, rw, rh) = spec.rect_px;
                let (fx, fy, fw, fh) = (rx.round(), ry.round(), rw.round(), rh.round());
                // Straddle the pane boundary like the focus border above —
                // an inset stroke tinted the outermost text row/column.
                let h = t / 2.0;
                let outer_w = fw + t;
                let outer_h = fh + t;
                // Top
                self.focus_border_quads.push(Quad::new(
                    [fx - h, fy - h],
                    [outer_w, t],
                    BROADCAST_ACCENT,
                    1.0,
                ));
                // Bottom
                self.focus_border_quads.push(Quad::new(
                    [fx - h, fy + fh - h],
                    [outer_w, t],
                    BROADCAST_ACCENT,
                    1.0,
                ));
                // Left
                self.focus_border_quads.push(Quad::new(
                    [fx - h, fy - h],
                    [t, outer_h],
                    BROADCAST_ACCENT,
                    1.0,
                ));
                // Right
                self.focus_border_quads.push(Quad::new(
                    [fx + fw - h, fy - h],
                    [t, outer_h],
                    BROADCAST_ACCENT,
                    1.0,
                ));
            }
        }

        // Queue per-pane header strips when enabled and the tab has
        // more than one pane. Each spec carries the header_rect_px
        // pre-computed by the App's walk_pane_tree (already physical px).
        if panes.len() > 1 && self.show_pane_headers {
            for spec in panes {
                let Some((hx, hy, hw, hh)) = spec.header_rect_px else {
                    continue;
                };
                let is_focused = spec.focused;

                // Background quad — focused pane gets the active tab
                // pill colour, inactive panes get the strip fill.
                let bg = if is_focused {
                    [0x1a_u8, 0x20, 0x33]
                } else {
                    [0x10_u8, 0x14, 0x1f]
                };
                self.pane_header_quads
                    .push(Quad::new([hx, hy], [hw, hh], bg, 1.0));

                // 1-px bottom separator.
                let sep_color = if is_focused {
                    [0x7d_u8, 0xa6, 0xff] // accent
                } else {
                    [0x22_u8, 0x28, 0x3a]
                };
                let sep_t = (1.0_f32).max(1.0);
                self.pane_header_quads.push(Quad::new(
                    [hx, hy + hh - sep_t],
                    [hw, sep_t],
                    sep_color,
                    1.0,
                ));

                // Title text — logical positions that match the tab
                // bar pattern; the TextArea uses `scale: scale_factor`
                // so glyphon handles the HiDPI multiplication.
                //
                // Close box geometry: 16 px square, 4 px right inset,
                // 3 px top inset — MUST match `pane_header_close_at` in
                // panes.rs so the render box aligns with the hit-test box.
                let close_box_w = 16.0 * scale;
                let close_box_h = 16.0 * scale;
                let close_inset_r = 4.0 * scale;
                let close_inset_y = 3.0 * scale;
                let max_text_w = (hw - close_box_w - close_inset_r - 24.0 * scale).max(1.0);
                let text_h = self.font_size * 0.85 * 1.0;
                let title_y = hy + (hh - text_h).max(0.0) * 0.5;

                let mut title_buf = Buffer::new(
                    &mut self.font_system,
                    Metrics::new(self.font_size * 0.85, self.font_size * 1.0),
                );
                title_buf.set_size(&mut self.font_system, Some(max_text_w), Some(hh));
                let title_color = if is_focused {
                    GlyphonColor::rgb(0xe6, 0xea, 0xf8)
                } else {
                    GlyphonColor::rgb(0xa8, 0xb1, 0xc4)
                };
                title_buf.set_text(
                    &mut self.font_system,
                    spec.title,
                    Attrs::new().family(Family::Monospace).color(title_color),
                    Shaping::Advanced,
                );
                // Position in physical px (the physical text pass reads
                // buffers as-is; only the tab-text pass multiplies by scale).
                let title_x = hx + 10.0 * scale;
                self.pane_header_text_buffers
                    .push((title_buf, [title_x, title_y]));

                // Close-X — crisp vector strokes (same as window controls).
                // Render box aligns with pane_header_close_at hit-test box.
                let bx = hx + hw - close_inset_r - close_box_w;
                let by = hy + close_inset_y;
                let hovered = self.pane_header_close_hovered == Some(spec.pane_id);

                // Optional chip background (Chip style or hovered danger-red).
                let show_chip = matches!(
                    self.close_button_style,
                    terminale_config::CloseButtonStyle::Chip
                );
                if show_chip || hovered {
                    let chip_color = if hovered {
                        system_icons::BG_CLOSE_HOVER
                    } else {
                        darken_tab_bg(bg, 0.72)
                    };
                    self.pane_header_quads.push(Quad::new(
                        [bx, by],
                        [close_box_w, close_box_h],
                        chip_color,
                        1.0,
                    ));
                }

                let x_stroke = if hovered {
                    system_icons::STROKE_CLOSE_HOVER
                } else {
                    [0xb8_u8, 0xc0, 0xd0]
                };
                let x_half = 3.0 * scale;
                let x_cx = bx + close_box_w * 0.5;
                let x_cy = by + close_box_h * 0.5;
                let thickness = system_icons::STROKE_PX * scale;
                for (from, to) in [
                    (
                        [x_cx - x_half, x_cy - x_half],
                        [x_cx + x_half, x_cy + x_half],
                    ),
                    (
                        [x_cx - x_half, x_cy + x_half],
                        [x_cx + x_half, x_cy - x_half],
                    ),
                ] {
                    self.pane_header_quads
                        .push(Quad::line(from, to, thickness, x_stroke, 1.0));
                }
            }
        }

        // Route the focused pane through the main render path with
        // its body-origin overridden to its sub-rect corner.
        let focused = &panes[focused_idx];
        self.pending_body_x = Some(focused.rect_px.0);
        self.pending_body_y = Some(focused.rect_px.1);
        self.scroll_lines = focused.scroll_lines;
        self.render(focused.emulator)
    }

    /// Snapshot a single pane's grid and push its background cells,
    /// per-cell underlines, and per-row text buffers into the
    /// renderer's `extra_pane_*` queues — all positioned at the pane's
    /// sub-rect. The next `render(emu)` call appends these to its own
    /// frame so non-focused panes ride along in the same draw pass.
    fn queue_extra_pane(
        &mut self,
        spec: &PaneSpec<'_>,
        scale: f32,
        cw_px: f32,
        ch_px: f32,
        pad_px: f32,
    ) {
        let emulator = spec.emulator;
        let (cols, rows) = emulator.size();
        let pane_x = spec.rect_px.0;
        let pane_y = spec.rect_px.1;

        // Snapshot the visible grid at this pane's scroll position.
        let mut grid_cells: Vec<Vec<CellSnapshot>> =
            vec![Vec::with_capacity(cols.into()); rows.into()];
        let dim_amount = self.dim_amount;
        let minimum_contrast = self.minimum_contrast;
        emulator.for_each_visible_cell_at_scroll(spec.scroll_lines, |col, row, snap| {
            if let Some(row_buf) = grid_cells.get_mut(row as usize) {
                let mut s = apply_sgr_attributes(snap, dim_amount);
                s.fg = enforce_min_contrast(s.fg, s.bg, minimum_contrast);
                row_buf.push(s);
                let _ = col;
            }
        });

        // Background quads — skip cells that match the window bg.
        for (row_idx, row_cells) in grid_cells.iter().enumerate() {
            for (col_idx, snap) in row_cells.iter().enumerate() {
                if color_close(snap.bg, self.background_rgb, 16) {
                    continue;
                }
                let pos = [
                    pane_x + pad_px + (col_idx as f32) * cw_px,
                    pane_y + (row_idx as f32) * ch_px,
                ];
                self.extra_pane_quads
                    .push(Quad::new(pos, [cw_px, ch_px], snap.bg, 1.0));
            }
        }

        // Per-cell underlines (SGR + OSC 8 hyperlinks). URL
        // autodetect underlines stay focused-pane-only (they live in
        // `extra_underlines` which only the focused emulator sets).
        let underline_thickness = (self.underline_thickness_px * scale).max(1.0);
        let accent_link = [0x7a, 0xa2, 0xf7];
        for (row_idx, row_cells) in grid_cells.iter().enumerate() {
            for (col_idx, snap) in row_cells.iter().enumerate() {
                let style = if snap.has_link {
                    UnderlineStyle::Single
                } else {
                    snap.underline_style
                };
                if style != UnderlineStyle::None {
                    let x = pane_x + pad_px + (col_idx as f32) * cw_px;
                    let baseline_y =
                        pane_y + (row_idx as f32) * ch_px + ch_px - underline_thickness - 1.0;
                    let color = if snap.has_link {
                        accent_link
                    } else {
                        snap.underline_color.unwrap_or(snap.fg)
                    };
                    let mut tmp: Vec<Quad> = Vec::new();
                    emit_underline_quads(
                        &mut tmp,
                        style,
                        x,
                        baseline_y,
                        cw_px,
                        underline_thickness,
                        color,
                    );
                    self.extra_pane_quads.extend(tmp);
                }
                // Strikethrough.
                if snap.strikethrough {
                    let x = pane_x + pad_px + (col_idx as f32) * cw_px;
                    let y =
                        pane_y + (row_idx as f32) * ch_px + ch_px * 0.5 - underline_thickness * 0.5;
                    self.extra_pane_quads.push(Quad::new(
                        [x, y],
                        [cw_px, underline_thickness],
                        snap.fg,
                        1.0,
                    ));
                }
                // Overline (alacritty_terminal 0.24: always false).
                if snap.overline {
                    let x = pane_x + pad_px + (col_idx as f32) * cw_px;
                    let y = pane_y + (row_idx as f32) * ch_px;
                    self.extra_pane_quads.push(Quad::new(
                        [x, y],
                        [cw_px, underline_thickness],
                        snap.fg,
                        1.0,
                    ));
                }
            }
        }

        // Procedural box-drawing / block-element quads. Emitted OUTSIDE the
        // (cached) text pass below: quads are rebuilt every frame, while the
        // shaped text may be reused from the cache — keeping them in the text
        // loop would make box glyphs vanish on every cache hit.
        let builtin_box_drawing_pane = self.builtin_box_drawing;
        if builtin_box_drawing_pane {
            for (row_idx, row_cells) in grid_cells.iter().enumerate() {
                for (col_idx, snap) in row_cells.iter().enumerate() {
                    if snap.hidden || !box_drawing::is_in_range(snap.ch) {
                        continue;
                    }
                    if let Some(rects) = box_drawing::box_rects(snap.ch) {
                        let cell_x = pane_x + pad_px + col_idx as f32 * cw_px;
                        let cell_y = pane_y + row_idx as f32 * ch_px;
                        for r in rects {
                            let qx = cell_x + r.x * cw_px;
                            let qy = cell_y + r.y * ch_px;
                            let qw = (r.w * cw_px).max(1.0);
                            let qh = (r.h * ch_px).max(1.0);
                            self.extra_pane_quads.push(Quad::new(
                                [qx, qy],
                                [qw, qh],
                                snap.fg,
                                r.alpha,
                            ));
                        }
                    }
                }
            }
        }

        // ── Per-row text, cached per pane (same contract as the focused
        // cache): hash every input the row-building loop reads; a hit means
        // the previously shaped buffers are identical, so the full re-shape
        // of every visible row — the dominant cost — is skipped.
        self.extra_pane_cache_seen.push(spec.pane_id);
        let pane_hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            self.font_size.to_bits().hash(&mut h);
            self.line_height.to_bits().hash(&mut h);
            self.font_family.hash(&mut h);
            self.font_bold_family.hash(&mut h);
            self.font_italic_family.hash(&mut h);
            self.font_bold_italic_family.hash(&mut h);
            self.ligatures.hash(&mut h);
            builtin_box_drawing_pane.hash(&mut h);
            cols.hash(&mut h);
            self.cell_width.to_bits().hash(&mut h);
            self.cell_height.to_bits().hash(&mut h);
            pane_x.to_bits().hash(&mut h);
            pane_y.to_bits().hash(&mut h);
            pad_px.to_bits().hash(&mut h);
            ch_px.to_bits().hash(&mut h);
            grid_cells.len().hash(&mut h);
            for row_cells in &grid_cells {
                row_cells.len().hash(&mut h);
                for snap in row_cells {
                    snap.hidden.hash(&mut h);
                    snap.ch.hash(&mut h);
                    snap.fg.hash(&mut h);
                    snap.bold.hash(&mut h);
                    snap.italic.hash(&mut h);
                }
            }
            h.finish()
        };
        if self
            .extra_pane_text_cache
            .get(&spec.pane_id)
            .is_some_and(|(h, bufs)| *h == pane_hash && !bufs.is_empty())
        {
            return;
        }

        let metrics = Metrics::new(self.font_size, self.font_size * self.line_height);
        let family_name = self.font_family.clone();
        let bold_family_name = self.font_bold_family.clone();
        let italic_family_name = self.font_italic_family.clone();
        let bold_italic_family_name = self.font_bold_italic_family.clone();
        let shaping = if self.ligatures {
            Shaping::Advanced
        } else {
            Shaping::Basic
        };
        let mut pane_text: Vec<(Buffer, [f32; 2])> = Vec::with_capacity(rows.into());
        for (row_idx, row_cells) in grid_cells.iter().enumerate() {
            let mut owned: Vec<(String, Attrs<'_>)> = Vec::with_capacity(row_cells.len());
            let mut last_attr: Option<([u8; 3], bool, bool)> = None;
            let mut current = String::new();
            for snap in row_cells {
                // Box-drawing cells get their geometry from the quad pass
                // above; substitute a space so no font glyph paints over it.
                let suppress_for_box = builtin_box_drawing_pane
                    && !snap.hidden
                    && box_drawing::is_in_range(snap.ch)
                    && box_drawing::box_rects(snap.ch).is_some();
                let effective_ch = if suppress_for_box || snap.ch == '\0' {
                    ' '
                } else {
                    snap.ch
                };
                let attr = (snap.fg, snap.bold, snap.italic);
                if last_attr.is_none_or(|a| a == attr) {
                    current.push(effective_ch);
                    last_attr = Some(attr);
                } else {
                    if let Some((fg, bold, italic)) = last_attr {
                        owned.push((
                            current,
                            attr_for(
                                fg,
                                bold,
                                italic,
                                &family_name,
                                bold_family_name.as_deref(),
                                italic_family_name.as_deref(),
                                bold_italic_family_name.as_deref(),
                            ),
                        ));
                    }
                    current = String::new();
                    current.push(effective_ch);
                    last_attr = Some(attr);
                }
            }
            if let Some((fg, bold, italic)) = last_attr {
                if !current.is_empty() {
                    owned.push((
                        current,
                        attr_for(
                            fg,
                            bold,
                            italic,
                            &family_name,
                            bold_family_name.as_deref(),
                            italic_family_name.as_deref(),
                            bold_italic_family_name.as_deref(),
                        ),
                    ));
                }
            }
            if owned.is_empty() {
                continue;
            }
            let mut buf = Buffer::new(&mut self.font_system, metrics);
            buf.set_size(
                &mut self.font_system,
                Some(f32::from(cols) * self.cell_width),
                Some(self.cell_height),
            );
            let spans: Vec<(&str, Attrs<'_>)> =
                owned.iter().map(|(s, a)| (s.as_str(), *a)).collect();
            let default_fam = if family_name.is_empty() {
                Family::Monospace
            } else {
                Family::Name(&family_name)
            };
            buf.set_rich_text(
                &mut self.font_system,
                spans,
                Attrs::new().family(default_fam),
                shaping,
            );
            let y = pane_y + row_idx as f32 * ch_px;
            pane_text.push((buf, [pane_x + pad_px, y]));
        }
        self.extra_pane_text_cache
            .insert(spec.pane_id, (pane_hash, pane_text));
    }

    /// Acquire the next swapchain frame, recovering from a lost/outdated
    /// surface by reconfiguring and retrying once.
    ///
    /// `Lost`/`Outdated` are *expected* lifecycle events — GPU reset (TDR),
    /// driver update, sleep/wake, RDP attach/detach, monitor hot-plug — not
    /// errors. Without the reconfigure the surface never heals and every
    /// subsequent frame fails the same way: a permanently frozen window
    /// until some resize happens to call `configure` again.
    fn acquire_frame(&mut self) -> Result<wgpu::SurfaceTexture, RenderError> {
        match self.surface.get_current_texture() {
            Ok(frame) => Ok(frame),
            Err(e @ (wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated)) => {
                // `Lost` is a *real* device reset — GPU TDR, driver crash,
                // sleep/wake, RDP attach — and is exactly the user-visible
                // "froze for a moment, then fixed itself". Surface it at WARN so
                // it lands in the log file at the default `info` level (with a
                // running total to expose how often it happens). `Outdated` is
                // the benign every-resize case; keep it at debug to avoid spam.
                static SURFACE_LOSSES: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(0);
                if matches!(e, wgpu::SurfaceError::Lost) {
                    let total =
                        SURFACE_LOSSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    tracing::warn!(
                        total,
                        "GPU surface lost (device reset / TDR / sleep-wake); \
                         reconfigured and recovered — this is the transient freeze"
                    );
                } else {
                    tracing::debug!("surface outdated; reconfiguring and retrying");
                }
                self.surface.configure(&self.device, &self.config);
                Ok(self.surface.get_current_texture()?)
            }
            // Timeout = compositor hiccup, skip the frame; OutOfMemory and
            // friends bubble to the caller (logged, frame dropped).
            Err(e) => Err(e.into()),
        }
    }

    /// Single-pane render entry point — kept for the simple cases (the
    /// ghost window, tests, scripts) and reused internally by
    /// [`Self::render_panes`] when it routes the focused pane through.
    ///
    /// # Errors
    ///
    /// Bubbles failures from surface acquisition or text-renderer
    /// preparation.
    pub fn render(&mut self, emulator: &Emulator) -> Result<(), RenderError> {
        let frame = self.acquire_frame()?;
        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("terminale frame encoder"),
            });

        let (cols, rows) = emulator.size();
        let (cursor_col, cursor_row) = emulator.cursor();
        let scale = self.scale_factor;
        let cw_px = self.cell_width * scale;
        let ch_px = self.cell_height * scale;
        let pad_px = self.padding_px * scale;
        let top_pad_px = self.top_offset_logical() * scale;
        // ── Body-origin: where this pane's grid + cursor + selection
        // anchor in surface space. For a single-pane tab the body sits
        // at `(0, top_pad_px)` (right below the tab bar); a multi-pane
        // call from [`Self::render_panes`] sets these to the focused
        // pane's rect_px corner so the focused pane draws inside its
        // sub-rect instead of across the full surface. Non-focused
        // panes' grids land in `self.extra_pane_quads` /
        // `self.extra_pane_text_cache` (populated by
        // `render_panes`) and get appended below.
        let body_x_origin = self.pending_body_x;
        // For a single-pane tab (pending_body_y == None) shift the grid
        // origin down by half the sub-cell vertical remainder so the top gap
        // and bottom gap are equal ("optically centred" grid).  Multi-pane
        // layouts (pending_body_y == Some) manage their own origins and must
        // not receive this correction.
        let body_y_origin = self
            .pending_body_y
            .unwrap_or(top_pad_px + self.grid_top_shift_px(self.config.height));
        let body_x_origin = body_x_origin.unwrap_or(0.0);

        // ── Snapshot the visible grid into a Vec keyed by (col, row) ──
        let mut grid_cells: Vec<Vec<CellSnapshot>> =
            vec![Vec::with_capacity(cols.into()); rows.into()];
        let dim_amount = self.dim_amount;
        let minimum_contrast = self.minimum_contrast;
        emulator.for_each_visible_cell_at_scroll(self.scroll_lines, |col, row, snap| {
            if let Some(row_buf) = grid_cells.get_mut(row as usize) {
                let mut s = apply_sgr_attributes(snap, dim_amount);
                s.fg = enforce_min_contrast(s.fg, s.bg, minimum_contrast);
                row_buf.push(s);
                let _ = col;
            }
        });
        // Snapshot the scrollback length: the selection draw below re-anchors
        // against it, and `set_selection` (called between frames on mouse
        // events) snapshots it as the selection's reference point.
        let history = emulator.history_size();
        self.last_history = history;

        // ── Build background quads ──
        let mut quads: Vec<Quad> = Vec::new();

        // Per-cell ANSI background colours (drawn first — text and cursor
        // overlay on top). Skip cells whose bg equals (or is visually
        // indistinguishable from) the window background, to suppress the
        // ghost-cell glitch.
        for (row_idx, row_cells) in grid_cells.iter().enumerate() {
            for (col_idx, snap) in row_cells.iter().enumerate() {
                if color_close(snap.bg, self.background_rgb, 16) {
                    continue;
                }
                let pos = [
                    body_x_origin + pad_px + (col_idx as f32) * cw_px,
                    body_y_origin + (row_idx as f32) * ch_px,
                ];
                quads.push(Quad::new(pos, [cw_px, ch_px], snap.bg, 1.0));
            }
        }

        // ── Procedural box-drawing / block-element geometry (U+2500–U+259F) ──
        // Emitted into the MAIN quad layer here — after the per-cell
        // backgrounds and BEFORE `main_quad_count` and the bg-quad upload — so
        // the quads sit on top of the cell background yet below the text pass.
        // (The text loop further down substitutes a space for these cells so
        // the font glyph never paints over the quads.) This MUST run before the
        // upload: a previous arrangement pushed these quads in the text loop,
        // which runs after the buffer is uploaded, so they were never drawn.
        if self.builtin_box_drawing {
            for (row_idx, row_cells) in grid_cells.iter().enumerate() {
                for (col_idx, snap) in row_cells.iter().enumerate() {
                    if snap.hidden || !box_drawing::is_in_range(snap.ch) {
                        continue;
                    }
                    if let Some(rects) = box_drawing::box_rects(snap.ch) {
                        let cell_x = body_x_origin + pad_px + col_idx as f32 * cw_px;
                        let cell_y = body_y_origin + row_idx as f32 * ch_px;
                        for r in rects {
                            let qx = cell_x + r.x * cw_px;
                            let qy = cell_y + r.y * ch_px;
                            let qw = (r.w * cw_px).max(1.0);
                            let qh = (r.h * ch_px).max(1.0);
                            quads.push(Quad::new([qx, qy], [qw, qh], snap.fg, r.alpha));
                        }
                    }
                }
            }
        }

        // ── Underlines: SGR-underline, OSC 8 hyperlinks, autodetected URLs ─
        // `underline_thickness` is the physical-pixel stroke width derived from
        // the user's `font.underline_thickness_px` config, scaled by the DPI
        // factor and floored to at least 1 physical pixel.
        let underline_thickness = (self.underline_thickness_px * scale).max(1.0);
        let accent = [0x7a, 0xa2, 0xf7];
        for (row_idx, row_cells) in grid_cells.iter().enumerate() {
            for (col_idx, snap) in row_cells.iter().enumerate() {
                let style = if snap.has_link {
                    // Link overrides the SGR style with a plain single underline
                    // drawn in the accent colour.
                    UnderlineStyle::Single
                } else {
                    snap.underline_style
                };
                if style != UnderlineStyle::None {
                    let x = body_x_origin + pad_px + (col_idx as f32) * cw_px;
                    let baseline_y = body_y_origin + (row_idx as f32) * ch_px + ch_px
                        - underline_thickness
                        - 1.0;
                    let color = if snap.has_link {
                        accent
                    } else {
                        snap.underline_color.unwrap_or(snap.fg)
                    };
                    emit_underline_quads(
                        &mut quads,
                        style,
                        x,
                        baseline_y,
                        cw_px,
                        underline_thickness,
                        color,
                    );
                }
                // Strikethrough: mid-cell horizontal bar (SGR 9).
                if snap.strikethrough {
                    let x = body_x_origin + pad_px + (col_idx as f32) * cw_px;
                    let y = body_y_origin + (row_idx as f32) * ch_px + ch_px * 0.5
                        - underline_thickness * 0.5;
                    quads.push(Quad::new(
                        [x, y],
                        [cw_px, underline_thickness],
                        snap.fg,
                        1.0,
                    ));
                }
                // Overline: top-of-cell horizontal bar (SGR 53).
                // alacritty_terminal 0.24 does not track overline; this fires
                // only when a future backend or custom handler sets the flag.
                if snap.overline {
                    let x = body_x_origin + pad_px + (col_idx as f32) * cw_px;
                    let y = body_y_origin + (row_idx as f32) * ch_px;
                    quads.push(Quad::new(
                        [x, y],
                        [cw_px, underline_thickness],
                        snap.fg,
                        1.0,
                    ));
                }
            }
        }
        for &(col_start, col_end, row) in &self.extra_underlines {
            let row_idx = row as usize;
            if row_idx >= grid_cells.len() {
                continue;
            }
            let x = body_x_origin + pad_px + (col_start as f32) * cw_px;
            let span = (col_end.saturating_sub(col_start) as f32 + 1.0) * cw_px;
            let y = body_y_origin + (row_idx as f32) * ch_px + ch_px - underline_thickness - 1.0;
            quads.push(Quad::new([x, y], [span, underline_thickness], accent, 1.0));
        }

        // ── Prompt-status gutter dots (OSC 133) ───────────────────────
        // A small filled square in the left margin: neutral grey when the exit
        // status is unknown, green for exit 0, red for any non-zero exit.
        // Only rendered when `set_prompt_marks` has been called with a
        // non-empty slice by the host (i.e. `show_prompt_marks` is on).
        if !self.prompt_marks.is_empty() {
            let dot_size_px = (4.0 * scale).max(2.0);
            // Centre the dot vertically in the cell, flush to the left edge.
            let dot_x = body_x_origin;
            for &(row, exit_code) in &self.prompt_marks {
                let row_idx = row as usize;
                if row_idx >= grid_cells.len() {
                    continue;
                }
                let dot_y = body_y_origin + (row_idx as f32) * ch_px + (ch_px - dot_size_px) * 0.5;
                let color = match exit_code {
                    None => [0x66u8, 0x70, 0x8a],    // neutral grey
                    Some(0) => [0x4fu8, 0xc0, 0x6a], // green
                    Some(_) => [0xe0u8, 0x4f, 0x4f], // red
                };
                quads.push(Quad::new(
                    [dot_x, dot_y],
                    [dot_size_px, dot_size_px],
                    color,
                    0.9,
                ));
            }
        }

        // ── Search bar ────────────────────────────────────────────────
        // A 28-logical-px bar pinned to the bottom showing the query
        // and current/total matches. Drawn before the bell flash so the
        // bell tint covers it for visual consistency.
        let mut search_text: Option<(String, f32, f32)> = None;
        if let Some(overlay) = &self.search_overlay {
            let bar_h_log = 30.0;
            let bar_h_px = bar_h_log * scale;
            let bar_y_px = self.config.height as f32 - bar_h_px;
            // Background rectangle.
            quads.push(Quad::new(
                [0.0, bar_y_px],
                [self.config.width as f32, bar_h_px],
                [0x18, 0x1c, 0x2c],
                0.97,
            ));
            // Top-edge accent line.
            quads.push(Quad::new(
                [0.0, bar_y_px],
                [self.config.width as f32, (1.0 * scale).max(1.0)],
                [0x7a, 0xa2, 0xf7],
                0.85,
            ));
            let status = if overlay.total == 0 {
                if overlay.query.is_empty() {
                    "type to search".to_string()
                } else {
                    "no matches".to_string()
                }
            } else {
                format!("{}/{}", overlay.current, overlay.total)
            };
            let text = format!(
                "Find: {}    {}    Enter next  ·  Shift+Enter prev  ·  Esc close",
                overlay.query, status,
            );
            search_text = Some((
                text,
                12.0 * scale,
                bar_y_px + (bar_h_px - self.font_size * 1.1) * 0.5,
            ));
        }
        let mut search_text_buffers: Vec<(Buffer, [f32; 2])> = Vec::new();
        if let Some((text, tx, ty)) = search_text {
            let mut buf = Buffer::new(
                &mut self.font_system,
                Metrics::new(self.font_size, self.font_size * 1.1),
            );
            buf.set_size(
                &mut self.font_system,
                Some(self.config.width as f32),
                Some(self.font_size * 1.2),
            );
            buf.set_text(
                &mut self.font_system,
                &text,
                Attrs::new()
                    .family(Family::Monospace)
                    .color(GlyphonColor::rgb(0xe6, 0xea, 0xf8)),
                Shaping::Advanced,
            );
            search_text_buffers.push((buf, [tx, ty]));
        }

        // ── Proactive AI command-suggestion bar ──────────────────────
        // Clean flat strip pinned to the bottom (panel + accent quads here,
        // text returned for the prepare pass below). No-op while hidden or
        // while the search bar owns the bottom.
        let suggestion_text_buffers = self.build_suggestion_bar(scale, &mut quads);

        // ── Bottom resource-indicator strip (CPU/RAM/GPU, pixel-art) ──
        let resource_text_buffers = self.build_resource_bar(scale, &mut quads);

        // ── Hover tooltip (URL preview etc.) ─────────────────────────
        let mut tooltip_text_buffer: Option<(Buffer, [f32; 2])> = None;
        if let Some(t) = &self.tooltip {
            // Rough autosize: 8.5 logical px per char, 26 px tall.
            let char_w = 8.5 * scale;
            let pad_x = 8.0 * scale;
            let pad_y = 4.0 * scale;
            let chars = t.text.chars().count() as f32;
            let tip_w = (chars * char_w + pad_x * 2.0).min(self.config.width as f32 - 24.0 * scale);
            let tip_h = (self.font_size * 1.2 + pad_y * 2.0) * scale;
            // Anchor under-and-to-the-right of the cursor by ~16 px.
            let mut x = t.anchor_px[0] + 12.0 * scale;
            let mut y = t.anchor_px[1] + 18.0 * scale;
            // Keep on screen.
            if x + tip_w > self.config.width as f32 {
                x = (self.config.width as f32 - tip_w).max(0.0);
            }
            if y + tip_h > self.config.height as f32 {
                y = (t.anchor_px[1] - tip_h - 6.0 * scale).max(0.0);
            }
            quads.push(Quad::new([x, y], [tip_w, tip_h], [0x18, 0x1c, 0x2c], 0.97));
            // 1px border accent.
            quads.push(Quad::new(
                [x, y],
                [tip_w, (1.0 * scale).max(1.0)],
                [0x7a, 0xa2, 0xf7],
                0.85,
            ));
            // Build buffer for the tooltip text.
            let mut buf = Buffer::new(
                &mut self.font_system,
                Metrics::new(self.font_size, self.font_size * 1.2),
            );
            buf.set_size(
                &mut self.font_system,
                Some(tip_w - pad_x * 2.0),
                Some(tip_h),
            );
            buf.set_text(
                &mut self.font_system,
                &t.text,
                Attrs::new()
                    .family(Family::Monospace)
                    .color(GlyphonColor::rgb(0xe6, 0xea, 0xf8)),
                Shaping::Advanced,
            );
            tooltip_text_buffer = Some((buf, [x + pad_x, y + pad_y]));
        }

        // ── Jump-highlight band ───────────────────────────────────────
        // A brief, fading one-row tint drawn over the target prompt row
        // after a prompt-navigation jump. Off the hot-path: the App
        // pre-computes the viewport row and alpha; we just draw the quad.
        if let Some((vp_row, alpha)) = self.jump_highlight_band {
            if alpha > 0.0 {
                let row_y = body_y_origin + f32::from(vp_row) * ch_px;
                quads.push(Quad::new(
                    [body_x_origin, row_y],
                    [self.config.width as f32 - body_x_origin, ch_px],
                    [0x7a, 0xa2, 0xf7], // accent blue (matches the default accent)
                    alpha.clamp(0.0, 0.35),
                ));
            }
        }

        // ── Visual bell flash ────────────────────────────────────────
        // A short, decaying full-window tint when an app emits BEL. Sits
        // on top of all text so it's unmistakeable, even on dark themes.
        if let Some(t) = self.bell_start {
            let elapsed = t.elapsed().as_millis() as u64;
            if elapsed < BELL_DURATION_MS {
                let progress = elapsed as f32 / BELL_DURATION_MS as f32;
                let alpha = (1.0 - progress) * 0.28;
                quads.push(Quad::new(
                    [0.0, 0.0],
                    [self.config.width as f32, self.config.height as f32],
                    [0xe6, 0xea, 0xf8],
                    alpha.clamp(0.0, 1.0),
                ));
            } else {
                self.bell_start = None;
            }
        }

        // ── Cursor ────────────────────────────────────────────────────
        // Hidden entirely while panning into the scrollback — the live
        // cursor isn't where the user is currently looking, drawing it
        // there would be confusing. Also hidden if the focused app
        // emitted DECSCUSR with shape=Hidden.
        let app_shape = emulator.cursor_shape();
        // The user's config sets the *default* shape; the app can override
        // it with anything other than Block. This keeps the shell prompt
        // looking how the user configured it while still letting vim's
        // insert-mode beam show through.
        let effective_shape = match app_shape {
            Some(AppCursorShape::Block) | None => self.cursor.style,
            Some(AppCursorShape::Underline) => CursorStyle::Underline,
            Some(AppCursorShape::Beam) => CursorStyle::Beam,
            Some(AppCursorShape::HollowBlock) => CursorStyle::OutlineBlock,
        };
        let cursor_hidden_by_app = app_shape.is_none();
        let in_scrollback = self.scroll_lines > 0;
        let cursor_x = body_x_origin + pad_px + f32::from(cursor_col) * cw_px;
        let cursor_y = body_y_origin + f32::from(cursor_row) * ch_px;
        let accent = self
            .cursor
            .color
            .or(self.cursor_theme_color)
            .unwrap_or([0x7d, 0xa6, 0xff]);
        let opacity = self.cursor.opacity.clamp(0.0, 1.0);

        // Blink: compute an effective opacity multiplier that is either
        // hard on/off (default) or a smooth fade using smoothstep easing.
        // When not blinking or unfocused, `blink_alpha = 1.0`.
        let (blink_off, blink_alpha) = if self.focused && self.cursor.blink {
            let elapsed = self.cursor_start.elapsed().as_millis() as u64;
            let period = u64::from(self.cursor.blink_rate_ms) * 2;
            if period == 0 {
                (false, 1.0_f32)
            } else if self.cursor.blink_ease {
                // Smoothstep easing: compute a [0..1] alpha based on the
                // blink phase. Phase `p ∈ [0,1)`: first half fades in,
                // second half fades out, producing a smooth pulsing glow.
                let phase = (elapsed % period) as f32 / period as f32; // 0..1
                let tri = if phase < 0.5 {
                    phase * 2.0 // 0→1 on first half
                } else {
                    (1.0 - phase) * 2.0 // 1→0 on second half
                };
                // smoothstep(0, 1, tri) = tri*tri*(3 - 2*tri)
                let alpha = tri * tri * (3.0 - 2.0 * tri);
                (false, alpha) // never full "off" — just very dim
            } else {
                let off = (elapsed % period) >= u64::from(self.cursor.blink_rate_ms);
                (off, 1.0)
            }
        } else {
            (false, 1.0)
        };

        if in_scrollback || cursor_hidden_by_app {
            // Skip cursor rendering entirely.
        } else if !self.focused {
            // 1-pixel outline border. Always shown so an unfocused
            // window still tells you where typing would land.
            let t = (1.0 * scale).max(1.0);
            quads.push(Quad::new([cursor_x, cursor_y], [cw_px, t], accent, 0.85));
            quads.push(Quad::new(
                [cursor_x, cursor_y + ch_px - t],
                [cw_px, t],
                accent,
                0.85,
            ));
            quads.push(Quad::new([cursor_x, cursor_y], [t, ch_px], accent, 0.85));
            quads.push(Quad::new(
                [cursor_x + cw_px - t, cursor_y],
                [t, ch_px],
                accent,
                0.85,
            ));
        } else if !blink_off {
            // Effective opacity is the user-set opacity multiplied by the
            // blink alpha (1.0 for hard blink; smoothstep value for easing).
            let opacity = opacity * blink_alpha;
            // Cell tint (optional).
            let tint = self.cursor.cell_tint_opacity.clamp(0.0, 1.0) * blink_alpha;
            if tint > 0.0 {
                quads.push(Quad::new(
                    [cursor_x, cursor_y],
                    [cw_px, ch_px],
                    accent,
                    tint,
                ));
            }
            let thickness = (self.cursor.thickness_px * scale).max(1.0);
            match effective_shape {
                CursorStyle::Block => {
                    quads.push(Quad::new(
                        [cursor_x, cursor_y],
                        [cw_px, ch_px],
                        accent,
                        opacity,
                    ));
                }
                CursorStyle::OutlineBlock => {
                    let t = thickness;
                    quads.push(Quad::new([cursor_x, cursor_y], [cw_px, t], accent, opacity));
                    quads.push(Quad::new(
                        [cursor_x, cursor_y + ch_px - t],
                        [cw_px, t],
                        accent,
                        opacity,
                    ));
                    quads.push(Quad::new([cursor_x, cursor_y], [t, ch_px], accent, opacity));
                    quads.push(Quad::new(
                        [cursor_x + cw_px - t, cursor_y],
                        [t, ch_px],
                        accent,
                        opacity,
                    ));
                }
                CursorStyle::Underline => {
                    quads.push(Quad::new(
                        [cursor_x, cursor_y + ch_px - thickness],
                        [cw_px, thickness],
                        accent,
                        opacity,
                    ));
                }
                CursorStyle::Beam => {
                    quads.push(Quad::new(
                        [cursor_x, cursor_y],
                        [thickness, ch_px],
                        accent,
                        opacity,
                    ));
                }
            }
        }

        // Selection highlight (drawn after bg so it tints them).
        if let Some(sel) = self.selection {
            // The selection rect is viewport-relative AS OF the moment it was
            // made. Re-anchor it to the text (see `reanchored_row`): the
            // highlight stays glued to the selected text both while panning
            // through history and while new output streams in at the live
            // bottom. (Once the scrollback ring is full and rotating, H stops
            // growing and the oldest selected text genuinely scrolls away —
            // the highlight follows it off-screen.)
            for (col, row) in sel.cells() {
                let Some(row_s) = reanchored_row(
                    row,
                    self.selection_scroll,
                    self.selection_history,
                    self.scroll_lines,
                    history,
                ) else {
                    continue; // shifted above the viewport
                };
                if row_s >= grid_cells.len() {
                    continue; // shifted below the viewport
                }
                let row_len = grid_cells[row_s].len();
                let max_col = u16::try_from(row_len.saturating_sub(1)).unwrap_or(0);
                if col > max_col {
                    continue;
                }
                #[allow(clippy::cast_precision_loss)]
                let pos = [
                    body_x_origin + pad_px + f32::from(col) * cw_px,
                    body_y_origin + row_s as f32 * ch_px,
                ];
                quads.push(Quad::new(
                    pos,
                    [cw_px, ch_px],
                    self.selection_rgb,
                    self.selection_opacity,
                ));
            }
        }

        // Menu overlay (background panel + per-item background for hovered).
        let mut overlay_text_areas: Vec<(Buffer, [f32; 2])> = Vec::new();
        if let Some(overlay) = &self.overlay {
            let item_h_logical = (self.cell_height * 1.7).max(28.0);
            let item_h_px = item_h_logical * scale;
            let menu_w_logical = overlay.width_px;
            let menu_w_px = menu_w_logical * scale;
            let separator_count =
                overlay.items.iter().filter(|i| i.separator_before).count() as f32;
            let menu_inner_pad_logical = 8.0;
            let menu_h_logical = item_h_logical * overlay.items.len() as f32
                + menu_inner_pad_logical * 2.0
                + separator_count * 8.0;
            let menu_h_px = menu_h_logical * scale;
            let origin_logical = overlay.origin_px;
            let origin_px = [origin_logical[0] * scale, origin_logical[1] * scale];

            // Soft drop shadow (slightly larger, lower opacity).
            quads.push(Quad::new(
                [origin_px[0] + 6.0 * scale, origin_px[1] + 10.0 * scale],
                [menu_w_px, menu_h_px],
                [0x00, 0x00, 0x00],
                0.30,
            ));
            quads.push(Quad::new(
                [origin_px[0] + 2.0 * scale, origin_px[1] + 4.0 * scale],
                [menu_w_px, menu_h_px],
                [0x00, 0x00, 0x00],
                0.20,
            ));

            // 1px border (slightly larger panel painted under the panel).
            let border_thickness = 1.0 * scale;
            quads.push(Quad::new(
                [
                    origin_px[0] - border_thickness,
                    origin_px[1] - border_thickness,
                ],
                [
                    menu_w_px + border_thickness * 2.0,
                    menu_h_px + border_thickness * 2.0,
                ],
                [0x34, 0x3a, 0x4c],
                1.0,
            ));

            // Panel.
            quads.push(Quad::new(
                origin_px,
                [menu_w_px, menu_h_px],
                [0x1a, 0x1d, 0x2a],
                0.98,
            ));

            // Subtle top highlight (1px lighter line).
            quads.push(Quad::new(
                origin_px,
                [menu_w_px, 1.0 * scale],
                [0x3b, 0x42, 0x5a],
                0.8,
            ));

            // Iterate items, accumulating y as we go (so separators offset
            // subsequent items).
            let mut y_logical = origin_logical[1] + menu_inner_pad_logical;
            for (idx, item) in overlay.items.iter().enumerate() {
                if item.separator_before && idx > 0 {
                    // 1px subtle separator with horizontal padding.
                    let sep_pad = 12.0 * scale;
                    let sep_y = (y_logical + 3.5) * scale;
                    quads.push(Quad::new(
                        [origin_px[0] + sep_pad, sep_y],
                        [menu_w_px - sep_pad * 2.0, 1.0 * scale],
                        [0x2f, 0x35, 0x47],
                        1.0,
                    ));
                    y_logical += 8.0;
                }

                let item_y_logical = y_logical;
                let item_y_px = item_y_logical * scale;

                if Some(idx) == overlay.hovered && item.enabled {
                    // Hovered: subtle blue tint + accent bar on the left.
                    quads.push(Quad::new(
                        [origin_px[0], item_y_px],
                        [menu_w_px, item_h_px],
                        [0x2c, 0x3c, 0x6a],
                        0.85,
                    ));
                    let bar_w = 3.0 * scale;
                    quads.push(Quad::new(
                        [origin_px[0], item_y_px],
                        [bar_w, item_h_px],
                        [0x7d, 0xa6, 0xff],
                        1.0,
                    ));
                }

                // Text — icon glyph + label, hotkey right-aligned.
                let text_color = if item.enabled {
                    GlyphonColor::rgb(0xe6, 0xea, 0xf8)
                } else {
                    GlyphonColor::rgb(0x55, 0x5b, 0x70)
                };
                let hotkey_color = GlyphonColor::rgb(0x70, 0x77, 0x8c);

                let text_baseline_offset = (item_h_logical - self.cell_height) * 0.5;
                let text_y_logical = item_y_logical + text_baseline_offset;

                // Main label (icon + text).
                let mut label_buf = Buffer::new(
                    &mut self.font_system,
                    Metrics::new(self.font_size, self.font_size * self.line_height),
                );
                label_buf.set_size(
                    &mut self.font_system,
                    Some(menu_w_logical - 24.0),
                    Some(self.cell_height),
                );
                let label_text = match &item.icon {
                    Some(icon) => format!("{}  {}", icon, item.label),
                    None => item.label.clone(),
                };
                label_buf.set_text(
                    &mut self.font_system,
                    &label_text,
                    Attrs::new().family(Family::Monospace).color(text_color),
                    Shaping::Advanced,
                );
                overlay_text_areas.push((label_buf, [origin_logical[0] + 16.0, text_y_logical]));

                // Hotkey (right-aligned).
                if let Some(hot) = &item.hotkey {
                    let mut hot_buf = Buffer::new(
                        &mut self.font_system,
                        Metrics::new(
                            self.font_size * 0.85,
                            self.font_size * 0.85 * self.line_height,
                        ),
                    );
                    hot_buf.set_size(
                        &mut self.font_system,
                        Some(menu_w_logical - 24.0),
                        Some(self.cell_height),
                    );
                    hot_buf.set_text(
                        &mut self.font_system,
                        hot,
                        Attrs::new().family(Family::Monospace).color(hotkey_color),
                        Shaping::Advanced,
                    );
                    // Compute width to right-align — fall back to a heuristic.
                    let est_width = hot.chars().count() as f32 * self.cell_width * 0.7;
                    let hot_x = origin_logical[0] + menu_w_logical - est_width - 14.0;
                    overlay_text_areas.push((hot_buf, [hot_x, text_y_logical + 1.0]));
                }

                y_logical += item_h_logical;
            }
        }

        // ── Tab bar ──
        let mut tab_text_areas: Vec<(Buffer, [f32; 2])> = Vec::new();
        let tab_bar_visible = self.tab_bar_visible_logical() > 0.0;
        // Clone the tab bar to decouple the immutable borrow of `self.tab_bar`
        // from the mutable borrows of `self.font_system` needed to build text
        // buffers below. The clone is a `Vec<TabBarItem>` of modest size; in
        // practice it is only taken when the bar is visible.
        let tab_bar_snapshot = self.tab_bar.clone();
        if let Some(bar) = &tab_bar_snapshot {
            if tab_bar_visible {
                // ── Route to vertical or horizontal rendering ────────────────────
                if self.tab_bar_placement.is_vertical() {
                    self.render_vertical_tab_strip(bar, scale, &mut quads, &mut tab_text_areas);
                } else {
                    // Physical-pixel y of the top of the tab bar strip.
                    let tab_bar_y_px = self.tab_bar_y_logical(self.config.height as f32) * scale;

                    // Background strip for the entire tab bar.
                    quads.push(Quad::new(
                        [0.0, tab_bar_y_px],
                        [self.config.width as f32, TAB_BAR_HEIGHT * scale],
                        [0x07, 0x09, 0x0e],
                        1.0,
                    ));
                    // Separator line (bottom edge for top bar, top edge for bottom bar).
                    let sep_y = match self.tab_bar_placement {
                        TabBarPlacement::Top => tab_bar_y_px + (TAB_BAR_HEIGHT - 1.0) * scale,
                        TabBarPlacement::Bottom
                        | TabBarPlacement::Left
                        | TabBarPlacement::Right => tab_bar_y_px,
                    };
                    quads.push(Quad::new(
                        [0.0, sep_y],
                        [self.config.width as f32, 1.0 * scale],
                        [0x22, 0x28, 0x3a],
                        1.0,
                    ));

                    // Logical-pixel y offset for every element inside the tab bar.
                    // `0.0` for Top placement; `surface_h_log - TAB_BAR_HEIGHT` for Bottom.
                    let tab_y_log = tab_bar_y_px / scale;

                    let layout = self.tab_layout(bar);
                    let tab_rects = &layout.tabs;
                    let plus_rect = &layout.plus;
                    for (idx, ((tab_rect, close_rect), item)) in
                        tab_rects.iter().zip(&bar.items).enumerate()
                    {
                        let active = item.active;
                        let hovered = bar.hovered == Some(idx);

                        // (Group boundary separator removed — replaced by pill + spines)

                        // Tab pill background — use the context-rule tint when set.
                        let base_bg: [u8; 3] = if active {
                            [0x1a, 0x20, 0x33]
                        } else if hovered {
                            [0x18, 0x1f, 0x33]
                        } else {
                            [0x0a, 0x0c, 0x14]
                        };
                        let bg_color: [u8; 3] = if let Some(tint) = item.color {
                            // Blend the tint at 40 % so the pill stays readable at any
                            // brightness while still being distinctly coloured.
                            blend_tint(base_bg, tint, 0.40)
                        } else {
                            base_bg
                        };
                        let bg_alpha = 1.0_f32;
                        quads.push(Quad::new(
                            [tab_rect.x * scale, (tab_y_log + tab_rect.y) * scale],
                            [tab_rect.w * scale, tab_rect.h * scale],
                            bg_color,
                            bg_alpha,
                        ));
                        // When a context-rule tint is active, draw a 2-px accent strip
                        // along the bottom of the pill in the raw tint colour so the
                        // colour reads clearly even on the blended background.
                        if let Some(tint) = item.color {
                            quads.push(Quad::new(
                                [
                                    tab_rect.x * scale,
                                    (tab_y_log + tab_rect.y + tab_rect.h - 2.0) * scale,
                                ],
                                [tab_rect.w * scale, 2.0 * scale],
                                tint,
                                1.0,
                            ));
                        } else if active {
                            // 2-px accent bar at the bottom of the active tab.
                            quads.push(Quad::new(
                                [
                                    tab_rect.x * scale,
                                    (tab_y_log + tab_rect.y + tab_rect.h - 2.0) * scale,
                                ],
                                [tab_rect.w * scale, 2.0 * scale],
                                [0x7d, 0xa6, 0xff],
                                1.0,
                            ));
                        } else if item.unread {
                            // Small accent dot in the top-right of inactive tabs
                            // that have produced output since the user last looked.
                            let dot = 6.0 * scale;
                            quads.push(Quad::new(
                                [
                                    (tab_rect.x + tab_rect.w - 14.0) * scale,
                                    (tab_y_log + tab_rect.y + 6.0) * scale,
                                ],
                                [dot, dot],
                                [0x7d, 0xa6, 0xff],
                                1.0,
                            ));
                        }

                        // Static "waiting for input" dot — bottom-right of tabs
                        // whose program rang the bell (e.g. Claude Code awaiting
                        // input). Amber, opposite corner from the blue unread
                        // dot so they never collide. Independent of the
                        // accent/unread chain so it shows on tinted tabs too.
                        // Visibility is decided build-side (see TabBarItem),
                        // so no `!active` guard here.
                        if item.attention {
                            let dot = 6.0 * scale;
                            quads.push(Quad::new(
                                [
                                    (tab_rect.x + tab_rect.w - 14.0) * scale,
                                    (tab_y_log + tab_rect.y + tab_rect.h - 12.0) * scale,
                                ],
                                [dot, dot],
                                [0xe0, 0x90, 0x30],
                                1.0,
                            ));
                        }

                        // ── Group accent: 4-px top stripe + 3-px left spine (+right
                        // spine on the last tab of a run). The bracket reads clearly
                        // across consecutive same-group tabs and is distinct from the
                        // active-tab underline highlight.
                        if let Some(ga) = item.group_accent {
                            // Top stripe.
                            quads.push(Quad::new(
                                [tab_rect.x * scale, (tab_y_log + tab_rect.y) * scale],
                                [tab_rect.w * scale, 4.0 * scale],
                                ga,
                                1.0,
                            ));
                            // Left spine (always — every grouped tab).
                            quads.push(Quad::new(
                                [tab_rect.x * scale, (tab_y_log + tab_rect.y) * scale],
                                [3.0 * scale, tab_rect.h * scale],
                                ga,
                                1.0,
                            ));
                            // Right spine — only on the last tab of this group run
                            // (next item has a different or no group accent).
                            let next_accent = bar.items.get(idx + 1).and_then(|n| n.group_accent);
                            let is_run_end = next_accent != Some(ga);
                            if is_run_end {
                                quads.push(Quad::new(
                                    [
                                        (tab_rect.x + tab_rect.w - 3.0) * scale,
                                        (tab_y_log + tab_rect.y) * scale,
                                    ],
                                    [3.0 * scale, tab_rect.h * scale],
                                    ga,
                                    1.0,
                                ));
                            }
                        }
                        // (On-tab group label removed — label is now on the floating pill)

                        if item.pinned {
                            // ── Compact pinned tab: pin-anchor glyph centred, no
                            // close-X, no text label.  A small "⊕" or the user icon
                            // (if set) fills the pill centre so the chip is
                            // unmistakably different from a normal tab.
                            let pin_text = item.icon.as_deref().unwrap_or("⊕");
                            let mut pin_buf = Buffer::new(
                                &mut self.font_system,
                                Metrics::new(self.font_size * 0.92, self.font_size * 1.1),
                            );
                            pin_buf.set_wrap(&mut self.font_system, Wrap::None);
                            pin_buf.set_size(
                                &mut self.font_system,
                                Some(tab_rect.w),
                                Some(self.font_size * 1.1),
                            );
                            let pin_color = if active {
                                GlyphonColor::rgb(0xe6, 0xea, 0xf8)
                            } else {
                                GlyphonColor::rgb(0xa8, 0xb1, 0xc4)
                            };
                            pin_buf.set_text(
                                &mut self.font_system,
                                pin_text,
                                Attrs::new().family(Family::Monospace).color(pin_color),
                                Shaping::Advanced,
                            );
                            // Centre the single glyph horizontally in the pill.
                            let pin_x = tab_rect.x + (tab_rect.w - self.font_size * 0.7) * 0.5;
                            let pin_y =
                                tab_y_log + tab_rect.y + (tab_rect.h - self.font_size * 1.1) * 0.5;
                            tab_text_areas.push((pin_buf, [pin_x, pin_y]));
                            // Small pin-indicator triangle in the top-left corner so
                            // the pinned state is clear even on very narrow chips.
                            let tri_size = 5.0 * scale;
                            quads.push(Quad::new(
                                [tab_rect.x * scale, (tab_y_log + tab_rect.y) * scale],
                                [tri_size, tri_size],
                                item.color.unwrap_or([0x7d, 0xa6, 0xff]),
                                1.0,
                            ));
                        } else {
                            // ── Close-button chip + vector X (Part A) ───────────────
                            // Square chip centred on the close_rect — 18 px square,
                            // optically centred. Uses a single Quad::new (no octagon).
                            let disc_cx = (close_rect.x + close_rect.w * 0.5) * scale;
                            let disc_cy = (tab_y_log + close_rect.y + close_rect.h * 0.5) * scale;
                            let disc_d = 18.0 * scale;
                            let disc_color = darken_tab_bg(bg_color, 0.72);
                            let show_chip = matches!(
                                self.close_button_style,
                                terminale_config::CloseButtonStyle::Chip
                            );
                            if show_chip {
                                quads.push(Quad::new(
                                    [disc_cx - disc_d * 0.5, disc_cy - disc_d * 0.5],
                                    [disc_d, disc_d],
                                    disc_color,
                                    1.0,
                                ));
                            }

                            // Tab text: icon + label (Part B — single line, with ellipsis)
                            let raw_label = match &item.icon {
                                Some(icon) => format!("{}  {}", icon, item.label),
                                None => item.label.clone(),
                            };
                            let avail_w = (tab_rect.w - 36.0).max(0.0);
                            let approx_char_w = self.font_size * 0.6;
                            let max_chars = (avail_w / approx_char_w).floor() as usize;
                            let label_text = truncate_tab_title(&raw_label, max_chars);

                            let mut buf = Buffer::new(
                                &mut self.font_system,
                                Metrics::new(self.font_size * 0.92, self.font_size * 1.1),
                            );
                            buf.set_wrap(&mut self.font_system, Wrap::None);
                            buf.set_size(
                                &mut self.font_system,
                                Some(avail_w),
                                Some(self.font_size * 1.1),
                            );
                            let text_color = if active {
                                GlyphonColor::rgb(0xe6, 0xea, 0xf8)
                            } else {
                                GlyphonColor::rgb(0xa8, 0xb1, 0xc4)
                            };
                            buf.set_text(
                                &mut self.font_system,
                                &label_text,
                                Attrs::new().family(Family::Monospace).color(text_color),
                                Shaping::Advanced,
                            );
                            let text_x = tab_rect.x + 12.0;
                            let text_y =
                                tab_y_log + tab_rect.y + (tab_rect.h - self.font_size * 1.1) * 0.5;
                            tab_text_areas.push((buf, [text_x, text_y]));

                            // ── Context-rule badge (top-left of the pill) ────────────────
                            if let Some(badge_text) = &item.badge {
                                if !badge_text.is_empty() {
                                    let badge_tint = item.color.unwrap_or([0xc0, 0x50, 0x50]);
                                    // Small pill background for the badge.
                                    let badge_font_size = self.font_size * 0.62;
                                    let badge_h = badge_font_size * 1.3;
                                    let badge_w =
                                        badge_font_size * badge_text.chars().count() as f32 * 0.72
                                            + 6.0;
                                    let badge_x = tab_rect.x + 4.0;
                                    let badge_y = tab_y_log + tab_rect.y + 2.0;
                                    quads.push(Quad::new(
                                        [badge_x * scale, badge_y * scale],
                                        [badge_w * scale, badge_h * scale],
                                        badge_tint,
                                        0.90,
                                    ));
                                    // Badge text.
                                    let mut badge_buf = Buffer::new(
                                        &mut self.font_system,
                                        Metrics::new(badge_font_size, badge_font_size * 1.3),
                                    );
                                    badge_buf.set_wrap(&mut self.font_system, Wrap::None);
                                    badge_buf.set_size(
                                        &mut self.font_system,
                                        Some(badge_w),
                                        Some(badge_h),
                                    );
                                    badge_buf.set_text(
                                        &mut self.font_system,
                                        badge_text,
                                        Attrs::new()
                                            .family(Family::SansSerif)
                                            .color(GlyphonColor::rgb(0xff, 0xff, 0xff)),
                                        Shaping::Advanced,
                                    );
                                    tab_text_areas
                                        .push((badge_buf, [badge_x + 3.0, badge_y + 1.0]));
                                }
                            }

                            // Close-X vector strokes — two diagonal Quad::line segments
                            // concentric with the chip, DPI-correct, no font glyph.
                            let x_half = 3.0 * scale;
                            let thickness = system_icons::STROKE_PX * scale;
                            let x_color = [0xb8_u8, 0xc0, 0xd0];
                            for (from, to) in [
                                (
                                    [disc_cx - x_half, disc_cy - x_half],
                                    [disc_cx + x_half, disc_cy + x_half],
                                ),
                                (
                                    [disc_cx - x_half, disc_cy + x_half],
                                    [disc_cx + x_half, disc_cy - x_half],
                                ),
                            ] {
                                quads.push(Quad::line(from, to, thickness, x_color, 1.0));
                            }
                        } // end !pinned
                    } // end for each tab item

                    // ── Group-label pills (Chrome-style, horizontal bar only) ─────────
                    // Drawn after all tab pills so they appear above them in z-order.
                    // Each pill: rounded filled quad in the group accent colour +
                    // a glyphon text area with the group name in contrast colour.
                    if self.show_tab_group_labels {
                        for &(pill_rect, first_idx) in &layout.group_pills {
                            let accent = bar.items[first_idx]
                                .group_accent
                                .unwrap_or([0x4e, 0xa8, 0xff]);
                            // Filled pill background quad.
                            quads.push(Quad::new(
                                [pill_rect.x * scale, (tab_y_log + pill_rect.y) * scale],
                                [pill_rect.w * scale, pill_rect.h * scale],
                                accent,
                                1.0,
                            ));
                            // Text in contrast colour, vertically centred in the pill.
                            if let Some(ref glabel) = bar.items[first_idx].group_label {
                                let pill_font_size = self.font_size * 0.72;
                                let mut pill_buf = Buffer::new(
                                    &mut self.font_system,
                                    Metrics::new(pill_font_size, pill_font_size * 1.2),
                                );
                                pill_buf.set_wrap(&mut self.font_system, Wrap::None);
                                pill_buf.set_size(
                                    &mut self.font_system,
                                    Some((pill_rect.w - GROUP_PILL_PAD_X * 2.0).max(1.0)),
                                    Some(pill_font_size * 1.2),
                                );
                                let text_color = contrast_text(accent);
                                pill_buf.set_text(
                                    &mut self.font_system,
                                    glabel.as_str(),
                                    Attrs::new().family(Family::Monospace).color(text_color),
                                    Shaping::Advanced,
                                );
                                let text_x = pill_rect.x + GROUP_PILL_PAD_X;
                                let text_y = tab_y_log
                                    + pill_rect.y
                                    + (pill_rect.h - pill_font_size * 1.2) * 0.5;
                                tab_text_areas.push((pill_buf, [text_x, text_y]));
                            }
                        }
                    }

                    // Plus button
                    let plus_bg = if bar.plus_hovered {
                        [0x1d, 0x26, 0x40]
                    } else {
                        [0x07, 0x09, 0x0e]
                    };
                    quads.push(Quad::new(
                        [plus_rect.x * scale, (tab_y_log + plus_rect.y) * scale],
                        [plus_rect.w * scale, plus_rect.h * scale],
                        plus_bg,
                        1.0,
                    ));
                    let mut plus_buf = Buffer::new(
                        &mut self.font_system,
                        Metrics::new(self.font_size * 1.0, self.font_size * 1.1),
                    );
                    plus_buf.set_size(&mut self.font_system, Some(plus_rect.w), Some(plus_rect.h));
                    plus_buf.set_text(
                        &mut self.font_system,
                        "+",
                        Attrs::new()
                            .family(Family::SansSerif)
                            .color(GlyphonColor::rgb(0xc0, 0xc8, 0xdc)),
                        Shaping::Advanced,
                    );
                    tab_text_areas.push((
                        plus_buf,
                        [plus_rect.x + 12.0, tab_y_log + plus_rect.y + 6.0],
                    ));

                    // ── Window controls (min / max / close) on the far right ──
                    use system_icons::{
                        icon_lines, SystemIcon, BG_CLOSE_HOVER, BG_HOVER, BG_IDLE,
                        STROKE_CLOSE_HOVER, STROKE_DEFAULT, STROKE_PX,
                    };
                    let thickness = STROKE_PX * scale;
                    for (which, rect) in [
                        (WindowCtrl::Minimize, &layout.min_btn),
                        (WindowCtrl::Maximize, &layout.max_btn),
                        (WindowCtrl::Close, &layout.close_btn),
                    ] {
                        let hovered = bar.window_ctrl_hovered == Some(which);
                        let bg = match (which, hovered) {
                            (WindowCtrl::Close, true) => BG_CLOSE_HOVER,
                            (_, true) => BG_HOVER,
                            _ => BG_IDLE,
                        };
                        quads.push(Quad::new(
                            [rect.x * scale, (tab_y_log + rect.y) * scale],
                            [rect.w * scale, rect.h * scale],
                            bg,
                            1.0,
                        ));
                        let stroke = if hovered && matches!(which, WindowCtrl::Close) {
                            STROKE_CLOSE_HOVER
                        } else {
                            STROKE_DEFAULT
                        };
                        let kind = match which {
                            WindowCtrl::Minimize => SystemIcon::Minimize,
                            WindowCtrl::Maximize => {
                                if bar.maximized {
                                    SystemIcon::Restore
                                } else {
                                    SystemIcon::Maximize
                                }
                            }
                            WindowCtrl::Close => SystemIcon::Close,
                        };
                        // Icon coordinates are in *logical* pixels relative to
                        // the button centre. Multiply by `scale` to land in
                        // physical-pixel space for the GPU primitive.
                        let cx_log = rect.x + rect.w * 0.5;
                        let cy_log = tab_y_log + rect.y + rect.h * 0.5;
                        for line in icon_lines(kind, cx_log, cy_log) {
                            quads.push(Quad::line(
                                [line.from.0 * scale, line.from.1 * scale],
                                [line.to.0 * scale, line.to.1 * scale],
                                thickness,
                                stroke,
                                1.0,
                            ));
                        }
                    }
                } // close else (horizontal rendering)
            } // close if tab_bar_visible
        }

        // ── Status bar ──
        // Drawn after the tab bar so it sits on top of the terminal body but
        // below the overlay layer (command palette, quick-connect, etc.).
        let mut status_bar_text_areas: Vec<(Buffer, [f32; 2])> = Vec::new();
        if let Some(sb) = &self.status_bar {
            let surface_w = self.config.width as f32;
            let surface_h = self.config.height as f32;
            let bar_h = STATUS_BAR_HEIGHT * scale;
            let bar_y = if sb.at_bottom {
                surface_h - bar_h
            } else {
                // Below the tab bar (when tab bar is at top) or at the very
                // top when there is no top tab bar.
                let tab_h = if self.tab_bar_visible_logical() > 0.0
                    && self.tab_bar_placement == TabBarPlacement::Top
                {
                    TAB_BAR_HEIGHT
                } else {
                    0.0
                };
                tab_h * scale
            };
            // Background strip.
            quads.push(Quad::new(
                [0.0, bar_y],
                [surface_w, bar_h],
                [0x07, 0x09, 0x0e],
                1.0,
            ));
            // 1px separator line (top edge for bottom bar, bottom edge for top bar).
            let sep_y = if sb.at_bottom {
                bar_y
            } else {
                bar_y + bar_h - 1.0 * scale
            };
            quads.push(Quad::new(
                [0.0, sep_y],
                [surface_w, 1.0 * scale],
                [0x22, 0x28, 0x3a],
                1.0,
            ));

            let text_metrics = Metrics::new(self.font_size * 0.85, STATUS_BAR_HEIGHT);
            let text_color = GlyphonColor::rgb(0xa0, 0xb0, 0xd0);
            // Logical width of the whole bar (surface_w is physical).
            let bar_w_logical = surface_w / scale;
            let half_w_logical = bar_w_logical * 0.5;

            // Left text — buffer width covers up to half the bar in logical px.
            if !sb.left.is_empty() {
                let mut buf = Buffer::new(&mut self.font_system, text_metrics);
                buf.set_size(
                    &mut self.font_system,
                    Some(half_w_logical),
                    Some(STATUS_BAR_HEIGHT),
                );
                buf.set_text(
                    &mut self.font_system,
                    &sb.left,
                    Attrs::new().family(Family::Monospace).color(text_color),
                    Shaping::Basic,
                );
                let tx = 8.0 * scale;
                let ty = bar_y;
                status_bar_text_areas.push((buf, [tx, ty]));
            }

            // Right text: right-aligned against the bar's right edge. The
            // x-offset uses the REAL shaped width measured from the buffer's
            // layout runs — a char-count × 0.6em estimate systematically
            // under-measured (the text is shaped at 0.85 × font_size while
            // cell_width is already ≈ 0.6 × font_size), pushing the clock
            // past the window edge where glyphon clips it.
            if !sb.right.is_empty() {
                let mut buf = Buffer::new(&mut self.font_system, text_metrics);
                // No width cap: keep the text on a single line so the
                // measured run width is the true width (a Some(width) here
                // would word-wrap long strings and corrupt the measurement).
                buf.set_size(&mut self.font_system, None, Some(STATUS_BAR_HEIGHT));
                buf.set_text(
                    &mut self.font_system,
                    &sb.right,
                    Attrs::new().family(Family::Monospace).color(text_color),
                    Shaping::Basic,
                );
                // Real shaped width in logical px (buffer metrics are
                // logical; the TextArea below multiplies by scale_factor).
                let text_w_logical = buf
                    .layout_runs()
                    .map(|run| run.line_w)
                    .fold(0.0_f32, f32::max);
                let tx = status_bar_right_tx(surface_w, text_w_logical, scale);
                let ty = bar_y;
                status_bar_text_areas.push((buf, [tx, ty]));
            }
        }

        // Append non-focused panes' bg + underline quads (set up by
        // `render_panes` before this call). These land in the main
        // layer so they sit BENEATH the tab bar / overlays — exactly
        // like the focused pane's own bg quads.
        quads.append(&mut self.extra_pane_quads);
        // Divider strokes between sibling panes — drawn after all pane
        // bg so the divider chrome is visible across the boundary.
        // NOTE: dividers now sit BELOW the focus border so the focused
        // pane's accent ring is never overwritten by adjacent dividers
        // at shared edges.  Intentional: the focused pane's corners
        // may show the accent border "over" a divider end — this is the
        // correct visual because the border is the primary indicator.
        quads.append(&mut self.divider_quads);
        // Focus-border strokes above dividers — so no adjacent divider
        // can overpaint any side of the indicator.
        quads.append(&mut self.focus_border_quads);
        // Per-pane header strip quads — drawn on top of dividers and
        // focus borders so the header visually overlays the top edge.
        quads.append(&mut self.pane_header_quads);

        // ── Overlay layer (drawn AFTER the main text pass) ──
        // Quads here ride in the same instance buffer but are recorded as a
        // separate index range so they paint on top of the terminal glyphs.
        let main_quad_count = quads.len() as u32;

        // Inactive-pane dim overlays — queued by `render_panes` into
        // `pane_dim_quads` and drained here so the tint sits above all
        // terminal glyphs (text pass already ran for Layer 1 at this point).
        // Must be the FIRST overlay quads so they are below the scrollbar,
        // palette, and other UI chrome drawn later in this section.
        quads.append(&mut self.pane_dim_quads);

        // Window-unfocused dim — a translucent black tint over the full grid
        // area drawn as the first overlay quad so it sits above all terminal
        // text but below UI chrome (scrollbar, search bar, palette, etc.).
        if !self.focused && self.unfocused_window_dim > 0.01 {
            quads.push(Quad::new(
                [0.0, 0.0],
                [self.config.width as f32, self.config.height as f32],
                [0x00, 0x00, 0x00],
                self.unfocused_window_dim,
            ));
        }

        // Scrollback scrollbar on the right edge. Interactive: the thumb can
        // be grabbed and dragged (see `handle_mouse` / `scrollbar_geometry`),
        // a track click jumps there. Visibility per `scrollbar_mode`:
        // `Auto` shows it while panning history OR while the pointer hovers
        // the right-edge band (so it can be grabbed from the live bottom),
        // `Always` whenever history exists, `Never` not at all. Geometry is
        // cached whenever history exists — hidden-in-Auto included — because
        // the hover-reveal hit-test needs the band. Thumb size ∝ visible
        // fraction; position ∝ how far up.
        // (`history` snapshotted above, next to the grid-cell capture.)
        self.last_scrollbar = None;
        if history > 0 && self.scrollbar_mode != ScrollbarMode::Never {
            let engaged = self.scrollbar_hover || self.scrollbar_active;
            let show = match self.scrollbar_mode {
                ScrollbarMode::Always => true,
                ScrollbarMode::Never => false,
                ScrollbarMode::Auto => self.scroll_lines > 0 || engaged,
            };
            let total = (history + rows as usize).max(1) as f32;
            let track_top = top_pad_px;
            let track_h = (self.config.height as f32 - track_top).max(1.0);
            // Wider while hovered / dragged so it's easy to grab.
            let bar_w = if engaged {
                (10.0 * scale).max(6.0)
            } else {
                (4.0 * scale).max(2.0)
            };
            let bar_x = self.config.width as f32 - bar_w - 2.0 * scale;
            let thumb_frac = (rows as f32 / total).clamp(0.04, 1.0);
            // Lines above the viewport's top, from the oldest scrollback line.
            let lines_above = history.saturating_sub(self.scroll_lines) as f32;
            let top_frac = (lines_above / total).clamp(0.0, 1.0 - thumb_frac);
            let thumb_h = (thumb_frac * track_h).max(16.0 * scale);
            let thumb_y = track_top + top_frac * track_h;
            self.last_scrollbar = Some(ScrollbarGeom {
                track_x: bar_x,
                track_top,
                track_w: bar_w,
                track_h,
                thumb_top: thumb_y,
                thumb_h,
                history,
                rows: rows as usize,
            });
            if show {
                // Faint track + accent thumb (brighter while engaged).
                quads.push(Quad::new(
                    [bar_x, track_top],
                    [bar_w, track_h],
                    [0x3b, 0x42, 0x5a],
                    if engaged { 0.4 } else { 0.25 },
                ));
                quads.push(Quad::new(
                    [bar_x, thumb_y],
                    [bar_w, thumb_h],
                    [0x7d, 0xa6, 0xff],
                    if engaged { 1.0 } else { 0.85 },
                ));
            }
        }

        // Merge drop-zone highlight — the half of a target pane a dragged
        // tab / pane would occupy on release. Accent tint + brighter frame,
        // drawn in the overlay layer so it reads above the terminal text.
        if let Some([zx, zy, zw, zh]) = self.drop_zone {
            const ZONE_ACCENT: [u8; 3] = [0x7d, 0xa6, 0xff];
            quads.push(Quad::new([zx, zy], [zw, zh], ZONE_ACCENT, 0.16));
            let bt = (2.0 * scale).max(2.0);
            // Frame strokes (top / bottom / left / right).
            quads.push(Quad::new([zx, zy], [zw, bt], ZONE_ACCENT, 0.9));
            quads.push(Quad::new([zx, zy + zh - bt], [zw, bt], ZONE_ACCENT, 0.9));
            quads.push(Quad::new([zx, zy], [bt, zh], ZONE_ACCENT, 0.9));
            quads.push(Quad::new([zx + zw - bt, zy], [bt, zh], ZONE_ACCENT, 0.9));
        }

        // Overlay text (label badges + palette rows + save-host toast labels)
        // all ride the same second text pass so they render on top of their panels.
        let mut palette_text_areas: Vec<(Buffer, [f32; 2])> = Vec::new();

        // Quick-select / pane-select label badges — drawn before the palette
        // so the command-palette dim covers them if both are somehow open.
        if !self.label_overlays.is_empty() {
            self.build_label_overlays(
                scale,
                body_x_origin,
                body_y_origin,
                &mut quads,
                &mut palette_text_areas,
            );
        }

        // "Save this SSH host?" toast — drawn before the palette so the
        // palette's dim covers it on the rare occasion both are open.
        if let Some(prompt) = self.save_host_prompt.clone() {
            self.build_save_host_prompt(&prompt, scale, &mut quads, &mut palette_text_areas);
        }

        // Command-palette modal (its own quads + a second text pass).
        if let Some(p) = self.command_palette.clone() {
            self.build_command_palette(&p, scale, &mut quads, &mut palette_text_areas);
        }

        // Snap-layout chooser overlay — drawn on top of everything except tab-drag.
        if let Some(chooser) = self.snap_chooser.clone() {
            self.build_snap_chooser(&chooser, scale, &mut quads, &mut palette_text_areas);
        }

        // ── Tab-drag overlays (drop indicator + floating ghost) ──
        // Drawn LAST in the overlay quad range so they sit on top of the tab
        // pills, and the ghost's label rides the `overlay_text_renderer` so
        // it paints above the ghost quad rather than under it. Order:
        // indicator (lowest), then the ghost body + accent so the ghost
        // stacks over the indicator. The ghost label is logical-px and goes
        // into `ghost_text_areas`, chained into the overlay text pass below.
        let mut ghost_text_areas: Vec<(Buffer, [f32; 2])> = Vec::new();
        if let Some(x_log) = self.tab_drop_indicator {
            // Vertical insertion bar at the landing boundary.
            // Positioned relative to the tab bar top so it works for
            // both Top and Bottom placement.
            let bar_top = self.tab_bar_y_logical(self.config.height as f32);
            let y = bar_top + 4.0;
            let h = TAB_BAR_HEIGHT - 8.0;
            let w = 3.0;
            quads.push(Quad::new(
                [(x_log - w * 0.5) * scale, y * scale],
                [w * scale, h * scale],
                [0x7d, 0xa6, 0xff],
                1.0,
            ));
        }
        if let Some(ghost) = self.tab_drag_ghost.clone() {
            let pill_h = TAB_BAR_HEIGHT - 8.0;
            let w = ghost.width;
            let x = ghost.center_x - w * 0.5;
            let y = ghost.center_y - pill_h * 0.5;
            // Soft drop shadow so the ghost reads as "lifted".
            quads.push(Quad::new(
                [(x + 3.0) * scale, (y + 5.0) * scale],
                [w * scale, pill_h * scale],
                [0x00, 0x00, 0x00],
                0.40,
            ));
            // Ghost body — translucent so the bar / content shows through.
            quads.push(Quad::new(
                [x * scale, y * scale],
                [w * scale, pill_h * scale],
                [0x1a, 0x20, 0x33],
                0.88,
            ));
            // Accent bottom bar (mirrors the active tab) for identity.
            quads.push(Quad::new(
                [x * scale, (y + pill_h - 2.0) * scale],
                [w * scale, 2.0 * scale],
                [0x7d, 0xa6, 0xff],
                1.0,
            ));
            // Label — built like a normal tab label, pushed in logical px so
            // the overlay text pass draws it above the ghost quad.
            let mut buf = Buffer::new(
                &mut self.font_system,
                Metrics::new(self.font_size * 0.92, self.font_size * 1.1),
            );
            buf.set_size(
                &mut self.font_system,
                Some((w - 24.0).max(0.0)),
                Some(pill_h),
            );
            buf.set_text(
                &mut self.font_system,
                &ghost.label,
                Attrs::new()
                    .family(Family::Monospace)
                    .color(GlyphonColor::rgb(0xe6, 0xea, 0xf8)),
                Shaping::Advanced,
            );
            let text_x = x + 12.0;
            let text_y = y + (pill_h - self.font_size * 1.1) * 0.5;
            ghost_text_areas.push((buf, [text_x, text_y]));
        }

        let overlay_quad_count = quads.len() as u32;

        // ── Upload bg quads (main layer + overlay layer) ──
        let viewport_px = [self.config.width as f32, self.config.height as f32];
        self.bg
            .upload(&self.device, &self.queue, viewport_px, &quads);

        // ── Upload background image (decoded once; params can change live) ──
        if self.bg_image_params.active() {
            let params = self.bg_image_params.clone();
            self.bg_image
                .upload(&self.device, &self.queue, viewport_px, &params);
        }

        // ── Upload animated background-effect params for this frame ──
        if self.bg_fx_params.active() {
            let t = self.bg_fx_start.elapsed().as_secs_f32() * self.bg_fx_params.speed;
            let pulse = self.bg_fx_pulse();
            // Convert CPU emitters to the GPU layout.
            let gpu_emitters: Vec<GpuEmitter> = self
                .bg_fx_emitters
                .iter()
                .map(|e| GpuEmitter {
                    birth: e.birth,
                    col: e.col,
                    seed: e.seed,
                    kind: e.kind,
                })
                .collect();
            // band_width / fall_speed are forwarded via BgFxParams — the
            // upload() method packs them into the .w components of color1/color2.
            self.bg_fx.upload(
                &self.queue,
                viewport_px,
                t,
                pulse,
                &self.bg_fx_params,
                &gpu_emitters,
            );
        }

        // ── Upload any new inline-image textures; evict stale ones ──
        // Collect the data we need before borrowing self.image_blit mutably.
        let live_ids = emulator.image_store().live_image_ids();
        // Upload textures for images not yet on the GPU.
        // We must collect the images first to avoid a simultaneous immutable
        // borrow of image_store and mutable borrow of image_blit.
        let images_to_upload: Vec<_> = emulator
            .image_store()
            .iter_images()
            .filter(|img| !self.image_blit.has_texture(img.id))
            .map(|img| {
                (
                    img.id,
                    img.rgba.clone(),
                    img.width_px,
                    img.height_px,
                    img.byte_size,
                )
            })
            .collect();
        for (id, rgba, width_px, height_px, byte_size) in images_to_upload {
            let proxy = terminale_term::InlineImage {
                id,
                rgba,
                width_px,
                height_px,
                byte_size,
            };
            self.image_blit
                .upload_image(&self.device, &self.queue, &proxy);
        }
        self.image_blit.drop_evicted(&live_ids);

        // Collect visible inline-image placements for this frame.
        // top_abs_line = lowest visible absolute line index (viewport top when scrolled).
        let top_abs_line = {
            let scroll = self.scroll_lines as i32;
            // Absolute line 0 is the top of the visible screen when scroll=0.
            // When scrolled, the top visible row corresponds to abs_line = -scroll.
            -scroll
        };
        let visible_image_placements = emulator
            .image_store()
            .placements_in_view(top_abs_line, rows);

        // ── Build one TextArea per visible row (no flow drift) ──
        //
        // Frame-level cache: hash every input the row-building loop below
        // reads; when the hash matches the previous frame the shaped buffers
        // in `self.cached_focused_text` are identical to what a rebuild
        // would produce, so the rebuild (the dominant render cost — a full
        // re-shape of every visible row) is skipped entirely. Hashing
        // ~`cols × rows` cells is orders of magnitude cheaper than shaping.
        // The cursor is a separate quad (not part of these buffers), so
        // cursor-blink / bg-FX / bell redraws hit the cache by design.
        let focused_hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            self.font_size.to_bits().hash(&mut h);
            self.line_height.to_bits().hash(&mut h);
            self.font_family.hash(&mut h);
            self.font_bold_family.hash(&mut h);
            self.font_italic_family.hash(&mut h);
            self.font_bold_italic_family.hash(&mut h);
            self.ligatures.hash(&mut h);
            self.builtin_box_drawing.hash(&mut h);
            cols.hash(&mut h);
            self.cell_width.to_bits().hash(&mut h);
            self.cell_height.to_bits().hash(&mut h);
            body_x_origin.to_bits().hash(&mut h);
            body_y_origin.to_bits().hash(&mut h);
            pad_px.to_bits().hash(&mut h);
            ch_px.to_bits().hash(&mut h);
            grid_cells.len().hash(&mut h);
            for row_cells in &grid_cells {
                row_cells.len().hash(&mut h);
                for snap in row_cells {
                    snap.hidden.hash(&mut h);
                    snap.ch.hash(&mut h);
                    snap.fg.hash(&mut h);
                    snap.bold.hash(&mut h);
                    snap.italic.hash(&mut h);
                }
            }
            h.finish()
        };

        // `cached non-empty` guards the post-constructor state (hash field
        // defaults to 0); a genuinely blank grid re-runs the loop below but
        // shapes nothing (every row is skipped as empty), so that's free.
        let rebuild_focused_text =
            focused_hash != self.focused_text_hash || self.cached_focused_text.is_empty();

        if rebuild_focused_text {
            let metrics = Metrics::new(self.font_size, self.font_size * self.line_height);
            let mut text_buffers: Vec<(Buffer, [f32; 2])> = Vec::with_capacity(rows.into());
            // Clone the family name(s) once per frame so the per-span Attrs can
            // borrow locals (avoids a self borrow conflict with font_system).
            let family_name = self.font_family.clone();
            let bold_family_name = self.font_bold_family.clone();
            let italic_family_name = self.font_italic_family.clone();
            let bold_italic_family_name = self.font_bold_italic_family.clone();
            // Ligatures ⇒ HarfBuzz-quality Advanced shaping; off ⇒ Basic
            // per-glyph shaping that performs no ligature substitution.
            let shaping = if self.ligatures {
                Shaping::Advanced
            } else {
                Shaping::Basic
            };

            // Snapshot the builtin_box_drawing flag for this frame so the hot-path
            // borrow checker is happy (avoids a `self` borrow inside the loop).
            let builtin_box_drawing = self.builtin_box_drawing;

            for (row_idx, row_cells) in grid_cells.iter().enumerate() {
                let mut owned: Vec<(String, Attrs<'_>)> = Vec::with_capacity(row_cells.len());
                let mut last_attr: Option<([u8; 3], bool, bool)> = None;
                let mut current = String::new();
                for snap in row_cells {
                    // ── Procedural box-drawing / block-element path ───────────────
                    // The geometry for these cells is emitted as quads in the main
                    // layer above (before the bg-quad upload). Here we only
                    // substitute a space so the font glyph does not paint over
                    // those quads. Unmapped in-range chars fall through to the font.
                    let suppress_for_box = builtin_box_drawing
                        && !snap.hidden
                        && box_drawing::is_in_range(snap.ch)
                        && box_drawing::box_rects(snap.ch).is_some();
                    let effective_ch = if suppress_for_box || snap.ch == '\0' {
                        ' '
                    } else {
                        snap.ch
                    };

                    let attr = (snap.fg, snap.bold, snap.italic);
                    if last_attr.is_none_or(|a| a == attr) {
                        current.push(effective_ch);
                        last_attr = Some(attr);
                    } else {
                        if let Some((fg, bold, italic)) = last_attr {
                            owned.push((
                                current,
                                attr_for(
                                    fg,
                                    bold,
                                    italic,
                                    &family_name,
                                    bold_family_name.as_deref(),
                                    italic_family_name.as_deref(),
                                    bold_italic_family_name.as_deref(),
                                ),
                            ));
                        }
                        current = String::new();
                        current.push(effective_ch);
                        last_attr = Some(attr);
                    }
                }
                if let Some((fg, bold, italic)) = last_attr {
                    if !current.is_empty() {
                        owned.push((
                            current,
                            attr_for(
                                fg,
                                bold,
                                italic,
                                &family_name,
                                bold_family_name.as_deref(),
                                italic_family_name.as_deref(),
                                bold_italic_family_name.as_deref(),
                            ),
                        ));
                    }
                }

                if owned.is_empty() {
                    continue;
                }

                let mut buf = Buffer::new(&mut self.font_system, metrics);
                buf.set_size(
                    &mut self.font_system,
                    Some(f32::from(cols) * self.cell_width),
                    Some(self.cell_height),
                );
                let spans: Vec<(&str, Attrs<'_>)> =
                    owned.iter().map(|(s, a)| (s.as_str(), *a)).collect();
                let default_fam = if family_name.is_empty() {
                    Family::Monospace
                } else {
                    Family::Name(&family_name)
                };
                buf.set_rich_text(
                    &mut self.font_system,
                    spans,
                    Attrs::new().family(default_fam),
                    shaping,
                );
                // Glyphon TextArea origins are PHYSICAL pixels. Background
                // cells + cursor are placed at `pad_px + col*cw_px` (physical),
                // so the text must use the same physical frame — otherwise on
                // HiDPI the glyphs drift left of the cells by padding*(scale-1)
                // and the cursor looks shifted to the right.
                let y = body_y_origin + row_idx as f32 * ch_px;
                text_buffers.push((buf, [body_x_origin + pad_px, y]));
            }
            self.cached_focused_text = text_buffers;
            self.focused_text_hash = focused_hash;
        }

        // ── Prepare glyphon ──
        // The focused pane's (possibly cached) rows chain with the
        // non-focused panes' (per-pane cached) rows and the pane header
        // titles. Non-focused panes draw exactly the entries listed in
        // `extra_pane_cache_seen` (queued by this frame's `render_panes`);
        // header titles are rebuilt per-frame and cleared after `prepare`.
        let text_areas_iter = self
            .cached_focused_text
            .iter()
            .chain(
                self.extra_pane_cache_seen
                    .iter()
                    .filter_map(|id| self.extra_pane_text_cache.get(id))
                    .flat_map(|(_hash, bufs)| bufs.iter()),
            )
            .chain(self.pane_header_text_buffers.iter())
            .map(|(buf, pos)| TextArea {
                buffer: buf,
                left: pos[0],
                top: pos[1],
                scale: self.scale_factor,
                bounds: TextBounds::default(),
                default_color: GlyphonColor::rgb(0xe0, 0xe6, 0xff),
                custom_glyphs: &[],
            })
            // overlay + tab text positions are computed in LOGICAL px
            // (from LogicalRect fields), but their background rects are
            // drawn at `rect * scale`. Scale the text origins to the same
            // physical frame so labels stay centred in their pills on HiDPI.
            .chain(overlay_text_areas.iter().map(|(buf, pos)| TextArea {
                buffer: buf,
                left: pos[0] * scale,
                top: pos[1] * scale,
                scale: self.scale_factor,
                bounds: TextBounds::default(),
                default_color: GlyphonColor::rgb(0xe0, 0xe6, 0xff),
                custom_glyphs: &[],
            }))
            .chain(tab_text_areas.iter().map(|(buf, pos)| TextArea {
                buffer: buf,
                left: pos[0] * scale,
                top: pos[1] * scale,
                scale: self.scale_factor,
                bounds: TextBounds::default(),
                default_color: GlyphonColor::rgb(0xe0, 0xe6, 0xff),
                custom_glyphs: &[],
            }))
            .chain(search_text_buffers.iter().map(|(buf, pos)| TextArea {
                buffer: buf,
                left: pos[0],
                top: pos[1],
                scale: self.scale_factor,
                bounds: TextBounds::default(),
                default_color: GlyphonColor::rgb(0xe0, 0xe6, 0xff),
                custom_glyphs: &[],
            }))
            .chain(tooltip_text_buffer.iter().map(|(buf, pos)| TextArea {
                buffer: buf,
                left: pos[0],
                top: pos[1],
                scale: self.scale_factor,
                bounds: TextBounds::default(),
                default_color: GlyphonColor::rgb(0xe0, 0xe6, 0xff),
                custom_glyphs: &[],
            }))
            // Status-bar text areas — positions are already in physical px
            // (computed above as bar_y + margin).
            .chain(status_bar_text_areas.iter().map(|(buf, pos)| TextArea {
                buffer: buf,
                left: pos[0],
                top: pos[1],
                scale: self.scale_factor,
                bounds: TextBounds::default(),
                default_color: GlyphonColor::rgb(0xa0, 0xb0, 0xd0),
                custom_glyphs: &[],
            }))
            // Suggestion-bar text — positions already physical px.
            .chain(suggestion_text_buffers.iter().map(|(buf, pos)| TextArea {
                buffer: buf,
                left: pos[0],
                top: pos[1],
                scale: self.scale_factor,
                bounds: TextBounds::default(),
                default_color: GlyphonColor::rgb(0xe6, 0xea, 0xf8),
                custom_glyphs: &[],
            }))
            // Resource-strip text — positions already physical px.
            .chain(resource_text_buffers.iter().map(|(buf, pos)| TextArea {
                buffer: buf,
                left: pos[0],
                top: pos[1],
                scale: self.scale_factor,
                bounds: TextBounds::default(),
                default_color: GlyphonColor::rgb(0xe6, 0xea, 0xf8),
                custom_glyphs: &[],
            }));

        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );

        self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            text_areas_iter,
            &mut self.swash_cache,
        )?;

        // Drain the per-frame buffers now that `prepare` consumed them.
        // Header titles are rebuilt by `render_panes` every frame; the
        // non-focused panes' shaped text stays in `extra_pane_text_cache`
        // across frames — only entries for panes that were NOT part of this
        // frame are evicted (closed panes / background tabs), so the cache
        // cannot grow unbounded.
        let seen = std::mem::take(&mut self.extra_pane_cache_seen);
        self.extra_pane_text_cache.retain(|id, _| seen.contains(id));
        self.pane_header_text_buffers.clear();

        // Overlay text (command palette) — its own prepared batch so it
        // renders in a second pass on top of the modal panel. Positions
        // are logical, scaled here like the menu/tab text.
        let palette_areas_iter = palette_text_areas
            .iter()
            .map(|(buf, pos)| TextArea {
                buffer: buf,
                left: pos[0] * scale,
                top: pos[1] * scale,
                scale: self.scale_factor,
                bounds: TextBounds::default(),
                default_color: GlyphonColor::rgb(0xe0, 0xe6, 0xff),
                custom_glyphs: &[],
            })
            // Drag-ghost label rides the same overlay pass so it paints on
            // top of the floating ghost quad. Logical px, scaled like the
            // palette text above.
            .chain(ghost_text_areas.iter().map(|(buf, pos)| TextArea {
                buffer: buf,
                left: pos[0] * scale,
                top: pos[1] * scale,
                scale: self.scale_factor,
                bounds: TextBounds::default(),
                default_color: GlyphonColor::rgb(0xe6, 0xea, 0xf8),
                custom_glyphs: &[],
            }));
        self.overlay_text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            palette_areas_iter,
            &mut self.swash_cache,
        )?;

        // ── Encode the pass ──
        let clear = clear_color(self.background_rgb, self.background_alpha);
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("terminale main pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(clear),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // Layer 0a: background image — drawn immediately after the clear,
            // before bg_fx and before every cell quad / glyph.
            if self.bg_image_params.active() {
                self.bg_image.draw(&mut pass);
            }
            // Layer 0b: animated background "wallpaper" — drawn over the clear
            // and over the bg image but under every cell quad / glyph.
            if self.bg_fx_params.active() {
                self.bg_fx.draw(&mut pass);
            }
            // Layer 0c: inline images (OSC 1337 / Sixel / APC graphics) — composited
            // above cell backgrounds but below text glyphs.
            if !visible_image_placements.is_empty() {
                self.image_blit.draw(
                    &mut pass,
                    &self.queue,
                    &visible_image_placements,
                    body_x_origin + pad_px,
                    body_y_origin,
                    cw_px,
                    ch_px,
                    self.config.width as f32,
                    self.config.height as f32,
                );
            }
            // Layer 1: terminal background, cursor, menus, tabs.
            self.bg.draw_range(&mut pass, 0..main_quad_count);
            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)?;
            // Layer 2: command-palette modal, drawn on top of everything
            // so terminal glyphs never bleed through the panel.
            self.bg
                .draw_range(&mut pass, main_quad_count..overlay_quad_count);
            self.overlay_text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)?;
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        self.atlas.trim();
        // Body-origin overrides are one-shot — clear them so the next
        // `render(emu)` call (without a wrapping `render_panes` setup)
        // falls back to the default `(0, top_pad_px)`.
        self.pending_body_x = None;
        self.pending_body_y = None;
        Ok(())
    }
}

/// Select the effective font family and whether weight/style synthesis
/// should be applied for a cell with the given `bold` / `italic` flags.
///
/// Rules (matches the `FontConfig` documentation):
/// - bold && italic → `bold_italic_override` if `Some`, else fall back to
///   `bold_override` then `italic_override`, then synthesize on `main`.
/// - bold only       → `bold_override` if `Some`, else synthesize on `main`.
/// - italic only     → `italic_override` if `Some`, else synthesize on `main`.
/// - neither         → always `main`, no synthesis.
///
/// Returns `(family, apply_bold_weight, apply_italic_style)`.
/// When an override is active the dedicated face provides the glyph, so
/// `apply_bold_weight` / `apply_italic_style` are `false` to avoid
/// double-applying weight/slant on top of an already-bold/italic face.
#[must_use]
pub(crate) fn select_font_family<'a>(
    main: &'a str,
    bold_override: Option<&'a str>,
    italic_override: Option<&'a str>,
    bold_italic_override: Option<&'a str>,
    bold: bool,
    italic: bool,
) -> (&'a str, bool, bool) {
    match (bold, italic) {
        (true, true) => {
            if let Some(bi) = bold_italic_override {
                (bi, false, false)
            } else if let Some(b) = bold_override {
                (b, false, true)
            } else if let Some(i) = italic_override {
                (i, true, false)
            } else {
                (main, true, true)
            }
        }
        (true, false) => {
            if let Some(b) = bold_override {
                (b, false, false)
            } else {
                (main, true, false)
            }
        }
        (false, true) => {
            if let Some(i) = italic_override {
                (i, false, false)
            } else {
                (main, false, true)
            }
        }
        (false, false) => (main, false, false),
    }
}

fn attr_for<'a>(
    fg: [u8; 3],
    bold: bool,
    italic: bool,
    main_family: &'a str,
    bold_family: Option<&'a str>,
    italic_family: Option<&'a str>,
    bold_italic_family: Option<&'a str>,
) -> Attrs<'a> {
    let (family, apply_bold, apply_italic) = select_font_family(
        main_family,
        bold_family,
        italic_family,
        bold_italic_family,
        bold,
        italic,
    );
    let fam = if family.is_empty() {
        Family::Monospace
    } else {
        Family::Name(family)
    };
    let mut a = Attrs::new()
        .family(fam)
        .color(GlyphonColor::rgb(fg[0], fg[1], fg[2]));
    if apply_bold {
        a = a.weight(glyphon::Weight::BOLD);
    }
    if apply_italic {
        a = a.style(glyphon::Style::Italic);
    }
    a
}

/// Resolve an optional per-style font family override against the font
/// database. Returns `None` when `requested` is `None` or empty, and
/// clears the value with a warning when the family cannot be found.
fn resolve_override_family(
    font_system: &FontSystem,
    requested: Option<&str>,
    field: &'static str,
) -> Option<String> {
    let name = requested?.trim();
    if name.is_empty() {
        return None;
    }
    if family_is_available(font_system, name) {
        Some(name.to_string())
    } else {
        tracing::warn!(
            family = name,
            config_field = field,
            "configured font override not found; ignoring (will synthesize from main family)"
        );
        None
    }
}

fn clear_color(bg: [u8; 3], alpha: f32) -> Color {
    // Convert sRGB byte to linear float for the sRGB framebuffer.
    let to_linear = |c: u8| {
        let v = f64::from(c) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let a = f64::from(alpha.clamp(0.0, 1.0));
    Color {
        // Premultiplied alpha — required for the compositor to blend
        // the window over the desktop without a colour shift.
        r: to_linear(bg[0]) * a,
        g: to_linear(bg[1]) * a,
        b: to_linear(bg[2]) * a,
        a,
    }
}

/// Probe the font system for a representative cell width, using the
/// given family name (or the system monospace when `None`).
fn cell_size_for(font_system: &mut FontSystem, font_size: f32, family: Option<&str>) -> (f32, f32) {
    let metrics = Metrics::new(font_size, font_size);
    let mut probe = Buffer::new(font_system, metrics);
    probe.set_size(font_system, Some(1024.0), Some(1024.0));
    let fam = match family {
        Some(name) if !name.is_empty() => Family::Name(name),
        _ => Family::Monospace,
    };
    probe.set_text(
        font_system,
        "M",
        Attrs::new().family(fam),
        Shaping::Advanced,
    );
    let mut width = font_size * 0.6;
    if let Some(run) = probe.layout_runs().next() {
        if let Some(g) = run.glyphs.first() {
            width = g.w;
        }
    }
    // A broken/empty face can report a zero-advance 'M'. A zero cell would
    // blow the grid math up (`usable / 0 → inf → 65535 cols`) and hand the
    // emulator an absurd allocation — floor both dimensions at 1px instead.
    let width = width.max(1.0);
    let height = font_size.max(1.0);
    (width, height)
}

/// Convenience: probe with the system monospace.
fn monospace_cell_size(font_system: &mut FontSystem, font_size: f32) -> (f32, f32) {
    cell_size_for(font_system, font_size, None)
}

/// Icon codepoints that the app actually paints in egui sub-windows and the
/// wgpu chrome (tab bar, overlay menus).  Used by the coverage test to assert
/// that every glyph resolves to a real face after `load_symbol_fonts` runs.
///
/// The geometric/arrow set (`↑ ↓ ⊕ ▐ ▲ ▼ ● ◀ ▶ ⬆ ⬇ ⬅ ➡ ◎ ⬜`) resolves only
/// in Hack — the emoji set (`✖ ✔ ⚙ ☑ • − ⚠`) is covered by NotoEmoji.
pub const ICON_CODEPOINTS: &[char] = &[
    // Geometric / arrow icons — covered by Hack
    '↑',  // U+2191
    '↓',  // U+2193
    '⊕',  // U+2295
    '▐',  // U+2590
    '▲',  // U+25B2
    '▼',  // U+25BC
    '●',  // U+25CF
    '◀',  // U+25C0
    '▶',  // U+25B6
    '⬆',  // U+2B06
    '⬇',  // U+2B07
    '⬅',  // U+2B05
    '➡',  // U+27A1
    '◎',  // U+25CE
    '⬜', // U+2B1C
    // Emoji / symbol icons — covered by NotoEmoji / emoji-icon-font
    '✖', // U+2716
    '✔', // U+2714
    '⚙', // U+2699
    '☑', // U+2611
    '•', // U+2022
    '−', // U+2212
    '⚠', // U+26A0
];

/// Tabler Icons subset font bytes, embedded at compile time.
///
/// This is the same file registered in the egui font system via
/// `terminale::egui_icons` — both paths must stay in sync so that every
/// codepoint used by the icon registry (`terminale::icons`) resolves in both
/// the wgpu chrome and egui sub-windows.
static TABLER_ICONS_SUBSET: &[u8] = include_bytes!("../assets/fonts/icons/TablerIcons-subset.ttf");

/// PUA codepoints from the bundled Tabler Icons subset that the wgpu chrome
/// (tab bar, overlay menus) actually paints.  The render-side coverage test
/// asserts that every entry here resolves to a real face after
/// `load_symbol_fonts` runs.
pub const TABLER_CODEPOINTS: &[char] = &[
    '\u{EA7A}', // copy
    '\u{EA6F}', // clipboard / paste
    '\u{EB20}', // settings
    '\u{EB55}', // x / close
    '\u{EB0B}', // plus
    '\u{EAD4}', // layout-columns / split-v
    '\u{EAD8}', // layout-rows / split-h
    '\u{EB04}', // pencil / rename
    '\u{EC9C}', // pin
    '\u{F6D7}', // sparkles / AI
    '\u{EB13}', // refresh
    '\u{EB41}', // trash
    '\u{EB1C}', // search
    '\u{EA25}', // arrow-up
    '\u{EA16}', // arrow-down
    '\u{EA19}', // arrow-left
    '\u{EA1F}', // arrow-right
    '\u{EA61}', // chevron-right
    '\u{EA5F}', // chevron-down
    '\u{EA62}', // chevron-up
    '\u{EA60}', // chevron-left
    '\u{F000}', // arrows-shuffle
    '\u{EB35}', // target
    '\u{EB2C}', // square
    '\u{EA5E}', // check
    '\u{EAAD}', // folder
    '\u{EB54}', // world
    '\u{EAFF}', // package
    '\u{EB01}', // palette
    '\u{EBC5}', // typography
    '\u{EA98}', // edit
    '\u{EBEF}', // terminal-2
    '\u{EB0A}', // photo
    '\u{EA35}', // bell
    '\u{EA96}', // download
    '\u{EAC7}', // key
    '\u{EBD9}', // plug
    '\u{EB63}', // device-gamepad
    '\u{EA59}', // chart-bar
    '\u{EF86}', // tags
    '\u{EB62}', // device-floppy
    '\u{EAE9}', // map
    '\u{EA39}', // book
    '\u{EAA4}', // file
    '\u{EA51}', // bulb
    '\u{EAEF}', // message
    '\u{EA3A}', // bookmark
    '\u{EA06}', // alert-triangle
    '\u{EA69}', // circle-plus
    '\u{EAF2}', // minus
    '\u{EAC5}', // info-circle
    '\u{EA67}', // circle-check
    '\u{EA6A}', // circle-x
    '\u{EB28}', // square-check
    '\u{EAEA}', // maximize
    '\u{EAF1}', // minimize
    '\u{EF06}', // window
    '\u{EB26}', // sort-ascending
    '\u{EB27}', // sort-descending
    '\u{EA9A}', // eye
    '\u{ECF0}', // eye-off
    '\u{EAE2}', // lock
    '\u{EAE1}', // lock-open
    '\u{EAE9}', // map (duplicate; dedup is fine)
];

/// Register the same symbol / emoji fonts that egui bundles into the glyphon
/// font database.
///
/// Four faces are loaded:
///
/// * **Hack** — provides geometric symbols and arrow icons (`↑ ↓ ▲ ▼ ⊕ ▐ ●`
///   etc.) that are absent from NotoEmoji and the emoji-icon-font.  Without
///   this face those glyphs tofu on the tab bar and overlay menus whenever the
///   user's terminal font does not carry them.
/// * **NotoEmoji-Regular** — monochrome emoji coverage for the bulk of the
///   symbol set (`✔ ✖ ⚙ ⚠` etc.).
/// * **emoji-icon-font** — PUA extension glyphs used by some UI elements.
/// * **Tabler Icons subset** — the bundled thin-line icon font (PUA E000–F8FF)
///   used when `appearance.bundled_icons = true`.
///
/// `glyphon` / `cosmic-text` performs automatic per-glyph fallback over
/// *every* face in the database, so once these are loaded the wgpu chrome
/// (tab-bar profile icons, overlay-menu icons) can render the exact same
/// glyphs the egui sub-windows do — identically on Windows, macOS and Linux
/// rather than depending on whatever emoji font the OS happens to ship.  All
/// faces are monochrome and rasterise cleanly into glyphon's `Accurate`
/// colour atlas.
fn load_symbol_fonts(font_system: &mut FontSystem) {
    let db = font_system.db_mut();
    // Hack supplies geometric/arrow icon coverage absent from the emoji faces.
    db.load_font_data(epaint_default_fonts::HACK_REGULAR.to_vec());
    db.load_font_data(epaint_default_fonts::NOTO_EMOJI_REGULAR.to_vec());
    db.load_font_data(epaint_default_fonts::EMOJI_ICON.to_vec());
    // Tabler Icons subset: bundled thin-line icons for the UI chrome and egui windows.
    db.load_font_data(TABLER_ICONS_SUBSET.to_vec());
}

/// True if `family` is present in the font system's database (matched
/// case-insensitively against every face's family names).
fn family_is_available(font_system: &FontSystem, family: &str) -> bool {
    font_system.db().faces().any(|face| {
        face.families
            .iter()
            .any(|(name, _lang)| name.eq_ignore_ascii_case(family))
    })
}

/// Selectable monospace family names from a font database: monospaced faces
/// only, excluding the bundled emoji/symbol fallback faces (loaded for icon
/// fallback, never a terminal text font), sorted case-insensitively and
/// de-duplicated. Shared by [`Renderer::available_monospace_families`] and its
/// test so the offered list and the test invariant can't drift.
fn monospace_families_in(db: &glyphon::fontdb::Database) -> Vec<String> {
    // Pre-compute the set of bundled family names so they are included even
    // when a specific TTF does not set the monospaced flag in its OS/2 table.
    let bundled = bundled_fonts::bundled_family_names();
    let mut names: Vec<String> = db
        .faces()
        .filter(|face| {
            face.monospaced
                || face
                    .families
                    .iter()
                    .any(|(n, _)| bundled.iter().any(|b| n.eq_ignore_ascii_case(b)))
        })
        .filter_map(|face| face.families.first().map(|(name, _lang)| name.clone()))
        .filter(|name| {
            let l = name.to_lowercase();
            !l.contains("emoji") && !l.contains("nerd") && !l.contains("tabler")
        })
        .collect();
    names.sort_unstable_by_key(|s| s.to_lowercase());
    names.dedup();
    names
}

/// Emit the quad(s) that represent one cell's underline into `quads`.
///
/// * `x`, `baseline_y` — physical-px top-left of the bottom single-underline
///   strip (callers compute this as `cell_bottom - thickness - 1`).
/// * `cell_w` — physical width of one cell.
/// * `t` — stroke thickness in physical pixels (already scaled).
/// * `color` — sRGB colour (underline_color or fg).
///
/// # Styles
/// - `Single`  → one thin quad at `baseline_y`.
/// - `Double`  → two thin quads with a `2*t` gap between them.
/// - `Dotted`  → alternating on/off quads of width `3*t`, gap `2*t`.
/// - `Dashed`  → alternating on/off quads of width `6*t`, gap `4*t`.
/// - `Curly`   → a row of small "zigzag" quads approximating an undercurl
///   (alternating top/bottom rows of `t × t` quads).
/// - `None`    → nothing (caller should skip this call entirely).
fn emit_underline_quads(
    quads: &mut Vec<Quad>,
    style: UnderlineStyle,
    x: f32,
    baseline_y: f32,
    cell_w: f32,
    t: f32,
    color: [u8; 3],
) {
    match style {
        UnderlineStyle::None => {}
        UnderlineStyle::Single => {
            quads.push(Quad::new([x, baseline_y], [cell_w, t], color, 1.0));
        }
        UnderlineStyle::Double => {
            // Two lines separated by a gap equal to twice the stroke thickness.
            let gap = t * 2.0;
            quads.push(Quad::new([x, baseline_y], [cell_w, t], color, 1.0));
            quads.push(Quad::new(
                [x, baseline_y - gap - t],
                [cell_w, t],
                color,
                1.0,
            ));
        }
        UnderlineStyle::Dotted => {
            // 3t-wide dots separated by 2t gaps, advancing left→right.
            let dot_w = (t * 3.0).max(1.0);
            let gap_w = (t * 2.0).max(1.0);
            let step = dot_w + gap_w;
            let mut cx = x;
            while cx < x + cell_w {
                let w = (dot_w).min((x + cell_w) - cx);
                if w > 0.0 {
                    quads.push(Quad::new([cx, baseline_y], [w, t], color, 1.0));
                }
                cx += step;
            }
        }
        UnderlineStyle::Dashed => {
            // 6t-wide dashes separated by 4t gaps.
            let dash_w = (t * 6.0).max(2.0);
            let gap_w = (t * 4.0).max(1.0);
            let step = dash_w + gap_w;
            let mut cx = x;
            while cx < x + cell_w {
                let w = (dash_w).min((x + cell_w) - cx);
                if w > 0.0 {
                    quads.push(Quad::new([cx, baseline_y], [w, t], color, 1.0));
                }
                cx += step;
            }
        }
        UnderlineStyle::Curly => {
            // Undercurl approximation: 2-row zigzag.
            // Each "segment" is one square of size t×t that alternates between
            // the baseline row and one t below it, creating a wave shape.
            // Amplitude = t so it reads as wavy at small sizes.
            let seg = (t * 2.0).max(1.0);
            let mut cx = x;
            let mut phase = false; // false = top of wave, true = bottom
            while cx < x + cell_w {
                let w = seg.min((x + cell_w) - cx);
                let row_y = if phase { baseline_y + t } else { baseline_y };
                quads.push(Quad::new([cx, row_y], [w, t], color, 1.0));
                cx += seg;
                phase = !phase;
            }
        }
    }
}

/// Pure pill-geometry helper: given a slice of items and layout parameters,
/// return `(tab_rects, group_pills)` without a GPU. Mirrors the core logic
/// of `tab_layout` for the group-pill path so it can be unit-tested.
///
/// `items` is `(has_label, label_char_count)` per tab. Returns
/// `(tab_x_starts, pill_rects_with_first_idx)`.
#[cfg(test)]
fn compute_pill_geometry_pure(
    items: &[(bool, usize)], // (has_group_label, label char count)
    tab_w: f32,
    font_size: f32,
    start_x: f32,
) -> (Vec<f32>, Vec<(LogicalRect, usize)>) {
    let y = 4.0;
    let tab_h = TAB_BAR_HEIGHT - 8.0;
    let mut x = start_x;
    let mut tab_xs = Vec::new();
    let mut pills = Vec::new();
    for (idx, &(has_label, char_count)) in items.iter().enumerate() {
        if has_label {
            let pill_w = (GROUP_PILL_PAD_X * 2.0 + font_size * 0.62 * 0.72 * char_count as f32)
                .clamp(24.0, 140.0);
            let pill_rect = LogicalRect {
                x: x + GROUP_PILL_GAP,
                y,
                w: pill_w,
                h: tab_h,
            };
            pills.push((pill_rect, idx));
            x += pill_w + GROUP_PILL_GAP * 2.0;
        }
        tab_xs.push(x);
        x += tab_w;
    }
    (tab_xs, pills)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── scrollbar_scroll_for_thumb (drag → scroll mapping) ────────────────────

    fn test_geom(history: usize, rows: usize) -> ScrollbarGeom {
        ScrollbarGeom {
            track_x: 1000.0,
            track_top: 40.0,
            track_w: 8.0,
            track_h: 600.0,
            thumb_top: 40.0,
            thumb_h: 60.0,
            history,
            rows,
        }
    }

    #[test]
    fn thumb_at_track_top_is_deepest_history() {
        let g = test_geom(500, 40);
        assert_eq!(scrollbar_scroll_for_thumb(&g, 40.0), 500);
        // Dragging past the top clamps.
        assert_eq!(scrollbar_scroll_for_thumb(&g, -100.0), 500);
    }

    #[test]
    fn thumb_at_track_bottom_is_live() {
        let g = test_geom(500, 40);
        assert_eq!(scrollbar_scroll_for_thumb(&g, 40.0 + 600.0), 0);
        // Past the bottom clamps too.
        assert_eq!(scrollbar_scroll_for_thumb(&g, 10_000.0), 0);
    }

    #[test]
    fn thumb_midway_is_about_half_the_history() {
        let g = test_geom(500, 40);
        let max_frac = 1.0 - (40.0_f32 / 540.0).max(0.04);
        let mid_y = 40.0 + 0.5 * max_frac * 600.0;
        let s = scrollbar_scroll_for_thumb(&g, mid_y);
        assert!((240..=260).contains(&s), "expected ~250, got {s}");
    }

    #[test]
    fn no_history_is_always_live() {
        let g = test_geom(0, 40);
        assert_eq!(scrollbar_scroll_for_thumb(&g, 40.0), 0);
    }

    // ── reanchored_row (selection follows the text) ───────────────────────────

    #[test]
    fn selection_stays_put_when_nothing_moves() {
        assert_eq!(reanchored_row(5, 0, 100, 0, 100), Some(5));
    }

    #[test]
    fn scrolling_into_history_moves_selection_down() {
        // Selected at the live bottom (S=0), then panned 3 lines up (S'=3):
        // the selected text appears 3 rows lower on screen.
        assert_eq!(reanchored_row(5, 0, 100, 3, 100), Some(8));
    }

    #[test]
    fn scrolling_back_toward_live_moves_selection_up() {
        // Selected while panned (S=10), then scrolled back down (S'=4).
        assert_eq!(reanchored_row(8, 10, 100, 4, 100), Some(2));
        // Far enough back that the line leaves through the top → None.
        assert_eq!(reanchored_row(3, 10, 100, 0, 100), None);
    }

    #[test]
    fn new_output_moves_selection_up() {
        // At the live bottom, 4 new lines push the text up 4 rows.
        assert_eq!(reanchored_row(6, 0, 100, 0, 104), Some(2));
        // Enough output that it scrolls off the top → None.
        assert_eq!(reanchored_row(6, 0, 100, 0, 110), None);
    }

    #[test]
    fn pinned_scroll_during_output_is_stable() {
        // Panned into history while output streams in: the terminal keeps
        // the viewport pinned by growing S with H, so the same text stays
        // at the same row and the selection must not move.
        assert_eq!(reanchored_row(7, 10, 100, 15, 105), Some(7));
    }

    #[test]
    fn cell_size_is_positive() {
        let mut fs = FontSystem::new();
        let (w, h) = monospace_cell_size(&mut fs, 14.0);
        assert!(w > 0.0);
        assert!(h > 0.0);
    }

    #[test]
    fn matrix_atlas_actually_rasterizes_glyphs() {
        let mut fs = FontSystem::new();
        let mut sc = SwashCache::new();
        let atlas = build_matrix_atlas(&mut fs, &mut sc);
        let nonzero = atlas.data.iter().filter(|&&b| b > 0).count();
        // Per-cell coverage: how many of the grid cells got any ink.
        let cell = (atlas.width / atlas.cols) as usize;
        let mut inked_cells = 0usize;
        for i in 0..atlas.count as usize {
            let cx = (i % atlas.cols as usize) * cell;
            let cy = (i / atlas.cols as usize) * cell;
            let mut any = false;
            for yy in 0..cell {
                for xx in 0..cell {
                    if atlas.data[(cy + yy) * atlas.width as usize + (cx + xx)] > 0 {
                        any = true;
                    }
                }
            }
            if any {
                inked_cells += 1;
            }
        }
        eprintln!(
            "matrix atlas: {}x{}, {} glyphs, {} nonzero px, {}/{} cells inked",
            atlas.width, atlas.height, atlas.count, nonzero, inked_cells, atlas.count
        );
        assert!(
            inked_cells > atlas.count as usize / 2,
            "most glyph cells should have ink (katakana fallback failing?) — only {inked_cells}/{} inked",
            atlas.count
        );
    }

    #[test]
    fn enumerated_monospace_families_are_all_resolvable() {
        // Every family the Settings picker would offer must pass
        // family_is_available, so selecting any of them resolves through
        // set_font_family instead of warning and falling back to the default.
        // Bundled fonts are included so the invariant covers them too.
        let mut fs = FontSystem::new();
        load_symbol_fonts(&mut fs);
        bundled_fonts::load_bundled_fonts(&mut fs);
        let names = monospace_families_in(fs.db());
        for n in &names {
            let l = n.to_lowercase();
            assert!(
                !l.contains("emoji") && !l.contains("nerd") && !l.contains("tabler"),
                "emoji/symbol/icon fallback font leaked into the picker list: {n}"
            );
            assert!(
                family_is_available(&fs, n),
                "enumerated monospace family is not resolvable: {n}"
            );
        }
    }

    #[test]
    fn symbol_fonts_register_into_empty_db() {
        // Start from an empty database so the assertions can't be satisfied
        // by whatever emoji font the host OS happens to ship — this proves
        // the *bundled* fonts are what get registered.
        let db = glyphon::fontdb::Database::new();
        let mut fs = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        assert_eq!(fs.db().len(), 0, "expected an empty database to start");

        load_symbol_fonts(&mut fs);

        // All four bundled faces must now be present
        // (Hack + NotoEmoji + emoji-icon-font + Tabler Icons subset).
        assert!(
            fs.db().len() >= 4,
            "all four bundled symbol fonts should be registered, got {}",
            fs.db().len()
        );
        // Hack provides geometric/arrow icon coverage.
        assert!(
            family_is_available(&fs, "Hack"),
            "Hack face should be queryable by family name"
        );
        // The monochrome NotoEmoji face carries the bulk of the emoji range.
        assert!(
            family_is_available(&fs, "Noto Emoji"),
            "NotoEmoji face should be queryable by family name"
        );
    }

    #[test]
    fn every_ui_icon_is_covered_no_tofu() {
        // Build an isolated FontSystem (no host fonts) and load only the
        // bundled symbol faces via load_symbol_fonts.  Then assert that every
        // codepoint in ICON_CODEPOINTS has at least one face in the database
        // that contains a non-.notdef glyph for it.
        //
        // This test FAILS before adding Hack to load_symbol_fonts (the
        // geometric/arrow codepoints ↑ ↓ ▲ ▼ ⊕ ▐ ● etc. have no coverage in
        // NotoEmoji or emoji-icon-font alone) and PASSES once Hack is loaded.
        let db = glyphon::fontdb::Database::new();
        let mut fs = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        load_symbol_fonts(&mut fs);

        for &ch in ICON_CODEPOINTS {
            let covered = fs.db().faces().any(|face| {
                fs.db()
                    .with_face_data(face.id, |data, face_index| {
                        ttf_parser::Face::parse(data, face_index)
                            .ok()
                            .and_then(|f| f.glyph_index(ch))
                            .is_some()
                    })
                    .unwrap_or(false)
            });
            assert!(
                covered,
                "icon codepoint U+{:04X} '{}' has no coverage in any loaded face \
                 (tofu would appear in the UI)",
                ch as u32, ch
            );
        }
    }

    #[test]
    fn tabler_icons_no_tofu() {
        // Build an isolated FontSystem (no host fonts) and load only the
        // bundled symbol faces via load_symbol_fonts. Assert that every
        // codepoint in TABLER_CODEPOINTS has at least one face that contains
        // a non-.notdef glyph — i.e. the Tabler subset is actually covering
        // the codepoints we embedded.
        let db = glyphon::fontdb::Database::new();
        let mut fs = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        load_symbol_fonts(&mut fs);

        // Collect unique codepoints (the list may contain duplicates).
        let mut unique: Vec<char> = TABLER_CODEPOINTS.to_vec();
        unique.sort_unstable();
        unique.dedup();

        for &ch in &unique {
            let covered = fs.db().faces().any(|face| {
                fs.db()
                    .with_face_data(face.id, |data, face_index| {
                        ttf_parser::Face::parse(data, face_index)
                            .ok()
                            .and_then(|f| f.glyph_index(ch))
                            .is_some()
                    })
                    .unwrap_or(false)
            });
            assert!(
                covered,
                "Tabler icon U+{:04X} '{}' has no coverage in any loaded face \
                 (tofu would appear when bundled_icons=true)",
                ch as u32, ch
            );
        }
    }

    #[test]
    fn tabler_family_excluded_from_monospace_picker() {
        // The Tabler Icons face must never appear in the font picker because
        // it only contains PUA codepoints and would produce tofu for any text.
        let db = glyphon::fontdb::Database::new();
        let mut fs = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        load_symbol_fonts(&mut fs);

        let names = monospace_families_in(fs.db());
        for n in &names {
            assert!(
                !n.to_lowercase().contains("tabler"),
                "Tabler Icons family leaked into the monospace picker: {n}"
            );
        }
    }

    // ── Bundled font tests ────────────────────────────────────────────────────

    #[test]
    fn bundled_fonts_register_into_empty_db() {
        // Start from an empty database so no host fonts can satisfy the
        // assertions — this proves the *bundled* faces are what get registered.
        let db = glyphon::fontdb::Database::new();
        let mut fs = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        assert_eq!(fs.db().len(), 0, "expected an empty database to start");

        bundled_fonts::load_bundled_fonts(&mut fs);

        // Each bundled family contributes at least a Regular + Bold face.
        let expected_min = bundled_fonts::BUNDLED_FONTS.len() * 2;
        assert!(
            fs.db().len() >= expected_min,
            "expected at least {expected_min} faces (2 per family), got {}",
            fs.db().len()
        );
        // Every declared family must now be queryable.
        for name in bundled_fonts::bundled_family_names() {
            assert!(
                family_is_available(&fs, name),
                "bundled family not resolvable after load_bundled_fonts: {name}"
            );
        }
    }

    #[test]
    fn bundled_families_appear_in_picker() {
        // After loading bundled fonts the picker enumeration must include
        // every bundled family — confirming monospace_families_in accepts them
        // even when the TTF OS/2 table doesn't set the monospaced flag.
        let db = glyphon::fontdb::Database::new();
        let mut fs = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        bundled_fonts::load_bundled_fonts(&mut fs);

        let names = monospace_families_in(fs.db());
        for family in bundled_fonts::bundled_family_names() {
            assert!(
                names.iter().any(|n| n.eq_ignore_ascii_case(family)),
                "bundled family missing from picker list: {family}"
            );
        }
    }

    #[test]
    fn bundled_font_produces_real_face_match_not_fallback() {
        // Shape a Buffer with Ubuntu Mono and confirm the resolved glyph
        // comes from a face whose first family name is "Ubuntu Mono" (i.e.
        // the bundled face was actually used, not a system fallback).
        let db = glyphon::fontdb::Database::new();
        let mut fs = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        bundled_fonts::load_bundled_fonts(&mut fs);

        let metrics = Metrics::new(14.0, 14.0);
        let mut buf = Buffer::new(&mut fs, metrics);
        buf.set_size(&mut fs, Some(1024.0), Some(1024.0));
        buf.set_text(
            &mut fs,
            "M",
            Attrs::new().family(Family::Name("Ubuntu Mono")),
            Shaping::Advanced,
        );

        let run = buf
            .layout_runs()
            .next()
            .expect("at least one layout run for 'M'");
        let glyph = run.glyphs.first().expect("at least one glyph in run");
        let face = fs.db().face(glyph.font_id).expect("font_id must resolve");
        let matched_family = face.families.first().map_or("", |(n, _)| n.as_str());
        assert_eq!(
            matched_family, "Ubuntu Mono",
            "expected glyph from Ubuntu Mono face, got '{matched_family}'"
        );
    }

    #[test]
    fn bundled_bold_weight_resolves() {
        // Shape bold text with Ubuntu Mono. The resolved face should be the
        // Bold cut (weight >= 700) rather than a synthesized fallback.
        use glyphon::Weight;
        let db = glyphon::fontdb::Database::new();
        let mut fs = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        bundled_fonts::load_bundled_fonts(&mut fs);

        let metrics = Metrics::new(14.0, 14.0);
        let mut buf = Buffer::new(&mut fs, metrics);
        buf.set_size(&mut fs, Some(1024.0), Some(1024.0));
        buf.set_text(
            &mut fs,
            "M",
            Attrs::new()
                .family(Family::Name("Ubuntu Mono"))
                .weight(Weight::BOLD),
            Shaping::Advanced,
        );

        let run = buf.layout_runs().next().expect("at least one layout run");
        let glyph = run.glyphs.first().expect("at least one glyph");
        let face = fs.db().face(glyph.font_id).expect("font_id must resolve");
        // Bold cut should have weight >= 700; Regular is typically 400.
        assert!(
            face.weight.0 >= 700,
            "expected Bold weight (>=700), got {} for Ubuntu Mono bold shaping",
            face.weight.0
        );
    }

    #[test]
    fn every_bundled_family_name_matches_embedded_face() {
        // For each bundled font, load ONLY its two faces into a fresh empty db
        // and verify family_is_available resolves with the declared name string.
        // This catches a mismatch between the constant in bundled_fonts.rs and
        // the actual name table inside the TTF.
        for bf in bundled_fonts::BUNDLED_FONTS {
            let db = glyphon::fontdb::Database::new();
            let mut fs = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
            fs.db_mut().load_font_data(bf.regular.to_vec());
            fs.db_mut().load_font_data(bf.bold.to_vec());
            assert!(
                family_is_available(&fs, bf.family),
                "declared family name '{}' does not match the TTF name table \
                 (family_is_available returned false with only that family loaded)",
                bf.family
            );
        }
    }

    #[test]
    fn bundled_includes_ubuntu_mono() {
        assert!(
            bundled_fonts::bundled_family_names()
                .iter()
                .any(|n| n.eq_ignore_ascii_case("Ubuntu Mono")),
            "Ubuntu Mono must be in the bundled font list"
        );
    }

    #[test]
    fn cell_rect_iterates_inclusive() {
        let r = CellRect {
            anchor: (3, 1),
            cursor: (5, 1),
            block: false,
        };
        let cells: Vec<_> = r.cells().collect();
        assert_eq!(cells, vec![(3, 1), (4, 1), (5, 1)]);
    }

    #[test]
    fn cells_for_subtracts_padding_and_floors() {
        // 1024×600 logical, 8×16 cells, 8px padding, 44px top offset, 8px bottom,
        // 0px left/right strip.
        // width: (1024 - 16 - 0 - 0)/8 = 126; height: (600 - 44 - 8)/16 = 34.25 → 34.
        assert_eq!(
            cells_for(
                1024.0,
                600.0,
                8.0,
                16.0,
                8.0,
                GridOffsets {
                    top: 44.0,
                    bottom: 8.0,
                    left: 0.0,
                    right: 0.0
                }
            ),
            (126, 34)
        );
        // Padding/top offset are both removed from height.
        assert_eq!(
            cells_for(
                108.0,
                100.0,
                10.0,
                10.0,
                4.0,
                GridOffsets {
                    top: 20.0,
                    bottom: 4.0,
                    left: 0.0,
                    right: 0.0
                }
            ),
            (10, 7)
        );
        // Never returns 0×0, even for an absurdly tiny window.
        assert_eq!(
            cells_for(
                5.0,
                5.0,
                8.0,
                16.0,
                8.0,
                GridOffsets {
                    top: 44.0,
                    bottom: 8.0,
                    left: 0.0,
                    right: 0.0
                }
            ),
            (1, 1)
        );
    }

    /// When a left vertical strip is present, cols must shrink accordingly.
    #[test]
    fn cells_for_vertical_strip_shrinks_cols() {
        // 1024×600, 8×16 cells, 8px padding, 44px top, 8px bottom, 180px left strip.
        // width: (1024 - 16 - 180 - 0)/8 = 828/8 = 103.5 → 103
        let (cols, _) = cells_for(
            1024.0,
            600.0,
            8.0,
            16.0,
            8.0,
            GridOffsets {
                top: 44.0,
                bottom: 8.0,
                left: 180.0,
                right: 0.0,
            },
        );
        assert_eq!(cols, 103, "left strip must reduce columns");

        // Right strip same math.
        let (cols_r, _) = cells_for(
            1024.0,
            600.0,
            8.0,
            16.0,
            8.0,
            GridOffsets {
                top: 44.0,
                bottom: 8.0,
                left: 0.0,
                right: 180.0,
            },
        );
        assert_eq!(cols_r, 103, "right strip must reduce columns");
    }

    /// When the status bar is at the bottom, the grid must have one fewer row
    /// than without it, given enough height.
    #[test]
    fn cells_for_status_bar_bottom_shrinks_rows() {
        // 1000×600 logical, 10×20 cells, 4px padding, 40px top_offset.
        // Without status bar (bottom_offset = 4):
        //   rows = (600 - 40 - 4) / 20 = 27.8 → 27
        let (_, rows_no_bar) = cells_for(
            1000.0,
            600.0,
            10.0,
            20.0,
            4.0,
            GridOffsets {
                top: 40.0,
                bottom: 4.0,
                left: 0.0,
                right: 0.0,
            },
        );
        // With status bar at bottom (bottom_offset = 4 + 22 = 26):
        //   rows = (600 - 40 - 26) / 20 = 26.7 → 26
        let (_, rows_with_bar) = cells_for(
            1000.0,
            600.0,
            10.0,
            20.0,
            4.0,
            GridOffsets {
                top: 40.0,
                bottom: 4.0 + STATUS_BAR_HEIGHT,
                left: 0.0,
                right: 0.0,
            },
        );
        assert_eq!(
            rows_no_bar,
            rows_with_bar + 1,
            "status bar at bottom must reduce rows by 1"
        );
    }

    #[test]
    fn sanitize_tab_widths_clamps_and_uninverts() {
        // In-range values pass through untouched.
        assert_eq!(sanitize_tab_widths(90.0, 260.0), (90.0, 260.0));
        // Out-of-range bounds are pinned to [16, 800].
        assert_eq!(sanitize_tab_widths(0.0, 5000.0), (16.0, 800.0));
        // An inverted pair is fixed by raising max to min.
        assert_eq!(sanitize_tab_widths(200.0, 100.0), (200.0, 200.0));
    }

    #[test]
    fn slot_from_midpoints_picks_insertion_index() {
        // Three 100px tabs starting at x=8: midpoints at 58, 158, 258.
        let tabs = [(8.0, 100.0), (108.0, 100.0), (208.0, 100.0)];
        // Far left → before the first tab.
        assert_eq!(slot_from_midpoints(&tabs, 0.0), 0);
        // Left half of tab 0 → still slot 0.
        assert_eq!(slot_from_midpoints(&tabs, 50.0), 0);
        // Past tab 0's midpoint but before tab 1's → slot 1.
        assert_eq!(slot_from_midpoints(&tabs, 60.0), 1);
        assert_eq!(slot_from_midpoints(&tabs, 150.0), 1);
        // Past tab 1's midpoint → slot 2.
        assert_eq!(slot_from_midpoints(&tabs, 200.0), 2);
        // Past the last midpoint → append (slot 3).
        assert_eq!(slot_from_midpoints(&tabs, 999.0), 3);
    }

    #[test]
    fn slot_from_midpoints_exact_midpoint_rounds_right() {
        // A cursor exactly on a midpoint is NOT `< midpoint`, so it lands in
        // the *next* slot — deterministic, no flicker at the boundary.
        let tabs = [(0.0, 100.0)];
        assert_eq!(slot_from_midpoints(&tabs, 50.0), 1);
        assert_eq!(slot_from_midpoints(&tabs, 49.9), 0);
    }

    #[test]
    fn slot_from_midpoints_empty_is_zero() {
        assert_eq!(slot_from_midpoints(&[], 123.0), 0);
    }

    #[test]
    fn save_prompt_layout_is_top_centred() {
        let l = save_prompt_layout(1000, 700, 1.0);
        // Card is inset from the top and horizontally centred.
        assert!((l.card.y - SAVE_PROMPT_TOP).abs() < 1e-3);
        let centre = l.card.x + l.card.w * 0.5;
        assert!((centre - 500.0).abs() < 1e-3, "card should be centred");
        // The three targets all sit inside the card.
        for sub in [l.checkbox, l.save, l.dismiss] {
            assert!(sub.x >= l.card.x && sub.x + sub.w <= l.card.x + l.card.w);
            assert!(sub.y >= l.card.y && sub.y + sub.h <= l.card.y + l.card.h);
        }
        // Save sits to the left of Dismiss, and they don't overlap.
        assert!(l.save.x + l.save.w <= l.dismiss.x);
    }

    #[test]
    fn save_prompt_layout_is_dpi_aware() {
        // Same logical placement regardless of DPI scale.
        let a = save_prompt_layout(1000, 700, 1.0);
        let b = save_prompt_layout(2000, 1400, 2.0);
        assert!((a.card.x - b.card.x).abs() < 1e-3);
        assert!((a.save.x - b.save.x).abs() < 1e-3);
        assert!((a.dismiss.x - b.dismiss.x).abs() < 1e-3);
    }

    #[test]
    fn save_prompt_hit_targets_are_distinct() {
        // Each target's centre maps to its own hit, and the targets don't
        // collide (a click in one never lands in another).
        let scale = 1.0;
        let l = save_prompt_layout(1000, 700, scale);
        let centre = |r: LogicalRect| (r.x + r.w * 0.5, r.y + r.h * 0.5);
        let in_box = |r: LogicalRect, p: (f32, f32)| r.contains(p.0, p.1);

        let cb = centre(l.checkbox);
        let sv = centre(l.save);
        let dm = centre(l.dismiss);
        assert!(in_box(l.checkbox, cb) && !in_box(l.save, cb) && !in_box(l.dismiss, cb));
        assert!(in_box(l.save, sv) && !in_box(l.checkbox, sv) && !in_box(l.dismiss, sv));
        assert!(in_box(l.dismiss, dm) && !in_box(l.checkbox, dm) && !in_box(l.save, dm));
    }

    // ── Focus-border geometry unit tests ──────────────────────────────────
    //
    // These tests exercise `compute_focus_border_quads` directly without
    // needing a full wgpu renderer — so we factor the geometry math into a
    // pure helper and test that.

    /// Pure helper that replicates the geometry in `render_panes`.
    /// Returns the four quads as `(x, y, w, h)` tuples, or `None` when
    /// `thickness` is `<= 0`.
    fn compute_focus_border_quads(
        rect_px: (f32, f32, f32, f32),
        thickness: f32,
    ) -> Option<[(f32, f32, f32, f32); 4]> {
        if thickness <= 0.0 {
            return None;
        }
        let (rx, ry, rw, rh) = rect_px;
        let (fx, fy, fw, fh) = (rx.round(), ry.round(), rw.round(), rh.round());
        let t = thickness;
        let i = t.ceil();
        let inner_w = (fw - 2.0 * i).max(0.0);
        let inner_h = (fh - 2.0 * i).max(0.0);
        Some([
            // Top
            (fx + i, fy + i, inner_w, t),
            // Bottom
            (fx + i, fy + fh - i - t, inner_w, t),
            // Left
            (fx + i, fy + i, t, inner_h),
            // Right
            (fx + fw - i - t, fy + i, t, inner_h),
        ])
    }

    /// True when the quad `(qx, qy, qw, qh)` is completely inside the
    /// pane rect `(px, py, pw, ph)`.
    fn quad_inside_pane(
        (qx, qy, qw, qh): (f32, f32, f32, f32),
        (px, py, pw, ph): (f32, f32, f32, f32),
    ) -> bool {
        qx >= px && qy >= py && qx + qw <= px + pw && qy + qh <= py + ph
    }

    /// True when the two rects `(x,y,w,h)` do NOT overlap at all.
    fn rects_disjoint(
        (ax, ay, aw, ah): (f32, f32, f32, f32),
        (bx, by, bw, bh): (f32, f32, f32, f32),
    ) -> bool {
        ax + aw <= bx || bx + bw <= ax || ay + ah <= by || by + bh <= ay
    }

    #[test]
    fn focus_border_produces_four_quads_vertical_split_left_focused() {
        // Left pane in a 1000×600 vertical split at x=0, focus on left.
        let pane = (0.0_f32, 0.0, 500.0, 600.0);
        let quads = compute_focus_border_quads(pane, 2.0).expect("must produce quads");
        assert_eq!(quads.len(), 4, "should produce exactly 4 quads");
        for q in &quads {
            assert!(
                quad_inside_pane(*q, pane),
                "quad {q:?} must be strictly inside pane {pane:?}"
            );
        }
    }

    #[test]
    fn focus_border_produces_four_quads_vertical_split_right_focused() {
        // Right pane in a 1000×600 vertical split at x=500.
        let pane = (500.0_f32, 0.0, 500.0, 600.0);
        let quads = compute_focus_border_quads(pane, 2.0).expect("must produce quads");
        assert_eq!(quads.len(), 4);
        for q in &quads {
            assert!(
                quad_inside_pane(*q, pane),
                "quad {q:?} must be inside pane {pane:?}"
            );
        }
    }

    #[test]
    fn focus_border_produces_four_quads_horizontal_split_top_focused() {
        let pane = (0.0_f32, 0.0, 1000.0, 300.0);
        let quads = compute_focus_border_quads(pane, 2.0).expect("must produce quads");
        assert_eq!(quads.len(), 4);
        for q in &quads {
            assert!(quad_inside_pane(*q, pane), "quad {q:?} inside {pane:?}");
        }
    }

    #[test]
    fn focus_border_produces_four_quads_horizontal_split_bottom_focused() {
        let pane = (0.0_f32, 300.0, 1000.0, 300.0);
        let quads = compute_focus_border_quads(pane, 2.0).expect("must produce quads");
        assert_eq!(quads.len(), 4);
        for q in &quads {
            assert!(quad_inside_pane(*q, pane), "quad {q:?} inside {pane:?}");
        }
    }

    #[test]
    fn focus_border_quads_do_not_overlap_divider_vertical_split() {
        // Vertical split: left pane ends at x=498 (aw=498), divider is
        // centred at x=498 with thickness=4 → visible rect [496, 0, 4, 600].
        let left_pane = (0.0_f32, 0.0, 498.0, 600.0);
        let right_pane = (498.0_f32, 0.0, 502.0, 600.0);
        let divider = (496.0_f32, 0.0, 4.0, 600.0); // ±2px around boundary

        for &pane in &[left_pane, right_pane] {
            let quads = compute_focus_border_quads(pane, 2.0).expect("must produce quads");
            for q in &quads {
                assert!(
                    rects_disjoint(*q, divider),
                    "focus-border quad {q:?} must not overlap divider {divider:?}"
                );
            }
        }
    }

    #[test]
    fn focus_border_quads_do_not_overlap_divider_horizontal_split() {
        // Horizontal split: top pane ends at y=298, divider at [0, 296, 1000, 4].
        let top_pane = (0.0_f32, 0.0, 1000.0, 298.0);
        let bottom_pane = (0.0_f32, 298.0, 1000.0, 302.0);
        let divider = (0.0_f32, 296.0, 1000.0, 4.0);

        for &pane in &[top_pane, bottom_pane] {
            let quads = compute_focus_border_quads(pane, 2.0).expect("must produce quads");
            for q in &quads {
                assert!(
                    rects_disjoint(*q, divider),
                    "focus-border quad {q:?} must not overlap divider {divider:?}"
                );
            }
        }
    }

    #[test]
    fn focus_border_disabled_at_zero_thickness() {
        let pane = (0.0_f32, 0.0, 500.0, 400.0);
        assert!(
            compute_focus_border_quads(pane, 0.0).is_none(),
            "thickness=0 must return None (disabled)"
        );
    }

    #[test]
    fn focus_border_inner_span_never_negative() {
        // A tiny pane where inner_w / inner_h would underflow.
        let pane = (0.0_f32, 0.0, 3.0, 3.0); // only 3×3 px, t=2 → i=2
        let quads = compute_focus_border_quads(pane, 2.0).expect("must produce quads");
        // Even though inner_w = max(3 - 4, 0) = 0 the quads must have
        // non-negative dimensions.
        for &(_, _, w, h) in &quads {
            assert!(w >= 0.0, "quad width must be non-negative, got {w}");
            assert!(h >= 0.0, "quad height must be non-negative, got {h}");
        }
    }

    #[test]
    fn focus_border_snaps_sub_pixel_rects() {
        // A rect at 0.5, 0.5 (sub-pixel) should snap to integer coords.
        let pane = (0.5_f32, 0.5, 499.5, 599.5);
        let quads = compute_focus_border_quads(pane, 2.0).expect("must produce quads");
        // After snapping: (1.0, 1.0, 500.0, 600.0) — still strictly inside.
        let snapped_pane = (
            pane.0.round(),
            pane.1.round(),
            pane.2.round(),
            pane.3.round(),
        );
        for q in &quads {
            assert!(
                quad_inside_pane(*q, snapped_pane),
                "quad {q:?} must be inside snapped pane {snapped_pane:?}"
            );
        }
    }

    #[test]
    fn focus_border_inset_clears_default_divider_thickness() {
        // Default divider is 4 logical px; half_thick=2 px on each side of
        // boundary. Focus border at thickness=2, inset i=ceil(2)=2. So the
        // border's outer edge at `pane_x + i = 2` is exactly at the divider's
        // inner edge — they don't overlap.
        //
        // Left pane: rect (0, 0, 500, 600).
        // Divider visible rect straddles x=500: [-2..+2] from boundary =>
        //   in absolute terms (498, 0, 4, 600).
        // Border right quad: x = 500 - i - t = 500 - 2 - 2 = 496, w = 2.
        //   right edge = 498. Divider left edge = 498. Disjoint (abutting).
        let left_pane = (0.0_f32, 0.0, 500.0, 600.0);
        let divider = (498.0_f32, 0.0, 4.0, 600.0);
        let quads = compute_focus_border_quads(left_pane, 2.0).expect("quads");
        for q in &quads {
            assert!(
                rects_disjoint(*q, divider),
                "quad {q:?} must not overlap divider {divider:?}"
            );
        }
    }

    // ── Tab-bar polish unit tests ─────────────────────────────────────────

    #[test]
    fn truncate_tab_title_short_stays_unchanged() {
        // A title that fits within the budget is returned as-is.
        assert_eq!(truncate_tab_title("bash", 10), "bash");
        assert_eq!(truncate_tab_title("bash", 4), "bash");
    }

    #[test]
    fn truncate_tab_title_exact_budget_is_unchanged() {
        // Exactly `max_chars` characters — no ellipsis needed.
        assert_eq!(truncate_tab_title("hello", 5), "hello");
    }

    #[test]
    fn truncate_tab_title_long_gets_ellipsis() {
        // A title longer than the budget is truncated and gains "…".
        let result = truncate_tab_title("hello world", 7);
        assert!(
            result.ends_with('\u{2026}'),
            "must end with ellipsis: {result:?}"
        );
        // Total visible characters must be exactly max_chars (take=6 + "…"=1).
        assert_eq!(result.chars().count(), 7);
        assert_eq!(result, "hello \u{2026}");
    }

    #[test]
    fn truncate_tab_title_budget_one_is_just_ellipsis() {
        // Budget of 1 → take 0 chars + ellipsis.
        assert_eq!(truncate_tab_title("hello", 1), "\u{2026}");
    }

    #[test]
    fn truncate_tab_title_budget_zero_returns_flat_unchanged() {
        // Budget of 0 is treated as "no limit" — return the flat string.
        assert_eq!(truncate_tab_title("hello", 0), "hello");
    }

    #[test]
    fn truncate_tab_title_strips_newlines() {
        // Embedded newlines (e.g. a multi-line custom_title) are removed.
        assert_eq!(truncate_tab_title("foo\nbar", 20), "foobar");
        assert_eq!(truncate_tab_title("a\r\nb", 20), "ab");
    }

    #[test]
    fn truncate_tab_title_empty_string() {
        assert_eq!(truncate_tab_title("", 10), "");
        assert_eq!(truncate_tab_title("", 0), "");
    }

    #[test]
    fn darken_tab_bg_scales_channels() {
        // Each channel is multiplied by factor and rounded.
        let result = darken_tab_bg([0x1a, 0x20, 0x33], 0.8);
        // 0x1a=26 * 0.8 = 20.8 → 21 = 0x15
        // 0x20=32 * 0.8 = 25.6 → 26 = 0x1a
        // 0x33=51 * 0.8 = 40.8 → 41 = 0x29
        assert_eq!(result, [21, 26, 41]);
    }

    #[test]
    fn darken_tab_bg_clamps_to_255() {
        // Even with a factor > 1 the result never exceeds 255.
        let result = darken_tab_bg([0xff, 0xff, 0xff], 2.0);
        assert_eq!(result, [255, 255, 255]);
    }

    // ── Part A: tab-bar reservation ──────────────────────────────────────

    #[test]
    fn tab_bar_reservation_enabled_multiple_tabs() {
        assert!(
            (tab_bar_reservation(true, false, 3) - TAB_BAR_HEIGHT).abs() < f32::EPSILON,
            "enabled with multiple tabs should reserve TAB_BAR_HEIGHT"
        );
    }

    #[test]
    fn tab_bar_reservation_disabled_returns_zero() {
        assert!(
            tab_bar_reservation(false, false, 3).abs() < f32::EPSILON,
            "disabled tab bar must not reserve any height"
        );
        assert!(
            tab_bar_reservation(false, true, 3).abs() < f32::EPSILON,
            "disabled tab bar must not reserve any height even with hide_if_single"
        );
    }

    #[test]
    fn tab_bar_reservation_hide_if_single_one_tab() {
        assert!(
            tab_bar_reservation(true, true, 1).abs() < f32::EPSILON,
            "hide_if_single with 1 tab must return 0"
        );
        assert!(
            tab_bar_reservation(true, true, 0).abs() < f32::EPSILON,
            "hide_if_single with 0 tabs must return 0"
        );
    }

    #[test]
    fn tab_bar_reservation_hide_if_single_two_tabs() {
        assert!(
            (tab_bar_reservation(true, true, 2) - TAB_BAR_HEIGHT).abs() < f32::EPSILON,
            "hide_if_single with 2 tabs must still reserve height"
        );
    }

    // ── Part B: cursor ease alpha ────────────────────────────────────────

    #[test]
    fn cursor_ease_alpha_at_start_is_zero() {
        // At t=0 the cursor is at the start of the fade-in: phase=0 → alpha=0.
        let a = cursor_ease_alpha(0, 1000);
        assert!(
            a.abs() < 1e-5,
            "alpha at start of period should be 0.0, got {a}"
        );
    }

    #[test]
    fn cursor_ease_alpha_at_quarter_period_is_smooth() {
        // At t=250ms in a 1000ms period, phase=0.25 → tri=0.5 → smoothstep≈0.5
        let a = cursor_ease_alpha(250, 1000);
        // smoothstep(0,1,0.5) = 0.5*0.5*(3-2*0.5) = 0.25*2 = 0.5
        assert!(
            (a - 0.5).abs() < 1e-4,
            "alpha at quarter period should be ~0.5, got {a}"
        );
    }

    #[test]
    fn cursor_ease_alpha_at_half_period_is_one() {
        // At t=500ms in a 1000ms period, phase=0.5 → tri=1.0 → smoothstep=1.0
        let a = cursor_ease_alpha(500, 1000);
        assert!(
            (a - 1.0).abs() < 1e-4,
            "alpha at half period should be 1.0 (peak), got {a}"
        );
    }

    #[test]
    fn cursor_ease_alpha_at_three_quarter_period_is_smooth() {
        // At t=750ms, phase=0.75 → tri=(1-0.75)*2=0.5 → smoothstep=0.5
        let a = cursor_ease_alpha(750, 1000);
        assert!(
            (a - 0.5).abs() < 1e-4,
            "alpha at 3/4 period should be ~0.5, got {a}"
        );
    }

    #[test]
    fn cursor_ease_alpha_zero_period_returns_one() {
        // Degenerate: period=0 should return 1.0 (no blink).
        let a = cursor_ease_alpha(12345, 0);
        assert!((a - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cursor_ease_alpha_is_always_in_range() {
        // Exhaustive sweep: alpha must always be in [0.0, 1.0].
        for t in (0..2000u64).step_by(13) {
            let a = cursor_ease_alpha(t, 1000);
            assert!(
                (0.0..=1.0).contains(&a),
                "cursor_ease_alpha({t}, 1000) = {a} is out of [0, 1]"
            );
        }
    }

    // ── Part C: cell-width multiplier ────────────────────────────────────

    #[test]
    fn apply_cell_width_multiplier_identity() {
        let result = apply_cell_width_multiplier(8.0, 1.0);
        assert!((result - 8.0).abs() < 1e-5);
    }

    #[test]
    fn apply_cell_width_multiplier_widens() {
        let result = apply_cell_width_multiplier(8.0, 1.5);
        assert!((result - 12.0).abs() < 1e-4);
    }

    #[test]
    fn apply_cell_width_multiplier_narrows() {
        let result = apply_cell_width_multiplier(10.0, 0.9);
        assert!((result - 9.0).abs() < 1e-4);
    }

    #[test]
    fn apply_cell_width_multiplier_clamps_low() {
        // Below 0.8 is clamped to 0.8.
        let result = apply_cell_width_multiplier(10.0, 0.5);
        assert!((result - 8.0).abs() < 1e-4, "should clamp to 0.8: {result}");
    }

    #[test]
    fn apply_cell_width_multiplier_clamps_high() {
        // Above 2.0 is clamped to 2.0.
        let result = apply_cell_width_multiplier(10.0, 3.0);
        assert!(
            (result - 20.0).abs() < 1e-4,
            "should clamp to 2.0: {result}"
        );
    }

    // ── Status bar right-text positioning ────────────────────────────────────
    //
    // These tests verify the pure arithmetic used to right-align the status
    // bar text without a GPU device. The formula must produce a physical-px
    // x-offset that keeps the text inside the bar at any DPI scale.

    /// Helper that mirrors the production formula for the status-bar
    #[test]
    fn status_bar_right_tx_is_inside_bar() {
        // At 1× DPI: 1280 wide surface, ~85 logical px of measured text.
        let tx = status_bar_right_tx(1280.0, 85.0, 1.0);
        // Must be left of the right edge minus right margin.
        assert!(tx < 1280.0 - 8.0, "x must leave space for right margin");
        assert!(tx > 0.0, "x must be positive");
        // The text's right edge (tx + width*scale) must sit exactly at the
        // 8px right margin — the property the old char-count estimate broke.
        assert!(
            ((tx + 85.0) - (1280.0 - 8.0)).abs() < 0.01,
            "right edge must land on the right margin"
        );
    }

    #[test]
    fn status_bar_right_tx_dpi_scaling() {
        // At 2× DPI the surface is twice as wide in physical px.
        // The text x must scale proportionally so the visual position is the same.
        let tx1 = status_bar_right_tx(1280.0, 85.0, 1.0);
        let tx2 = status_bar_right_tx(2560.0, 85.0, 2.0);
        // tx2 should be approximately tx1 * 2 (same logical position, double physical).
        let ratio = tx2 / tx1;
        assert!(
            (ratio - 2.0).abs() < 0.01,
            "right-text x should scale with DPI: ratio={ratio}"
        );
    }

    #[test]
    fn status_bar_right_tx_clamps_to_zero() {
        // For an absurdly long text on a tiny surface the formula must not go negative.
        let tx = status_bar_right_tx(50.0, 800.0, 1.0);
        assert!(
            tx.abs() < f32::EPSILON,
            "must clamp to zero, not go negative"
        );
    }

    // ── Part D: vertical tab bar layout ─────────────────────────────────────

    /// A left-side vertical strip shifts the grid's x-origin by the strip width.
    /// Verify via `cells_for` that the column count shrinks by the expected amount.
    #[test]
    fn vertical_strip_left_shrinks_columns() {
        // 1000 logical px wide, 10 px cell, 4 px padding, no top/bottom offset.
        // Without strip: (1000 - 8) / 10 = 99 cols
        let (cols_no_strip, _) = cells_for(
            1000.0,
            600.0,
            10.0,
            20.0,
            4.0,
            GridOffsets {
                top: 4.0,
                bottom: 4.0,
                left: 0.0,
                right: 0.0,
            },
        );
        // With 180-px left strip: (1000 - 8 - 180) / 10 = 81 cols
        let (cols_with_strip, _) = cells_for(
            1000.0,
            600.0,
            10.0,
            20.0,
            4.0,
            GridOffsets {
                top: 4.0,
                bottom: 4.0,
                left: 180.0,
                right: 0.0,
            },
        );
        assert_eq!(
            cols_no_strip as i32 - cols_with_strip as i32,
            18,
            "left strip of 180 px must reduce cols by 18 (180/10)"
        );
    }

    /// A right-side vertical strip reduces columns by the same amount as left.
    #[test]
    fn vertical_strip_right_shrinks_columns() {
        let (cols_no_strip, _) = cells_for(
            1000.0,
            600.0,
            10.0,
            20.0,
            4.0,
            GridOffsets {
                top: 4.0,
                bottom: 4.0,
                left: 0.0,
                right: 0.0,
            },
        );
        let (cols_with_strip, _) = cells_for(
            1000.0,
            600.0,
            10.0,
            20.0,
            4.0,
            GridOffsets {
                top: 4.0,
                bottom: 4.0,
                left: 0.0,
                right: 180.0,
            },
        );
        assert_eq!(
            cols_no_strip as i32 - cols_with_strip as i32,
            18,
            "right strip of 180 px must reduce cols by 18 (180/10)"
        );
    }

    /// `TabBarPlacement::is_vertical` returns `true` for Left/Right only.
    #[test]
    fn tab_bar_placement_is_vertical() {
        assert!(TabBarPlacement::Left.is_vertical());
        assert!(TabBarPlacement::Right.is_vertical());
        assert!(!TabBarPlacement::Top.is_vertical());
        assert!(!TabBarPlacement::Bottom.is_vertical());
    }

    // ── SGR attribute helpers ────────────────────────────────────────────────

    /// Build a minimal CellSnapshot for testing the SGR helpers.
    fn test_snap(fg: [u8; 3], bg: [u8; 3], dim: bool, inverse: bool, hidden: bool) -> CellSnapshot {
        CellSnapshot {
            ch: 'A',
            fg,
            bg,
            bold: false,
            italic: false,
            underline_style: terminale_term::UnderlineStyle::None,
            underline_color: None,
            strikethrough: false,
            overline: false,
            has_link: false,
            dim,
            inverse,
            hidden,
        }
    }

    /// lerp_u8(0, 255, 0.5) should be ~128.
    #[test]
    fn lerp_u8_midpoint() {
        let result = lerp_u8(0, 255, 0.5);
        // Expect 128 (0 + 255*0.5 = 127.5, rounds to 128).
        assert!(
            result == 127 || result == 128,
            "midpoint lerp should be ~128, got {result}"
        );
    }

    /// lerp_u8 at t=0.0 returns the first colour unchanged.
    #[test]
    fn lerp_u8_at_zero() {
        assert_eq!(lerp_u8(100, 200, 0.0), 100);
    }

    /// lerp_u8 at t=1.0 returns the second colour.
    #[test]
    fn lerp_u8_at_one() {
        assert_eq!(lerp_u8(100, 200, 1.0), 200);
    }

    /// Inverse swaps fg and bg; dim=false, hidden=false means no further change.
    #[test]
    fn apply_inverse_swaps_fg_bg() {
        let snap = test_snap([255, 0, 0], [0, 0, 255], false, true, false);
        let out = apply_sgr_attributes(snap, 0.5);
        assert_eq!(out.fg, [0, 0, 255], "inverse must swap fg to original bg");
        assert_eq!(out.bg, [255, 0, 0], "inverse must swap bg to original fg");
    }

    /// Dim at amount=0.5 blends fg halfway toward bg.
    #[test]
    fn apply_dim_blends_fg_toward_bg() {
        // fg=[200,200,200], bg=[0,0,0], amount=1.0 → fg should become [0,0,0]
        let snap = test_snap([200, 200, 200], [0, 0, 0], true, false, false);
        let out = apply_sgr_attributes(snap, 1.0);
        assert_eq!(out.fg, [0, 0, 0], "dim at 1.0 must collapse fg to bg");
    }

    /// Dim at amount=0.0 leaves fg unchanged.
    #[test]
    fn apply_dim_zero_amount_leaves_fg_unchanged() {
        let snap = test_snap([150, 100, 50], [0, 0, 0], true, false, false);
        let out = apply_sgr_attributes(snap, 0.0);
        assert_eq!(out.fg, [150, 100, 50], "dim=0.0 must not change fg");
    }

    /// Hidden forces fg = bg regardless of original fg value.
    #[test]
    fn apply_hidden_sets_fg_to_bg() {
        let snap = test_snap([200, 100, 50], [10, 20, 30], false, false, true);
        let out = apply_sgr_attributes(snap, 0.5);
        assert_eq!(out.fg, out.bg, "hidden must set fg == bg");
        assert_eq!(out.bg, [10, 20, 30], "bg must remain unchanged");
    }

    /// No flags set → snap is unchanged.
    #[test]
    fn apply_no_flags_leaves_snap_unchanged() {
        let snap = test_snap([200, 150, 100], [10, 20, 30], false, false, false);
        let out = apply_sgr_attributes(snap, 0.5);
        assert_eq!(out.fg, [200, 150, 100]);
        assert_eq!(out.bg, [10, 20, 30]);
    }

    /// inverse + dim: swap first, then dim the (post-swap) fg.
    #[test]
    fn apply_inverse_then_dim() {
        // Original: fg=[200,200,200], bg=[0,0,0]
        // After inverse: fg=[0,0,0], bg=[200,200,200]
        // After dim at 1.0: fg blended to new bg=[200,200,200] fully → fg=[200,200,200]
        let snap = test_snap([200, 200, 200], [0, 0, 0], true, true, false);
        let out = apply_sgr_attributes(snap, 1.0);
        assert_eq!(
            out.fg,
            [200, 200, 200],
            "dim after inverse should produce fg==new-bg"
        );
        assert_eq!(out.bg, [200, 200, 200]);
    }

    // ── WCAG contrast tests ───────────────────────────────────────────────────

    /// White on black must be 21:1.
    #[test]
    fn wcag_contrast_white_on_black_is_21() {
        let r = wcag_contrast([255, 255, 255], [0, 0, 0]);
        assert!(
            (r - 21.0).abs() < 0.1,
            "white-on-black must be ~21:1, got {r}"
        );
    }

    /// Black on black must be 1:1.
    #[test]
    fn wcag_contrast_identical_colours_is_1() {
        let r = wcag_contrast([100, 100, 100], [100, 100, 100]);
        assert!(
            (r - 1.0).abs() < 0.01,
            "identical colours must be 1:1, got {r}"
        );
    }

    /// Dark grey (#333) on black: confirm contrast ratio is small (< 3:1).
    #[test]
    fn wcag_contrast_dark_grey_on_black_is_low() {
        let r = wcag_contrast([0x33, 0x33, 0x33], [0x00, 0x00, 0x00]);
        assert!(r < 3.0, "dark grey on black must be < 3:1, got {r}");
    }

    /// enforce_min_contrast with ratio=1.0 is a no-op.
    #[test]
    fn enforce_min_contrast_ratio_one_is_noop() {
        let fg = [0x33, 0x33, 0x33];
        let bg = [0x00, 0x00, 0x00];
        assert_eq!(
            enforce_min_contrast(fg, bg, 1.0),
            fg,
            "ratio=1.0 must be a no-op"
        );
    }

    /// enforce_min_contrast with fg==bg returns fg unchanged (concealed text).
    #[test]
    fn enforce_min_contrast_fg_eq_bg_is_noop() {
        let colour = [0x55, 0x55, 0x55];
        assert_eq!(
            enforce_min_contrast(colour, colour, 7.0),
            colour,
            "fg==bg must be returned unchanged"
        );
    }

    /// enforce_min_contrast raises a low-contrast pair to >= the target ratio.
    #[test]
    fn enforce_min_contrast_raises_low_contrast_pair() {
        // Dark grey on black — naturally low contrast.
        let fg = [0x33, 0x33, 0x33];
        let bg = [0x00, 0x00, 0x00];
        let target = 4.5;
        let result = enforce_min_contrast(fg, bg, target);
        let achieved = wcag_contrast(result, bg);
        assert!(
            achieved >= target - 0.05,
            "enforce_min_contrast must raise contrast to >= {target}, achieved {achieved}"
        );
    }

    /// enforce_min_contrast on a pair that already meets the ratio returns fg.
    #[test]
    fn enforce_min_contrast_already_meets_ratio_returns_fg() {
        // White on black — 21:1 already exceeds any reasonable target.
        let fg = [255, 255, 255];
        let bg = [0, 0, 0];
        assert_eq!(
            enforce_min_contrast(fg, bg, 4.5),
            fg,
            "already-compliant fg must be returned unchanged"
        );
    }

    /// enforce_min_contrast with a light background pushes fg toward black.
    #[test]
    fn enforce_min_contrast_dark_fg_on_light_bg() {
        // Near-white fg on white bg — should be nudged toward black.
        let fg = [0xee, 0xee, 0xee];
        let bg = [0xff, 0xff, 0xff];
        let target = 4.5;
        let result = enforce_min_contrast(fg, bg, target);
        let achieved = wcag_contrast(result, bg);
        assert!(
            achieved >= target - 0.05,
            "light bg: enforce_min_contrast must raise contrast to >= {target}, achieved {achieved}"
        );
        // Result must be darker than the original fg (pushed toward black).
        assert!(
            u32::from(result[0]) <= u32::from(fg[0]),
            "result must be darker (lower R) than original fg on light bg"
        );
    }

    // ── per-style font-family selection helper ────────────────────────────

    /// Plain text: no overrides set, no synthesis needed.
    #[test]
    fn select_font_family_plain_text_returns_main() {
        let (fam, bold_w, italic_s) = select_font_family("Main", None, None, None, false, false);
        assert_eq!(fam, "Main");
        assert!(!bold_w);
        assert!(!italic_s);
    }

    /// Bold with no override → synthesize on main family.
    #[test]
    fn select_font_family_bold_no_override_synthesizes() {
        let (fam, bold_w, italic_s) = select_font_family("Main", None, None, None, true, false);
        assert_eq!(fam, "Main");
        assert!(bold_w);
        assert!(!italic_s);
    }

    /// Italic with no override → synthesize on main family.
    #[test]
    fn select_font_family_italic_no_override_synthesizes() {
        let (fam, bold_w, italic_s) = select_font_family("Main", None, None, None, false, true);
        assert_eq!(fam, "Main");
        assert!(!bold_w);
        assert!(italic_s);
    }

    /// Bold-italic with no overrides → synthesize both on main family.
    #[test]
    fn select_font_family_bold_italic_no_overrides_synthesizes() {
        let (fam, bold_w, italic_s) = select_font_family("Main", None, None, None, true, true);
        assert_eq!(fam, "Main");
        assert!(bold_w);
        assert!(italic_s);
    }

    /// Bold override set → use override family, no synthesis.
    #[test]
    fn select_font_family_bold_override_used_no_synthesis() {
        let (fam, bold_w, italic_s) =
            select_font_family("Main", Some("BoldFont"), None, None, true, false);
        assert_eq!(fam, "BoldFont");
        assert!(!bold_w);
        assert!(!italic_s);
    }

    /// Italic override set → use override family, no synthesis.
    #[test]
    fn select_font_family_italic_override_used_no_synthesis() {
        let (fam, bold_w, italic_s) =
            select_font_family("Main", None, Some("ItalicFont"), None, false, true);
        assert_eq!(fam, "ItalicFont");
        assert!(!bold_w);
        assert!(!italic_s);
    }

    /// Bold-italic override set → takes priority, no synthesis.
    #[test]
    fn select_font_family_bold_italic_override_used() {
        let (fam, bold_w, italic_s) = select_font_family(
            "Main",
            Some("BoldFont"),
            Some("ItalicFont"),
            Some("BoldItalicFont"),
            true,
            true,
        );
        assert_eq!(fam, "BoldItalicFont");
        assert!(!bold_w);
        assert!(!italic_s);
    }

    /// Bold-italic: no bold_italic_override, bold_override present → use bold
    /// family and still apply italic style on it.
    #[test]
    fn select_font_family_bold_italic_falls_back_to_bold_override() {
        let (fam, bold_w, italic_s) =
            select_font_family("Main", Some("BoldFont"), None, None, true, true);
        assert_eq!(fam, "BoldFont");
        assert!(!bold_w, "dedicated bold face; weight synthesis not needed");
        assert!(
            italic_s,
            "italic synthesis still applied on top of bold face"
        );
    }

    /// Bold-italic: no bold_italic or bold override, italic_override present →
    /// use italic family and still apply bold weight on it.
    #[test]
    fn select_font_family_bold_italic_falls_back_to_italic_override() {
        let (fam, bold_w, italic_s) =
            select_font_family("Main", None, Some("ItalicFont"), None, true, true);
        assert_eq!(fam, "ItalicFont");
        assert!(bold_w, "bold synthesis still applied on top of italic face");
        assert!(
            !italic_s,
            "dedicated italic face; slant synthesis not needed"
        );
    }

    /// Bold override does NOT affect italic-only cells.
    #[test]
    fn select_font_family_bold_override_not_used_for_italic_only() {
        let (fam, bold_w, italic_s) =
            select_font_family("Main", Some("BoldFont"), None, None, false, true);
        assert_eq!(fam, "Main");
        assert!(!bold_w);
        assert!(italic_s);
    }

    /// Italic override does NOT affect bold-only cells.
    #[test]
    fn select_font_family_italic_override_not_used_for_bold_only() {
        let (fam, bold_w, italic_s) =
            select_font_family("Main", None, Some("ItalicFont"), None, true, false);
        assert_eq!(fam, "Main");
        assert!(bold_w);
        assert!(!italic_s);
    }

    // ── Inactive-pane dim overlay unit tests ──────────────────────────────
    //
    // `render_panes` itself needs a live wgpu context, so we extract the
    // guard logic into a pure helper and test it here.

    /// Replicates the guard condition and quad geometry from `render_panes`
    /// for the inactive-pane dim overlay.  Returns one `(x, y, w, h)` tuple
    /// per *non-focused* pane when the overlay should be emitted, or an empty
    /// vec when the guard suppresses it.
    fn compute_pane_dim_quads(
        pane_rects: &[(f32, f32, f32, f32)],
        focused_idx: usize,
        alpha: f32,
    ) -> Vec<(f32, f32, f32, f32)> {
        if pane_rects.len() <= 1 || alpha <= 0.01 {
            return Vec::new();
        }
        pane_rects
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != focused_idx)
            .map(|(_, &(rx, ry, rw, rh))| (rx, ry, rw, rh))
            .collect()
    }

    /// With two panes and alpha 0.7, exactly one overlay quad is emitted for
    /// the non-focused pane and it covers that pane's rect exactly.
    #[test]
    fn pane_dim_emits_one_quad_for_non_focused_pane() {
        let left = (0.0_f32, 0.0, 500.0, 600.0);
        let right = (500.0_f32, 0.0, 500.0, 600.0);
        let quads = compute_pane_dim_quads(&[left, right], 0, 0.7);
        assert_eq!(
            quads.len(),
            1,
            "exactly one overlay quad for the non-focused pane"
        );
        assert_eq!(
            quads[0], right,
            "quad must cover the non-focused pane's rect exactly"
        );
    }

    /// With alpha == 0.0 no overlay quads are emitted (dim disabled).
    #[test]
    fn pane_dim_suppressed_when_alpha_zero() {
        let left = (0.0_f32, 0.0, 500.0, 600.0);
        let right = (500.0_f32, 0.0, 500.0, 600.0);
        let quads = compute_pane_dim_quads(&[left, right], 0, 0.0);
        assert!(quads.is_empty(), "no quads when alpha is 0.0 (dim off)");
    }

    /// With a single pane no overlay quad is emitted regardless of alpha.
    #[test]
    fn pane_dim_suppressed_for_single_pane() {
        let only = (0.0_f32, 0.0, 1000.0, 600.0);
        let quads = compute_pane_dim_quads(&[only], 0, 0.7);
        assert!(quads.is_empty(), "no quads when there is only one pane");
    }

    /// With three panes and focus on the middle one, two quads are emitted
    /// covering the left and right panes respectively.
    #[test]
    fn pane_dim_emits_two_quads_for_three_pane_split_focus_middle() {
        let left = (0.0_f32, 0.0, 333.0, 600.0);
        let middle = (333.0_f32, 0.0, 334.0, 600.0);
        let right = (667.0_f32, 0.0, 333.0, 600.0);
        let quads = compute_pane_dim_quads(&[left, middle, right], 1, 0.5);
        assert_eq!(
            quads.len(),
            2,
            "two overlay quads for two non-focused panes"
        );
        assert!(quads.contains(&left), "left pane must be dimmed");
        assert!(quads.contains(&right), "right pane must be dimmed");
        assert!(!quads.contains(&middle), "focused pane must NOT be dimmed");
    }

    // ── Vertical-remainder / grid-centering tests ────────────────────────────

    /// vertical_remainder must return the sub-cell leftover in [0, cell_h).
    /// With 600px height, 20px cell, 40px top, 4px bottom:
    ///   usable = 600 - 40 - 4 = 556; rows = floor(556/20) = 27; leftover = 556 - 540 = 16.
    #[test]
    fn vertical_remainder_is_sub_cell() {
        let rem = vertical_remainder(600.0, 20.0, 40.0, 4.0);
        assert!(
            (rem - 16.0).abs() < 0.01,
            "expected 16px leftover, got {rem}"
        );
        assert!(
            (0.0..20.0).contains(&rem),
            "remainder must be in [0, cell_h)"
        );
    }

    /// The remainder must always be strictly less than one cell height,
    /// regardless of input.
    #[test]
    fn vertical_remainder_always_less_than_cell_h() {
        // A surface that leaves exactly 0 remainder.
        let exact = vertical_remainder(580.0, 20.0, 40.0, 20.0); // usable=520, rows=26
        assert!(
            exact.abs() < 0.01,
            "exact fit must leave 0 remainder, got {exact}"
        );
        // Non-zero remainder must still be in [0, cell_h).
        let odd = vertical_remainder(607.0, 20.0, 40.0, 4.0); // usable=563, rows=28, leftover=3
        assert!(
            (0.0..20.0).contains(&odd),
            "remainder must be in [0, cell_h), got {odd}"
        );
    }

    /// REGRESSION (bottom-gap bug): the rows obtained by converting the
    /// chrome-free body rect with the no-offset math (`rect_to_cells`) must
    /// equal the rows `cells_for` computes from the full window. The old
    /// resize path fed the body rect back through the offset-subtracting
    /// conversion, double-counting the tab/status-bar chrome and losing
    /// several rows — a dead band at the bottom after every resize, which
    /// a freshly spawned tab (sized from the full window) did not have.
    #[test]
    fn body_rect_round_trip_matches_full_window_rows() {
        let cell_w = 8.4_f32;
        let cell_h = 16.8_f32;
        let padding = 8.0_f32;
        for &scale in &[1.0_f32, 1.25, 1.5, 2.0] {
            for &(top, bottom) in &[
                (44.0_f32, 8.0_f32), // tab bar top + padding
                (66.0, 8.0),         // tab bar + status bar top
                (44.0, 30.0),        // tab bar top + status bar bottom
                (66.0, 56.0),        // everything: both bars + resource strip
                (66.0, 86.0),        // + the AI suggestion bar band (30px)
            ] {
                for h_logical in [400.0_f32, 600.0, 768.0, 911.0, 1080.0] {
                    let h_px = (h_logical * scale).round();
                    // Family A — full window through cells_for (reference).
                    let (_, rows_full) = cells_for(
                        1024.0,
                        h_px / scale,
                        cell_w,
                        cell_h,
                        padding,
                        GridOffsets {
                            top,
                            bottom,
                            left: 0.0,
                            right: 0.0,
                        },
                    );
                    // Family B — the body rect exactly as resize_all_tabs
                    // builds it: body_top_px includes the half-remainder
                    // centering shift, body_bottom_px strips the bottom
                    // offset (all physical px).
                    let shift =
                        vertical_remainder(h_px, cell_h * scale, top * scale, bottom * scale) / 2.0;
                    let body_top_px = top * scale + shift;
                    let body_bottom_px = h_px - bottom * scale;
                    let body_h_px = (body_bottom_px - body_top_px).max(0.0);
                    // rect_to_cells row math (no offsets — pure replica).
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let rows_body = (((body_h_px / scale).max(cell_h)) / cell_h)
                        .floor()
                        .max(1.0) as u16;
                    assert_eq!(
                        rows_body, rows_full,
                        "rows mismatch: scale={scale} top={top} bottom={bottom} h={h_logical}"
                    );
                }
            }
        }
    }

    /// Shifting the grid origin down by remainder/2 distributes the sub-cell
    /// leftover symmetrically: the extra space above the first row (the shift
    /// itself) equals the extra space below the last row (leftover - shift).
    /// Both halves must differ by at most 1 px.
    #[test]
    fn grid_centering_splits_remainder_symmetrically() {
        // Physical-px values: surface=1200, cell_h=40, chrome_top=80, chrome_bottom=8.
        // usable = 1200 - 80 - 8 = 1112; rows = floor(1112/40) = 27; leftover = 32.
        // shift = 16; space_above_grid = 16; space_below_grid = 32 - 16 = 16.
        let surface_h: f32 = 1200.0;
        let cell_h_px: f32 = 40.0;
        let top_px: f32 = 80.0;
        let bottom_px: f32 = 8.0;
        let leftover = vertical_remainder(surface_h, cell_h_px, top_px, bottom_px);
        let shift = leftover * 0.5;
        let rows = ((surface_h - top_px - bottom_px).max(cell_h_px) / cell_h_px)
            .floor()
            .max(1.0);

        // "Space above grid" = the extra gap inserted between chrome_top and
        // the first row = shift.  "Space below grid" = leftover - shift.
        let space_above = shift;
        let space_below = leftover - shift;
        // Double-check via geometry: total - chrome - grid = leftover, all at bottom before fix.
        let geometric_leftover = surface_h - top_px - rows * cell_h_px - bottom_px;
        assert!(
            (geometric_leftover - leftover).abs() < 0.01,
            "geometric leftover {geometric_leftover} must equal vertical_remainder {leftover}"
        );
        // After centering the two halves must differ by at most 1 px.
        let diff = (space_above - space_below).abs();
        assert!(
            diff <= 1.0,
            "space above ({space_above}) and below ({space_below}) grid differ by {diff} > 1 px"
        );
    }

    /// After centering, the total bottom gap (bottom_offset + remainder/2) must
    /// be less than one full cell height — no wasted row.
    #[test]
    fn bottom_gap_at_most_one_cell() {
        let surface_h: f32 = 1000.0;
        let cell_h: f32 = 20.0;
        let top: f32 = 44.0;
        let bottom: f32 = 8.0;
        let rows = ((surface_h - top - bottom).max(cell_h) / cell_h)
            .floor()
            .max(1.0);
        let bottom_gap = surface_h - top - rows * cell_h - bottom;
        assert!(
            (0.0..cell_h).contains(&bottom_gap),
            "bottom gap {bottom_gap} must be in [0, cell_h={cell_h})"
        );
    }

    /// A top tab bar contributes nothing to bottom_offset_logical, so the
    /// bottom gap equals padding alone.
    #[test]
    fn hidden_bars_contribute_zero_to_offsets() {
        // Simulate: no status bar, top tab bar (contributes to top, not bottom).
        // Use cells_for directly to verify row counts.
        // With no bottom bar: bottom_offset == padding.
        let padding = 8.0;
        let top_with_tab = TAB_BAR_HEIGHT + padding; // tab at top contributes to top only
        let bottom_no_bar = padding; // nothing at bottom

        let (_, rows_top_tab) = cells_for(
            800.0,
            600.0,
            8.0,
            16.0,
            padding,
            GridOffsets {
                top: top_with_tab,
                bottom: bottom_no_bar,
                left: 0.0,
                right: 0.0,
            },
        );
        // Same computation with a bottom bar: adds STATUS_BAR_HEIGHT to bottom_offset.
        let bottom_with_bar = padding + STATUS_BAR_HEIGHT;
        let (_, rows_bottom_bar) = cells_for(
            800.0,
            600.0,
            8.0,
            16.0,
            padding,
            GridOffsets {
                top: top_with_tab,
                bottom: bottom_with_bar,
                left: 0.0,
                right: 0.0,
            },
        );
        assert!(
            rows_top_tab > rows_bottom_bar,
            "adding a bottom status bar must reduce row count"
        );
        // Also verify the bottom remainder with no bottom bar is < cell_h.
        let usable = (600.0_f32 - top_with_tab - bottom_no_bar).max(16.0);
        let rem = usable % 16.0;
        assert!(
            (0.0..16.0).contains(&rem),
            "bottom remainder must be in [0, cell_h), got {rem}"
        );
    }

    // ── Group pill geometry tests ─────────────────────────────────────────────

    /// Bar: [ungrouped, group-run-start(label), group-run-cont, ungrouped]
    /// Expect exactly one pill between tabs[0] and tabs[1].
    #[test]
    fn group_pill_geometry_left_of_run() {
        // items: (has_label, char_count)
        let items = [
            (false, 0), // ungrouped
            (true, 5),  // "Build" — run start, has label
            (false, 0), // continuation of run (same group, no label)
            (false, 0), // ungrouped
        ];
        let tab_w = 120.0;
        let font_size = 14.0;
        let (tab_xs, pills) = compute_pill_geometry_pure(&items, tab_w, font_size, 8.0);

        assert_eq!(pills.len(), 1, "exactly one pill for one group run");
        assert_eq!(pills[0].1, 1, "pill must reference first_idx=1");

        // Pill must sit between tab[0]'s right edge and tab[1]'s left edge.
        let tab0_right = tab_xs[0] + tab_w;
        let tab1_left = tab_xs[1];
        let pill_x = pills[0].0.x;
        let pill_right = pill_x + pills[0].0.w;

        assert!(
            pill_x >= tab0_right,
            "pill must start after tab[0] right edge (pill_x={pill_x}, tab0_right={tab0_right})"
        );
        assert!(
            pill_right <= tab1_left,
            "pill must end before tab[1] (pill_right={pill_right}, tab1_left={tab1_left})"
        );
    }

    /// A click inside the pill rect returns GroupLabel(first_idx).
    #[test]
    fn group_pill_hit_returns_first_idx() {
        let items = [
            (false, 0),
            (true, 5), // run start at first_idx=1
            (false, 0),
        ];
        let tab_w = 120.0;
        let font_size = 14.0;
        let (_tab_xs, pills) = compute_pill_geometry_pure(&items, tab_w, font_size, 8.0);

        assert_eq!(pills.len(), 1);
        let (pill_rect, first_idx) = pills[0];
        assert_eq!(first_idx, 1);

        // A point at the centre of the pill must be inside.
        let cx = pill_rect.x + pill_rect.w * 0.5;
        let cy = pill_rect.y + pill_rect.h * 0.5;
        assert!(
            pill_rect.contains(cx, cy),
            "centre of pill must be inside the rect"
        );
    }

    /// Two separate group runs produce two pills.
    #[test]
    fn two_runs_two_pills() {
        let items = [
            (false, 0), // ungrouped
            (true, 5),  // run A start ("Build")
            (false, 0), // run A cont
            (true, 6),  // run B start ("Deploy")
            (false, 0), // run B cont
        ];
        let tab_w = 100.0;
        let font_size = 14.0;
        let (_tab_xs, pills) = compute_pill_geometry_pure(&items, tab_w, font_size, 8.0);
        assert_eq!(pills.len(), 2, "two group runs must produce two pills");
        assert_eq!(pills[0].1, 1, "first pill for run-start at idx 1");
        assert_eq!(pills[1].1, 3, "second pill for run-start at idx 3");
    }

    // ── contrast_text tests ───────────────────────────────────────────────────

    #[test]
    fn contrast_text_light_vs_dark() {
        // Very bright yellow → near-black text.
        let light = contrast_text([0xff, 0xff, 0x00]);
        // contrast_text returns white for luma <= 140.
        // Luma([255,255,0]) = 0.299*255 + 0.587*255 + 0.114*0 ≈ 226 → near-black.
        assert_eq!(
            light,
            GlyphonColor::rgb(0x1a, 0x1a, 0x1a),
            "bright colour must use near-black text"
        );

        // Dark navy → white text.
        let dark = contrast_text([0x07, 0x09, 0x0e]);
        // Luma ≈ 8 → white.
        assert_eq!(
            dark,
            GlyphonColor::rgb(0xff, 0xff, 0xff),
            "dark colour must use white text"
        );
    }

    // ── vertical drag slot computation (pure, no GPU) ─────────────────────────

    /// `slot_from_midpoints` with y-range pairs: drop above first midpoint → 0,
    /// drop below last midpoint → len, midpoints respected.
    #[test]
    fn slot_from_midpoints_y_axis() {
        // Simulate three tabs at rows: y0=36, y1=72, y2=108 (each h=36).
        // Midpoints: 54, 90, 126.
        let rows: Vec<(f32, f32)> = vec![(36.0, 36.0), (72.0, 36.0), (108.0, 36.0)];
        // Above first midpoint (y < 54) → slot 0.
        assert_eq!(slot_from_midpoints(&rows, 10.0), 0);
        assert_eq!(slot_from_midpoints(&rows, 53.9), 0);
        // Past first midpoint but before second (54 <= y < 90) → slot 1.
        assert_eq!(slot_from_midpoints(&rows, 54.1), 1);
        assert_eq!(slot_from_midpoints(&rows, 89.9), 1);
        // Past second midpoint (90 <= y < 126) → slot 2.
        assert_eq!(slot_from_midpoints(&rows, 90.0), 2);
        // Past last midpoint → slot 3 (append).
        assert_eq!(slot_from_midpoints(&rows, 126.1), 3);
        assert_eq!(slot_from_midpoints(&rows, 9999.0), 3);
    }

    #[test]
    fn slot_from_midpoints_single_tab() {
        // One tab at y=36, h=36; midpoint=54.
        let rows = vec![(36.0, 36.0_f32)];
        assert_eq!(slot_from_midpoints(&rows, 0.0), 0, "above midpoint → 0");
        assert_eq!(
            slot_from_midpoints(&rows, 54.0),
            1,
            "at/past midpoint → 1 (append)"
        );
        assert_eq!(slot_from_midpoints(&rows, 100.0), 1, "well past → 1");
    }

    #[test]
    fn slot_from_midpoints_empty() {
        // No tabs → always slot 0.
        let rows: Vec<(f32, f32)> = vec![];
        assert_eq!(slot_from_midpoints(&rows, 0.0), 0);
        assert_eq!(slot_from_midpoints(&rows, 500.0), 0);
    }

    /// `live_reorder` logic (shared reorder helper): moving tab i to slot j
    /// must correctly reposition the element and return the landed index.
    /// We test `slot_from_midpoints` directly as it is the pure kernel.
    #[test]
    fn live_reorder_slot_accounting() {
        // 4 tabs at y-midpoints 18, 54, 90, 126 (row_h=36, area_top=36 means
        // rows at 36,72,108,144 but offset not critical — we only test the
        // slot→dest mapping here). Use x-axis version for simplicity since
        // the arithmetic is identical in both axes.
        let tabs: Vec<(f32, f32)> = vec![(0.0, 36.0), (36.0, 36.0), (72.0, 36.0), (108.0, 36.0)];
        // Drop cursor just past midpoint of tab[2] (midpoint = 90) → slot 3.
        let slot = slot_from_midpoints(&tabs, 91.0);
        assert_eq!(slot, 3);
        // When dragging from index 2 to slot 3 the dest is: slot>from → dest = slot-1 = 2.
        // (Mirrors the `live_reorder` adjustment: `dest = if dest > from { dest - 1 } else { dest }`.)
        let from = 2_usize;
        let dest = if slot > from { slot - 1 } else { slot };
        assert_eq!(dest, from, "reorder to adjacent slot is a no-op");

        // Drop at slot 0 when dragging from index 2 → dest stays 0.
        let slot0 = slot_from_midpoints(&tabs, 5.0);
        assert_eq!(slot0, 0);
        let dest0 = if slot0 > from { slot0 - 1 } else { slot0 };
        assert_eq!(dest0, 0, "tab from[2] dragged to slot 0 lands at index 0");
    }
}
