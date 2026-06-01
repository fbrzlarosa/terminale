//! Sixel graphics protocol decoder.
//!
//! Decodes a DCS Sixel payload (the bytes between the `q` introducer and the
//! ST string terminator) into a flat RGBA8 pixel buffer.
//!
//! # Sixel data format recap
//!
//! A sixel sequence looks like:
//! ```text
//! ESC P <params> q <sixel-data> ST
//! ```
//! where `<params>` is optional (aspect-ratio / background flags, largely
//! ignored here) and `<sixel-data>` contains:
//!
//! - `# <n> ; <type> ; <a> ; <b> ; <c>` — define color register `<n>`.
//!   `type == 2` ⟹ RGB (0–100 percent); `type == 1` ⟹ HLS.
//! - `# <n>` — select color register `<n>` (already defined).
//! - `! <count> <sixel-char>` — repeat the sixel char `<count>` times.
//! - `$ ` — carriage-return: move to column 0, stay in the same 6-px band.
//! - `-` — newline: advance to the next 6-px band (column 0).
//! - `0x3F–0x7E` — sixel character: the 6 LSBs encode which rows (0=top)
//!   within the current 6-px band are lit.
//!
//! Colors that have not been assigned via `#` use the default 16-color VGA
//! palette for registers 0–15, and grey ramp for higher indices.

use std::collections::HashMap;

// ── Public surface ────────────────────────────────────────────────────────────

/// Result of a successful sixel decode.
#[derive(Debug)]
pub struct SixelImage {
    /// Raw RGBA8 pixels, row-major, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Decode a raw sixel payload (everything after the `q` introducer and before
/// ST) into RGBA8. Returns `None` on empty / undecodable input.
///
/// # Memory note
///
/// The decoder grows the canvas lazily. The returned pixel buffer is trimmed
/// to the actual rendered extent (trailing blank bands are removed).
#[must_use]
pub fn decode(payload: &[u8]) -> Option<SixelImage> {
    let mut dec = Decoder::new();
    dec.feed(payload);
    dec.finish()
}

// ── Internal implementation ───────────────────────────────────────────────────

/// Maximum canvas area in pixels, as a safety cap against a malformed stream
/// that would try to allocate an enormous buffer. ~64 MiB of RGBA.
const MAX_PIXELS: usize = 16_384 * 16_384;

/// Default VGA-style 16-color palette for registers 0–15. Higher indices
/// default to a grey shade: `(index - 16) * 255 / 239` clamped.
#[rustfmt::skip]
const DEFAULT_PALETTE: [[u8; 3]; 16] = [
    [0,   0,   0  ], // 0  Black
    [0,   0,   170], // 1  Blue
    [0,   170, 0  ], // 2  Green
    [0,   170, 170], // 3  Cyan
    [170, 0,   0  ], // 4  Red
    [170, 0,   170], // 5  Magenta
    [170, 170, 0  ], // 6  Brown/Dark Yellow
    [170, 170, 170], // 7  Light Gray
    [85,  85,  85 ], // 8  Dark Gray
    [85,  85,  255], // 9  Bright Blue
    [85,  255, 85 ], // 10 Bright Green
    [85,  255, 255], // 11 Bright Cyan
    [255, 85,  85 ], // 12 Bright Red
    [255, 85,  255], // 13 Bright Magenta
    [255, 255, 85 ], // 14 Bright Yellow
    [255, 255, 255], // 15 White
];

/// Resolve a color register to its RGB triple.
fn palette_color(reg: usize, custom: &HashMap<usize, [u8; 3]>) -> [u8; 3] {
    if let Some(&c) = custom.get(&reg) {
        return c;
    }
    if reg < 16 {
        return DEFAULT_PALETTE[reg];
    }
    // Grey ramp for high registers: just use a proportional grey.
    let v = ((reg as u32).saturating_sub(16).min(239) * 255 / 239) as u8;
    [v, v, v]
}

/// Convert HLS (hue 0–360, lightness 0–100, saturation 0–100) to RGB 0–255.
///
/// Reference: sixel standard type-1 color spec.
fn hls_to_rgb(h: u32, l: u32, s: u32) -> [u8; 3] {
    // Normalize l, s to [0.0, 1.0].
    let l = (l.min(100) as f32) / 100.0;
    let s = (s.min(100) as f32) / 100.0;
    let h = (h % 360) as f32;

    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return [v, v, v];
    }

    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;

    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    [
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
    ]
}

