//! UI icon registry — maps each semantic icon to both a Tabler Icons
//! PUA codepoint (bundled subset font, clean outlined line style) and the
//! original emoji/Unicode glyph (legacy fallback for `bundled_icons = false`).
//!
//! # Usage
//! ```ignore
//! let bundled = config.appearance.bundled_icons;
//! ui.label(icons::glyph(&icons::SETTINGS, bundled));
//! ```
//!
//! # Tabler Icons codepoint reference
//! All `bundled` values are single chars from the Tabler Icons PUA range.
//! The subset TTF at `terminale-render/assets/fonts/icons/TablerIcons-subset.ttf`
//! contains exactly these codepoints and nothing else.
//!
//! Codepoints were looked up in the Tabler CSS file:
//! `.ti-<name>:before { content: "\eXXXX"; }`

/// A semantic icon that can be rendered either as a bundled Tabler glyph
/// (clean thin line style) or as the legacy emoji/Unicode representation.
#[derive(Clone, Copy, Debug)]
pub struct Icon {
    /// Single-character string containing the Tabler PUA codepoint.
    pub bundled: &'static str,
    /// The emoji or Unicode character used before the bundled font existed.
    pub legacy: &'static str,
}

/// Return the glyph string for `icon` based on the `bundled` flag.
///
/// When `bundled` is `true` (the default), returns `icon.bundled` — a
/// Tabler Icons PUA character rendered by the subset TTF. When `false`,
/// returns `icon.legacy` — the original emoji used before the icon font.
#[must_use]
#[inline]
pub fn glyph(icon: &Icon, bundled: bool) -> &'static str {
    if bundled {
        icon.bundled
    } else {
        icon.legacy
    }
}

// ── Context menu / actions ────────────────────────────────────────────────────

/// Copy to clipboard — ti-copy U+EA7A
pub const COPY: Icon = Icon {
    bundled: "\u{EA7A}",
    legacy: "\u{1F4CB}",
}; // 📋
/// Paste from clipboard — ti-clipboard U+EA6F
pub const PASTE: Icon = Icon {
    bundled: "\u{EA6F}",
    legacy: "\u{1F4C4}",
}; // 📄
/// Settings / preferences — ti-settings U+EB20
pub const SETTINGS: Icon = Icon {
    bundled: "\u{EB20}",
    legacy: "\u{2699}",
}; // ⚙
/// Close / dismiss / X — ti-x U+EB55
pub const CLOSE: Icon = Icon {
    bundled: "\u{EB55}",
    legacy: "\u{2716}",
}; // ✖
/// Add / new tab — ti-plus U+EB0B
pub const PLUS: Icon = Icon {
    bundled: "\u{EB0B}",
    legacy: "\u{FF0B}",
}; // ＋
/// Split vertically (left|right columns) — ti-layout-columns U+EAD4
pub const SPLIT_V: Icon = Icon {
    bundled: "\u{EAD4}",
    legacy: "\u{25B6}",
}; // ▶
/// Split horizontally (top/bottom rows) — ti-layout-rows U+EAD8
pub const SPLIT_H: Icon = Icon {
    bundled: "\u{EAD8}",
    legacy: "\u{2B07}",
}; // ⬇
/// Rename / pencil — ti-pencil U+EB04
pub const RENAME: Icon = Icon {
    bundled: "\u{EB04}",
    legacy: "\u{1F4DD}",
}; // 📝
/// Pin — ti-pin U+EC9C
pub const PIN: Icon = Icon {
    bundled: "\u{EC9C}",
    legacy: "\u{1F4CC}",
}; // 📌
/// AI / sparkles — ti-sparkles U+F6D7
pub const AI: Icon = Icon {
    bundled: "\u{F6D7}",
    legacy: "\u{2728}",
}; // ✨
/// Reload / refresh — ti-refresh U+EB13
pub const REFRESH: Icon = Icon {
    bundled: "\u{EB13}",
    legacy: "\u{1F504}",
}; // 🔄
/// Delete / trash — ti-trash U+EB41
pub const TRASH: Icon = Icon {
    bundled: "\u{EB41}",
    legacy: "\u{1F5D1}",
}; // 🗑
/// Search / magnify — ti-search U+EB1C
pub const SEARCH: Icon = Icon {
    bundled: "\u{EB1C}",
    legacy: "\u{1F50D}",
}; // 🔍
/// Select all / check box — ti-square-check U+EB28
pub const SELECT_ALL: Icon = Icon {
    bundled: "\u{EB28}",
    legacy: "\u{2611}",
}; // ☑
/// Split pane (generic, used on menu "Split" parent) — ti-layout-columns U+EAD4
pub const SPLIT: Icon = Icon {
    bundled: "\u{EAD4}",
    legacy: "\u{1F500}",
}; // 🔀
/// Position / window frame — ti-window U+EF06
pub const POSITION: Icon = Icon {
    bundled: "\u{EF06}",
    legacy: "\u{1F5BC}",
}; // 🖼
/// Explain selection / lightbulb — ti-bulb U+EA51
pub const BULB: Icon = Icon {
    bundled: "\u{EA51}",
    legacy: "\u{1F4A1}",
}; // 💡
/// Ask AI / message bubble — ti-message U+EAEF
pub const MESSAGE: Icon = Icon {
    bundled: "\u{EAEF}",
    legacy: "\u{1F4AC}",
}; // 💬
/// Check mark (state indicator) — ti-check U+EA5E
pub const CHECK: Icon = Icon {
    bundled: "\u{EA5E}",
    legacy: "\u{2714}",
}; // ✔

