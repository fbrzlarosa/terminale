//! APC graphics protocol parser (ESC _G … ST).
//!
//! Decodes APC sequences (`ESC _ G … ST`) into RGBA8 pixel data
//! that can be passed to [`crate::images::ImageStore`].
//!
//! Wire format: `ESC _ G <control-data> ; <payload> ST`
//! - Introducer: `ESC _` (0x1B 0x5F) followed by `G` (0x47).
//! - control-data: comma-separated `key=value` pairs (e.g. `a=T,f=100,m=1`).
//! - payload: base64-encoded image data after the first `;`.
//! - Terminator: ST = `ESC \` (0x1B 0x5C) or C1 ST (0x9C).
//!
//! Multi-chunk: `m=1` = more chunks follow; `m=0`/absent = final chunk.
//!
//! # Supported features
//!
//! - Actions: `a=t` (transmit only), `a=T` (transmit+display), `a=p` (put), `a=d` (delete).
//! - Formats: `f=32` (raw RGBA8), `f=24` (raw RGB8, expanded), `f=100` (PNG/container).
//! - Medium: `t=d` (direct base64 inline, the default). Others silently ignored.
//! - Image id (`i=`) and placement id (`p=`) tracking.
//! - Multi-chunk assembly keyed by image id.
//!
//! # Out of scope
//!
//! - File/temp/shared-memory medium (`t=f`/`t=t`/`t=s`).
//! - Zlib compression (`o=z`).
//! - Query/ack writeback (`a=q`) -- follow-up item.

use std::collections::HashMap;

// ---- Public surface ---------------------------------------------------------

/// Parsed APC graphics control-data fields (`ESC _ G … ST`).
///
/// All fields are defaulted: unknown/absent keys are silently ignored
/// for forward compatibility.
#[derive(Debug, Clone, Default)]
pub struct ApcControl {
    /// Action: `a=t` transmit, `a=T` transmit+display, `a=p` put,
    /// `a=d` delete. Default `T`.
    pub action: ApcAction,
    /// Pixel format: `f=32` RGBA8, `f=24` RGB8, `f=100` PNG/container.
    /// Default `32`.
    pub format: u32,
    /// Medium: `d` = direct base64 inline (supported). Others out of scope.
    pub medium: char,
    /// Image width in pixels (required for raw formats; `s=` key).
    pub width_px: u32,
    /// Image height in pixels (required for raw formats; `v=` key).
    pub height_px: u32,
    /// Requested placement width in cells (`c=`). `0` = auto.
    pub cols: u16,
    /// Requested placement height in cells (`r=`). `0` = auto.
    pub rows: u16,
    /// Image id (`i=`). `0` = no id / single anonymous image.
    pub image_id: u32,
    /// Placement id (`p=`). `0` = no explicit placement.
    pub placement_id: u32,
    /// More chunks follow (`m=1`); this is the final chunk (`m=0`/absent).
    pub more: bool,
    /// Compression method (`o=`). `z` = zlib (unsupported; payload dropped).
    pub compression: char,
}

/// APC graphics action codes (`a=` key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApcAction {
    /// Transmit only (store without displaying). `a=t`.
    TransmitOnly,
    /// Transmit and display at the current cursor position. `a=T`. Default.
    #[default]
    TransmitAndDisplay,
    /// Put (display a previously stored image). `a=p`.
    Put,
    /// Delete placements. `a=d`.
    Delete,
}

/// Result of successfully completing an APC graphics chunk assembly.
#[derive(Debug)]
pub struct ApcImage {
    /// Raw RGBA8 pixels, row-major, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The control fields that completed this image.
    pub control: ApcControl,
}

// ---- Control-data parser ----------------------------------------------------

