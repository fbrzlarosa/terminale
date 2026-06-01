//! Visual appearance — theme selection, tab sizing, dividers, pane headers.

use crate::theme::{builtin_themes, ResolvedTheme, Theme};
use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How a background image is scaled to fill the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BgImageFit {
    /// Uniformly scale the image up (never down) so it covers the full
    /// window — the same as CSS `object-fit: cover`. May crop edges.
    Fill,
    /// Uniformly scale the image so it fits entirely inside the window —
    /// the same as CSS `object-fit: contain`. May show letterboxing.
    Fit,
    /// Non-uniformly stretch the image to exactly fill the window.
    Stretch,
    /// Do not scale; center the image at its natural size.
    Center,
    /// Tile the image repeatedly across the window.
    Tile,
}

impl Default for BgImageFit {
    fn default() -> Self {
        Self::Fill
    }
}

impl BgImageFit {
    /// All variants in display order — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 5] {
        [
            Self::Fill,
            Self::Fit,
            Self::Stretch,
            Self::Center,
            Self::Tile,
        ]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Fill => "Fill (cover)",
            Self::Fit => "Fit (contain)",
            Self::Stretch => "Stretch",
            Self::Center => "Center",
            Self::Tile => "Tile",
        }
    }
}

/// Background image settings. The image is drawn behind the terminal grid
/// but above the plain window background colour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct BackgroundImageConfig {
    /// Absolute path to the image file (PNG, JPEG, WebP, or GIF).
    /// `None` = no background image (feature disabled).
    pub path: Option<String>,
    /// Opacity of the image in `0.0..=1.0`. `1.0` = fully opaque.
    pub opacity: f32,
    /// How the image is scaled / positioned to fill the window.
    pub fit: BgImageFit,
    /// Brightness multiplier in `0.0..=2.0`. `1.0` = unchanged.
    pub brightness: f32,
    /// Saturation multiplier in `0.0..=2.0`. `1.0` = unchanged.
    pub saturation: f32,
    /// Hue rotation in degrees (`0.0..360.0`). `0.0` = unchanged.
    pub hue: f32,
}

impl Default for BackgroundImageConfig {
    fn default() -> Self {
        Self {
            path: None,
            opacity: 1.0,
            fit: BgImageFit::Fill,
            brightness: 1.0,
            saturation: 1.0,
            hue: 0.0,
        }
    }
}

impl BackgroundImageConfig {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if !(0.0..=1.0).contains(&self.opacity) {
            return Err(ConfigError::Invalid {
                field: "appearance.background_image.opacity",
                message: "must be between 0.0 and 1.0",
            });
        }
        if !(0.0..=2.0).contains(&self.brightness) {
            return Err(ConfigError::Invalid {
                field: "appearance.background_image.brightness",
                message: "must be between 0.0 and 2.0",
            });
        }
        if !(0.0..=2.0).contains(&self.saturation) {
            return Err(ConfigError::Invalid {
                field: "appearance.background_image.saturation",
                message: "must be between 0.0 and 2.0",
            });
        }
        if !(0.0..=360.0).contains(&self.hue) {
            return Err(ConfigError::Invalid {
                field: "appearance.background_image.hue",
                message: "must be between 0.0 and 360.0",
            });
        }
        Ok(())
    }
}

/// Visual style for the close-X buttons on tab chips and pane headers.
///
/// `Chip` (default) draws a small filled square behind the X strokes — mirrors
/// the project's square-corner aesthetic and makes the hit area visually
/// obvious. `Bare` draws only the X strokes with no background chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloseButtonStyle {
    /// A small filled square chip drawn behind the X strokes (default).
    Chip,
    /// Only the vector X strokes — no chip background.
    Bare,
}

impl Default for CloseButtonStyle {
    fn default() -> Self {
        Self::Chip
    }
}

impl CloseButtonStyle {
    /// All variants in display order — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 2] {
        [Self::Chip, Self::Bare]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Chip => "Chip (filled square)",
            Self::Bare => "Bare (strokes only)",
        }
    }
}

/// Where the tab bar is rendered relative to the terminal body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TabBarPosition {
    /// Tab bar drawn above the terminal grid (default).
    Top,
    /// Tab bar drawn below the terminal grid.
    Bottom,
    /// Vertical tab strip on the left side of the window.
    Left,
    /// Vertical tab strip on the right side of the window.
    Right,
}

impl Default for TabBarPosition {
    fn default() -> Self {
        Self::Top
    }
}