// ── Navigation arrows ─────────────────────────────────────────────────────────

/// Arrow up — ti-arrow-up U+EA25
pub const ARROW_UP: Icon = Icon {
    bundled: "\u{EA25}",
    legacy: "\u{2B06}",
}; // ⬆
/// Arrow down — ti-arrow-down U+EA16
pub const ARROW_DOWN: Icon = Icon {
    bundled: "\u{EA16}",
    legacy: "\u{2B07}",
}; // ⬇
/// Arrow left — ti-arrow-left U+EA19
pub const ARROW_LEFT: Icon = Icon {
    bundled: "\u{EA19}",
    legacy: "\u{2B05}",
}; // ⬅
/// Arrow right — ti-arrow-right U+EA1F
pub const ARROW_RIGHT: Icon = Icon {
    bundled: "\u{EA1F}",
    legacy: "\u{27A1}",
}; // ➡
/// Chevron right — ti-chevron-right U+EA61
pub const CHEVRON_RIGHT: Icon = Icon {
    bundled: "\u{EA61}",
    legacy: "\u{25B6}",
}; // ▶
/// Chevron down — ti-chevron-down U+EA5F
pub const CHEVRON_DOWN: Icon = Icon {
    bundled: "\u{EA5F}",
    legacy: "\u{25BC}",
}; // ▼
/// Chevron up — ti-chevron-up U+EA62
pub const CHEVRON_UP: Icon = Icon {
    bundled: "\u{EA62}",
    legacy: "\u{25B2}",
}; // ▲
/// Chevron left — ti-chevron-left U+EA60
pub const CHEVRON_LEFT: Icon = Icon {
    bundled: "\u{EA60}",
    legacy: "\u{25C0}",
}; // ◀
/// Shuffle / random — ti-arrows-shuffle U+F000
pub const SHUFFLE: Icon = Icon {
    bundled: "\u{F000}",
    legacy: "\u{1F500}",
}; // 🔀
/// Target / focus — ti-target U+EB35
pub const TARGET: Icon = Icon {
    bundled: "\u{EB35}",
    legacy: "\u{25CE}",
}; // ◎
/// Square / maximize placeholder — ti-square U+EB2C
pub const SQUARE: Icon = Icon {
    bundled: "\u{EB2C}",
    legacy: "\u{2B1C}",
}; // ⬜

// ── Settings sidebar sections ─────────────────────────────────────────────────