/// Parse the `key=value,...` control-data string from an APC graphics sequence.
///
/// Unknown keys are silently ignored. Malformed values fall back to defaults.
#[must_use]
pub fn parse_control(data: &str) -> ApcControl {
    let mut ctrl = ApcControl {
        format: 32,
        medium: 'd',
        ..Default::default()
    };
    for pair in data.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let eq = match pair.find('=') {
            Some(p) => p,
            None => continue,
        };
        let key = &pair[..eq];
        let val = &pair[eq + 1..];
        match key {
            "a" => {
                ctrl.action = match val {
                    "t" => ApcAction::TransmitOnly,
                    "T" => ApcAction::TransmitAndDisplay,
                    "p" => ApcAction::Put,
                    "d" => ApcAction::Delete,
                    _ => ApcAction::TransmitAndDisplay,
                };
            }
            "f" => {
                ctrl.format = val.parse().unwrap_or(32);
            }
            "t" => {
                ctrl.medium = val.chars().next().unwrap_or('d');
            }
            "s" => {
                ctrl.width_px = val.parse().unwrap_or(0);
            }
            "v" => {
                ctrl.height_px = val.parse().unwrap_or(0);
            }
            "c" => {
                ctrl.cols = val.parse().unwrap_or(0);
            }
            "r" => {
                ctrl.rows = val.parse().unwrap_or(0);
            }
            "i" => {
                ctrl.image_id = val.parse().unwrap_or(0);
            }
            "p" => {
                ctrl.placement_id = val.parse().unwrap_or(0);
            }
            "m" => {
                ctrl.more = val == "1";
            }
            "o" => {
                ctrl.compression = val.chars().next().unwrap_or('\0');
            }
            _ => {}
        }
    }
    ctrl
}

// ---- Chunk assembler --------------------------------------------------------

/// Maximum bytes accumulated for a single APC graphics image payload.
/// Protects against a runaway sender that never closes the frame.
pub const APC_GRAPHICS_BUF_CAP: usize = 8 * 1024 * 1024; // 8 MiB

/// In-flight chunk accumulation entry, keyed by image id.
#[derive(Debug, Default)]
struct InFlight {
    /// Accumulated base64 payload bytes across all `m=1` chunks.
    payload_b64: Vec<u8>,
    /// Control fields from the first chunk (carries format and pixel dimensions).
    first_ctrl: Option<ApcControl>,
}

/// Stateful assembler that accumulates multi-chunk APC graphics images and emits
/// a completed [`ApcImage`] when the final chunk arrives.
///
/// Keyed by image id; anonymous images (id = 0) accumulate into the same
/// slot and are committed as soon as `m=0` is seen.
#[derive(Debug, Default)]
pub struct ApcGraphicsAssembler {
    in_flight: HashMap<u32, InFlight>,
}

impl ApcGraphicsAssembler {
    /// Create a new, empty assembler.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one parsed chunk into the assembler.
    ///
    /// Returns `None` when more data is expected or decoding fails.
    /// Returns `Some(ApcImage)` on successful completion.
    pub fn feed(&mut self, ctrl: ApcControl, payload_b64: &[u8]) -> Option<ApcImage> {
        let key = ctrl.image_id;

        // Ignore out-of-scope media types.
        if ctrl.medium != 'd' && ctrl.medium != '\0' {
            tracing::debug!(
                medium = %ctrl.medium,
                "apc_graphics: non-direct medium ignored (out of scope)"
            );
            self.in_flight.remove(&key);
            return None;
        }

        // Ignore zlib-compressed payloads.
        if ctrl.compression == 'z' {
            tracing::debug!(
                image_id = ctrl.image_id,
                "apc_graphics: zlib compression not supported, dropping payload"
            );
            self.in_flight.remove(&key);
            return None;
        }

        let entry = self.in_flight.entry(key).or_default();
        // Save first-chunk control so we retain format/size info on final chunk.
        if entry.first_ctrl.is_none() {
            entry.first_ctrl = Some(ctrl.clone());
        }

        // Append payload bytes (capped at APC_GRAPHICS_BUF_CAP).
        let space_left = APC_GRAPHICS_BUF_CAP.saturating_sub(entry.payload_b64.len());
        let to_take = payload_b64.len().min(space_left);
        entry.payload_b64.extend_from_slice(&payload_b64[..to_take]);

        if ctrl.more {
            // More chunks expected; defer decoding.
            return None;
        }

        // Final chunk: consume the accumulated state.
        let InFlight { payload_b64: b64_buf, first_ctrl } =
            self.in_flight.remove(&key).unwrap_or_default();
        let first_ctrl = first_ctrl.unwrap_or_else(|| ctrl.clone());

        // Decode base64.
        let Ok(raw_bytes) = base64_decode_bytes(&b64_buf) else {
            tracing::warn!(
                image_id = ctrl.image_id,
                "apc_graphics: base64 decode failed, dropping image"
            );
            return None;
        };

        // Build effective control: format/dimensions from the first chunk,
        // action/cols/rows from the final chunk.
        let effective_ctrl = ApcControl {
            format: first_ctrl.format,
            width_px: first_ctrl.width_px,
            height_px: first_ctrl.height_px,
            action: ctrl.action,
            cols: ctrl.cols,
            rows: ctrl.rows,
            image_id: ctrl.image_id,
            placement_id: ctrl.placement_id,
            medium: ctrl.medium,
            compression: ctrl.compression,
            more: false,
        };

        let (rgba, w, h) = decode_pixels(&raw_bytes, &effective_ctrl)?;
        Some(ApcImage {
            rgba,
            width: w,
            height: h,
            control: effective_ctrl,
        })
    }

