//! Scroll-aware inline image store for the terminal.
//!
//! Images are stored decoded (RGBA8) and keyed by a monotonically
//! increasing `u64` id. Each image can have one or more *placements*:
//! a placement records where on the terminal grid (in absolute-line
//! coordinates — the same coordinate space used by OSC 133 prompt marks)
//! the image should be rendered, and how many cell columns × rows it
//! occupies.
//!
//! The store implements a simple LRU eviction policy based on a
//! configurable total-byte cap so it never grows unboundedly.

use std::collections::HashMap;

// ── Public types ──────────────────────────────────────────────────────────────

/// Unique identifier for a stored image.
pub type ImageId = u64;

/// A single decoded inline image (RGBA8 pixels, row-major).
#[derive(Debug, Clone)]
pub struct InlineImage {
    /// Image id (mirrors the key in [`ImageStore::images`]).
    pub id: ImageId,
    /// Raw RGBA8 pixel data, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    /// Width in pixels.
    pub width_px: u32,
    /// Height in pixels.
    pub height_px: u32,
    /// Byte size of `rgba` — cached to avoid recomputing for LRU accounting.
    pub byte_size: usize,
}

/// Where one image appears on the terminal grid.
///
/// All positional coordinates use the *absolute* line index from
/// alacritty's grid (negative = scrollback history), matching the
/// coordinate space used by OSC 133 prompt marks.
#[derive(Debug, Clone, Copy)]
pub struct ImagePlacement {
    /// Which image to draw.
    pub image_id: ImageId,
    /// Absolute grid line of the top-left corner.
    pub anchor_line: i32,
    /// Column of the top-left corner.
    pub anchor_col: u16,
    /// Width of the placement in terminal cells.
    pub cols: u16,
    /// Height of the placement in terminal cells.
    pub rows: u16,
    /// Z-order relative to other placements on the same cell (higher =
    /// drawn on top). Reserved for future use; always `0` from parsers.
    pub z: i32,
}

/// An on-screen placement together with its pixel-decoded position in the
/// *current viewport*. Returned by [`ImageStore::placements_in_view`].
#[derive(Debug, Clone, Copy)]
pub struct VisiblePlacement {
    /// The underlying placement record.
    pub placement: ImagePlacement,
    /// Row (0 = top of viewport) at which the placement's top edge sits.
    pub viewport_row: u16,
}

// ── ImageStore ────────────────────────────────────────────────────────────────

/// Inline-image store: decoded images + placements with scroll-aware culling
/// and LRU byte-cap eviction.
///
/// Thread safety: not `Send`/`Sync` by itself — wrap in `Mutex` together with
/// the enclosing `Emulator` when crossing thread boundaries.
pub struct ImageStore {
    images: HashMap<ImageId, InlineImage>,
    placements: Vec<ImagePlacement>,
    next_id: ImageId,
    total_bytes: usize,
    /// Maximum bytes the store may hold before it starts evicting the oldest
    /// images. Defaults to [`DEFAULT_MAX_BYTES`].
    max_bytes: usize,
    /// Monotonically increasing insertion-order counter used to find the
    /// oldest entry for LRU eviction.
    insertion_order: HashMap<ImageId, u64>,
    order_counter: u64,
}

/// Default memory cap: 64 MiB of decoded pixel data.
pub const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

impl std::fmt::Debug for ImageStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageStore")
            .field("images", &self.images.len())
            .field("placements", &self.placements.len())
            .field("total_bytes", &self.total_bytes)
            .field("max_bytes", &self.max_bytes)
            .finish()
    }
}

impl ImageStore {
    /// Create an empty store with the default byte cap.
    #[must_use]
    pub fn new() -> Self {
        Self {
            images: HashMap::new(),
            placements: Vec::new(),
            next_id: 1,
            total_bytes: 0,
            max_bytes: DEFAULT_MAX_BYTES,
            insertion_order: HashMap::new(),
            order_counter: 0,
        }
    }

    /// Create an empty store with a custom byte cap.
    #[must_use]
    pub fn with_max_bytes(max_bytes: usize) -> Self {
        let mut s = Self::new();
        s.max_bytes = max_bytes;
        s
    }