/// Profiles / folder — ti-folder U+EAAD
pub const FOLDER: Icon = Icon {
    bundled: "\u{EAAD}",
    legacy: "\u{1F4C2}",
}; // 📂
/// SSH / globe — ti-world U+EB54
pub const WORLD: Icon = Icon {
    bundled: "\u{EB54}",
    legacy: "\u{1F310}",
}; // 🌐
/// Backup / package — ti-package U+EAFF
pub const PACKAGE: Icon = Icon {
    bundled: "\u{EAFF}",
    legacy: "\u{1F4E6}",
}; // 📦
/// Appearance / palette — ti-palette U+EB01
pub const PALETTE: Icon = Icon {
    bundled: "\u{EB01}",
    legacy: "\u{1F3A8}",
}; // 🎨
/// Font / typography — ti-typography U+EBC5
pub const TYPOGRAPHY: Icon = Icon {
    bundled: "\u{EBC5}",
    legacy: "\u{1F520}",
}; // 🔠
/// Cursor / edit — ti-edit U+EA98
pub const EDIT: Icon = Icon {
    bundled: "\u{EA98}",
    legacy: "\u{1F4DD}",
}; // 📝
/// Terminal — ti-terminal-2 U+EBEF
pub const TERMINAL: Icon = Icon {
    bundled: "\u{EBEF}",
    legacy: "\u{1F5A5}",
}; // 🖥
/// Window / image — ti-photo U+EB0A
pub const PHOTO: Icon = Icon {
    bundled: "\u{EB0A}",
    legacy: "\u{1F5BC}",
}; // 🖼
/// Bell / notifications — ti-bell U+EA35
pub const BELL: Icon = Icon {
    bundled: "\u{EA35}",
    legacy: "\u{1F514}",
}; // 🔔
/// Quake / download-into-screen — ti-download U+EA96
pub const DOWNLOAD: Icon = Icon {
    bundled: "\u{EA96}",
    legacy: "\u{1F4E5}",
}; // 📥
/// Key / shortcuts — ti-key U+EAC7
pub const KEY: Icon = Icon {
    bundled: "\u{EAC7}",
    legacy: "\u{1F511}",
}; // 🔑
/// Plugins / plug — ti-plug U+EBD9
pub const PLUG: Icon = Icon {
    bundled: "\u{EBD9}",
    legacy: "\u{1F50C}",
}; // 🔌
/// GPU / gamepad — ti-device-gamepad U+EB63
pub const GAMEPAD: Icon = Icon {
    bundled: "\u{EB63}",
    legacy: "\u{1F3AE}",
}; // 🎮
/// Status bar / chart — ti-chart-bar U+EA59
pub const CHART_BAR: Icon = Icon {
    bundled: "\u{EA59}",
    legacy: "\u{1F4CA}",
}; // 📊
/// Snippets / clipboard — ti-clipboard U+EA6F (same as PASTE)
pub const CLIPBOARD: Icon = Icon {
    bundled: "\u{EA6F}",
    legacy: "\u{1F4CB}",
}; // 📋
/// Context rules / tags — ti-tags U+EF86 (ti-tag is outside BMP)
pub const TAGS: Icon = Icon {
    bundled: "\u{EF86}",
    legacy: "\u{1F3F7}",
}; // 🏷
/// Workspaces / floppy — ti-device-floppy U+EB62
pub const FLOPPY: Icon = Icon {
    bundled: "\u{EB62}",
    legacy: "\u{1F4BE}",
}; // 💾
/// Directory jump / map — ti-map U+EAE9
pub const MAP: Icon = Icon {
    bundled: "\u{EAE9}",
    legacy: "\u{1F5FA}",
}; // 🗺
/// About / book — ti-book U+EA39
pub const BOOK: Icon = Icon {
    bundled: "\u{EA39}",
    legacy: "\u{1F4D6}",
}; // 📖

// ── Misc UI ───────────────────────────────────────────────────────────────────

/// File — ti-file U+EAA4
pub const FILE: Icon = Icon {
    bundled: "\u{EAA4}",
    legacy: "\u{1F4C4}",
}; // 📄
/// Bookmark — ti-bookmark U+EA3A
pub const BOOKMARK: Icon = Icon {
    bundled: "\u{EA3A}",
    legacy: "\u{1F516}",
}; // 🔖
/// Warning / alert triangle — ti-alert-triangle U+EA06
pub const WARNING: Icon = Icon {
    bundled: "\u{EA06}",
    legacy: "\u{26A0}",
}; // ⚠
/// Circle plus — ti-circle-plus U+EA69
pub const CIRCLE_PLUS: Icon = Icon {
    bundled: "\u{EA69}",
    legacy: "\u{2295}",
}; // ⊕
/// Minus — ti-minus U+EAF2
pub const MINUS: Icon = Icon {
    bundled: "\u{EAF2}",
    legacy: "\u{2212}",
}; // −
/// Info — ti-info-circle U+EAC5
pub const INFO: Icon = Icon {
    bundled: "\u{EAC5}",
    legacy: "\u{2139}",
}; // ℹ
/// Eye (visible) — ti-eye U+EA9A
pub const EYE: Icon = Icon {
    bundled: "\u{EA9A}",
    legacy: "\u{1F441}",
}; // 👁
/// Eye off (hidden) — ti-eye-off U+ECF0
pub const EYE_OFF: Icon = Icon {
    bundled: "\u{ECF0}",
    legacy: "\u{1F648}",
}; // 🙈
/// Lock — ti-lock U+EAE2
pub const LOCK: Icon = Icon {
    bundled: "\u{EAE2}",
    legacy: "\u{1F512}",
}; // 🔒
/// Lock open — ti-lock-open U+EAE1
pub const LOCK_OPEN: Icon = Icon {
    bundled: "\u{EAE1}",
    legacy: "\u{1F513}",
}; // 🔓
/// Sort ascending — ti-sort-ascending U+EB26
pub const SORT_ASC: Icon = Icon {
    bundled: "\u{EB26}",
    legacy: "\u{2191}",
}; // ↑
/// Sort descending — ti-sort-descending U+EB27
pub const SORT_DESC: Icon = Icon {
    bundled: "\u{EB27}",
    legacy: "\u{2193}",
}; // ↓
/// Circle check — ti-circle-check U+EA67
pub const CIRCLE_CHECK: Icon = Icon {
    bundled: "\u{EA67}",
    legacy: "\u{2714}",
}; // ✔
/// Circle X — ti-circle-x U+EA6A
pub const CIRCLE_X: Icon = Icon {
    bundled: "\u{EA6A}",
    legacy: "\u{2716}",
}; // ✖
/// Maximize — ti-maximize U+EAEA
pub const MAXIMIZE: Icon = Icon {
    bundled: "\u{EAEA}",
    legacy: "\u{2B1C}",
}; // ⬜
/// Minimize — ti-minimize U+EAF1
pub const MINIMIZE: Icon = Icon {
    bundled: "\u{EAF1}",
    legacy: "\u{2212}",
}; // −