/// Convert RGB (0–100 percent each) to 0–255 linear.
fn rgb_pct_to_u8(v: u32) -> u8 {
    ((v.min(100) * 255) / 100) as u8
}

/// Sixel canvas: grows right and down on demand.
struct Canvas {
    pixels: Vec<u8>, // RGBA row-major, width * height * 4
    width: usize,
    height: usize,
}

impl Canvas {
    fn new() -> Self {
        Self {
            pixels: Vec::new(),
            width: 0,
            height: 0,
        }
    }

    /// Ensure the canvas is at least `need_w` × `need_h` pixels,
    /// expanding (zero-padded) if necessary. Returns `false` if the
    /// required size would exceed `MAX_PIXELS`.
    fn ensure(&mut self, need_w: usize, need_h: usize) -> bool {
        if need_w * need_h > MAX_PIXELS {
            return false;
        }
        let new_w = self.width.max(need_w);
        let new_h = self.height.max(need_h);
        if new_w == self.width && new_h == self.height {
            return true;
        }
        // Reallocate: copy old rows, fill new space with transparent black.
        let mut new_pix = vec![0u8; new_w * new_h * 4];
        for row in 0..self.height {
            let src = row * self.width * 4;
            let dst = row * new_w * 4;
            let row_bytes = self.width * 4;
            new_pix[dst..dst + row_bytes].copy_from_slice(&self.pixels[src..src + row_bytes]);
        }
        self.pixels = new_pix;
        self.width = new_w;
        self.height = new_h;
        true
    }

    /// Paint `color` RGBA into pixel `(col, row)`. Silently ignores
    /// coordinates that would exceed `MAX_PIXELS` after growth.
    fn set(&mut self, col: usize, row: usize, rgba: [u8; 4]) {
        if !self.ensure(col + 1, row + 1) {
            return;
        }
        let off = (row * self.width + col) * 4;
        self.pixels[off..off + 4].copy_from_slice(&rgba);
    }

    /// Take the canvas data, trimming trailing all-zero (transparent) rows.
    fn into_image(mut self) -> Option<SixelImage> {
        if self.width == 0 || self.height == 0 {
            return None;
        }
        // Trim trailing rows that are all transparent.
        let row_bytes = self.width * 4;
        let mut last_filled_row = 0usize;
        let mut found_any = false;
        for r in 0..self.height {
            let start = r * row_bytes;
            let row_slice = &self.pixels[start..start + row_bytes];
            // A row is "non-empty" when any alpha byte is non-zero.
            if row_slice.chunks(4).any(|p| p[3] != 0) {
                last_filled_row = r;
                found_any = true;
            }
        }
        if !found_any {
            return None;
        }
        let trimmed_h = last_filled_row + 1;
        self.pixels.truncate(trimmed_h * row_bytes);
        Some(SixelImage {
            width: self.width as u32,
            height: trimmed_h as u32,
            rgba: self.pixels,
        })
    }
}

/// DFA-based decoder that can consume payload bytes in multiple `feed` calls.
struct Decoder {
    canvas: Canvas,
    /// Color registers: index → RGB triple.
    palette: HashMap<usize, [u8; 3]>,
    /// Currently selected color register.
    current_color: usize,
    /// Current pixel column (x) within the canvas.
    col: usize,
    /// Current 6-pixel band (y = band * 6).
    band: usize,
    /// Decoder state machine.
    state: State,
    /// Accumulator for numeric parameters.
    num_buf: Vec<u32>,
    /// Single accumulated number (shared digit buffer).
    cur_num: u32,
    cur_num_valid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Consuming data characters and commands.
    Normal,
    /// Inside `#` color command — reading numeric params separated by `;`.
    Color,
    /// Inside `!<count><char>` repeat command — reading the count.
    Repeat,
}

impl Decoder {
    fn new() -> Self {
        Self {
            canvas: Canvas::new(),
            palette: HashMap::new(),
            current_color: 0,
            col: 0,
            band: 0,
            state: State::Normal,
            num_buf: Vec::with_capacity(5),
            cur_num: 0,
            cur_num_valid: false,
        }
    }

    fn feed(&mut self, data: &[u8]) {
        for &b in data {
            self.process_byte(b);
        }
    }

    fn process_byte(&mut self, b: u8) {
        match self.state {
            State::Normal => self.normal(b),
            State::Color => self.color_byte(b),
            State::Repeat => self.repeat_byte(b),
        }
    }