    /// Discard all in-flight buffers. Called when the protocol is disabled
    /// mid-stream so no half-buffered frame lingers.
    pub fn clear(&mut self) {
        self.in_flight.clear();
    }
}

// ---- Pixel decoding ---------------------------------------------------------

/// Maximum canvas area in pixels, as a safety cap.
const MAX_APC_PIXELS: usize = 16_384 * 16_384;

/// Decode raw bytes into `(rgba, width, height)` per the `f=` format field.
/// Returns `None` on invalid input.
fn decode_pixels(raw: &[u8], ctrl: &ApcControl) -> Option<(Vec<u8>, u32, u32)> {
    match ctrl.format {
        // f=100: PNG / JPEG / WebP or any format the `image` crate handles.
        100 => {
            let img = image::load_from_memory(raw)
                .map_err(|e| tracing::warn!(error = ?e, "apc_graphics: container image decode failed"))
                .ok()?;
            let rgba8 = img.into_rgba8();
            let w = rgba8.width();
            let h = rgba8.height();
            if w == 0 || h == 0 {
                tracing::warn!("apc_graphics: zero-dimension decoded image discarded");
                return None;
            }
            Some((rgba8.into_raw(), w, h))
        }
        // f=32: raw RGBA8, 4 bytes per pixel.
        32 => {
            let w = ctrl.width_px;
            let h = ctrl.height_px;
            if w == 0 || h == 0 {
                tracing::warn!("apc_graphics: f=32 without s=/v= dimensions, dropping");
                return None;
            }
            let expected = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
            if expected > MAX_APC_PIXELS * 4 {
                tracing::warn!("apc_graphics: f=32 image exceeds size cap, dropping");
                return None;
            }
            if raw.len() != expected {
                tracing::warn!(
                    got = raw.len(),
                    expected,
                    "apc_graphics: f=32 byte count mismatch, dropping"
                );
                return None;
            }
            Some((raw.to_vec(), w, h))
        }
        // f=24: raw RGB8, 3 bytes per pixel, expanded to RGBA.
        24 => {
            let w = ctrl.width_px;
            let h = ctrl.height_px;
            if w == 0 || h == 0 {
                tracing::warn!("apc_graphics: f=24 without s=/v= dimensions, dropping");
                return None;
            }
            let n_pixels = (w as usize).checked_mul(h as usize)?;
            if n_pixels > MAX_APC_PIXELS {
                tracing::warn!("apc_graphics: f=24 image exceeds size cap, dropping");
                return None;
            }
            let expected = n_pixels.checked_mul(3)?;
            if raw.len() != expected {
                tracing::warn!(
                    got = raw.len(),
                    expected,
                    "apc_graphics: f=24 byte count mismatch, dropping"
                );
                return None;
            }
            // Expand RGB -> RGBA with full opacity.
            let mut rgba = Vec::with_capacity(n_pixels * 4);
            for rgb in raw.chunks_exact(3) {
                rgba.push(rgb[0]);
                rgba.push(rgb[1]);
                rgba.push(rgb[2]);
                rgba.push(0xFF);
            }
            Some((rgba, w, h))
        }
        f => {
            tracing::warn!(format = f, "apc_graphics: unknown pixel format, dropping");
            None
        }
    }
}