impl TabBarPosition {
    /// All variants in display order — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 4] {
        [Self::Top, Self::Bottom, Self::Left, Self::Right]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::Left => "Left (vertical)",
            Self::Right => "Right (vertical)",
        }
    }

    /// Returns `true` when the tab bar occupies a horizontal side strip
    /// (`Left` or `Right`) rather than the top or bottom edge.
    #[must_use]
    pub fn is_vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// Theme / palette settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AppearanceConfig {
    /// Name of the active theme (must match a builtin or a user-defined theme).
    pub theme: String,
    /// User-defined themes (inline TOML). Built-in presets are merged in when
    /// resolving. Drop-in `.toml` files in `themes_dir` are loaded separately
    /// at startup and appended after these inline entries.
    pub themes: Vec<Theme>,
    /// Directory scanned at startup for drop-in theme TOML files.
    ///
    /// Each `*.toml` file is parsed as a [`Theme`] and appended to the
    /// available-theme list after built-ins and inline `themes`. Files that
    /// fail to parse are skipped (warning logged). A drop-in whose `name`
    /// collides with a built-in or an inline theme is also skipped (the
    /// existing definition wins).
    ///
    /// `None` = use the OS-standard default (`config_dir/themes`).
    /// Set to an empty string to disable directory scanning entirely.
    #[schemars(with = "Option<String>")]
    pub themes_dir: Option<PathBuf>,
    /// Lower bound (logical px) a tab shrinks to when many tabs share the
    /// bar. Must be `> 0` and `<= tab_max_width`.
    pub tab_min_width: f32,
    /// Upper bound (logical px) a tab grows to for a long label. Must be
    /// `>= tab_min_width`.
    pub tab_max_width: f32,
    /// When `true`, dragging a tab shows a Chrome-style animated ghost that
    /// follows the cursor plus a drop indicator in the target tab bar, and
    /// the new window only materialises on release. When `false`, the drag
    /// still reorders / attaches / tears out on release, but without the
    /// floating ghost and indicator (a plainer, lower-overhead drag).
    pub animated_tab_drag: bool,
    /// Visible stroke thickness of the divider drawn between split panes,
    /// in logical pixels. Also acts as the inner span of the grab band —
    /// the hit-test grows by [`Self::divider_grab_padding_logical`] on each
    /// side so a user doesn't need pixel precision to start a drag.
    /// Range: `1.0..=12.0`. Default: `4.0`.
    pub divider_thickness_logical: f32,
    /// Extra logical pixels on each side of the visible divider stroke that
    /// still count as a hit when hit-testing for a drag start. Total grab
    /// band width is `divider_thickness_logical + 2 * divider_grab_padding_logical`.
    /// Range: `0.0..=20.0`. Default: `3.0`.
    pub divider_grab_padding_logical: f32,
    /// Override colour for the divider stroke. `None` lets the renderer pick
    /// a neutral background-derived tone that matches the focus accent at
    /// reduced alpha.
    #[schemars(with = "Option<String>")]
    pub divider_color: Option<[u8; 3]>,
    /// Visible stroke thickness of the focus border drawn around the focused
    /// split pane, in logical pixels. Set to `0.0` to disable the indicator
    /// entirely. Range: `0.0..=8.0`. Default: `2.0`.
    pub focus_border_thickness_logical: f32,
    /// Override colour for the focus-border stroke. `None` falls back to the
    /// built-in accent colour `[0x7d, 0xa6, 0xff]`.
    #[schemars(with = "Option<String>")]
    pub focus_border_color: Option<[u8; 3]>,
    /// When `true` and a tab holds more than one pane, each pane shows a
    /// 22 px header strip with its title and a close X. Set `false` to
    /// reclaim the vertical space.
    pub show_pane_headers: bool,
    /// Whether the tab bar is shown at all. When `false` the tab bar is hidden
    /// completely and its space is reclaimed for the terminal grid.
    /// Default: `true`.
    pub tab_bar_enabled: bool,
    /// Position of the tab bar relative to the terminal grid.
    /// `Top` = above the grid (default), `Bottom` = below the grid.
    pub tab_bar_position: TabBarPosition,
    /// When `true`, the tab bar is hidden automatically when there is exactly
    /// one tab open, and reappears as soon as a second tab is opened.
    /// Default: `false`.
    pub tab_bar_hide_if_single: bool,
    /// Background image drawn behind the terminal grid.
    /// Disabled by default (`path = None`).
    pub background_image: BackgroundImageConfig,
    /// Visual style for the close-X buttons on tab chips and pane-header
    /// strips. `Chip` (default) draws a small filled square behind the X
    /// strokes; `Bare` draws only the X strokes with no chip.
    pub close_button_style: CloseButtonStyle,
    /// When `true`, holding a pane header and dragging it out of its tab
    /// lifts that pane into a new tab (drop on a tab bar) or a new window
    /// (drop outside all windows). Requires `show_pane_headers = true`.
    /// Has no effect when a tab contains only one pane.
    /// Default: `true`.
    pub pane_tear_out: bool,
    /// Width of the vertical tab strip in logical pixels, used when
    /// `tab_bar_position` is `Left` or `Right`. Range: `120..=360`.
    /// Default: `180`.
    pub vertical_tab_bar_width: f32,
    /// Controls how strongly SGR 2 (faint/dim) text is darkened toward the
    /// cell background colour. `0.0` = no dimming; `1.0` = fully blended into
    /// the background (text invisible); `0.5` = halfway (default). Range: `0.0..=1.0`.
    pub dim_amount: f32,
    /// Minimum WCAG contrast ratio enforced between a cell's foreground and
    /// its background. When a pair falls below this threshold the foreground
    /// is nudged lighter or darker until the ratio is met.
    ///
    /// `1.0` (default) = feature disabled — all colours are rendered as-is.
    /// `4.5` matches WCAG AA; `7.0` matches WCAG AAA. Valid range: `1.0..=21.0`.
    ///
    /// Cells where `fg == bg` (SGR 8 concealed text) are always skipped.
    pub minimum_contrast: f32,
    /// Alpha of a translucent black overlay drawn over inactive (non-focused)
    /// panes when a tab contains more than one pane. `0.0` = no dimming
    /// (feature off, default); higher values darken non-focused panes so the
    /// active one stands out. Range: `0.0..=0.9`.
    pub inactive_pane_dim: f32,
    /// Alpha of a translucent black overlay drawn over the entire terminal
    /// grid when the window loses OS focus. `0.0` = no dimming (feature off,
    /// default); higher values reduce brightness while unfocused so the window
    /// reads as "in the background". Range: `0.0..=0.9`.
    pub unfocused_window_dim: f32,
    /// Fixed width in logical pixels rendered for pinned (compact) tab chips.
    /// Pinned tabs always sit at the leading edge of the bar and show only the
    /// icon glyph (no label text). Range: `24..=120`. Default: `44`.
    pub pinned_tab_width: f32,
    /// When `true` (default), box-drawing (U+2500–U+257F) and block-element
    /// (U+2580–U+259F) characters are rendered as crisp procedural quads
    /// aligned to the cell grid, eliminating font-glyph seams in TUI boxes,
    /// tables, and progress bars. When `false`, the font glyph is used for
    /// every character (original behaviour).
    ///
    /// Note: diagonal separators in the private-use "Powerline" range are
    /// axis-aligned and not covered here; they always fall back to the font.
    pub builtin_box_drawing: bool,
    /// When `true` (default), the small group label text is drawn at the start
    /// of each consecutive run of tabs that belong to the same named group.
    /// The colour-accent stripe is always shown regardless of this setting.
    /// Default: `true`.
    pub show_tab_group_labels: bool,
    /// Auto-cycle colour palette used when assigning a colour to a new tab
    /// group. Each entry is an `[R, G, B]` triple. The nth group picks
    /// `palette[n % len]`. Must contain at least one colour.
    #[schemars(with = "Vec<[u8; 3]>")]
    pub tab_group_colors: Vec<[u8; 3]>,
    /// When `true` (default), the UI renders icons using the bundled Tabler
    /// Icons subset font — clean, thin outlined line icons consistent with
    /// modern terminal aesthetics. When `false`, the classic emoji / Unicode
    /// glyphs used before the icon font was added are shown instead.
    pub bundled_icons: bool,
    /// When `true` (default), a braille-dots animated spinner is prepended to
    /// the tab label (and to each pane header when split) while the pane has a
    /// command running or received output recently. Set `false` to disable the
    /// animation entirely. Live-applied from the Settings window.
    pub tab_activity_spinner: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: "Tokyo Night".into(),
            themes: Vec::new(),
            themes_dir: None,
            // Mirror the renderer's historical TAB_MIN_WIDTH / TAB_MAX_WIDTH
            // constants so the default layout is unchanged.
            tab_min_width: 90.0,
            tab_max_width: 260.0,
            animated_tab_drag: true,
            divider_thickness_logical: 4.0,
            divider_grab_padding_logical: 3.0,
            divider_color: None,
            focus_border_thickness_logical: 2.0,
            focus_border_color: None,
            show_pane_headers: true,
            tab_bar_enabled: true,
            tab_bar_position: TabBarPosition::Top,
            tab_bar_hide_if_single: false,
            background_image: BackgroundImageConfig::default(),
            close_button_style: CloseButtonStyle::default(),
            pane_tear_out: true,
            vertical_tab_bar_width: 180.0,
            dim_amount: 0.5,
            minimum_contrast: 1.0,
            inactive_pane_dim: 0.0,
            unfocused_window_dim: 0.0,
            pinned_tab_width: 44.0,
            builtin_box_drawing: true,
            show_tab_group_labels: true,
            bundled_icons: true,
            tab_group_colors: vec![
                [0x4e, 0xa8, 0xff],
                [0x4e, 0xd4, 0x84],
                [0xff, 0xa0, 0x3c],
                [0xff, 0x6b, 0x8a],
                [0xc0, 0x80, 0xff],
                [0x40, 0xd0, 0xd0],
                [0xff, 0xd0, 0x40],
                [0xff, 0x70, 0xd0],
            ],
            tab_activity_spinner: true,
        }
    }
}