    /// Paint the 6 vertical pixels encoded in `sixel_byte` at the current
    /// column and band, using the currently selected color.
    fn paint_sixel(&mut self, sixel_byte: u8) {
        let bits = sixel_byte - 0x3f; // 6-bit value: bit 0 = top pixel
        let rgb = palette_color(self.current_color, &self.palette);
        let col = self.col;
        let base_row = self.band * 6;
        for bit in 0..6u8 {
            if bits & (1 << bit) != 0 {
                let row = base_row + bit as usize;
                self.canvas.set(col, row, [rgb[0], rgb[1], rgb[2], 0xFF]);
            }
        }
        self.col += 1;
    }

    fn normal(&mut self, b: u8) {
        match b {
            // Sixel data character: 0x3F ('?') through 0x7E ('~').
            0x3f..=0x7e => {
                self.paint_sixel(b);
            }
            // Carriage return: back to column 0, same band.
            b'$' => {
                self.col = 0;
            }
            // Line feed: next 6-pixel band.
            b'-' => {
                self.band += 1;
                self.col = 0;
            }
            // Color introducer.
            b'#' => {
                self.flush_num();
                self.num_buf.clear();
                self.cur_num = 0;
                self.cur_num_valid = false;
                self.state = State::Color;
            }
            // Repeat introducer.
            b'!' => {
                self.flush_num();
                self.cur_num = 0;
                self.cur_num_valid = false;
                self.state = State::Repeat;
            }
            // Digit — start of a standalone param (ignored in Normal, but
            // the param string before 'q' is already consumed by the DCS
            // accumulator, so we should not normally see digits here).
            b'0'..=b'9' => {}
            // Ignored: whitespace and other control-ish bytes.
            _ => {}
        }
    }

    fn flush_num(&mut self) {
        if self.cur_num_valid {
            self.num_buf.push(self.cur_num);
        }
        self.cur_num = 0;
        self.cur_num_valid = false;
    }

    fn color_byte(&mut self, b: u8) {
        match b {
            b'0'..=b'9' => {
                let d = u32::from(b - b'0');
                self.cur_num = self.cur_num.saturating_mul(10).saturating_add(d);
                self.cur_num_valid = true;
            }
            b';' => {
                self.flush_num();
            }
            _ => {
                // End of color command — parse it.
                self.flush_num();
                self.apply_color();
                self.num_buf.clear();
                self.cur_num = 0;
                self.cur_num_valid = false;
                // Re-process the current byte in Normal or the appropriate state.
                self.state = State::Normal;
                self.normal(b);
            }
        }
    }

    fn repeat_byte(&mut self, b: u8) {
        match b {
            b'0'..=b'9' => {
                let d = u32::from(b - b'0');
                self.cur_num = self.cur_num.saturating_mul(10).saturating_add(d);
                self.cur_num_valid = true;
            }
            0x3f..=0x7e => {
                // The character to repeat. Cap at the max canvas dimension so a
                // malformed stream like `!999999999~` doesn't loop for minutes.
                const MAX_REPEAT: usize = 16_384;
                let raw_count = if self.cur_num_valid {
                    self.cur_num as usize
                } else {
                    1
                };
                let count = raw_count.min(MAX_REPEAT);
                for _ in 0..count {
                    self.paint_sixel(b);
                }
                self.cur_num = 0;
                self.cur_num_valid = false;
                self.state = State::Normal;
            }
            _ => {
                // Malformed: abandon repeat and process normally.
                self.cur_num = 0;
                self.cur_num_valid = false;
                self.state = State::Normal;
                self.normal(b);
            }
        }
    }

    fn apply_color(&mut self) {
        // `num_buf` layout:
        //   [0]        → register index  (always present)
        //   [1]        → type (2 = RGB, 1 = HLS)
        //   [2],[3],[4]→ channel values
        let reg = match self.num_buf.first() {
            Some(&r) => r as usize,
            None => return,
        };
        if self.num_buf.len() == 1 {
            // `#<n>` — select color register.
            self.current_color = reg;
            return;
        }
        if self.num_buf.len() < 5 {
            // `#<n>;<type>` — incomplete define; just select.
            self.current_color = reg;
            return;
        }
        let color_type = self.num_buf[1];
        let a = self.num_buf[2];
        let b = self.num_buf[3];
        let c = self.num_buf[4];

        let rgb = match color_type {
            // Type 2: RGB (0–100 percent each).
            2 => [rgb_pct_to_u8(a), rgb_pct_to_u8(b), rgb_pct_to_u8(c)],
            // Type 1: HLS (hue 0–360, lightness 0–100, saturation 0–100).
            1 => hls_to_rgb(a, b, c),
            // Unknown type: fall back to default palette.
            _ => palette_color(reg, &self.palette),
        };
        self.palette.insert(reg, rgb);
        self.current_color = reg;
    }