// ---- Base64 decoder ---------------------------------------------------------

/// Decode standard base64 from a byte slice (APC payloads are raw PTY bytes).
/// Tolerates embedded whitespace (LF, CR, SP, TAB).
/// Returns `Err(())` on any invalid character.
fn base64_decode_bytes(input: &[u8]) -> Result<Vec<u8>, ()> {
    let table: [u8; 256] = {
        let mut t = [0xffu8; 256];
        for (i, &c) in
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
                .iter()
                .enumerate()
        {
            t[c as usize] = i as u8;
        }
        t[b'=' as usize] = 0;
        t
    };
    let mut out = Vec::with_capacity(input.len() * 3 / 4 + 1);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &b in input {
        // Skip whitespace silently.
        if b == b'\n' || b == b'\r' || b == b' ' || b == b'\t' {
            continue;
        }
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

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode bytes to standard base64 (test-only helper).
    fn b64encode(data: &[u8]) -> Vec<u8> {
        const ALPHA: &[u8] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let b0 = data[i];
            let b1 = if i + 1 < data.len() { data[i + 1] } else { 0 };
            let b2 = if i + 2 < data.len() { data[i + 2] } else { 0 };
            out.push(ALPHA[(b0 >> 2) as usize]);
            out.push(ALPHA[((b0 & 3) << 4 | b1 >> 4) as usize]);
            if i + 1 < data.len() {
                out.push(ALPHA[((b1 & 0xf) << 2 | b2 >> 6) as usize]);
            } else {
                out.push(b'=');
            }
            if i + 2 < data.len() {
                out.push(ALPHA[(b2 & 0x3f) as usize]);
            } else {
                out.push(b'=');
            }
            i += 3;
        }
        out
    }

    /// Tiny 1x1 solid-blue RGBA PNG (encoded via the `image` crate).
    fn tiny_blue_png() -> Vec<u8> {
        use image::{ImageBuffer, Rgba};
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(1, 1, Rgba([0u8, 0, 255, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)
            .expect("encode tiny PNG");
        buf.into_inner()
    }

    // -- parse_control ---------------------------------------------------------

    #[test]
    fn parse_control_defaults() {
        let ctrl = parse_control("");
        assert_eq!(ctrl.format, 32, "default format must be 32");
        assert_eq!(ctrl.medium, 'd', "default medium must be 'd'");
        assert_eq!(ctrl.action, ApcAction::TransmitAndDisplay);
        assert!(!ctrl.more, "default more must be false");
        assert_eq!(ctrl.image_id, 0);
    }

    #[test]
    fn parse_control_all_known_keys() {
        let ctrl =
            parse_control("a=t,f=100,t=d,s=640,v=480,c=80,r=24,i=42,p=7,m=1,o=z");
        assert_eq!(ctrl.action, ApcAction::TransmitOnly);
        assert_eq!(ctrl.format, 100);
        assert_eq!(ctrl.medium, 'd');
        assert_eq!(ctrl.width_px, 640);
        assert_eq!(ctrl.height_px, 480);
        assert_eq!(ctrl.cols, 80);
        assert_eq!(ctrl.rows, 24);
        assert_eq!(ctrl.image_id, 42);
        assert_eq!(ctrl.placement_id, 7);
        assert!(ctrl.more);
        assert_eq!(ctrl.compression, 'z');
    }

    #[test]
    fn parse_control_action_variants() {
        assert_eq!(
            parse_control("a=T").action,
            ApcAction::TransmitAndDisplay
        );
        assert_eq!(parse_control("a=t").action, ApcAction::TransmitOnly);
        assert_eq!(parse_control("a=p").action, ApcAction::Put);
        assert_eq!(parse_control("a=d").action, ApcAction::Delete);
    }

    #[test]
    fn parse_control_more_flag() {
        assert!(parse_control("m=1").more);
        assert!(!parse_control("m=0").more);
        assert!(!parse_control("").more);
    }

    #[test]
    fn parse_control_unknown_keys_ignored() {
        // Must not panic; known keys still parsed correctly.
        let ctrl = parse_control("f=100,x=unknown,q=bogus,i=5");
        assert_eq!(ctrl.format, 100);
        assert_eq!(ctrl.image_id, 5);
    }

    #[test]
    fn parse_control_malformed_value_falls_back() {
        // f=abc must not panic; falls back to default (32).
        let ctrl = parse_control("f=abc");
        assert_eq!(ctrl.format, 32);
    }

    // -- base64_decode_bytes ---------------------------------------------------

    #[test]
    fn base64_roundtrip_small_payload() {
        let orig = b"hello world";
        let encoded = b64encode(orig);
        let decoded = base64_decode_bytes(&encoded).expect("decode must succeed");
        assert_eq!(decoded, orig);
    }

    #[test]
    fn base64_tolerates_embedded_newlines() {
        let orig = b"abcdef";
        let mut encoded = b64encode(orig);
        // Insert a newline in the middle.
        encoded.insert(4, b'\n');
        let decoded = base64_decode_bytes(&encoded).expect("decode with newline");
        assert_eq!(decoded, orig);
    }

    #[test]
    fn base64_invalid_character_returns_err() {
        // '!' is not in the base64 alphabet.
        assert!(base64_decode_bytes(b"SGVsb!8=").is_err());
    }

    // -- decode_pixels ---------------------------------------------------------

    #[test]
    fn decode_pixels_f32_rgba_roundtrip() {
        // 2x2 RGBA8 raw: 16 bytes.
        let rgba_in: Vec<u8> = (0u8..16).collect();
        let ctrl = ApcControl {
            format: 32,
            width_px: 2,
            height_px: 2,
            ..Default::default()
        };
        let (rgba, w, h) = decode_pixels(&rgba_in, &ctrl).expect("f=32 decode");
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert_eq!(rgba, rgba_in);
    }

    #[test]
    fn decode_pixels_f24_rgb_expands_to_rgba() {
        // 1x1 RGB8: [10, 20, 30]
        let rgb_in: Vec<u8> = vec![10, 20, 30];
        let ctrl = ApcControl {
            format: 24,
            width_px: 1,
            height_px: 1,
            ..Default::default()
        };
        let (rgba, w, h) = decode_pixels(&rgb_in, &ctrl).expect("f=24 decode");
        assert_eq!(w, 1);
        assert_eq!(h, 1);
        assert_eq!(rgba, vec![10, 20, 30, 0xFF]);
    }

    #[test]
    fn decode_pixels_f100_png() {
        let png = tiny_blue_png();
        let ctrl = ApcControl {
            format: 100,
            ..Default::default()
        };
        let (rgba, w, h) = decode_pixels(&png, &ctrl).expect("f=100 PNG decode");
        assert_eq!(w, 1);
        assert_eq!(h, 1);
        assert_eq!(&rgba[..4], &[0, 0, 255, 255], "pixel must be blue");
    }

    #[test]
    fn decode_pixels_f32_wrong_size_returns_none() {
        // Claim 2x2 but supply only 4 bytes (should be 16).
        let ctrl = ApcControl {
            format: 32,
            width_px: 2,
            height_px: 2,
            ..Default::default()
        };
        assert!(decode_pixels(&[0u8; 4], &ctrl).is_none());
    }

    #[test]
    fn decode_pixels_f32_zero_dims_returns_none() {
        let ctrl = ApcControl {
            format: 32,
            width_px: 0,
            height_px: 0,
            ..Default::default()
        };
        assert!(decode_pixels(&[], &ctrl).is_none());
    }

    #[test]
    fn decode_pixels_unknown_format_returns_none() {
        let ctrl = ApcControl {
            format: 99,
            ..Default::default()
        };
        assert!(decode_pixels(&[0u8; 4], &ctrl).is_none());
    }

    // -- ApcGraphicsAssembler --------------------------------------------------

    #[test]
    fn assembler_single_chunk_png_produces_image() {
        let png = tiny_blue_png();
        let encoded = b64encode(&png);
        let ctrl = ApcControl {
            format: 100,
            action: ApcAction::TransmitAndDisplay,
            ..Default::default()
        };
        let mut asm = ApcGraphicsAssembler::new();
        let result = asm.feed(ctrl, &encoded);
        assert!(result.is_some(), "single-chunk PNG must produce an image");
        let img = result.unwrap();
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 1);
        assert_eq!(&img.rgba[..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn assembler_chunked_f32_assembly() {
        // 2x2 RGBA8 raw, split into two base64 chunks.
        let rgba: Vec<u8> = (0u8..16).collect();
        let encoded = b64encode(&rgba);
        let mid = encoded.len() / 2;

        let mut asm = ApcGraphicsAssembler::new();

        // First chunk: m=1 (more follows).
        let ctrl1 = ApcControl {
            format: 32,
            width_px: 2,
            height_px: 2,
            image_id: 1,
            more: true,
            ..Default::default()
        };
        let r1 = asm.feed(ctrl1, &encoded[..mid]);
        assert!(r1.is_none(), "first chunk (m=1) must return None");

        // Second chunk: m=0 (final).
        let ctrl2 = ApcControl {
            format: 32,
            width_px: 2,
            height_px: 2,
            image_id: 1,
            more: false,
            action: ApcAction::TransmitAndDisplay,
            ..Default::default()
        };
        let r2 = asm.feed(ctrl2, &encoded[mid..]);
        assert!(r2.is_some(), "final chunk (m=0) must produce image");
        let img = r2.unwrap();
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        assert_eq!(img.rgba, rgba, "assembled pixels must match original");
    }

    #[test]
    fn assembler_clear_discards_in_flight() {
        let png = tiny_blue_png();
        let encoded = b64encode(&png);

        let mut asm = ApcGraphicsAssembler::new();
        // Start a multi-chunk sequence.
        let ctrl = ApcControl {
            format: 100,
            image_id: 5,
            more: true,
            ..Default::default()
        };
        let r = asm.feed(ctrl, &encoded);
        assert!(r.is_none());
        // Clear drops in-flight buffer.
        asm.clear();
        // Feed the final chunk alone: buffer was cleared, so no complete assembly.
        let ctrl_final = ApcControl {
            format: 100,
            image_id: 5,
            more: false,
            ..Default::default()
        };
        // Either None or a partial decode is acceptable; must not panic.
        let _ = asm.feed(ctrl_final, &encoded);
    }

    #[test]
    fn assembler_zlib_payload_dropped() {
        let ctrl = ApcControl {
            format: 32,
            width_px: 1,
            height_px: 1,
            compression: 'z',
            more: false,
            ..Default::default()
        };
        let mut asm = ApcGraphicsAssembler::new();
        let result = asm.feed(ctrl, b"anything");
        assert!(result.is_none(), "zlib payload must be dropped");
    }

    #[test]
    fn assembler_file_medium_dropped() {
        let ctrl = ApcControl {
            format: 100,
            medium: 'f',
            more: false,
            ..Default::default()
        };
        let mut asm = ApcGraphicsAssembler::new();
        let result = asm.feed(ctrl, b"anything");
        assert!(result.is_none(), "file medium must be ignored");
    }
}