/// Scan `dir` for `*.toml` files, parse each as a [`Theme`], and return
/// the successfully-loaded themes. Files that fail to parse are silently
/// skipped (a warning is emitted via `tracing`). The returned list is in
/// file-system order (no guaranteed sort).
///
/// Returns an empty `Vec` when `dir` does not exist or cannot be read.
#[must_use]
pub fn scan_themes_dir(dir: &std::path::Path) -> Vec<Theme> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Theme>(&text) {
                Ok(t) => out.push(t),
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "failed to parse drop-in theme file — skipping"
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to read drop-in theme file — skipping"
                );
            }
        }
    }
    out
}

impl AppearanceConfig {
    /// Resolve the effective themes directory: the configured override (if any)
    /// or the OS-standard default (`config_dir/themes`).
    ///
    /// Returns `None` when the path is explicitly set to an empty string
    /// (opt-out of directory scanning) or when no home directory is available.
    #[must_use]
    pub fn effective_themes_dir(&self) -> Option<PathBuf> {
        match &self.themes_dir {
            Some(p) if p.as_os_str().is_empty() => None,
            Some(p) => Some(p.clone()),
            None => crate::paths::themes_dir(),
        }
    }

    /// All themes available to the user — built-ins followed by inline
    /// user-defined themes, then drop-ins loaded from `themes_dir`.
    ///
    /// Deduplication is by name: built-ins win over inline entries, which win
    /// over drop-ins. Within each tier, the first occurrence wins.
    #[must_use]
    pub fn all_themes(&self) -> Vec<Theme> {
        let mut all = builtin_themes();
        // Collect existing names so we can deduplicate as we extend.
        let mut seen: std::collections::HashSet<String> =
            all.iter().map(|t| t.name.clone()).collect();
        // Inline user themes.
        for t in &self.themes {
            if seen.insert(t.name.clone()) {
                all.push(t.clone());
            }
        }
        // Drop-in themes from the themes directory.
        if let Some(dir) = self.effective_themes_dir() {
            for t in scan_themes_dir(&dir) {
                if seen.insert(t.name.clone()) {
                    all.push(t);
                }
            }
        }
        all
    }

