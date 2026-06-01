//! Shared geometry for the window-control icons (min / max / restore /
//! close). Both the wgpu-rendered main window and the egui-driven
//! settings window consume this so the buttons look identical pixel
//! for pixel.
//!
//! This module is *just* geometry — it returns line endpoints and
//! colour/thickness constants. Each consumer translates the line list
//! into its native primitive (`Quad::line` for bg_pipeline, egui's
//! `painter().line_segment()` for the settings window).

/// Which system icon to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemIcon {
    /// Minimize — a single horizontal bar through the centre.
    Minimize,
    /// Maximize — a hollow square outline. Used when the window is
    /// **not** maximised.
    Maximize,
    /// Restore — two overlapping squares (the "windowed" glyph). Used
    /// when the window **is** maximised.
    Restore,
    /// Close — an X (two diagonal lines).
    Close,
}

/// A single line segment in logical pixels (caller's coordinate system).
#[derive(Debug, Clone, Copy)]
pub struct IconLine {
    /// Start `(x, y)`.
    pub from: (f32, f32),
    /// End `(x, y)`.
    pub to: (f32, f32),
}

/// Stroke thickness in logical px. 1.2 hits the sweet spot between
/// "feels solid" and "doesn't look bold" at 96 DPI; consumers scale by
/// the display factor before issuing GPU work.
pub const STROKE_PX: f32 = 1.2;

/// Half-size of an icon glyph in logical px (a 10×10 square centred on
/// the button). All icons fit inside this bounding box.
pub const HALF_SIZE: f32 = 5.0;

/// Half-size of one square in the "restore" overlapped-squares glyph.
const RESTORE_HALF: f32 = HALF_SIZE * 0.85;

/// Offset between the two squares of the "restore" glyph.
const RESTORE_OFFSET: f32 = 1.5;

/// Default stroke colour (matches the rest of the chrome — soft pale
/// blue). Hover variants are caller-controlled.
pub const STROKE_DEFAULT: [u8; 3] = [0xc8, 0xd0, 0xe0];
/// Stroke colour to use when the close button is hovered (white-on-red).
pub const STROKE_CLOSE_HOVER: [u8; 3] = [0xff, 0xff, 0xff];
/// Hover background for the close button (Windows-style danger red).
pub const BG_CLOSE_HOVER: [u8; 3] = [0xe8, 0x39, 0x39];
/// Hover background for the minimize / maximize / restore buttons.
pub const BG_HOVER: [u8; 3] = [0x2c, 0x35, 0x4b];
/// Idle background (matches the title bar fill).
pub const BG_IDLE: [u8; 3] = [0x07, 0x09, 0x0e];

/// Return the line segments needed to draw `icon` centred at `(cx, cy)`
/// in logical pixels. Use [`HALF_SIZE`] as the canonical icon size so
/// every consumer agrees on geometry.
#[must_use]
pub fn icon_lines(icon: SystemIcon, cx: f32, cy: f32) -> Vec<IconLine> {
    let h = HALF_SIZE;
    match icon {
        SystemIcon::Minimize => vec![IconLine {
            from: (cx - h, cy),
            to: (cx + h, cy),
        }],
        SystemIcon::Maximize => {
            // 4 lines forming a square outline. Drawn as separate
            // segments so the same primitive is used everywhere.
            let l = cx - h;
            let r = cx + h;
            let t = cy - h;
            let b = cy + h;
            vec![
                IconLine {
                    from: (l, t),
                    to: (r, t),
                },
                IconLine {
                    from: (l, b),
                    to: (r, b),
                },
                IconLine {
                    from: (l, t),
                    to: (l, b),
                },
                IconLine {
                    from: (r, t),
                    to: (r, b),
                },
            ]
        }
        SystemIcon::Restore => {
            // Two overlapping squares with a small offset.
            let mut lines = Vec::with_capacity(8);
            let rh = RESTORE_HALF;
            for (ox, oy) in [
                (-RESTORE_OFFSET, RESTORE_OFFSET),
                (RESTORE_OFFSET, -RESTORE_OFFSET),
            ] {
                let l = cx + ox - rh;
                let r = cx + ox + rh;
                let t = cy + oy - rh;
                let b = cy + oy + rh;
                lines.push(IconLine {
                    from: (l, t),
                    to: (r, t),
                });
                lines.push(IconLine {
                    from: (l, b),
                    to: (r, b),
                });
                lines.push(IconLine {
                    from: (l, t),
                    to: (l, b),
                });
                lines.push(IconLine {
                    from: (r, t),
                    to: (r, b),
                });
            }
            lines
        }
        SystemIcon::Close => vec![
            IconLine {
                from: (cx - h, cy - h),
                to: (cx + h, cy + h),
            },
            IconLine {
                from: (cx - h, cy + h),
                to: (cx + h, cy - h),
            },
        ],
    }
}