    /// Decode raw image bytes (PNG / JPEG / WebP / GIF first frame) and store
    /// the result. Returns the assigned [`ImageId`] on success, or `None` if
    /// the image cannot be decoded.
    ///
    /// The store takes ownership of the decoded pixel data. If the store is
    /// at capacity, the oldest image(s) are evicted to make room.
    pub fn add_image(&mut self, raw: &[u8]) -> Option<ImageId> {
        let img = image::load_from_memory(raw)
            .map_err(|e| {
                tracing::warn!(error = ?e, "inline-image: decode failed");
            })
            .ok()?;
        let rgba = img.into_rgba8();
        let (width_px, height_px) = (rgba.width(), rgba.height());
        if width_px == 0 || height_px == 0 {
            tracing::warn!("inline-image: zero-dimension image discarded");
            return None;
        }
        let data = rgba.into_raw();
        let byte_size = data.len();

        // Evict until we have room (or only one slot is left).
        self.evict_to_fit(byte_size);

        let id = self.next_id;
        self.next_id += 1;
        self.total_bytes += byte_size;
        self.insertion_order.insert(id, self.order_counter);
        self.order_counter += 1;
        self.images.insert(
            id,
            InlineImage {
                id,
                rgba: data,
                width_px,
                height_px,
                byte_size,
            },
        );
        tracing::debug!(
            id,
            width_px,
            height_px,
            byte_size,
            total_bytes = self.total_bytes,
            "inline-image: added"
        );
        Some(id)
    }

    /// Register a placement for an existing image at absolute grid
    /// coordinates. Returns `false` when `image_id` is unknown.
    pub fn place(
        &mut self,
        image_id: ImageId,
        anchor_line: i32,
        anchor_col: u16,
        cols: u16,
        rows: u16,
    ) -> bool {
        if !self.images.contains_key(&image_id) {
            return false;
        }
        self.placements.push(ImagePlacement {
            image_id,
            anchor_line,
            anchor_col,
            cols,
            rows,
            z: 0,
        });
        true
    }

    /// Store raw RGBA8 pixel data directly, bypassing container decoding.
    ///
    /// `rgba` must be exactly `width * height * 4` bytes (row-major RGBA8).
    /// Both `width` and `height` must be non-zero. The store takes ownership
    /// of the pixel buffer and runs the same LRU eviction as [`Self::add_image`].
    ///
    /// Returns the assigned [`ImageId`] on success, or `None` if dimensions
    /// are invalid or the buffer length does not match.
    pub fn add_rgba8(&mut self, width: u32, height: u32, rgba: Vec<u8>) -> Option<ImageId> {
        if width == 0 || height == 0 {
            tracing::warn!("inline-image: zero-dimension raw RGBA8 discarded");
            return None;
        }
        let expected = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
        if rgba.len() != expected {
            tracing::warn!(
                got = rgba.len(),
                expected,
                "inline-image: raw RGBA8 byte count mismatch"
            );
            return None;
        }
        let byte_size = rgba.len();
        self.evict_to_fit(byte_size);
        let id = self.next_id;
        self.next_id += 1;
        self.total_bytes += byte_size;
        self.insertion_order.insert(id, self.order_counter);
        self.order_counter += 1;
        self.images.insert(
            id,
            InlineImage {
                id,
                rgba,
                width_px: width,
                height_px: height,
                byte_size,
            },
        );
        tracing::debug!(
            id,
            width_px = width,
            height_px = height,
            byte_size,
            total_bytes = self.total_bytes,
            "inline-image: added (raw RGBA8)"
        );
        Some(id)
    }

    /// Borrow the decoded image with the given `id`, if present.
    #[must_use]
    pub fn get_image(&self, id: ImageId) -> Option<&InlineImage> {
        self.images.get(&id)
    }

    /// Iterate all images currently in the store.
    pub fn iter_images(&self) -> impl Iterator<Item = &InlineImage> {
        self.images.values()
    }