    fn finish(self) -> Option<SixelImage> {
        self.canvas.into_image()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Build a minimal sixel payload that draws a 2×6 block of solid color.
    ///
    /// Color register 0 = pure red (#0;2;100;0;0).
    /// Sixel char 0x7F is '~' - 1 = 0x7E → bits = 0x3F → all 6 rows lit.
    fn tiny_red_block_payload() -> Vec<u8> {
        // #0;2;100;0;0   → define register 0 as red (100% R, 0% G, 0% B)
        // #0             → select register 0 (redundant but explicit)
        // ~~             → two sixel chars, all-6-bits set → 2×6 block
        b"#0;2;100;0;0#0~~".to_vec()
    }

    // ── Decode smoke test ─────────────────────────────────────────────────────

    #[test]
    fn decode_tiny_red_block() {
        let img = decode(&tiny_red_block_payload()).expect("must decode");
        assert_eq!(img.width, 2, "width must be 2");
        assert_eq!(img.height, 6, "height must be 6 (one band)");
        assert_eq!(img.rgba.len(), 2 * 6 * 4);

        // Every pixel in the 2×6 block must be red (255, 0, 0, 255).
        for chunk in img.rgba.chunks(4) {
            assert_eq!(chunk[0], 255, "R must be 255");
            assert_eq!(chunk[1], 0, "G must be 0");
            assert_eq!(chunk[2], 0, "B must be 0");
            assert_eq!(chunk[3], 255, "A must be 255 (opaque)");
        }
    }

    // ── Color register: RGB percent → u8 ─────────────────────────────────────

    #[test]
    fn rgb_pct_100_maps_to_255() {
        assert_eq!(rgb_pct_to_u8(100), 255);
    }

    #[test]
    fn rgb_pct_0_maps_to_0() {
        assert_eq!(rgb_pct_to_u8(0), 0);
    }

    #[test]
    fn rgb_pct_50_maps_to_approximately_127() {
        let v = rgb_pct_to_u8(50);
        // 50*255/100 = 127 (integer division).
        assert_eq!(v, 127);
    }

    #[test]
    fn rgb_pct_clamps_above_100() {
        // Over-range values must not panic or overflow.
        assert_eq!(rgb_pct_to_u8(200), 255);
    }

    // ── Color register: select with `#<n>` ───────────────────────────────────

    #[test]
    fn color_register_define_and_select() {
        // Define reg 1 = green, define reg 2 = blue, select reg 2, draw 1 col.
        let payload = b"#1;2;0;100;0#2;2;0;0;100#2~";
        let img = decode(payload).expect("must decode");
        assert_eq!(img.width, 1);
        // col 0 should be blue.
        let px = &img.rgba[0..4];
        assert_eq!(px, &[0, 0, 255, 255], "pixel must be blue");
    }

    // ── Repeat operator `!<n><char>` ─────────────────────────────────────────

    #[test]
    fn repeat_operator_expands_width() {
        // Define register 0 = white, then repeat '~' (all-bits set) 8 times.
        let payload = b"#0;2;100;100;100!8~";
        let img = decode(payload).expect("must decode");
        assert_eq!(img.width, 8, "repeat !8~ must produce 8 columns");
        assert_eq!(img.height, 6, "one band = 6 rows");
        // All pixels white and opaque.
        for chunk in img.rgba.chunks(4) {
            assert_eq!(chunk, &[255, 255, 255, 255]);
        }
    }

    // ── Carriage-return `$` stays in the same band ────────────────────────────

    #[test]
    fn carriage_return_overlays_same_band() {
        // Draw red column 0, then $ → back to col 0, draw blue on top.
        // Result: column 0 = blue (overwrites red), canvas width = 1.
        let payload = b"#0;2;100;0;0~$#1;2;0;0;100~";
        let img = decode(payload).expect("must decode");
        assert_eq!(img.width, 1, "only one column was drawn net");
        let px = &img.rgba[0..4];
        assert_eq!(px, &[0, 0, 255, 255], "blue overwrites red after $");
    }

    // ── Line feed `-` advances to the next band ───────────────────────────────

    #[test]
    fn newline_advances_band() {
        // Band 0: red (register 0). Band 1: green (register 1).
        // Each draw covers exactly 1 column, all 6 bits set.
        let payload = b"#0;2;100;0;0~-#1;2;0;100;0~";
        let img = decode(payload).expect("must decode");
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 12, "two bands = 12 rows");
        // Row 0 (band 0 top) = red.
        let r = &img.rgba[0..4];
        assert_eq!(r, &[255, 0, 0, 255], "band 0 must be red");
        // Row 6 (band 1 top) = green.
        let g = &img.rgba[6 * 4..6 * 4 + 4];
        assert_eq!(g, &[0, 255, 0, 255], "band 1 must be green");
    }

