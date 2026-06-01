//! Procedural geometry for box-drawing (U+2500–U+257F) and block-element
//! (U+2580–U+259F) Unicode characters.
//!
//! Instead of relying on font glyphs — which may be shifted, scaled, or
//! antialiased differently across fonts — we render these characters as crisp
//! axis-aligned filled rectangles derived purely from cell metrics. Adjacent
//! cells that share a box-drawing character therefore join seamlessly, with no
//! gaps or seams, regardless of the installed font.
//!
//! # Coordinate system
//!
//! All rectangle fields (`x`, `y`, `w`, `h`) are **normalised** to the cell,
//! in the range `0.0..=1.0` where `(0, 0)` is the top-left of the cell and
//! `(1, 1)` is the bottom-right. The caller multiplies by the physical-pixel
//! cell size to produce final quad coordinates.
//!
//! # Coverage
//!
//! This module maps the most common codepoints precisely. Codepoints in the
//! U+2500–U+259F range that are *not* listed in [`box_rects`] return `None`,
//! causing the renderer to fall back to the font glyph for that character.
//!
//! # Powerline / triangle separators
//!
//! Characters in the private-use "Powerline" range (U+E0B0–U+E0B7 and
//! related) require diagonal / triangular geometry that cannot be represented
//! as axis-aligned quads. They are explicitly **out of scope** for this module
//! and should be rendered via the normal font-glyph path.

/// A single filled rectangle, normalised to the cell `(0.0..=1.0)` on both
/// axes. `(0,0)` is top-left, `(1,1)` is bottom-right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellRect {
    /// Left edge, normalised.
    pub x: f32,
    /// Top edge, normalised.
    pub y: f32,
    /// Width, normalised.
    pub w: f32,
    /// Height, normalised.
    pub h: f32,
    /// Extra alpha multiplier applied on top of the cell foreground colour.
    /// `1.0` = fully opaque; `0.25 / 0.5 / 0.75` for the shading characters
    /// ░ ▒ ▓ (U+2591–U+2593).
    pub alpha: f32,
}

impl CellRect {
    /// Shorthand for a fully-opaque rectangle.
    #[inline]
    const fn opaque(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h, alpha: 1.0 }
    }
    /// Shorthand for a partially-transparent rectangle.
    #[inline]
    const fn tinted(x: f32, y: f32, w: f32, h: f32, alpha: f32) -> Self {
        Self { x, y, w, h, alpha }
    }
}

// ── Line-thickness helpers (compile-time constants) ──────────────────────────

/// Light stroke thickness as a fraction of the cell height.
const LIGHT: f32 = 0.125;
/// Heavy stroke thickness (approx. ×2 of light).
const HEAVY: f32 = 0.250;
/// Each bar of a double-line character.
const THIN_DOUBLE: f32 = 0.0625;
/// Inner gap between the two bars of a double-line character.
const DOUBLE_GAP: f32 = 0.0625;

// ── Centre offsets ────────────────────────────────────────────────────────────

const H_LO: f32 = 0.5 - LIGHT * 0.5;
const H_HI: f32 = 0.5 + LIGHT * 0.5;
const HH_LO: f32 = 0.5 - HEAVY * 0.5;
const HH_HI: f32 = 0.5 + HEAVY * 0.5;
const MID: f32 = 0.5;

const D_LO1: f32 = MID - DOUBLE_GAP * 0.5 - THIN_DOUBLE;
const D_LO2: f32 = MID + DOUBLE_GAP * 0.5;

// A convenience macro for building the small Vec returns without noise.
// These are function-local macros so they don't pollute the crate namespace.
macro_rules! v1 {
    ($r0:expr) => { Some(vec![$r0]) }
}
macro_rules! v2 {
    ($r0:expr, $r1:expr) => { Some(vec![$r0, $r1]) }
}
macro_rules! v3 {
    ($r0:expr, $r1:expr, $r2:expr) => { Some(vec![$r0, $r1, $r2]) }
}
macro_rules! v4 {
    ($r0:expr, $r1:expr, $r2:expr, $r3:expr) => { Some(vec![$r0, $r1, $r2, $r3]) }
}

use CellRect as R;