/// Every icon in the registry — used by the coverage test.
pub const ALL: &[Icon] = &[
    COPY,
    PASTE,
    SETTINGS,
    CLOSE,
    PLUS,
    SPLIT_V,
    SPLIT_H,
    RENAME,
    PIN,
    AI,
    REFRESH,
    TRASH,
    SEARCH,
    SELECT_ALL,
    SPLIT,
    POSITION,
    BULB,
    MESSAGE,
    CHECK,
    ARROW_UP,
    ARROW_DOWN,
    ARROW_LEFT,
    ARROW_RIGHT,
    CHEVRON_RIGHT,
    CHEVRON_DOWN,
    CHEVRON_UP,
    CHEVRON_LEFT,
    SHUFFLE,
    TARGET,
    SQUARE,
    FOLDER,
    WORLD,
    PACKAGE,
    PALETTE,
    TYPOGRAPHY,
    EDIT,
    TERMINAL,
    PHOTO,
    BELL,
    DOWNLOAD,
    KEY,
    PLUG,
    GAMEPAD,
    CHART_BAR,
    CLIPBOARD,
    TAGS,
    FLOPPY,
    MAP,
    BOOK,
    FILE,
    BOOKMARK,
    WARNING,
    CIRCLE_PLUS,
    MINUS,
    INFO,
    EYE,
    EYE_OFF,
    LOCK,
    LOCK_OPEN,
    SORT_ASC,
    SORT_DESC,
    CIRCLE_CHECK,
    CIRCLE_X,
    MAXIMIZE,
    MINIMIZE,
];

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Tabler Icons are allocated in the Unicode PUA range. The webfont uses
    /// the range E000–F8FF (standard BMP PUA) for all icons we subset.
    const TABLER_PUA_START: u32 = 0xE000;
    const TABLER_PUA_END: u32 = 0xF8FF;

    #[test]
    fn every_bundled_icon_is_pua() {
        for icon in ALL {
            let ch = icon
                .bundled
                .chars()
                .next()
                .expect("bundled must be non-empty");
            let cp = ch as u32;
            assert!(
                (TABLER_PUA_START..=TABLER_PUA_END).contains(&cp),
                "bundled codepoint U+{cp:04X} for {:?} is outside Tabler PUA range \
                 (E000..=F8FF)",
                icon.bundled,
            );
            // Each bundled entry is exactly one character.
            assert_eq!(
                icon.bundled.chars().count(),
                1,
                "bundled must be a single char, got {:?}",
                icon.bundled,
            );
        }
    }

    #[test]
    fn glyph_toggle_picks_representation() {
        for icon in ALL {
            assert_eq!(
                glyph(icon, true),
                icon.bundled,
                "glyph(_, true) must return bundled"
            );
            assert_eq!(
                glyph(icon, false),
                icon.legacy,
                "glyph(_, false) must return legacy"
            );
        }
    }

    #[test]
    fn all_legacy_glyphs_nonempty() {
        for icon in ALL {
            assert!(
                !icon.legacy.is_empty(),
                "legacy must be non-empty for {:?}",
                icon.bundled,
            );
        }
    }
}