    // ── HLS color type ────────────────────────────────────────────────────────

    #[test]
    fn hls_pure_red_decodes() {
        // H=0, L=50, S=100 → pure red in HSL.
        let rgb = hls_to_rgb(0, 50, 100);
        assert_eq!(rgb[0], 255, "R=255 for pure HSL red");
        assert_eq!(rgb[1], 0, "G=0");
        assert_eq!(rgb[2], 0, "B=0");
    }

    #[test]
    fn hls_white_decodes() {
        // H=any, L=100, S=0 → white.
        let rgb = hls_to_rgb(0, 100, 0);
        assert_eq!(rgb, [255, 255, 255]);
    }

    #[test]
    fn hls_black_decodes() {
        // H=any, L=0, S=0 → black.
        let rgb = hls_to_rgb(0, 0, 0);
        assert_eq!(rgb, [0, 0, 0]);
    }

    // ── Default palette fallback ──────────────────────────────────────────────

    #[test]
    fn default_palette_register_0_is_black() {
        let custom = HashMap::new();
        assert_eq!(palette_color(0, &custom), [0, 0, 0]);
    }

    #[test]
    fn default_palette_register_15_is_white() {
        let custom = HashMap::new();
        assert_eq!(palette_color(15, &custom), [255, 255, 255]);
    }

    #[test]
    fn default_palette_high_register_is_grey() {
        let custom = HashMap::new();
        let [r, g, b] = palette_color(16, &custom);
        assert_eq!(r, g, "high register must be grey (r==g)");
        assert_eq!(g, b, "high register must be grey (g==b)");
    }

    // ── Empty/trivial payloads ────────────────────────────────────────────────

    #[test]
    fn empty_payload_returns_none() {
        assert!(decode(b"").is_none(), "empty payload must return None");
    }

    #[test]
    fn payload_with_only_cr_returns_none() {
        // No pixels drawn → None.
        assert!(decode(b"$$$$").is_none());
    }

    // ── Oversized-buffer guard ────────────────────────────────────────────────

    #[test]
    fn oversized_repeat_does_not_allocate_unboundedly() {
        // A huge repeat count must be silently capped at MAX_REPEAT (16 384).
        // The image must still decode (just truncated) and must not hang or OOM.
        let mut payload = b"#0;2;100;0;0".to_vec();
        payload.extend_from_slice(b"!999999999~"); // way above the cap
                                                   // Must complete quickly and not panic.
        let result = decode(&payload);
        // The repeat is capped at 16 384 columns — image is Some with width <= cap.
        if let Some(img) = result {
            assert!(
                img.width <= 16_384,
                "width {} must be capped at 16384",
                img.width
            );
        }
        // None is also acceptable (cap hit → canvas guard returns false).
    }

    // ── Multi-chunk accumulation: same result as single-chunk ─────────────────

    #[test]
    fn split_payload_yields_same_image_as_whole() {
        let full = tiny_red_block_payload();
        let img_full = decode(&full).expect("single chunk must decode");

        // Split at a few different boundaries.
        for split in [1, 5, 8, full.len() / 2, full.len() - 1] {
            if split == 0 || split >= full.len() {
                continue;
            }
            let mut dec = Decoder::new();
            dec.feed(&full[..split]);
            dec.feed(&full[split..]);
            let img_split = dec.finish().expect("split feed must decode");
            assert_eq!(
                img_split.rgba, img_full.rgba,
                "split at {split} must produce same pixels"
            );
            assert_eq!(img_split.width, img_full.width);
            assert_eq!(img_split.height, img_full.height);
        }
    }
}