/// Returns the list of axis-aligned rectangles that represent `ch` inside its
/// cell, or `None` when `ch` is not in the mapped subset of U+2500–U+259F (in
/// which case the caller should fall back to the normal font-glyph path).
///
/// The rectangles are in *normalised* cell coordinates: `(0,0)` is the
/// top-left of the cell, `(1,1)` the bottom-right. Multiply by the physical
/// cell size (width for `x`/`w`, height for `y`/`h`) to obtain pixel-space
/// coordinates.
///
/// Returns an owned `Vec` to avoid lifetime constraints on inline arrays.
/// Each Vec holds at most 4 elements so the allocation is trivially small.
///
/// # Hot-path note
/// Call [`is_in_range`] first to do a cheap range check before calling here.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn box_rects(ch: char) -> Option<Vec<CellRect>> {
    match ch {
        // ── Box-drawing: U+2500–U+257F ────────────────────────────────────────

        // ─ U+2500  ━ U+2501
        '\u{2500}' => v1!(R::opaque(0.0, H_LO, 1.0, LIGHT)),
        '\u{2501}' => v1!(R::opaque(0.0, HH_LO, 1.0, HEAVY)),
        // │ U+2502  ┃ U+2503
        '\u{2502}' => v1!(R::opaque(H_LO, 0.0, LIGHT, 1.0)),
        '\u{2503}' => v1!(R::opaque(HH_LO, 0.0, HEAVY, 1.0)),

        // ┄ U+2504 LIGHT TRIPLE DASH HORIZONTAL (3 segments)
        '\u{2504}' => v3!(
            R::opaque(0.0,  H_LO, 0.28, LIGHT),
            R::opaque(0.36, H_LO, 0.28, LIGHT),
            R::opaque(0.72, H_LO, 0.28, LIGHT)
        ),
        // ┅ U+2505 HEAVY TRIPLE DASH HORIZONTAL
        '\u{2505}' => v3!(
            R::opaque(0.0,  HH_LO, 0.28, HEAVY),
            R::opaque(0.36, HH_LO, 0.28, HEAVY),
            R::opaque(0.72, HH_LO, 0.28, HEAVY)
        ),
        // ┆ U+2506 LIGHT TRIPLE DASH VERTICAL
        '\u{2506}' => v3!(
            R::opaque(H_LO, 0.0,  LIGHT, 0.28),
            R::opaque(H_LO, 0.36, LIGHT, 0.28),
            R::opaque(H_LO, 0.72, LIGHT, 0.28)
        ),
        // ┇ U+2507 HEAVY TRIPLE DASH VERTICAL
        '\u{2507}' => v3!(
            R::opaque(HH_LO, 0.0,  HEAVY, 0.28),
            R::opaque(HH_LO, 0.36, HEAVY, 0.28),
            R::opaque(HH_LO, 0.72, HEAVY, 0.28)
        ),

        // ┈ U+2508 LIGHT QUADRUPLE DASH HORIZONTAL (4 segments)
        '\u{2508}' => v4!(
            R::opaque(0.0,  H_LO, 0.18, LIGHT),
            R::opaque(0.27, H_LO, 0.18, LIGHT),
            R::opaque(0.54, H_LO, 0.18, LIGHT),
            R::opaque(0.81, H_LO, 0.18, LIGHT)
        ),
        // ┉ U+2509 HEAVY QUADRUPLE DASH HORIZONTAL
        '\u{2509}' => v4!(
            R::opaque(0.0,  HH_LO, 0.18, HEAVY),
            R::opaque(0.27, HH_LO, 0.18, HEAVY),
            R::opaque(0.54, HH_LO, 0.18, HEAVY),
            R::opaque(0.81, HH_LO, 0.18, HEAVY)
        ),
        // ┊ U+250A LIGHT QUADRUPLE DASH VERTICAL
        '\u{250A}' => v4!(
            R::opaque(H_LO, 0.0,  LIGHT, 0.18),
            R::opaque(H_LO, 0.27, LIGHT, 0.18),
            R::opaque(H_LO, 0.54, LIGHT, 0.18),
            R::opaque(H_LO, 0.81, LIGHT, 0.18)
        ),
        // ┋ U+250B HEAVY QUADRUPLE DASH VERTICAL
        '\u{250B}' => v4!(
            R::opaque(HH_LO, 0.0,  HEAVY, 0.18),
            R::opaque(HH_LO, 0.27, HEAVY, 0.18),
            R::opaque(HH_LO, 0.54, HEAVY, 0.18),
            R::opaque(HH_LO, 0.81, HEAVY, 0.18)
        ),

        // ┌ U+250C LIGHT DOWN AND RIGHT (top-left corner)
        '\u{250C}' => v2!(
            R::opaque(H_LO, H_LO, 1.0 - H_LO, LIGHT),
            R::opaque(H_LO, H_LO, LIGHT, 1.0 - H_LO)
        ),
        '\u{250D}' => v2!(
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, LIGHT),
            R::opaque(HH_LO, H_LO, HEAVY, 1.0 - H_LO)
        ),
        '\u{250E}' => v2!(
            R::opaque(H_LO, HH_LO, 1.0 - H_LO, HEAVY),
            R::opaque(H_LO, HH_LO, LIGHT, 1.0 - HH_LO)
        ),
        // ┏ U+250F HEAVY DOWN AND RIGHT
        '\u{250F}' => v2!(
            R::opaque(HH_LO, HH_LO, 1.0 - HH_LO, HEAVY),
            R::opaque(HH_LO, HH_LO, HEAVY, 1.0 - HH_LO)
        ),

        // ┐ U+2510 LIGHT DOWN AND LEFT (top-right corner)
        '\u{2510}' => v2!(
            R::opaque(0.0, H_LO, H_HI, LIGHT),
            R::opaque(H_LO, H_LO, LIGHT, 1.0 - H_LO)
        ),
        '\u{2511}' => v2!(
            R::opaque(0.0, H_LO, HH_HI, LIGHT),
            R::opaque(HH_LO, H_LO, HEAVY, 1.0 - H_LO)
        ),
        '\u{2512}' => v2!(
            R::opaque(0.0, HH_LO, H_HI, HEAVY),
            R::opaque(H_LO, HH_LO, LIGHT, 1.0 - HH_LO)
        ),
        // ┓ U+2513 HEAVY DOWN AND LEFT
        '\u{2513}' => v2!(
            R::opaque(0.0, HH_LO, HH_HI, HEAVY),
            R::opaque(HH_LO, HH_LO, HEAVY, 1.0 - HH_LO)
        ),

        // └ U+2514 LIGHT UP AND RIGHT (bottom-left corner)
        '\u{2514}' => v2!(
            R::opaque(H_LO, H_LO, 1.0 - H_LO, LIGHT),
            R::opaque(H_LO, 0.0, LIGHT, H_HI)
        ),
        '\u{2515}' => v2!(
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, LIGHT),
            R::opaque(HH_LO, 0.0, HEAVY, H_HI)
        ),
        '\u{2516}' => v2!(
            R::opaque(H_LO, HH_LO, 1.0 - H_LO, HEAVY),
            R::opaque(H_LO, 0.0, LIGHT, HH_HI)
        ),
        // ┗ U+2517 HEAVY UP AND RIGHT
        '\u{2517}' => v2!(
            R::opaque(HH_LO, HH_LO, 1.0 - HH_LO, HEAVY),
            R::opaque(HH_LO, 0.0, HEAVY, HH_HI)
        ),

        // ┘ U+2518 LIGHT UP AND LEFT (bottom-right corner)
        '\u{2518}' => v2!(
            R::opaque(0.0, H_LO, H_HI, LIGHT),
            R::opaque(H_LO, 0.0, LIGHT, H_HI)
        ),
        '\u{2519}' => v2!(
            R::opaque(0.0, H_LO, HH_HI, LIGHT),
            R::opaque(HH_LO, 0.0, HEAVY, H_HI)
        ),
        '\u{251A}' => v2!(
            R::opaque(0.0, HH_LO, H_HI, HEAVY),
            R::opaque(H_LO, 0.0, LIGHT, HH_HI)
        ),
        // ┛ U+251B HEAVY UP AND LEFT
        '\u{251B}' => v2!(
            R::opaque(0.0, HH_LO, HH_HI, HEAVY),
            R::opaque(HH_LO, 0.0, HEAVY, HH_HI)
        ),

        // ├ U+251C LIGHT VERTICAL AND RIGHT (left tee)
        '\u{251C}' => v2!(
            R::opaque(H_LO, 0.0, LIGHT, 1.0),
            R::opaque(H_LO, H_LO, 1.0 - H_LO, LIGHT)
        ),
        '\u{251D}' => v2!(
            R::opaque(H_LO, 0.0, LIGHT, 1.0),
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, LIGHT)
        ),
        '\u{251E}' => v3!(
            R::opaque(HH_LO, 0.0, HEAVY, H_HI),
            R::opaque(H_LO, H_LO, LIGHT, 1.0 - H_LO),
            R::opaque(H_LO, H_LO, 1.0 - H_LO, LIGHT)
        ),
        '\u{251F}' => v3!(
            R::opaque(H_LO, 0.0, LIGHT, H_HI),
            R::opaque(HH_LO, H_LO, HEAVY, 1.0 - H_LO),
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, LIGHT)
        ),
        '\u{2520}' => v2!(
            R::opaque(HH_LO, 0.0, HEAVY, 1.0),
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, LIGHT)
        ),
        '\u{2521}' => v3!(
            R::opaque(HH_LO, 0.0, HEAVY, H_HI),
            R::opaque(H_LO, H_LO, LIGHT, 1.0 - H_LO),
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, HEAVY)
        ),
        '\u{2522}' => v3!(
            R::opaque(H_LO, 0.0, LIGHT, H_HI),
            R::opaque(HH_LO, H_LO, HEAVY, 1.0 - H_LO),
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, HEAVY)
        ),
        // ┣ U+2523 HEAVY VERTICAL AND RIGHT
        '\u{2523}' => v2!(
            R::opaque(HH_LO, 0.0, HEAVY, 1.0),
            R::opaque(HH_LO, HH_LO, 1.0 - HH_LO, HEAVY)
        ),

        // ┤ U+2524 LIGHT VERTICAL AND LEFT (right tee)
        '\u{2524}' => v2!(
            R::opaque(H_LO, 0.0, LIGHT, 1.0),
            R::opaque(0.0, H_LO, H_HI, LIGHT)
        ),
        '\u{2525}' => v2!(
            R::opaque(H_LO, 0.0, LIGHT, 1.0),
            R::opaque(0.0, H_LO, HH_HI, LIGHT)
        ),
        '\u{2526}' => v3!(
            R::opaque(HH_LO, 0.0, HEAVY, H_HI),
            R::opaque(H_LO, H_LO, LIGHT, 1.0 - H_LO),
            R::opaque(0.0, H_LO, H_HI, LIGHT)
        ),
        '\u{2527}' => v3!(
            R::opaque(H_LO, 0.0, LIGHT, H_HI),
            R::opaque(HH_LO, H_LO, HEAVY, 1.0 - H_LO),
            R::opaque(0.0, H_LO, HH_HI, LIGHT)
        ),
        '\u{2528}' => v2!(
            R::opaque(HH_LO, 0.0, HEAVY, 1.0),
            R::opaque(0.0, H_LO, HH_LO, LIGHT)
        ),
        '\u{2529}' => v3!(
            R::opaque(HH_LO, 0.0, HEAVY, H_HI),
            R::opaque(H_LO, H_LO, LIGHT, 1.0 - H_LO),
            R::opaque(0.0, HH_LO, HH_LO, HEAVY)
        ),
        '\u{252A}' => v3!(
            R::opaque(H_LO, 0.0, LIGHT, H_HI),
            R::opaque(HH_LO, H_LO, HEAVY, 1.0 - H_LO),
            R::opaque(0.0, HH_LO, HH_LO, HEAVY)
        ),
        // ┫ U+252B HEAVY VERTICAL AND LEFT
        '\u{252B}' => v2!(
            R::opaque(HH_LO, 0.0, HEAVY, 1.0),
            R::opaque(0.0, HH_LO, HH_LO, HEAVY)
        ),

        // ┬ U+252C LIGHT DOWN AND HORIZONTAL (top tee)
        '\u{252C}' => v2!(
            R::opaque(0.0, H_LO, 1.0, LIGHT),
            R::opaque(H_LO, H_LO, LIGHT, 1.0 - H_LO)
        ),
        '\u{252D}' => v3!(
            R::opaque(0.0, H_LO, HH_HI, LIGHT),
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, HEAVY),
            R::opaque(HH_LO, H_LO, HEAVY, 1.0 - H_LO)
        ),
        '\u{252E}' => v3!(
            R::opaque(0.0, H_LO, H_HI, HEAVY),
            R::opaque(H_LO, H_LO, 1.0 - H_LO, LIGHT),
            R::opaque(H_LO, H_LO, LIGHT, 1.0 - H_LO)
        ),
        '\u{252F}' => v2!(
            R::opaque(0.0, H_LO, 1.0, LIGHT),
            R::opaque(HH_LO, H_LO, HEAVY, 1.0 - H_LO)
        ),
        '\u{2530}' => v2!(
            R::opaque(0.0, HH_LO, 1.0, HEAVY),
            R::opaque(H_LO, HH_LO, LIGHT, 1.0 - HH_LO)
        ),
        '\u{2531}' => v3!(
            R::opaque(0.0, H_LO, HH_HI, HEAVY),
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, LIGHT),
            R::opaque(HH_LO, H_LO, HEAVY, 1.0 - H_LO)
        ),
        '\u{2532}' => v3!(
            R::opaque(0.0, HH_LO, H_HI, LIGHT),
            R::opaque(H_LO, H_LO, 1.0 - H_LO, HEAVY),
            R::opaque(H_LO, H_LO, LIGHT, 1.0 - H_LO)
        ),
        // ┳ U+2533 HEAVY DOWN AND HORIZONTAL
        '\u{2533}' => v2!(
            R::opaque(0.0, HH_LO, 1.0, HEAVY),
            R::opaque(HH_LO, HH_LO, HEAVY, 1.0 - HH_LO)
        ),

        // ┴ U+2534 LIGHT UP AND HORIZONTAL (bottom tee)
        '\u{2534}' => v2!(
            R::opaque(0.0, H_LO, 1.0, LIGHT),
            R::opaque(H_LO, 0.0, LIGHT, H_HI)
        ),
        '\u{2535}' => v3!(
            R::opaque(0.0, H_LO, HH_HI, LIGHT),
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, HEAVY),
            R::opaque(HH_LO, 0.0, HEAVY, H_HI)
        ),
        '\u{2536}' => v3!(
            R::opaque(0.0, H_LO, H_HI, HEAVY),
            R::opaque(H_LO, H_LO, 1.0 - H_LO, LIGHT),
            R::opaque(H_LO, 0.0, LIGHT, H_HI)
        ),
        '\u{2537}' => v2!(
            R::opaque(0.0, H_LO, 1.0, LIGHT),
            R::opaque(HH_LO, 0.0, HEAVY, H_HI)
        ),
        '\u{2538}' => v2!(
            R::opaque(0.0, HH_LO, 1.0, HEAVY),
            R::opaque(H_LO, 0.0, LIGHT, HH_HI)
        ),
        '\u{2539}' => v3!(
            R::opaque(0.0, H_LO, HH_HI, HEAVY),
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, LIGHT),
            R::opaque(HH_LO, 0.0, HEAVY, H_HI)
        ),
        '\u{253A}' => v3!(
            R::opaque(0.0, HH_LO, H_HI, LIGHT),
            R::opaque(H_LO, H_LO, 1.0 - H_LO, HEAVY),
            R::opaque(H_LO, 0.0, LIGHT, H_HI)
        ),
        // ┻ U+253B HEAVY UP AND HORIZONTAL
        '\u{253B}' => v2!(
            R::opaque(0.0, HH_LO, 1.0, HEAVY),
            R::opaque(HH_LO, 0.0, HEAVY, HH_HI)
        ),

        // ┼ U+253C LIGHT VERTICAL AND HORIZONTAL (cross)
        '\u{253C}' => v2!(
            R::opaque(0.0, H_LO, 1.0, LIGHT),
            R::opaque(H_LO, 0.0, LIGHT, 1.0)
        ),
        '\u{253D}' => v3!(
            R::opaque(0.0, H_LO, HH_HI, LIGHT),
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, HEAVY),
            R::opaque(H_LO, 0.0, LIGHT, 1.0)
        ),
        '\u{253E}' => v3!(
            R::opaque(0.0, H_LO, H_HI, HEAVY),
            R::opaque(H_LO, H_LO, 1.0 - H_LO, LIGHT),
            R::opaque(H_LO, 0.0, LIGHT, 1.0)
        ),
        '\u{253F}' => v2!(
            R::opaque(0.0, H_LO, 1.0, LIGHT),
            R::opaque(HH_LO, 0.0, HEAVY, 1.0)
        ),
        '\u{2540}' => v3!(
            R::opaque(0.0, HH_LO, 1.0, HEAVY),
            R::opaque(H_LO, 0.0, LIGHT, HH_HI),
            R::opaque(H_LO, HH_HI, LIGHT, 1.0 - HH_HI)
        ),
        '\u{2541}' => v3!(
            R::opaque(0.0, H_LO, 1.0, LIGHT),
            R::opaque(H_LO, 0.0, LIGHT, H_LO),
            R::opaque(HH_LO, H_HI, HEAVY, 1.0 - H_HI)
        ),
        '\u{2542}' => v2!(
            R::opaque(0.0, HH_LO, 1.0, HEAVY),
            R::opaque(H_LO, 0.0, LIGHT, 1.0)
        ),
        '\u{2543}' => v4!(
            R::opaque(0.0, H_LO, HH_HI, HEAVY),
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, LIGHT),
            R::opaque(HH_LO, 0.0, HEAVY, H_LO),
            R::opaque(H_LO, H_HI, LIGHT, 1.0 - H_HI)
        ),
        '\u{2544}' => v4!(
            R::opaque(0.0, H_LO, H_HI, LIGHT),
            R::opaque(H_HI, H_LO, 1.0 - H_HI, HEAVY),
            R::opaque(H_LO, 0.0, LIGHT, H_LO),
            R::opaque(HH_LO, H_HI, HEAVY, 1.0 - H_HI)
        ),
        '\u{2545}' => v4!(
            R::opaque(0.0, HH_LO, HH_HI, LIGHT),
            R::opaque(HH_LO, H_LO, 1.0 - HH_LO, HEAVY),
            R::opaque(H_LO, 0.0, LIGHT, HH_LO),
            R::opaque(HH_LO, HH_HI, HEAVY, 1.0 - HH_HI)
        ),
        '\u{2546}' => v4!(
            R::opaque(0.0, H_LO, HH_HI, HEAVY),
            R::opaque(HH_HI, HH_LO, 1.0 - HH_HI, LIGHT),
            R::opaque(HH_LO, 0.0, HEAVY, HH_LO),
            R::opaque(H_LO, HH_HI, LIGHT, 1.0 - HH_HI)
        ),
        '\u{2547}' => v3!(
            R::opaque(0.0, H_LO, 1.0, LIGHT),
            R::opaque(HH_LO, 0.0, HEAVY, H_LO),
            R::opaque(HH_LO, H_HI, HEAVY, 1.0 - H_HI)
        ),
        '\u{2548}' => v3!(
            R::opaque(0.0, HH_LO, 1.0, HEAVY),
            R::opaque(H_LO, 0.0, LIGHT, HH_LO),
            R::opaque(H_LO, HH_HI, LIGHT, 1.0 - HH_HI)
        ),
        '\u{2549}' => v3!(
            R::opaque(0.0, H_LO, HH_HI, HEAVY),
            R::opaque(HH_HI, HH_LO, 1.0 - HH_HI, LIGHT),
            R::opaque(HH_LO, 0.0, HEAVY, 1.0)
        ),
        '\u{254A}' => v3!(
            R::opaque(0.0, HH_LO, H_HI, LIGHT),
            R::opaque(H_HI, H_LO, 1.0 - H_HI, HEAVY),
            R::opaque(H_LO, 0.0, LIGHT, 1.0)
        ),
        // ╋ U+254B HEAVY VERTICAL AND HORIZONTAL
        '\u{254B}' => v2!(
            R::opaque(0.0, HH_LO, 1.0, HEAVY),
            R::opaque(HH_LO, 0.0, HEAVY, 1.0)
        ),

        // ╌ U+254C LIGHT DOUBLE DASH HORIZONTAL
        '\u{254C}' => v2!(
            R::opaque(0.0,  H_LO, 0.42, LIGHT),
            R::opaque(0.58, H_LO, 0.42, LIGHT)
        ),
        // ╍ U+254D HEAVY DOUBLE DASH HORIZONTAL
        '\u{254D}' => v2!(
            R::opaque(0.0,  HH_LO, 0.42, HEAVY),
            R::opaque(0.58, HH_LO, 0.42, HEAVY)
        ),
        // ╎ U+254E LIGHT DOUBLE DASH VERTICAL
        '\u{254E}' => v2!(
            R::opaque(H_LO,  0.0,  LIGHT, 0.42),
            R::opaque(H_LO,  0.58, LIGHT, 0.42)
        ),
        // ╏ U+254F HEAVY DOUBLE DASH VERTICAL
        '\u{254F}' => v2!(
            R::opaque(HH_LO, 0.0,  HEAVY, 0.42),
            R::opaque(HH_LO, 0.58, HEAVY, 0.42)
        ),

        // ═ U+2550 DOUBLE HORIZONTAL (two parallel thin bars)
        '\u{2550}' => v2!(
            R::opaque(0.0, D_LO1, 1.0, THIN_DOUBLE),
            R::opaque(0.0, D_LO2, 1.0, THIN_DOUBLE)
        ),
        // ║ U+2551 DOUBLE VERTICAL
        '\u{2551}' => v2!(
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, 1.0),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, 1.0)
        ),

        // ╒–╝ double-line corners / tees (U+2552–U+255D) — approximate
        // with combinations of single-bar geometry at double positions.
        '\u{2552}' => v4!(
            R::opaque(D_LO1, D_LO1, 1.0 - D_LO1, THIN_DOUBLE),
            R::opaque(D_LO2, D_LO2, 1.0 - D_LO2, THIN_DOUBLE),
            R::opaque(D_LO1, D_LO1, THIN_DOUBLE, 1.0 - D_LO1),
            R::opaque(D_LO2, D_LO2, THIN_DOUBLE, 1.0 - D_LO2)
        ),
        '\u{2553}' => v3!(
            R::opaque(D_LO1, D_LO1, 1.0 - D_LO1, THIN_DOUBLE),
            R::opaque(D_LO1, D_LO1, THIN_DOUBLE, 1.0 - D_LO1),
            R::opaque(D_LO2, D_LO2, THIN_DOUBLE, 1.0 - D_LO2)
        ),
        '\u{2554}' => v4!(
            R::opaque(D_LO1, D_LO1, 1.0 - D_LO1, THIN_DOUBLE),
            R::opaque(D_LO2, D_LO2, 1.0 - D_LO2, THIN_DOUBLE),
            R::opaque(D_LO1, D_LO1, THIN_DOUBLE, 1.0 - D_LO1),
            R::opaque(D_LO2, D_LO2, THIN_DOUBLE, 1.0 - D_LO2)
        ),
        '\u{2555}' => v4!(
            R::opaque(0.0, D_LO1, 1.0 - D_LO1, THIN_DOUBLE),
            R::opaque(0.0, D_LO2, 1.0 - D_LO2, THIN_DOUBLE),
            R::opaque(D_LO1, D_LO1, THIN_DOUBLE, 1.0 - D_LO1),
            R::opaque(D_LO2, D_LO2, THIN_DOUBLE, 1.0 - D_LO2)
        ),
        '\u{2556}' => v3!(
            R::opaque(0.0, D_LO1, D_LO2 + THIN_DOUBLE, THIN_DOUBLE),
            R::opaque(D_LO1, D_LO1, THIN_DOUBLE, 1.0 - D_LO1),
            R::opaque(D_LO2, D_LO2, THIN_DOUBLE, 1.0 - D_LO2)
        ),
        '\u{2557}' => v4!(
            R::opaque(0.0, D_LO1, 1.0 - D_LO1, THIN_DOUBLE),
            R::opaque(0.0, D_LO2, 1.0 - D_LO2, THIN_DOUBLE),
            R::opaque(D_LO1, D_LO1, THIN_DOUBLE, 1.0 - D_LO1),
            R::opaque(D_LO2, D_LO2, THIN_DOUBLE, 1.0 - D_LO2)
        ),
        '\u{2558}' => v4!(
            R::opaque(D_LO1, D_LO1, 1.0 - D_LO1, THIN_DOUBLE),
            R::opaque(D_LO2, D_LO2, 1.0 - D_LO2, THIN_DOUBLE),
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, D_LO2 + THIN_DOUBLE),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, D_LO2 + THIN_DOUBLE)
        ),
        '\u{2559}' => v3!(
            R::opaque(D_LO1, D_LO1, 1.0 - D_LO1, THIN_DOUBLE),
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, D_LO1 + THIN_DOUBLE),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, D_LO2 + THIN_DOUBLE)
        ),
        '\u{255A}' => v4!(
            R::opaque(D_LO1, D_LO1, 1.0 - D_LO1, THIN_DOUBLE),
            R::opaque(D_LO2, D_LO2, 1.0 - D_LO2, THIN_DOUBLE),
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, D_LO1 + THIN_DOUBLE),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, D_LO2 + THIN_DOUBLE)
        ),
        '\u{255B}' => v4!(
            R::opaque(0.0, D_LO1, D_LO2 + THIN_DOUBLE, THIN_DOUBLE),
            R::opaque(0.0, D_LO2, D_LO1 + THIN_DOUBLE, THIN_DOUBLE),
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, D_LO2 + THIN_DOUBLE),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, D_LO1 + THIN_DOUBLE)
        ),
        '\u{255C}' => v3!(
            R::opaque(0.0, D_LO1, D_LO1 + THIN_DOUBLE, THIN_DOUBLE),
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, D_LO1 + THIN_DOUBLE),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, D_LO2 + THIN_DOUBLE)
        ),
        '\u{255D}' => v4!(
            R::opaque(0.0, D_LO1, D_LO1 + THIN_DOUBLE, THIN_DOUBLE),
            R::opaque(0.0, D_LO2, D_LO2 + THIN_DOUBLE, THIN_DOUBLE),
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, D_LO1 + THIN_DOUBLE),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, D_LO2 + THIN_DOUBLE)
        ),

        // ╞–╠ vertical+horizontal mixed (U+255E–U+2560)
        '\u{255E}' => v3!(
            R::opaque(H_LO, 0.0, LIGHT, 1.0),
            R::opaque(H_LO, D_LO1, 1.0 - H_LO, THIN_DOUBLE),
            R::opaque(H_LO, D_LO2, 1.0 - H_LO, THIN_DOUBLE)
        ),
        '\u{255F}' => v3!(
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, 1.0),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, 1.0),
            R::opaque(D_LO1, H_LO, 1.0 - D_LO1, LIGHT)
        ),
        '\u{2560}' => v4!(
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, 1.0),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, 1.0),
            R::opaque(D_LO1, D_LO1, 1.0 - D_LO1, THIN_DOUBLE),
            R::opaque(D_LO2, D_LO2, 1.0 - D_LO2, THIN_DOUBLE)
        ),

        // ╡–╣
        '\u{2561}' => v3!(
            R::opaque(H_LO, 0.0, LIGHT, 1.0),
            R::opaque(0.0, D_LO1, H_HI, THIN_DOUBLE),
            R::opaque(0.0, D_LO2, H_HI, THIN_DOUBLE)
        ),
        '\u{2562}' => v3!(
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, 1.0),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, 1.0),
            R::opaque(0.0, H_LO, D_LO2, LIGHT)
        ),
        '\u{2563}' => v4!(
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, 1.0),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, 1.0),
            R::opaque(0.0, D_LO1, D_LO2, THIN_DOUBLE),
            R::opaque(0.0, D_LO2, D_LO1, THIN_DOUBLE)
        ),

        // ╤–╦
        '\u{2564}' => v3!(
            R::opaque(0.0, D_LO1, 1.0, THIN_DOUBLE),
            R::opaque(0.0, D_LO2, 1.0, THIN_DOUBLE),
            R::opaque(H_LO, D_LO2, LIGHT, 1.0 - D_LO2)
        ),
        '\u{2565}' => v3!(
            R::opaque(0.0, H_LO, 1.0, LIGHT),
            R::opaque(D_LO1, H_LO, THIN_DOUBLE, 1.0 - H_LO),
            R::opaque(D_LO2, H_LO, THIN_DOUBLE, 1.0 - H_LO)
        ),
        '\u{2566}' => v4!(
            R::opaque(0.0, D_LO1, 1.0, THIN_DOUBLE),
            R::opaque(0.0, D_LO2, 1.0, THIN_DOUBLE),
            R::opaque(D_LO1, D_LO2, THIN_DOUBLE, 1.0 - D_LO2),
            R::opaque(D_LO2, D_LO2, THIN_DOUBLE, 1.0 - D_LO2)
        ),

        // ╧–╩
        '\u{2567}' => v3!(
            R::opaque(0.0, D_LO1, 1.0, THIN_DOUBLE),
            R::opaque(0.0, D_LO2, 1.0, THIN_DOUBLE),
            R::opaque(H_LO, 0.0, LIGHT, D_LO1 + THIN_DOUBLE)
        ),
        '\u{2568}' => v3!(
            R::opaque(0.0, H_LO, 1.0, LIGHT),
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, H_HI),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, H_HI)
        ),
        '\u{2569}' => v4!(
            R::opaque(0.0, D_LO1, 1.0, THIN_DOUBLE),
            R::opaque(0.0, D_LO2, 1.0, THIN_DOUBLE),
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, D_LO1 + THIN_DOUBLE),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, D_LO2 + THIN_DOUBLE)
        ),

        // ╪–╬
        '\u{256A}' => v3!(
            R::opaque(0.0, D_LO1, 1.0, THIN_DOUBLE),
            R::opaque(0.0, D_LO2, 1.0, THIN_DOUBLE),
            R::opaque(H_LO, 0.0, LIGHT, 1.0)
        ),
        '\u{256B}' => v3!(
            R::opaque(0.0, H_LO, 1.0, LIGHT),
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, 1.0),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, 1.0)
        ),
        // ╬ U+256C DOUBLE VERTICAL AND HORIZONTAL (double cross)
        '\u{256C}' => v4!(
            R::opaque(0.0, D_LO1, 1.0, THIN_DOUBLE),
            R::opaque(0.0, D_LO2, 1.0, THIN_DOUBLE),
            R::opaque(D_LO1, 0.0, THIN_DOUBLE, 1.0),
            R::opaque(D_LO2, 0.0, THIN_DOUBLE, 1.0)
        ),

        // ╭╮╰╯ U+256D–U+2570 arc connectors — approximate with plain light corners
        '\u{256D}' => v2!(
            R::opaque(H_LO, H_LO, 1.0 - H_LO, LIGHT),
            R::opaque(H_LO, H_LO, LIGHT, 1.0 - H_LO)
        ),
        '\u{256E}' => v2!(
            R::opaque(0.0, H_LO, H_HI, LIGHT),
            R::opaque(H_LO, H_LO, LIGHT, 1.0 - H_LO)
        ),
        '\u{256F}' => v2!(
            R::opaque(0.0, H_LO, H_HI, LIGHT),
            R::opaque(H_LO, 0.0, LIGHT, H_HI)
        ),
        '\u{2570}' => v2!(
            R::opaque(H_LO, H_LO, 1.0 - H_LO, LIGHT),
            R::opaque(H_LO, 0.0, LIGHT, H_HI)
        ),

        // ╱╲╳ U+2571–U+2573 diagonal connectors — no axis-aligned representation
        '\u{2571}' | '\u{2572}' | '\u{2573}' => None,

        // Half connectors U+2574–U+257B
        '\u{2574}' => v1!(R::opaque(0.0,   H_LO,  MID,  LIGHT)),
        '\u{2575}' => v1!(R::opaque(H_LO,  0.0,   LIGHT, MID)),
        '\u{2576}' => v1!(R::opaque(MID,   H_LO,  MID,  LIGHT)),
        '\u{2577}' => v1!(R::opaque(H_LO,  MID,   LIGHT, MID)),
        '\u{2578}' => v1!(R::opaque(0.0,   HH_LO, MID,  HEAVY)),
        '\u{2579}' => v1!(R::opaque(HH_LO, 0.0,   HEAVY, MID)),
        '\u{257A}' => v1!(R::opaque(MID,   HH_LO, MID,  HEAVY)),
        '\u{257B}' => v1!(R::opaque(HH_LO, MID,   HEAVY, MID)),

        // Light-heavy hybrid halves U+257C–U+257F
        '\u{257C}' => v2!(
            R::opaque(0.0, H_LO,  MID, LIGHT),
            R::opaque(MID, HH_LO, MID, HEAVY)
        ),
        '\u{257D}' => v2!(
            R::opaque(H_LO,  0.0, LIGHT, MID),
            R::opaque(HH_LO, MID, HEAVY, MID)
        ),
        '\u{257E}' => v2!(
            R::opaque(0.0, HH_LO, MID, HEAVY),
            R::opaque(MID, H_LO,  MID, LIGHT)
        ),
        '\u{257F}' => v2!(
            R::opaque(HH_LO, 0.0, HEAVY, MID),
            R::opaque(H_LO,  MID, LIGHT, MID)
        ),

        // ── Block elements: U+2580–U+259F ─────────────────────────────────────

        // ▀ U+2580 UPPER HALF BLOCK
        '\u{2580}' => v1!(R::opaque(0.0, 0.0, 1.0, 0.5)),
        // ▁ U+2581 LOWER ONE EIGHTH BLOCK
        '\u{2581}' => v1!(R::opaque(0.0, 0.875, 1.0, 0.125)),
        // ▂ U+2582 LOWER ONE QUARTER BLOCK
        '\u{2582}' => v1!(R::opaque(0.0, 0.75, 1.0, 0.25)),
        // ▃ U+2583 LOWER THREE EIGHTHS BLOCK
        '\u{2583}' => v1!(R::opaque(0.0, 0.625, 1.0, 0.375)),
        // ▄ U+2584 LOWER HALF BLOCK
        '\u{2584}' => v1!(R::opaque(0.0, 0.5, 1.0, 0.5)),
        // ▅ U+2585 LOWER FIVE EIGHTHS BLOCK
        '\u{2585}' => v1!(R::opaque(0.0, 0.375, 1.0, 0.625)),
        // ▆ U+2586 LOWER THREE QUARTERS BLOCK
        '\u{2586}' => v1!(R::opaque(0.0, 0.25, 1.0, 0.75)),
        // ▇ U+2587 LOWER SEVEN EIGHTHS BLOCK
        '\u{2587}' => v1!(R::opaque(0.0, 0.125, 1.0, 0.875)),
        // █ U+2588 FULL BLOCK
        '\u{2588}' => v1!(R::opaque(0.0, 0.0, 1.0, 1.0)),

        // ▉ U+2589 LEFT SEVEN EIGHTHS BLOCK
        '\u{2589}' => v1!(R::opaque(0.0, 0.0, 0.875, 1.0)),
        // ▊ U+258A LEFT THREE QUARTERS BLOCK
        '\u{258A}' => v1!(R::opaque(0.0, 0.0, 0.75, 1.0)),
        // ▋ U+258B LEFT FIVE EIGHTHS BLOCK
        '\u{258B}' => v1!(R::opaque(0.0, 0.0, 0.625, 1.0)),
        // ▌ U+258C LEFT HALF BLOCK
        '\u{258C}' => v1!(R::opaque(0.0, 0.0, 0.5, 1.0)),
        // ▍ U+258D LEFT THREE EIGHTHS BLOCK
        '\u{258D}' => v1!(R::opaque(0.0, 0.0, 0.375, 1.0)),
        // ▎ U+258E LEFT ONE QUARTER BLOCK
        '\u{258E}' => v1!(R::opaque(0.0, 0.0, 0.25, 1.0)),
        // ▏ U+258F LEFT ONE EIGHTH BLOCK
        '\u{258F}' => v1!(R::opaque(0.0, 0.0, 0.125, 1.0)),
        // ▐ U+2590 RIGHT HALF BLOCK
        '\u{2590}' => v1!(R::opaque(0.5, 0.0, 0.5, 1.0)),

        // ░ U+2591 LIGHT SHADE — full-cell fg at α 0.25
        '\u{2591}' => v1!(R::tinted(0.0, 0.0, 1.0, 1.0, 0.25)),
        // ▒ U+2592 MEDIUM SHADE — full-cell fg at α 0.50
        '\u{2592}' => v1!(R::tinted(0.0, 0.0, 1.0, 1.0, 0.50)),
        // ▓ U+2593 DARK SHADE — full-cell fg at α 0.75
        '\u{2593}' => v1!(R::tinted(0.0, 0.0, 1.0, 1.0, 0.75)),

        // ▔ U+2594 UPPER ONE EIGHTH BLOCK
        '\u{2594}' => v1!(R::opaque(0.0, 0.0, 1.0, 0.125)),
        // ▕ U+2595 RIGHT ONE EIGHTH BLOCK
        '\u{2595}' => v1!(R::opaque(0.875, 0.0, 0.125, 1.0)),

        // ▖ U+2596 QUADRANT LOWER LEFT
        '\u{2596}' => v1!(R::opaque(0.0, 0.5, 0.5, 0.5)),
        // ▗ U+2597 QUADRANT LOWER RIGHT
        '\u{2597}' => v1!(R::opaque(0.5, 0.5, 0.5, 0.5)),
        // ▘ U+2598 QUADRANT UPPER LEFT
        '\u{2598}' => v1!(R::opaque(0.0, 0.0, 0.5, 0.5)),
        // ▙ U+2599 QUADRANT UPPER LEFT AND LOWER LEFT AND LOWER RIGHT
        '\u{2599}' => v2!(
            R::opaque(0.0, 0.0, 0.5, 0.5),
            R::opaque(0.0, 0.5, 1.0, 0.5)
        ),
        // ▚ U+259A QUADRANT UPPER LEFT AND LOWER RIGHT
        '\u{259A}' => v2!(
            R::opaque(0.0, 0.0, 0.5, 0.5),
            R::opaque(0.5, 0.5, 0.5, 0.5)
        ),
        // ▛ U+259B QUADRANT UPPER LEFT AND UPPER RIGHT AND LOWER LEFT
        '\u{259B}' => v2!(
            R::opaque(0.0, 0.0, 1.0, 0.5),
            R::opaque(0.0, 0.5, 0.5, 0.5)
        ),
        // ▜ U+259C QUADRANT UPPER LEFT AND UPPER RIGHT AND LOWER RIGHT
        '\u{259C}' => v2!(
            R::opaque(0.0, 0.0, 1.0, 0.5),
            R::opaque(0.5, 0.5, 0.5, 0.5)
        ),
        // ▝ U+259D QUADRANT UPPER RIGHT
        '\u{259D}' => v1!(R::opaque(0.5, 0.0, 0.5, 0.5)),
        // ▞ U+259E QUADRANT UPPER RIGHT AND LOWER LEFT
        '\u{259E}' => v2!(
            R::opaque(0.5, 0.0, 0.5, 0.5),
            R::opaque(0.0, 0.5, 0.5, 0.5)
        ),
        // ▟ U+259F QUADRANT UPPER RIGHT AND LOWER LEFT AND LOWER RIGHT
        '\u{259F}' => v2!(
            R::opaque(0.5, 0.0, 0.5, 0.5),
            R::opaque(0.0, 0.5, 1.0, 0.5)
        ),

        // Any other character → font fallback
        _ => None,
    }
}