    /// Return all placements whose top edge lies within the viewport
    /// `[top_abs_line, top_abs_line + viewport_rows)`. The returned
    /// [`VisiblePlacement`] includes the viewport row (0-based) at which
    /// the top of the placement sits.
    ///
    /// Partially-visible placements (clipped at the top because the user
    /// scrolled partway into them) are excluded — renderers must handle
    /// clipping via scissor rects.
    #[must_use]
    pub fn placements_in_view(
        &self,
        top_abs_line: i32,
        viewport_rows: u16,
    ) -> Vec<VisiblePlacement> {
        let bottom = top_abs_line + i32::from(viewport_rows);
        let mut out = Vec::new();
        for p in &self.placements {
            if p.anchor_line >= top_abs_line && p.anchor_line < bottom {
                let viewport_row =
                    u16::try_from(p.anchor_line - top_abs_line).unwrap_or(u16::MAX);
                out.push(VisiblePlacement {
                    placement: *p,
                    viewport_row,
                });
            }
        }
        // Stable sort: by absolute line, then by column, then by z.
        out.sort_by_key(|v| (v.placement.anchor_line, v.placement.anchor_col, v.placement.z));
        out
    }

    /// Remove all placements whose `anchor_line` is above `oldest_line`
    /// (i.e. lines that have scrolled out of the scrollback buffer). Called
    /// after each [`Emulator::advance`] with the grid's `topmost_line`.
    pub fn prune_placements(&mut self, oldest_line: i32) {
        self.placements
            .retain(|p| p.anchor_line >= oldest_line);
    }

    /// The set of image ids for which textures must remain uploaded to the
    /// GPU. The host should call this each frame and free any GPU texture
    /// whose id is absent from the returned set.
    #[must_use]
    pub fn live_image_ids(&self) -> Vec<ImageId> {
        self.images.keys().copied().collect()
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn evict_to_fit(&mut self, incoming_bytes: usize) {
        // Nothing to evict if we have room.
        if self.total_bytes + incoming_bytes <= self.max_bytes {
            return;
        }
        // Collect ids ordered by insertion (oldest first).
        let mut order: Vec<(u64, ImageId)> = self
            .insertion_order
            .iter()
            .map(|(&id, &ord)| (ord, id))
            .collect();
        order.sort_unstable_by_key(|(ord, _)| *ord);

        for (_, id) in order {
            if self.total_bytes + incoming_bytes <= self.max_bytes {
                break;
            }
            if let Some(img) = self.images.remove(&id) {
                self.total_bytes = self.total_bytes.saturating_sub(img.byte_size);
                self.insertion_order.remove(&id);
                // Drop placements that reference the evicted image.
                self.placements.retain(|p| p.image_id != id);
                tracing::debug!(
                    id,
                    byte_size = img.byte_size,
                    remaining_bytes = self.total_bytes,
                    "inline-image: evicted (LRU)"
                );
            }
        }
    }
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny 1×1 red RGBA image encoded as PNG bytes using the `image` crate.
    fn tiny_png() -> Vec<u8> {
        use image::{ImageBuffer, Rgba};
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(1, 1, Rgba([255u8, 0, 0, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)
            .expect("encode tiny PNG");
        buf.into_inner()
    }

    #[test]
    fn store_is_empty_by_default() {
        let store = ImageStore::new();
        assert_eq!(store.images.len(), 0);
        assert_eq!(store.placements.len(), 0);
        assert_eq!(store.total_bytes, 0);
    }

    #[test]
    fn add_image_returns_incrementing_ids() {
        let mut store = ImageStore::new();
        let raw = tiny_png();
        let id1 = store.add_image(&raw).expect("first image must decode");
        let id2 = store.add_image(&raw).expect("second image must decode");
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(store.images.len(), 2);
    }

    #[test]
    fn add_image_rejects_invalid_bytes() {
        let mut store = ImageStore::new();
        assert!(store.add_image(b"not-an-image").is_none());
        assert_eq!(store.images.len(), 0);
        assert_eq!(store.total_bytes, 0);
    }

    #[test]
    fn place_returns_false_for_unknown_id() {
        let mut store = ImageStore::new();
        assert!(!store.place(999, 0, 0, 10, 5));
    }

    #[test]
    fn place_and_placements_in_view() {
        let mut store = ImageStore::new();
        let raw = tiny_png();
        let id = store.add_image(&raw).expect("decode");
        assert!(store.place(id, 10, 0, 20, 5));

        // Viewport starting at absolute line 8, 10 rows visible → includes line 10.
        let visible = store.placements_in_view(8, 10);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].placement.image_id, id);
        assert_eq!(visible[0].viewport_row, 2); // 10 - 8 = 2

        // Viewport starting at absolute line 0, 5 rows → does NOT include line 10.
        let hidden = store.placements_in_view(0, 5);
        assert_eq!(hidden.len(), 0);
    }

    #[test]
    fn placements_in_view_boundary_inclusive_exclusive() {
        let mut store = ImageStore::new();
        let raw = tiny_png();
        let id = store.add_image(&raw).expect("decode");
        // anchor_line == top_abs_line → included (viewport_row = 0)
        store.place(id, 5, 0, 2, 1);
        let v = store.placements_in_view(5, 3);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].viewport_row, 0);

