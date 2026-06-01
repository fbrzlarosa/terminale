//! Helper to ensure the Hack typeface and Tabler Icons subset font are included
//! in egui's font fallback chains.
//!
//! - **Hack** fills in geometric and arrow icon glyphs (`↑ ↓ ▲ ▼ ⊕ ▐ ●` etc.)
//!   absent from Ubuntu-Light, NotoEmoji, and emoji-icon-font.
//! - **Tabler Icons** (PUA codepoints E000–F8FF) provides the bundled thin
//!   outlined icon set used when `appearance.bundled_icons = true`.
//!
//! Call [`install_icon_font`] once immediately after creating each per-window
//! [`egui::Context`] so that every egui sub-window (context menu, settings,
//! AI assistant, password prompt, paste guard) can render those codepoints
//! without falling back to tofu squares.

/// Key used to register the Tabler Icons subset font in egui's font data map.
pub const TABLER_FONT_KEY: &str = "TablerIcons";

/// Tabler Icons subset font bytes, embedded at compile time.
const TABLER_SUBSET_BYTES: &[u8] =
    include_bytes!("../../terminale-render/assets/fonts/icons/TablerIcons-subset.ttf");

/// Build an [`egui::FontDefinitions`] that extends egui's defaults by:
/// 1. Appending `"Hack"` to the `Proportional` family fallback list (geometric
///    symbols and arrows).
/// 2. Registering the `"TablerIcons"` subset font and appending it to **both**
///    the `Proportional` and `Monospace` family fallback lists so that Tabler
///    PUA codepoints (U+E000–U+F8FF) resolve everywhere in egui.
///
/// egui's default already registers Hack in `font_data` under the key `"Hack"`
/// and lists it first in the `Monospace` family, but omits it from
/// `Proportional`. Appending it there (as a last resort) gives Proportional
/// text the same geometric/arrow glyph coverage Monospace already enjoys,
/// without altering the rendering of any codepoint the existing faces carry.
pub fn icon_font_definitions() -> egui::FontDefinitions {
    let mut defs = egui::FontDefinitions::default();

    // ── Hack → Proportional (geometric/arrow coverage) ───────────────────────
    // "Hack" is the exact key egui / epaint uses for its bundled Hack-Regular
    // typeface (see epaint::text::FontDefinitions::default).
    let hack_key = "Hack".to_owned();

    // Guard: only append if Hack is actually present in font_data (it always
    // should be when the `default_fonts` feature of egui/epaint is enabled,
    // but be defensive so we never push a key that would later fail to resolve).
    if defs.font_data.contains_key(&hack_key) {
        if let Some(proportional) = defs.families.get_mut(&egui::FontFamily::Proportional) {
            if !proportional.iter().any(|name| name == &hack_key) {
                proportional.push(hack_key);
            }
        }
    }

    // ── Tabler Icons → Proportional + Monospace (PUA icon coverage) ──────────
    // Register the subset TTF under TABLER_FONT_KEY, then append it as a
    // last-resort fallback on both families. The Tabler PUA range (E000–F8FF)
    // does not conflict with any standard Unicode text, so fallback order is
    // irrelevant for correctness; appending to the end is the safest choice.
    defs.font_data.insert(
        TABLER_FONT_KEY.to_owned(),
        egui::FontData::from_static(TABLER_SUBSET_BYTES),
    );
    for family in [&egui::FontFamily::Proportional, &egui::FontFamily::Monospace] {
        if let Some(list) = defs.families.get_mut(family) {
            if !list.iter().any(|k| k == TABLER_FONT_KEY) {
                list.push(TABLER_FONT_KEY.to_owned());
            }
        }
    }

    defs
}

/// Configure `ctx` so that its `Proportional` font family includes Hack as a
/// last-resort fallback, eliminating tofu for geometric/arrow icon glyphs.
///
/// Must be called once right after `egui::Context::default()` and before the
/// first frame is rendered.
pub fn install_icon_font(ctx: &egui::Context) {
    ctx.set_fonts(icon_font_definitions());
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_font_definitions_adds_hack_to_proportional() {
        let defs = icon_font_definitions();

        // Proportional must contain "Hack" after the call.
        let proportional = defs
            .families
            .get(&egui::FontFamily::Proportional)
            .expect("Proportional family must exist");
        assert!(
            proportional.iter().any(|n| n == "Hack"),
            "Hack should appear in the Proportional fallback chain; got: {proportional:?}"
        );

        // Monospace must still contain "Hack" (egui default; we must not have
        // removed it).
        let monospace = defs
            .families
            .get(&egui::FontFamily::Monospace)
            .expect("Monospace family must exist");
        assert!(
            monospace.iter().any(|n| n == "Hack"),
            "Hack must remain in the Monospace fallback chain; got: {monospace:?}"
        );

        // The Hack font data must be present in the definitions so the key can
        // actually resolve to bytes.
        assert!(
            defs.font_data.contains_key("Hack"),
            "Hack font data must be present in FontDefinitions"
        );
    }

    #[test]
    fn icon_font_definitions_registers_tabler_in_both_families() {
        let defs = icon_font_definitions();

        // The Tabler subset bytes must be registered.
        assert!(
            defs.font_data.contains_key(TABLER_FONT_KEY),
            "TablerIcons font data must be present in FontDefinitions"
        );

        // Must appear in Proportional fallback so egui labels can render PUA icons.
        let proportional = defs
            .families
            .get(&egui::FontFamily::Proportional)
            .expect("Proportional family must exist");
        assert!(
            proportional.iter().any(|n| n == TABLER_FONT_KEY),
            "TablerIcons must appear in the Proportional fallback chain; got: {proportional:?}"
        );

        // Must also appear in Monospace fallback (terminal text, input widgets).
        let monospace = defs
            .families
            .get(&egui::FontFamily::Monospace)
            .expect("Monospace family must exist");
        assert!(
            monospace.iter().any(|n| n == TABLER_FONT_KEY),
            "TablerIcons must appear in the Monospace fallback chain; got: {monospace:?}"
        );
    }

    #[test]
    fn tabler_subset_bytes_valid_font_signature() {
        // A valid TTF/OTF starts with a version tag. The Tabler subset is a
        // TrueType font, which starts with 0x00 0x01 0x00 0x00 ('sfversion=1').
        // (CFF/OTF would start with 'OTTO'.) Accept either signature.
        let is_truetype = TABLER_SUBSET_BYTES.starts_with(&[0x00, 0x01, 0x00, 0x00]);
        let is_opentype = TABLER_SUBSET_BYTES.starts_with(b"OTTO");
        assert!(
            is_truetype || is_opentype,
            "Tabler subset bytes must start with a valid TTF/OTF signature; len={}",
            TABLER_SUBSET_BYTES.len()
        );
    }
}