/// Returns `true` when `ch` is in the U+2500–U+259F range covered by this
/// module. Use as a cheap hot-path pre-check before calling [`box_rects`].
#[inline]
#[must_use]
pub fn is_in_range(ch: char) -> bool {
    ('\u{2500}'..='\u{259F}').contains(&ch)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    fn rect_eq(r: &CellRect, x: f32, y: f32, w: f32, h: f32, alpha: f32) -> bool {
        approx_eq(r.x, x)
            && approx_eq(r.y, y)
            && approx_eq(r.w, w)
            && approx_eq(r.h, h)
            && approx_eq(r.alpha, alpha)
    }

    // ── U+2500 ─ LIGHT HORIZONTAL ───────────────────────────────────────
    #[test]
    fn light_horizontal_one_rect() {
        let rects = box_rects('\u{2500}').expect("─ must be mapped");
        assert_eq!(rects.len(), 1, "─ must emit exactly one rect");
        let r = &rects[0];
        assert!(approx_eq(r.x, 0.0), "x must be 0");
        assert!(approx_eq(r.w, 1.0), "w must be 1");
        assert!(approx_eq(r.y, H_LO), "y must be H_LO");
        assert!(approx_eq(r.h, LIGHT), "h must be LIGHT");
        assert!(approx_eq(r.alpha, 1.0), "alpha must be 1.0");
    }

    // ── U+2502 │ LIGHT VERTICAL ──────────────────────────────────────────
    #[test]
    fn light_vertical_one_rect() {
        let rects = box_rects('\u{2502}').expect("│ must be mapped");
        assert_eq!(rects.len(), 1, "│ must emit exactly one rect");
        let r = &rects[0];
        assert!(approx_eq(r.y, 0.0), "y must be 0");
        assert!(approx_eq(r.h, 1.0), "h must be 1");
        assert!(approx_eq(r.x, H_LO), "x must be H_LO");
        assert!(approx_eq(r.w, LIGHT), "w must be LIGHT");
    }

    // ── U+250C ┌ LIGHT DOWN AND RIGHT (top-left corner) ────────────────
    #[test]
    fn corner_top_left_two_rects() {
        let rects = box_rects('\u{250C}').expect("┌ must be mapped");
        assert_eq!(rects.len(), 2, "┌ must emit two rects (h-arm + v-arm)");
        let h = &rects[0];
        assert!(approx_eq(h.x, H_LO), "h-arm x must start at H_LO");
        assert!(approx_eq(h.w, 1.0 - H_LO), "h-arm spans to right edge");
        assert!(approx_eq(h.y, H_LO));
        assert!(approx_eq(h.h, LIGHT));
        let v = &rects[1];
        assert!(approx_eq(v.x, H_LO));
        assert!(approx_eq(v.w, LIGHT));
        assert!(approx_eq(v.y, H_LO), "v-arm y must start at H_LO");
        assert!(approx_eq(v.h, 1.0 - H_LO));
    }

    // ── U+2588 █ FULL BLOCK ──────────────────────────────────────────────
    #[test]
    fn full_block_one_rect() {
        let rects = box_rects('\u{2588}').expect("█ must be mapped");
        assert_eq!(rects.len(), 1);
        assert!(rect_eq(&rects[0], 0.0, 0.0, 1.0, 1.0, 1.0), "█ must be a full-cell opaque rect");
    }

    // ── U+2580 ▀ UPPER HALF BLOCK ────────────────────────────────────────
    #[test]
    fn upper_half_block() {
        let rects = box_rects('\u{2580}').expect("▀ must be mapped");
        assert_eq!(rects.len(), 1);
        assert!(rect_eq(&rects[0], 0.0, 0.0, 1.0, 0.5, 1.0), "▀ must cover the top half");
    }

    // ── U+2584 ▄ LOWER HALF BLOCK ────────────────────────────────────────
    #[test]
    fn lower_half_block() {
        let rects = box_rects('\u{2584}').expect("▄ must be mapped");
        assert_eq!(rects.len(), 1);
        assert!(rect_eq(&rects[0], 0.0, 0.5, 1.0, 0.5, 1.0), "▄ must cover the bottom half");
    }

    // ── U+2591 ░ LIGHT SHADE ─────────────────────────────────────────────
    #[test]
    fn light_shade_alpha_quarter() {
        let rects = box_rects('\u{2591}').expect("░ must be mapped");
        assert_eq!(rects.len(), 1);
        assert!(rect_eq(&rects[0], 0.0, 0.0, 1.0, 1.0, 0.25), "░ must be full-cell at alpha 0.25");
    }

    // ── U+2592 ▒ MEDIUM SHADE ────────────────────────────────────────────
    #[test]
    fn medium_shade_alpha_half() {
        let rects = box_rects('\u{2592}').expect("▒ must be mapped");
        assert_eq!(rects.len(), 1);
        assert!(approx_eq(rects[0].alpha, 0.50), "▒ must be at alpha 0.50");
        assert!(approx_eq(rects[0].w, 1.0));
        assert!(approx_eq(rects[0].h, 1.0));
    }

    // ── U+2593 ▓ DARK SHADE ──────────────────────────────────────────────
    #[test]
    fn dark_shade_alpha_three_quarters() {
        let rects = box_rects('\u{2593}').expect("▓ must be mapped");
        assert_eq!(rects.len(), 1);
        assert!(approx_eq(rects[0].alpha, 0.75), "▓ must be at alpha 0.75");
    }

    // ── U+2596 ▖ QUADRANT LOWER LEFT ────────────────────────────────────
    #[test]
    fn quadrant_lower_left() {
        let rects = box_rects('\u{2596}').expect("▖ must be mapped");
        assert_eq!(rects.len(), 1);
        assert!(rect_eq(&rects[0], 0.0, 0.5, 0.5, 0.5, 1.0), "▖ must be lower-left quarter");
    }

    // ── U+2597 ▗ QUADRANT LOWER RIGHT ───────────────────────────────────
    #[test]
    fn quadrant_lower_right() {
        let rects = box_rects('\u{2597}').expect("▗ must be mapped");
        assert_eq!(rects.len(), 1);
        assert!(rect_eq(&rects[0], 0.5, 0.5, 0.5, 0.5, 1.0));
    }

    // ── U+2598 ▘ QUADRANT UPPER LEFT ────────────────────────────────────
    #[test]
    fn quadrant_upper_left() {
        let rects = box_rects('\u{2598}').expect("▘ must be mapped");
        assert_eq!(rects.len(), 1);
        assert!(rect_eq(&rects[0], 0.0, 0.0, 0.5, 0.5, 1.0));
    }

    // ── U+259D ▝ QUADRANT UPPER RIGHT ───────────────────────────────────
    #[test]
    fn quadrant_upper_right() {
        let rects = box_rects('\u{259D}').expect("▝ must be mapped");
        assert_eq!(rects.len(), 1);
        assert!(rect_eq(&rects[0], 0.5, 0.0, 0.5, 0.5, 1.0));
    }

    // ── U+259A ▚ QUADRANT UPPER LEFT AND LOWER RIGHT ────────────────────
    #[test]
    fn quadrant_upper_left_lower_right() {
        let rects = box_rects('\u{259A}').expect("▚ must be mapped");
        assert_eq!(rects.len(), 2);
        assert!(rect_eq(&rects[0], 0.0, 0.0, 0.5, 0.5, 1.0));
        assert!(rect_eq(&rects[1], 0.5, 0.5, 0.5, 0.5, 1.0));
    }

    // ── U+259E ▞ QUADRANT UPPER RIGHT AND LOWER LEFT ────────────────────
    #[test]
    fn quadrant_upper_right_lower_left() {
        let rects = box_rects('\u{259E}').expect("▞ must be mapped");
        assert_eq!(rects.len(), 2);
        assert!(rect_eq(&rects[0], 0.5, 0.0, 0.5, 0.5, 1.0));
        assert!(rect_eq(&rects[1], 0.0, 0.5, 0.5, 0.5, 1.0));
    }

    // ── Diagonal connectors return None ─────────────────────────────────
    #[test]
    fn diagonal_connectors_are_unmapped() {
        assert!(box_rects('\u{2571}').is_none(), "╱ must be unmapped");
        assert!(box_rects('\u{2572}').is_none(), "╲ must be unmapped");
        assert!(box_rects('\u{2573}').is_none(), "╳ must be unmapped");
    }

    // ── ASCII and emoji outside the range always return None ─────────────
    #[test]
    fn ascii_outside_range_returns_none() {
        assert!(box_rects('A').is_none());
        assert!(box_rects(' ').is_none());
        assert!(box_rects('\u{1F600}').is_none());
    }

    // ── is_in_range ──────────────────────────────────────────────────────
    #[test]
    fn is_in_range_boundaries() {
        assert!(is_in_range('\u{2500}'));
        assert!(is_in_range('\u{259F}'));
        assert!(!is_in_range('\u{24FF}'));
        assert!(!is_in_range('\u{25A0}'));
        assert!(!is_in_range('A'));
    }

    // ── ▁–▇ eighth-blocks: coverage increases monotonically ─────────────
    #[test]
    fn eighth_blocks_monotonic_coverage() {
        let mut prev_h = 0.0_f32;
        for cp in 0x2581_u32..=0x2587 {
            let ch = char::from_u32(cp).unwrap();
            let rects = box_rects(ch).expect("eighth block must be mapped");
            assert_eq!(rects.len(), 1);
            assert!(
                rects[0].h > prev_h,
                "h must increase for cp {cp:#06X}, prev_h={prev_h}, got {}",
                rects[0].h
            );
            prev_h = rects[0].h;
        }
    }

    // ── Double horizontal has two parallel full-width bars ───────────────
    #[test]
    fn double_horizontal_two_parallel_bars() {
        let rects = box_rects('\u{2550}').expect("═ must be mapped");
        assert_eq!(rects.len(), 2, "═ must have two parallel bars");
        assert!(approx_eq(rects[0].x, 0.0));
        assert!(approx_eq(rects[0].w, 1.0));
        assert!(approx_eq(rects[1].x, 0.0));
        assert!(approx_eq(rects[1].w, 1.0));
        assert!(rects[1].y > rects[0].y, "second bar must be lower");
        assert!(approx_eq(rects[0].h, rects[1].h));
    }

    // ── Cross (┼) has two full-span bars ────────────────────────────────
    #[test]
    fn cross_has_two_full_span_bars() {
        let rects = box_rects('\u{253C}').expect("┼ must be mapped");
        assert_eq!(rects.len(), 2);
        let h = rects.iter().find(|r| approx_eq(r.w, 1.0)).expect("cross must have a full-width rect");
        assert!(approx_eq(h.h, LIGHT));
        let v = rects.iter().find(|r| approx_eq(r.h, 1.0)).expect("cross must have a full-height rect");
        assert!(approx_eq(v.w, LIGHT));
    }

    // ── ▌ LEFT HALF BLOCK ────────────────────────────────────────────────
    #[test]
    fn left_half_block() {
        let rects = box_rects('\u{258C}').expect("▌ must be mapped");
        assert_eq!(rects.len(), 1);
        assert!(rect_eq(&rects[0], 0.0, 0.0, 0.5, 1.0, 1.0));
    }

    // ── ▐ RIGHT HALF BLOCK ───────────────────────────────────────────────
    #[test]
    fn right_half_block() {
        let rects = box_rects('\u{2590}').expect("▐ must be mapped");
        assert_eq!(rects.len(), 1);
        assert!(rect_eq(&rects[0], 0.5, 0.0, 0.5, 1.0, 1.0));
    }
}