        // anchor_line == top_abs_line + viewport_rows → excluded
        let v2 = store.placements_in_view(5, 1); // bottom = 6, anchor=5 → included
        assert_eq!(v2.len(), 1);
        let v3 = store.placements_in_view(6, 1); // bottom = 7, anchor=5 < 6 → excluded
        assert_eq!(v3.len(), 0);
    }

    #[test]
    fn eviction_respects_byte_cap() {
        // 1×1 RGBA8 = 4 bytes; cap to 10 bytes so the third image forces eviction.
        let mut store = ImageStore::with_max_bytes(10);
        let raw = tiny_png(); // decodes to 1×1 = 4 bytes
        let id1 = store.add_image(&raw).expect("id1");
        let id2 = store.add_image(&raw).expect("id2");
        // 8 bytes total; adding a third (4 bytes) → 12 > 10 → evict id1
        let id3 = store.add_image(&raw).expect("id3");
        assert!(!store.images.contains_key(&id1), "id1 should have been evicted");
        assert!(store.images.contains_key(&id2));
        assert!(store.images.contains_key(&id3));
        assert!(store.total_bytes <= store.max_bytes + 4); // at most cap + one new item
    }

    #[test]
    fn eviction_drops_placements_for_evicted_image() {
        let mut store = ImageStore::with_max_bytes(4); // only one 1×1 image fits
        let raw = tiny_png();
        let id1 = store.add_image(&raw).expect("id1");
        store.place(id1, 0, 0, 2, 1);
        assert_eq!(store.placements.len(), 1);

        // Adding a second image evicts id1, which should remove its placements.
        let _id2 = store.add_image(&raw).expect("id2");
        assert!(!store.images.contains_key(&id1));
        assert_eq!(
            store.placements.len(),
            0,
            "placements for evicted image must be removed"
        );
    }

    #[test]
    fn prune_placements_removes_old_lines() {
        let mut store = ImageStore::new();
        let raw = tiny_png();
        let id = store.add_image(&raw).expect("decode");
        store.place(id, -100, 0, 2, 1);
        store.place(id, 0, 0, 2, 1);
        store.place(id, 50, 0, 2, 1);
        assert_eq!(store.placements.len(), 3);

        // Prune everything before absolute line -10
        store.prune_placements(-10);
        assert_eq!(store.placements.len(), 2, "line -100 should be pruned");
        for p in &store.placements {
            assert!(p.anchor_line >= -10);
        }
    }

    #[test]
    fn add_rgba8_roundtrip() {
        let mut store = ImageStore::new();
        // 2x2 RGBA8.
        let rgba: Vec<u8> = (0u8..16).collect();
        let id = store.add_rgba8(2, 2, rgba.clone()).expect("add_rgba8 must succeed");
        let img = store.get_image(id).expect("image must be present");
        assert_eq!(img.width_px, 2);
        assert_eq!(img.height_px, 2);
        assert_eq!(img.rgba, rgba, "pixel data must be stored verbatim");
    }

    #[test]
    fn add_rgba8_rejects_zero_dims() {
        let mut store = ImageStore::new();
        assert!(store.add_rgba8(0, 1, vec![]).is_none(), "zero width must fail");
        assert!(store.add_rgba8(1, 0, vec![]).is_none(), "zero height must fail");
    }

    #[test]
    fn add_rgba8_rejects_size_mismatch() {
        let mut store = ImageStore::new();
        // Claim 2x2 (16 bytes) but supply only 4.
        assert!(
            store.add_rgba8(2, 2, vec![0u8; 4]).is_none(),
            "size mismatch must fail"
        );
    }

    #[test]
    fn live_image_ids_reflects_stored_images() {
        let mut store = ImageStore::new();
        assert!(store.live_image_ids().is_empty());
        let raw = tiny_png();
        let id = store.add_image(&raw).expect("decode");
        let ids = store.live_image_ids();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], id);
    }
}
