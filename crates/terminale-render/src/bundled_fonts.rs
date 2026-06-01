//! Bundled (embedded) monospace fonts shipped inside the binary.
//!
//! Each entry in [`BUNDLED_FONTS`] embeds a Regular + Bold cut of an
//! open-source typeface via [`include_bytes!`]. Calling
//! [`load_bundled_fonts`] registers all of them into a glyphon
//! `FontSystem` so they are selectable in the font picker on any
//! machine — regardless of what is installed at the OS level.
//!
//! The embedded typefaces and their SPDX license identifiers:
//! - **Ubuntu Mono**     — UFL-1.0
//! - **Source Code Pro** — OFL-1.1
//! - **IBM Plex Mono**   — OFL-1.1
//! - **JetBrains Mono**  — OFL-1.1
//! - **Inconsolata**     — OFL-1.1
//!
//! Full license texts live in `assets/fonts/<slug>/` and are catalogued
//! in `assets/fonts/THIRD-PARTY-FONTS.md`.

/// A single embedded font family (Regular + Bold cuts).
pub struct BundledFont {
    /// Exact family name as registered in the font's name table.
    pub family: &'static str,
    /// Raw TTF/OTF bytes for the Regular cut.
    pub regular: &'static [u8],
    /// Raw TTF/OTF bytes for the Bold cut.
    pub bold: &'static [u8],
}

/// All bundled monospace families. Order determines display order in the
/// font picker when no system fonts override the list.
pub const BUNDLED_FONTS: &[BundledFont] = &[
    BundledFont {
        family: "Ubuntu Mono",
        regular: include_bytes!("../assets/fonts/ubuntu-mono/UbuntuMono-Regular.ttf"),
        bold: include_bytes!("../assets/fonts/ubuntu-mono/UbuntuMono-Bold.ttf"),
    },
    BundledFont {
        family: "Source Code Pro",
        regular: include_bytes!("../assets/fonts/source-code-pro/SourceCodePro-Regular.ttf"),
        bold: include_bytes!("../assets/fonts/source-code-pro/SourceCodePro-Bold.ttf"),
    },
    BundledFont {
        family: "IBM Plex Mono",
        regular: include_bytes!("../assets/fonts/ibm-plex-mono/IBMPlexMono-Regular.ttf"),
        bold: include_bytes!("../assets/fonts/ibm-plex-mono/IBMPlexMono-Bold.ttf"),
    },
    BundledFont {
        family: "JetBrains Mono",
        regular: include_bytes!("../assets/fonts/jetbrains-mono/JetBrainsMono-Regular.ttf"),
        bold: include_bytes!("../assets/fonts/jetbrains-mono/JetBrainsMono-Bold.ttf"),
    },
    BundledFont {
        family: "Inconsolata",
        regular: include_bytes!("../assets/fonts/inconsolata/Inconsolata-Regular.ttf"),
        bold: include_bytes!("../assets/fonts/inconsolata/Inconsolata-Bold.ttf"),
    },
];

/// Returns the family name strings for all bundled fonts.
pub fn bundled_family_names() -> Vec<&'static str> {
    BUNDLED_FONTS.iter().map(|f| f.family).collect()
}

/// Load every bundled font into `font_system`'s database.
///
/// This is idempotent — registering the same bytes a second time is a
/// no-op in fontdb (it deduplicates by content hash).
pub fn load_bundled_fonts(font_system: &mut glyphon::FontSystem) {
    let db = font_system.db_mut();
    for f in BUNDLED_FONTS {
        db.load_font_data(f.regular.to_vec());
        db.load_font_data(f.bold.to_vec());
    }
}
