//! Load and rasterise the bundled `icon.svg` into a `winit::window::Icon`.
//!
//! Both the main and settings windows call into this on startup so the
//! taskbar / alt-tab thumbnail / window-list entry all show the same
//! brand glyph regardless of which secondary window the user is looking
//! at.
//!
//! Rasterisation happens at runtime via `resvg` so we keep a single
//! source of truth (the `assets/icons/icon.svg`) without juggling
//! pre-baked PNGs for each DPI.

use winit::window::Icon;
use winit::window::WindowAttributes;

/// SVG bytes, bundled at compile time so the binary stays self-contained.
const ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/icon.svg");

/// Tag window attributes with the application identity the desktop
/// environment uses to group windows and resolve the launcher icon.
///
/// On Linux this sets the Wayland `app_id` and the X11 `WM_CLASS` (winit
/// stores a single application name used for both). Compositors match it
/// against the `terminale.desktop` entry installed by `desktop_entry.rs`
/// (`StartupWMClass=terminale`) — without it the window has an empty/default
/// identity and GNOME/KDE show a generic gear instead of the brand icon.
/// Every window builder (main, settings, AI, prompts…) must route through
/// this so all windows group under the same dock entry.
///
/// On other platforms this is a no-op: Windows uses the embedded `.ico` +
/// `with_window_icon`, macOS the `.icns` in the app bundle.
pub fn with_app_identity(attrs: WindowAttributes) -> WindowAttributes {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use winit::platform::wayland::WindowAttributesExtWayland;
        // Called via UFCS: the X11 extension trait declares an identical
        // `with_name`, and both write the same underlying field.
        WindowAttributesExtWayland::with_name(attrs, "terminale", "terminale")
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    attrs
}

/// Target side length, in pixels, of the rasterised window icon. 256
/// is what Windows / GNOME / KDE prefer for high-DPI taskbar entries.
const ICON_SIZE_PX: u32 = 256;

/// Cached, rasterised icon ready for [`winit::window::Window::set_window_icon`].
///
/// Returns `None` only if the bundled SVG fails to parse — which would
/// be a bug at build time, not a runtime concern.
#[must_use]
pub fn load_app_icon() -> Option<Icon> {
    let opt = usvg::Options::default();
    let tree = match usvg::Tree::from_data(ICON_SVG, &opt) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(?e, "could not parse bundled icon.svg");
            return None;
        }
    };

    let mut pixmap = tiny_skia::Pixmap::new(ICON_SIZE_PX, ICON_SIZE_PX)?;
    // Scale the SVG's natural size onto our target square while
    // preserving aspect ratio.
    let svg_size = tree.size();
    let scale_x = ICON_SIZE_PX as f32 / svg_size.width();
    let scale_y = ICON_SIZE_PX as f32 / svg_size.height();
    let scale = scale_x.min(scale_y);
    let tx = (ICON_SIZE_PX as f32 - svg_size.width() * scale) * 0.5;
    let ty = (ICON_SIZE_PX as f32 - svg_size.height() * scale) * 0.5;
    // Scale around (0,0), then translate to centre the icon in the
    // target square. `post_translate` runs after the scale.
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // tiny-skia hands us premultiplied BGRA; winit wants straight RGBA.
    let mut rgba: Vec<u8> = Vec::with_capacity((ICON_SIZE_PX * ICON_SIZE_PX * 4) as usize);
    for pixel in pixmap.pixels() {
        let a = pixel.alpha();
        if a == 0 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        // Un-premultiply so winit's icon blends correctly with the OS
        // chrome (Windows DWM expects straight alpha).
        let unmul = |c: u8| -> u8 {
            let v = (u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a);
            v.min(255) as u8
        };
        rgba.push(unmul(pixel.red()));
        rgba.push(unmul(pixel.green()));
        rgba.push(unmul(pixel.blue()));
        rgba.push(a);
    }

    match Icon::from_rgba(rgba, ICON_SIZE_PX, ICON_SIZE_PX) {
        Ok(icon) => Some(icon),
        Err(e) => {
            tracing::warn!(?e, "winit rejected the rasterised icon");
            None
        }
    }
}