    /// Resolve the active theme, falling back to "Tokyo Night" if missing.
    #[must_use]
    pub fn resolved(&self) -> ResolvedTheme {
        for theme in self.all_themes() {
            if theme.name == self.theme {
                return theme.resolved();
            }
        }
        builtin_themes()[0].resolved()
    }

    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if !(16.0..=800.0).contains(&self.tab_min_width) {
            return Err(ConfigError::Invalid {
                field: "appearance.tab_min_width",
                message: "must be between 16.0 and 800.0",
            });
        }
        if !(16.0..=800.0).contains(&self.tab_max_width) {
            return Err(ConfigError::Invalid {
                field: "appearance.tab_max_width",
                message: "must be between 16.0 and 800.0",
            });
        }
        if self.tab_max_width < self.tab_min_width {
            return Err(ConfigError::Invalid {
                field: "appearance.tab_max_width",
                message: "must be >= tab_min_width",
            });
        }
        if !(1.0..=12.0).contains(&self.divider_thickness_logical) {
            return Err(ConfigError::Invalid {
                field: "appearance.divider_thickness_logical",
                message: "must be between 1.0 and 12.0",
            });
        }
        if !(0.0..=20.0).contains(&self.divider_grab_padding_logical) {
            return Err(ConfigError::Invalid {
                field: "appearance.divider_grab_padding_logical",
                message: "must be between 0.0 and 20.0",
            });
        }
        if !(0.0..=8.0).contains(&self.focus_border_thickness_logical) {
            return Err(ConfigError::Invalid {
                field: "appearance.focus_border_thickness_logical",
                message: "must be between 0.0 and 8.0",
            });
        }
        if !(120.0..=360.0).contains(&self.vertical_tab_bar_width) {
            return Err(ConfigError::Invalid {
                field: "appearance.vertical_tab_bar_width",
                message: "must be between 120.0 and 360.0",
            });
        }
        if !(0.0..=1.0).contains(&self.dim_amount) {
            return Err(ConfigError::Invalid {
                field: "appearance.dim_amount",
                message: "must be between 0.0 and 1.0",
            });
        }
        if !(1.0..=21.0).contains(&self.minimum_contrast) {
            return Err(ConfigError::Invalid {
                field: "appearance.minimum_contrast",
                message: "must be between 1.0 and 21.0",
            });
        }
        if !(0.0..=0.9).contains(&self.inactive_pane_dim) {
            return Err(ConfigError::Invalid {
                field: "appearance.inactive_pane_dim",
                message: "must be between 0.0 and 0.9",
            });
        }
        if !(0.0..=0.9).contains(&self.unfocused_window_dim) {
            return Err(ConfigError::Invalid {
                field: "appearance.unfocused_window_dim",
                message: "must be between 0.0 and 0.9",
            });
        }
        if !(24.0..=120.0).contains(&self.pinned_tab_width) {
            return Err(ConfigError::Invalid {
                field: "appearance.pinned_tab_width",
                message: "must be between 24.0 and 120.0",
            });
        }
        if self.tab_group_colors.is_empty() {
            return Err(ConfigError::Invalid {
                field: "appearance.tab_group_colors",
                message: "must contain at least one colour",
            });
        }
        self.background_image.validate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_defaults_valid() {
        let cfg = AppearanceConfig::default();
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.tab_bar_position, TabBarPosition::Top);
        assert!(cfg.tab_bar_enabled);
        assert!(!cfg.tab_bar_hide_if_single);
    }

    #[test]
    fn tab_bar_position_roundtrip() {
        let json = serde_json::to_string(&TabBarPosition::Bottom).unwrap();
        let parsed: TabBarPosition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, TabBarPosition::Bottom);
    }

    #[test]
    fn close_button_style_default_is_chip() {
        assert_eq!(CloseButtonStyle::default(), CloseButtonStyle::Chip);
    }

    #[test]
    fn close_button_style_roundtrip() {
        let json = serde_json::to_string(&CloseButtonStyle::Bare).unwrap();
        let parsed: CloseButtonStyle = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, CloseButtonStyle::Bare);

        let json2 = serde_json::to_string(&CloseButtonStyle::Chip).unwrap();
        let parsed2: CloseButtonStyle = serde_json::from_str(&json2).unwrap();
        assert_eq!(parsed2, CloseButtonStyle::Chip);
    }

    #[test]
    fn appearance_defaults_include_close_button_style() {
        let cfg = AppearanceConfig::default();
        assert_eq!(cfg.close_button_style, CloseButtonStyle::Chip);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn pane_tear_out_default_is_true() {
        let cfg = AppearanceConfig::default();
        assert!(cfg.pane_tear_out, "pane_tear_out must default to true");
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn pane_tear_out_roundtrip() {
        let cfg = AppearanceConfig {
            pane_tear_out: false,
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert!(!parsed.pane_tear_out);
    }

    #[test]
    fn tab_bar_enabled_false_roundtrip() {
        let cfg = AppearanceConfig {
            tab_bar_enabled: false,
            tab_bar_hide_if_single: true,
            tab_bar_position: TabBarPosition::Bottom,
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert!(!parsed.tab_bar_enabled);
        assert!(parsed.tab_bar_hide_if_single);
        assert_eq!(parsed.tab_bar_position, TabBarPosition::Bottom);
    }

    #[test]
    fn tab_bar_position_left_right_roundtrip() {
        for pos in [TabBarPosition::Left, TabBarPosition::Right] {
            let json = serde_json::to_string(&pos).unwrap();
            let parsed: TabBarPosition = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, pos);
        }
    }

    #[test]
    fn tab_bar_position_is_vertical() {
        assert!(TabBarPosition::Left.is_vertical());
        assert!(TabBarPosition::Right.is_vertical());
        assert!(!TabBarPosition::Top.is_vertical());
        assert!(!TabBarPosition::Bottom.is_vertical());
    }

    #[test]
    fn vertical_tab_bar_width_default_valid() {
        let cfg = AppearanceConfig::default();
        assert!((cfg.vertical_tab_bar_width - 180.0).abs() < f32::EPSILON);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn vertical_tab_bar_width_validation() {
        let bad_low = AppearanceConfig {
            vertical_tab_bar_width: 50.0,
            ..Default::default()
        };
        assert!(bad_low.validate().is_err());
        let bad_high = AppearanceConfig {
            vertical_tab_bar_width: 400.0,
            ..Default::default()
        };
        assert!(bad_high.validate().is_err());
        let good = AppearanceConfig {
            vertical_tab_bar_width: 200.0,
            ..Default::default()
        };
        assert!(good.validate().is_ok());
    }

    #[test]
    fn vertical_tab_bar_width_roundtrip() {
        let cfg = AppearanceConfig {
            tab_bar_position: TabBarPosition::Left,
            vertical_tab_bar_width: 220.0,
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.tab_bar_position, TabBarPosition::Left);
        assert!((parsed.vertical_tab_bar_width - 220.0).abs() < f32::EPSILON);
    }

    // ── dim_amount ─────────────────────────────────────────────────────────

    #[test]
    fn dim_amount_default_is_half() {
        let cfg = AppearanceConfig::default();
        assert!(
            (cfg.dim_amount - 0.5).abs() < f32::EPSILON,
            "dim_amount default must be 0.5"
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn dim_amount_bounds_validate() {
        // Below 0.0
        let bad_low = AppearanceConfig {
            dim_amount: -0.1,
            ..Default::default()
        };
        assert!(bad_low.validate().is_err(), "-0.1 must be rejected");
        // Above 1.0
        let bad_high = AppearanceConfig {
            dim_amount: 1.1,
            ..Default::default()
        };
        assert!(bad_high.validate().is_err(), "1.1 must be rejected");
        // Boundary values pass
        let zero = AppearanceConfig {
            dim_amount: 0.0,
            ..Default::default()
        };
        assert!(zero.validate().is_ok(), "0.0 must be accepted");
        let one = AppearanceConfig {
            dim_amount: 1.0,
            ..Default::default()
        };
        assert!(one.validate().is_ok(), "1.0 must be accepted");
    }

    #[test]
    fn dim_amount_roundtrips() {
        let cfg = AppearanceConfig {
            dim_amount: 0.3,
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert!(
            (parsed.dim_amount - 0.3).abs() < 1e-5,
            "dim_amount must survive a TOML roundtrip"
        );
        parsed
            .validate()
            .expect("roundtripped config must validate");
    }

    // ── minimum_contrast ───────────────────────────────────────────────────

    #[test]
    fn minimum_contrast_default_is_one() {
        let cfg = AppearanceConfig::default();
        assert!(
            (cfg.minimum_contrast - 1.0).abs() < f32::EPSILON,
            "minimum_contrast default must be 1.0 (feature disabled)"
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn minimum_contrast_bounds_validate() {
        // Below 1.0 is invalid.
        let bad_low = AppearanceConfig {
            minimum_contrast: 0.5,
            ..Default::default()
        };
        assert!(bad_low.validate().is_err(), "0.5 must be rejected");
        // Above 21.0 is invalid.
        let bad_high = AppearanceConfig {
            minimum_contrast: 22.0,
            ..Default::default()
        };
        assert!(bad_high.validate().is_err(), "22.0 must be rejected");
        // Boundary values pass.
        let at_one = AppearanceConfig {
            minimum_contrast: 1.0,
            ..Default::default()
        };
        assert!(at_one.validate().is_ok(), "1.0 must be accepted");
        let at_max = AppearanceConfig {
            minimum_contrast: 21.0,
            ..Default::default()
        };
        assert!(at_max.validate().is_ok(), "21.0 must be accepted");
    }

    #[test]
    fn minimum_contrast_roundtrips_toml() {
        let cfg = AppearanceConfig {
            minimum_contrast: 4.5,
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert!(
            (parsed.minimum_contrast - 4.5).abs() < 1e-4,
            "minimum_contrast must survive a TOML roundtrip"
        );
        parsed
            .validate()
            .expect("roundtripped config must validate");
    }

    // ── inactive_pane_dim ─────────────────────────────────────────────────────

    #[test]
    fn inactive_pane_dim_default_is_zero() {
        let cfg = AppearanceConfig::default();
        assert!(
            cfg.inactive_pane_dim.abs() < f32::EPSILON,
            "inactive_pane_dim default must be 0.0 (off)"
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn inactive_pane_dim_bounds_validate() {
        let bad_low = AppearanceConfig {
            inactive_pane_dim: -0.1,
            ..Default::default()
        };
        assert!(bad_low.validate().is_err(), "-0.1 must be rejected");
        let bad_high = AppearanceConfig {
            inactive_pane_dim: 0.95,
            ..Default::default()
        };
        assert!(
            bad_high.validate().is_err(),
            "0.95 must be rejected (> 0.9)"
        );
        let zero = AppearanceConfig {
            inactive_pane_dim: 0.0,
            ..Default::default()
        };
        assert!(zero.validate().is_ok(), "0.0 must be accepted");
        let max = AppearanceConfig {
            inactive_pane_dim: 0.9,
            ..Default::default()
        };
        assert!(max.validate().is_ok(), "0.9 must be accepted");
    }

    #[test]
    fn inactive_pane_dim_roundtrips_toml() {
        let cfg = AppearanceConfig {
            inactive_pane_dim: 0.4,
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert!(
            (parsed.inactive_pane_dim - 0.4).abs() < 1e-5,
            "inactive_pane_dim must survive a TOML roundtrip"
        );
        parsed
            .validate()
            .expect("roundtripped config must validate");
    }

    // ── unfocused_window_dim ──────────────────────────────────────────────────

    #[test]
    fn unfocused_window_dim_default_is_zero() {
        let cfg = AppearanceConfig::default();
        assert!(
            cfg.unfocused_window_dim.abs() < f32::EPSILON,
            "unfocused_window_dim default must be 0.0 (off)"
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn unfocused_window_dim_bounds_validate() {
        let bad_low = AppearanceConfig {
            unfocused_window_dim: -0.1,
            ..Default::default()
        };
        assert!(bad_low.validate().is_err(), "-0.1 must be rejected");
        let bad_high = AppearanceConfig {
            unfocused_window_dim: 1.0,
            ..Default::default()
        };
        assert!(bad_high.validate().is_err(), "1.0 must be rejected (> 0.9)");
        let zero = AppearanceConfig {
            unfocused_window_dim: 0.0,
            ..Default::default()
        };
        assert!(zero.validate().is_ok(), "0.0 must be accepted");
        let max = AppearanceConfig {
            unfocused_window_dim: 0.9,
            ..Default::default()
        };
        assert!(max.validate().is_ok(), "0.9 must be accepted");
    }

    #[test]
    fn unfocused_window_dim_roundtrips_toml() {
        let cfg = AppearanceConfig {
            unfocused_window_dim: 0.35,
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert!(
            (parsed.unfocused_window_dim - 0.35).abs() < 1e-5,
            "unfocused_window_dim must survive a TOML roundtrip"
        );
        parsed
            .validate()
            .expect("roundtripped config must validate");
    }

    /// Pure helper: compute the alpha of the dim overlay for a given pane
    /// focus state, pane count, and dim level.
    fn dim_alpha_for_pane(focused: bool, pane_count: usize, dim: f32) -> f32 {
        if focused || pane_count <= 1 || dim < 0.01 {
            0.0
        } else {
            dim
        }
    }

    #[test]
    fn dim_alpha_for_inactive_pane_logic() {
        // No dim when focused.
        assert!(
            (dim_alpha_for_pane(true, 3, 0.4) - 0.0).abs() < f32::EPSILON,
            "focused pane must not be dimmed"
        );
        // No dim when only one pane.
        assert!(
            (dim_alpha_for_pane(false, 1, 0.4) - 0.0).abs() < f32::EPSILON,
            "single-pane tab must not produce a dim overlay"
        );
        // No dim when value is zero.
        assert!(
            (dim_alpha_for_pane(false, 2, 0.0) - 0.0).abs() < f32::EPSILON,
            "dim=0.0 must produce no overlay"
        );
        // Dim applied to unfocused pane in a multi-pane tab.
        assert!(
            (dim_alpha_for_pane(false, 2, 0.4) - 0.4).abs() < f32::EPSILON,
            "unfocused pane in multi-pane tab must get the configured dim alpha"
        );
    }

    // ── pinned_tab_width ──────────────────────────────────────────────────────

    #[test]
    fn pinned_tab_width_default_is_44() {
        let cfg = AppearanceConfig::default();
        assert!(
            (cfg.pinned_tab_width - 44.0).abs() < f32::EPSILON,
            "pinned_tab_width default must be 44.0"
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn pinned_tab_width_bounds_validate() {
        let bad_low = AppearanceConfig {
            pinned_tab_width: 10.0,
            ..Default::default()
        };
        assert!(bad_low.validate().is_err(), "10.0 must be rejected");
        let bad_high = AppearanceConfig {
            pinned_tab_width: 200.0,
            ..Default::default()
        };
        assert!(bad_high.validate().is_err(), "200.0 must be rejected");
        let at_min = AppearanceConfig {
            pinned_tab_width: 24.0,
            ..Default::default()
        };
        assert!(at_min.validate().is_ok(), "24.0 must be accepted");
        let at_max = AppearanceConfig {
            pinned_tab_width: 120.0,
            ..Default::default()
        };
        assert!(at_max.validate().is_ok(), "120.0 must be accepted");
    }

    #[test]
    fn pinned_tab_width_roundtrips_toml() {
        let cfg = AppearanceConfig {
            pinned_tab_width: 56.0,
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert!(
            (parsed.pinned_tab_width - 56.0).abs() < 1e-4,
            "pinned_tab_width must survive a TOML roundtrip"
        );
        parsed
            .validate()
            .expect("roundtripped config must validate");
    }

    // ── Theme import: config field ────────────────────────────────────────────

    #[test]
    fn themes_dir_default_is_none() {
        let cfg = AppearanceConfig::default();
        assert!(cfg.themes_dir.is_none(), "themes_dir must default to None");
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn themes_dir_roundtrips_toml() {
        use std::path::PathBuf;
        let cfg = AppearanceConfig {
            themes_dir: Some(PathBuf::from("/custom/themes")),
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.themes_dir, Some(PathBuf::from("/custom/themes")));
        parsed
            .validate()
            .expect("roundtripped config must validate");
    }

    #[test]
    fn effective_themes_dir_none_falls_back_to_platform_default() {
        let cfg = AppearanceConfig::default();
        // On a typical host with a home directory, this returns Some.
        // We can't assert the exact path, but we verify it doesn't panic and
        // is consistent with the paths module.
        let effective = cfg.effective_themes_dir();
        let from_paths = crate::paths::themes_dir();
        assert_eq!(effective, from_paths);
    }

    #[test]
    fn effective_themes_dir_empty_path_disables_scanning() {
        use std::path::PathBuf;
        let cfg = AppearanceConfig {
            themes_dir: Some(PathBuf::from("")),
            ..Default::default()
        };
        assert!(
            cfg.effective_themes_dir().is_none(),
            "empty-string themes_dir must disable directory scanning"
        );
    }

    // ── Theme import: deserialise a Theme from a TOML string ─────────────────

    #[test]
    fn theme_deserializes_from_toml_string() {
        // Use a multi-line TOML string built without raw-string delimiters to
        // avoid the r#"..."# delimiter colliding with the "#rrggbb" colour
        // strings inside the TOML arrays.
        let toml_src = concat!(
            "name = \"Test Theme\"\n",
            "background = \"#1a1a2e\"\n",
            "foreground = \"#e0e0e0\"\n",
            "cursor = \"#ff6b6b\"\n",
            "selection = \"#3a3a5c\"\n",
            "normal = [\"#1a1a2e\",\"#e06c75\",\"#98c379\",\"#e5c07b\",",
            "\"#61afef\",\"#c678dd\",\"#56b6c2\",\"#abb2bf\"]\n",
            "bright = [\"#5c6370\",\"#e06c75\",\"#98c379\",\"#e5c07b\",",
            "\"#61afef\",\"#c678dd\",\"#56b6c2\",\"#ffffff\"]\n",
        );
        let theme: crate::theme::Theme = toml::from_str(toml_src).expect("theme must parse");
        assert_eq!(theme.name, "Test Theme");
        assert_eq!(theme.background, "#1a1a2e");
        let resolved = theme.resolved();
        assert_eq!(resolved.background, [0x1a, 0x1a, 0x2e]);
        assert_eq!(resolved.cursor, [0xff, 0x6b, 0x6b]);
    }

    // ── Theme import: merge built-ins + dir themes with name-dedupe ───────────

    /// Helper: create an 8-element `[String; 8]` filled with `s`.
    fn eight_str(s: &str) -> [String; 8] {
        std::array::from_fn(|_| s.to_owned())
    }

    #[test]
    fn all_themes_deduplicates_by_name_builtin_wins() {
        // A user-defined theme with a built-in name should be silently skipped.
        let builtin_name = crate::theme::builtin_themes()[0].name.clone();
        let cfg = AppearanceConfig {
            themes: vec![crate::theme::Theme {
                name: builtin_name.clone(),
                background: "#ff0000".into(), // different colour
                foreground: "#ffffff".into(),
                cursor: "#00ff00".into(),
                selection: "#0000ff".into(),
                normal: eight_str("#000000"),
                bright: eight_str("#111111"),
            }],
            ..Default::default()
        };
        let all = cfg.all_themes();
        // Exactly one entry for the built-in name.
        let count = all.iter().filter(|t| t.name == builtin_name).count();
        assert_eq!(count, 1, "duplicate name must appear exactly once");
        // The built-in's colour must have survived, not the user-defined one.
        let winner = all.iter().find(|t| t.name == builtin_name).unwrap();
        assert_ne!(
            winner.background, "#ff0000",
            "built-in must win over user-defined"
        );
    }

    #[test]
    fn all_themes_includes_inline_user_themes() {
        let custom = crate::theme::Theme {
            name: "My Custom Theme".into(),
            background: "#aabbcc".into(),
            foreground: "#ddeeff".into(),
            cursor: "#112233".into(),
            selection: "#445566".into(),
            normal: eight_str("#000000"),
            bright: eight_str("#111111"),
        };
        let cfg = AppearanceConfig {
            themes: vec![custom.clone()],
            ..Default::default()
        };
        let all = cfg.all_themes();
        assert!(
            all.iter().any(|t| t.name == "My Custom Theme"),
            "inline user theme must appear in all_themes()"
        );
    }

    // ── Theme import: scan_themes_dir with a temp directory ──────────────────

    #[test]
    fn scan_themes_dir_loads_valid_toml_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        // Build the TOML string without raw-string delimiters to avoid the
        // r#"..."# delimiter colliding with the "#rrggbb" colour strings.
        let theme_toml = concat!(
            "name = \"Drop-in Red\"\n",
            "background = \"#330000\"\n",
            "foreground = \"#ffcccc\"\n",
            "cursor = \"#ff6666\"\n",
            "selection = \"#660000\"\n",
            "normal = [\"#000000\",\"#ff0000\",\"#00ff00\",\"#ffff00\",",
            "\"#0000ff\",\"#ff00ff\",\"#00ffff\",\"#ffffff\"]\n",
            "bright = [\"#444444\",\"#ff4444\",\"#44ff44\",\"#ffff44\",",
            "\"#4444ff\",\"#ff44ff\",\"#44ffff\",\"#ffffff\"]\n",
        );
        std::fs::write(dir.path().join("red.toml"), theme_toml).unwrap();
        let themes = scan_themes_dir(dir.path());
        assert_eq!(themes.len(), 1, "one valid theme file must yield one Theme");
        assert_eq!(themes[0].name, "Drop-in Red");
    }

    #[test]
    fn scan_themes_dir_skips_invalid_toml() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("bad.toml"), "not valid toml !!!\n???").unwrap();
        let themes = scan_themes_dir(dir.path());
        assert!(
            themes.is_empty(),
            "invalid TOML must be skipped, not panicked"
        );
    }

    #[test]
    fn scan_themes_dir_ignores_non_toml_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("theme.json"), r#"{"name":"ignored"}"#).unwrap();
        std::fs::write(dir.path().join("theme.yaml"), "name: ignored").unwrap();
        let themes = scan_themes_dir(dir.path());
        assert!(themes.is_empty(), "non-.toml files must be ignored");
    }

    #[test]
    fn all_themes_merges_dir_themes_with_deduplication() {
        let dir = tempfile::tempdir().expect("temp dir");
        // Write a drop-in that collides with a built-in name — it should be skipped.
        let builtin_name = crate::theme::builtin_themes()[0].name.clone();
        // Use a helper to avoid r#"..."# delimiter issues with "#rrggbb" strings.
        let eight_black = "\"#000000\",\"#000000\",\"#000000\",\"#000000\",\
                           \"#000000\",\"#000000\",\"#000000\",\"#000000\"";
        let eight_grey = "\"#111111\",\"#111111\",\"#111111\",\"#111111\",\
                          \"#111111\",\"#111111\",\"#111111\",\"#111111\"";
        let collision_toml = format!(
            "name = \"{builtin_name}\"\n\
             background = \"#ff0000\"\nforeground = \"#ffffff\"\ncursor = \"#00ff00\"\n\
             selection = \"#0000ff\"\nnormal = [{eight_black}]\nbright = [{eight_grey}]\n"
        );
        std::fs::write(dir.path().join("collision.toml"), &collision_toml).unwrap();
        // Write a unique drop-in theme.
        let unique_toml = format!(
            "name = \"Drop-in Unique\"\n\
             background = \"#001122\"\nforeground = \"#aabbcc\"\ncursor = \"#ff0000\"\n\
             selection = \"#334455\"\nnormal = [{eight_black}]\nbright = [{eight_grey}]\n"
        );
        std::fs::write(dir.path().join("unique.toml"), &unique_toml).unwrap();

        let cfg = AppearanceConfig {
            themes_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let all = cfg.all_themes();
        // Collision must appear exactly once (the built-in wins).
        let collision_count = all.iter().filter(|t| t.name == builtin_name).count();
        assert_eq!(
            collision_count, 1,
            "colliding drop-in must not create a duplicate"
        );
        // The unique drop-in must be present.
        assert!(
            all.iter().any(|t| t.name == "Drop-in Unique"),
            "unique drop-in theme must appear in all_themes()"
        );
    }

    // ── builtin_box_drawing ───────────────────────────────────────────────────

    #[test]
    fn builtin_box_drawing_default_is_true() {
        let cfg = AppearanceConfig::default();
        assert!(
            cfg.builtin_box_drawing,
            "builtin_box_drawing must default to true"
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn builtin_box_drawing_roundtrips_toml() {
        let cfg = AppearanceConfig {
            builtin_box_drawing: false,
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert!(
            !parsed.builtin_box_drawing,
            "builtin_box_drawing=false must survive a TOML roundtrip"
        );
        parsed
            .validate()
            .expect("roundtripped config must validate");
    }

    // ── tab_group_colors ──────────────────────────────────────────────────────

    #[test]
    fn tab_group_colors_default_has_eight() {
        let cfg = AppearanceConfig::default();
        assert_eq!(
            cfg.tab_group_colors.len(),
            8,
            "default tab_group_colors must have exactly 8 entries"
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn tab_group_colors_empty_fails_validate() {
        let cfg = AppearanceConfig {
            tab_group_colors: Vec::new(),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        match err {
            ConfigError::Invalid { field, .. } => {
                assert_eq!(field, "appearance.tab_group_colors");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn tab_group_colors_roundtrips_toml() {
        let cfg = AppearanceConfig {
            tab_group_colors: vec![[0x11, 0x22, 0x33], [0xaa, 0xbb, 0xcc]],
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.tab_group_colors.len(), 2);
        assert_eq!(parsed.tab_group_colors[0], [0x11, 0x22, 0x33]);
        assert_eq!(parsed.tab_group_colors[1], [0xaa, 0xbb, 0xcc]);
        parsed
            .validate()
            .expect("roundtripped config must validate");
    }

    // ── show_tab_group_labels ─────────────────────────────────────────────────

    #[test]
    fn show_tab_group_labels_default_is_true() {
        let cfg = AppearanceConfig::default();
        assert!(
            cfg.show_tab_group_labels,
            "show_tab_group_labels must default to true"
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn show_tab_group_labels_roundtrips_toml() {
        let cfg = AppearanceConfig {
            show_tab_group_labels: false,
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert!(
            !parsed.show_tab_group_labels,
            "show_tab_group_labels=false must survive a TOML roundtrip"
        );
        parsed
            .validate()
            .expect("roundtripped config must validate");
    }

    // ── bundled_icons ─────────────────────────────────────────────────────────

    #[test]
    fn bundled_icons_default_is_true() {
        let cfg = AppearanceConfig::default();
        assert!(
            cfg.bundled_icons,
            "bundled_icons must default to true (Tabler Icons enabled)"
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn bundled_icons_roundtrips_toml() {
        let cfg = AppearanceConfig {
            bundled_icons: false,
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert!(
            !parsed.bundled_icons,
            "bundled_icons=false must survive a TOML roundtrip"
        );
        parsed
            .validate()
            .expect("roundtripped config must validate");
    }

    // ── tab_activity_spinner ──────────────────────────────────────────────────

    #[test]
    fn tab_activity_spinner_default_is_true() {
        let cfg = AppearanceConfig::default();
        assert!(
            cfg.tab_activity_spinner,
            "tab_activity_spinner must default to true"
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn tab_activity_spinner_roundtrips_toml() {
        let cfg = AppearanceConfig {
            tab_activity_spinner: false,
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: AppearanceConfig = toml::from_str(&toml).unwrap();
        assert!(
            !parsed.tab_activity_spinner,
            "tab_activity_spinner=false must survive a TOML roundtrip"
        );
        parsed
            .validate()
            .expect("roundtripped config must validate");
    }
}
