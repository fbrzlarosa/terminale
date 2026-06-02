//! Colour themes — a curated set of popular palettes plus user-defined ones.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::theme_catalog::catalog_themes;

/// Hex `#rrggbb` colour string parsed into 0-255 components.
fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r, g, b])
}

/// One colour palette.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Theme {
    /// Display name (matches `appearance.theme`).
    pub name: String,
    /// Window background.
    pub background: String,
    /// Default foreground (text).
    pub foreground: String,
    /// Cursor accent colour.
    pub cursor: String,
    /// Selection highlight.
    pub selection: String,
    /// 8 normal ANSI colours: black, red, green, yellow, blue, magenta, cyan, white.
    pub normal: [String; 8],
    /// 8 bright ANSI colours, same order.
    pub bright: [String; 8],
}

impl Theme {
    /// Convert all string colours to byte triplets in one call.
    #[must_use]
    pub fn resolved(&self) -> ResolvedTheme {
        let parse = |s: &str, fb: [u8; 3]| parse_hex(s).unwrap_or(fb);
        ResolvedTheme {
            name: self.name.clone(),
            background: parse(&self.background, [0x0d, 0x10, 0x17]),
            foreground: parse(&self.foreground, [0xe6, 0xea, 0xf8]),
            cursor: parse(&self.cursor, [0x7d, 0xa6, 0xff]),
            selection: parse(&self.selection, [0x33, 0x46, 0x7c]),
            normal: [
                parse(&self.normal[0], [0x1a, 0x1b, 0x26]),
                parse(&self.normal[1], [0xf7, 0x76, 0x8e]),
                parse(&self.normal[2], [0x9e, 0xce, 0x6a]),
                parse(&self.normal[3], [0xe0, 0xaf, 0x68]),
                parse(&self.normal[4], [0x7a, 0xa2, 0xf7]),
                parse(&self.normal[5], [0xbb, 0x9a, 0xf7]),
                parse(&self.normal[6], [0x7d, 0xcf, 0xff]),
                parse(&self.normal[7], [0xa9, 0xb1, 0xd6]),
            ],
            bright: [
                parse(&self.bright[0], [0x41, 0x48, 0x68]),
                parse(&self.bright[1], [0xff, 0x75, 0x7f]),
                parse(&self.bright[2], [0xb9, 0xf2, 0x7c]),
                parse(&self.bright[3], [0xff, 0x9e, 0x64]),
                parse(&self.bright[4], [0x7d, 0xa6, 0xff]),
                parse(&self.bright[5], [0xbb, 0x9a, 0xf7]),
                parse(&self.bright[6], [0x0d, 0xb9, 0xd7]),
                parse(&self.bright[7], [0xc0, 0xca, 0xf5]),
            ],
        }
    }
}

/// `Theme` with all colours pre-decoded into RGB triplets.
#[derive(Debug, Clone)]
pub struct ResolvedTheme {
    /// Human-readable theme name, as shown in the theme picker.
    pub name: String,
    /// Default window/cell background colour.
    pub background: [u8; 3],
    /// Default text (foreground) colour.
    pub foreground: [u8; 3],
    /// Cursor block colour.
    pub cursor: [u8; 3],
    /// Selection highlight colour.
    pub selection: [u8; 3],
    /// The 8 normal-intensity ANSI palette colours (indices 0–7).
    pub normal: [[u8; 3]; 8],
    /// The 8 bright-intensity ANSI palette colours (indices 8–15).
    pub bright: [[u8; 3]; 8],
}

/// All built-in themes the settings UI offers as presets.
///
/// Returns the curated core set followed by the extended catalog, with
/// deduplication by name (core themes always win over catalog entries).
#[must_use]
pub fn builtin_themes() -> Vec<Theme> {
    let core = core_themes();
    let existing_names: std::collections::HashSet<String> =
        core.iter().map(|t| t.name.clone()).collect();
    let mut all = core;
    for t in catalog_themes() {
        if !existing_names.contains(t.name.as_str()) {
            all.push(t);
        }
    }
    all
}

/// The small set of hand-tuned themes that ship with terminale itself.
/// These take precedence over any identically-named catalog entry.
#[must_use]
fn core_themes() -> Vec<Theme> {
    vec![
        Theme {
            name: "Tokyo Night".into(),
            background: "#0d1017".into(),
            foreground: "#a9b1d6".into(),
            cursor: "#7da6ff".into(),
            selection: "#33467c".into(),
            normal: [
                "#1a1b26".into(),
                "#f7768e".into(),
                "#9ece6a".into(),
                "#e0af68".into(),
                "#7aa2f7".into(),
                "#bb9af7".into(),
                "#7dcfff".into(),
                "#a9b1d6".into(),
            ],
            bright: [
                "#414868".into(),
                "#ff757f".into(),
                "#b9f27c".into(),
                "#ff9e64".into(),
                "#7da6ff".into(),
                "#bb9af7".into(),
                "#0db9d7".into(),
                "#c0caf5".into(),
            ],
        },
        Theme {
            name: "Matrix".into(),
            background: "#000000".into(),
            foreground: "#00ff41".into(),
            cursor: "#22ff66".into(),
            selection: "#0a4a1a".into(),
            normal: [
                "#000000".into(), // black
                "#22ff66".into(), // "red" reimagined as bright green for headers
                "#00ff41".into(), // green
                "#9aff7c".into(), // yellow → pale lime
                "#1faa3a".into(), // blue → deep green
                "#4cff9f".into(), // magenta → mint
                "#39ffb5".into(), // cyan → aqua-green
                "#a8ffb7".into(), // white → washed mint
            ],
            bright: [
                "#0a3a18".into(), // bright black (dim foliage)
                "#5cff8a".into(),
                "#22ff66".into(),
                "#c8ffae".into(),
                "#39ffb5".into(),
                "#7affd0".into(),
                "#7affd0".into(),
                "#e6ffe6".into(),
            ],
        },
        Theme {
            name: "Dracula".into(),
            background: "#282a36".into(),
            foreground: "#f8f8f2".into(),
            cursor: "#bd93f9".into(),
            selection: "#44475a".into(),
            normal: [
                "#21222c".into(),
                "#ff5555".into(),
                "#50fa7b".into(),
                "#f1fa8c".into(),
                "#bd93f9".into(),
                "#ff79c6".into(),
                "#8be9fd".into(),
                "#f8f8f2".into(),
            ],
            bright: [
                "#6272a4".into(),
                "#ff6e6e".into(),
                "#69ff94".into(),
                "#ffffa5".into(),
                "#d6acff".into(),
                "#ff92df".into(),
                "#a4ffff".into(),
                "#ffffff".into(),
            ],
        },
        Theme {
            name: "Gruvbox Dark".into(),
            background: "#282828".into(),
            foreground: "#ebdbb2".into(),
            cursor: "#fe8019".into(),
            selection: "#504945".into(),
            normal: [
                "#282828".into(),
                "#cc241d".into(),
                "#98971a".into(),
                "#d79921".into(),
                "#458588".into(),
                "#b16286".into(),
                "#689d6a".into(),
                "#a89984".into(),
            ],
            bright: [
                "#928374".into(),
                "#fb4934".into(),
                "#b8bb26".into(),
                "#fabd2f".into(),
                "#83a598".into(),
                "#d3869b".into(),
                "#8ec07c".into(),
                "#ebdbb2".into(),
            ],
        },
        Theme {
            name: "Catppuccin Mocha".into(),
            background: "#1e1e2e".into(),
            foreground: "#cdd6f4".into(),
            cursor: "#f5e0dc".into(),
            selection: "#585b70".into(),
            normal: [
                "#45475a".into(),
                "#f38ba8".into(),
                "#a6e3a1".into(),
                "#f9e2af".into(),
                "#89b4fa".into(),
                "#f5c2e7".into(),
                "#94e2d5".into(),
                "#bac2de".into(),
            ],
            bright: [
                "#585b70".into(),
                "#f38ba8".into(),
                "#a6e3a1".into(),
                "#f9e2af".into(),
                "#89b4fa".into(),
                "#f5c2e7".into(),
                "#94e2d5".into(),
                "#a6adc8".into(),
            ],
        },
        Theme {
            name: "Nord".into(),
            background: "#2e3440".into(),
            foreground: "#d8dee9".into(),
            cursor: "#88c0d0".into(),
            selection: "#434c5e".into(),
            normal: [
                "#3b4252".into(),
                "#bf616a".into(),
                "#a3be8c".into(),
                "#ebcb8b".into(),
                "#81a1c1".into(),
                "#b48ead".into(),
                "#88c0d0".into(),
                "#e5e9f0".into(),
            ],
            bright: [
                "#4c566a".into(),
                "#bf616a".into(),
                "#a3be8c".into(),
                "#ebcb8b".into(),
                "#81a1c1".into(),
                "#b48ead".into(),
                "#8fbcbb".into(),
                "#eceff4".into(),
            ],
        },
        Theme {
            name: "Solarized Dark".into(),
            background: "#002b36".into(),
            foreground: "#839496".into(),
            cursor: "#93a1a1".into(),
            selection: "#073642".into(),
            normal: [
                "#073642".into(),
                "#dc322f".into(),
                "#859900".into(),
                "#b58900".into(),
                "#268bd2".into(),
                "#d33682".into(),
                "#2aa198".into(),
                "#eee8d5".into(),
            ],
            bright: [
                "#002b36".into(),
                "#cb4b16".into(),
                "#586e75".into(),
                "#657b83".into(),
                "#839496".into(),
                "#6c71c4".into(),
                "#93a1a1".into(),
                "#fdf6e3".into(),
            ],
        },
        Theme {
            name: "One Dark".into(),
            background: "#282c34".into(),
            foreground: "#abb2bf".into(),
            cursor: "#528bff".into(),
            selection: "#3e4451".into(),
            normal: [
                "#1e2127".into(),
                "#e06c75".into(),
                "#98c379".into(),
                "#d19a66".into(),
                "#61afef".into(),
                "#c678dd".into(),
                "#56b6c2".into(),
                "#abb2bf".into(),
            ],
            bright: [
                "#5c6370".into(),
                "#e06c75".into(),
                "#98c379".into(),
                "#d19a66".into(),
                "#61afef".into(),
                "#c678dd".into(),
                "#56b6c2".into(),
                "#ffffff".into(),
            ],
        },
        Theme {
            name: "Tokyo Night Storm".into(),
            background: "#24283b".into(),
            foreground: "#c0caf5".into(),
            cursor: "#c0caf5".into(),
            selection: "#364a82".into(),
            normal: [
                "#1d202f".into(),
                "#f7768e".into(),
                "#9ece6a".into(),
                "#e0af68".into(),
                "#7aa2f7".into(),
                "#bb9af7".into(),
                "#7dcfff".into(),
                "#a9b1d6".into(),
            ],
            bright: [
                "#414868".into(),
                "#f7768e".into(),
                "#9ece6a".into(),
                "#e0af68".into(),
                "#7aa2f7".into(),
                "#bb9af7".into(),
                "#7dcfff".into(),
                "#c0caf5".into(),
            ],
        },
        Theme {
            name: "Rosé Pine".into(),
            background: "#191724".into(),
            foreground: "#e0def4".into(),
            cursor: "#ebbcba".into(),
            selection: "#403d52".into(),
            normal: [
                "#26233a".into(),
                "#eb6f92".into(),
                "#31748f".into(),
                "#f6c177".into(),
                "#9ccfd8".into(),
                "#c4a7e7".into(),
                "#ebbcba".into(),
                "#e0def4".into(),
            ],
            bright: [
                "#6e6a86".into(),
                "#eb6f92".into(),
                "#31748f".into(),
                "#f6c177".into(),
                "#9ccfd8".into(),
                "#c4a7e7".into(),
                "#ebbcba".into(),
                "#e0def4".into(),
            ],
        },
        Theme {
            name: "Ayu Dark".into(),
            background: "#0a0e14".into(),
            foreground: "#b3b1ad".into(),
            cursor: "#e6b450".into(),
            selection: "#273747".into(),
            normal: [
                "#01060e".into(),
                "#ea6c73".into(),
                "#91b362".into(),
                "#f9af4f".into(),
                "#53bdfa".into(),
                "#fae994".into(),
                "#90e1c6".into(),
                "#c7c7c7".into(),
            ],
            bright: [
                "#686868".into(),
                "#f07178".into(),
                "#c2d94c".into(),
                "#ffb454".into(),
                "#59c2ff".into(),
                "#ffee99".into(),
                "#95e6cb".into(),
                "#ffffff".into(),
            ],
        },
        Theme {
            // A light theme so day-time / high-glare users aren't stuck
            // squinting at a dark palette.
            name: "Catppuccin Latte".into(),
            background: "#eff1f5".into(),
            foreground: "#4c4f69".into(),
            cursor: "#dc8a78".into(),
            selection: "#acb0be".into(),
            normal: [
                "#5c5f77".into(),
                "#d20f39".into(),
                "#40a02b".into(),
                "#df8e1d".into(),
                "#1e66f5".into(),
                "#ea76cb".into(),
                "#179299".into(),
                "#acb0be".into(),
            ],
            bright: [
                "#6c6f85".into(),
                "#d20f39".into(),
                "#40a02b".into(),
                "#df8e1d".into(),
                "#1e66f5".into(),
                "#ea76cb".into(),
                "#179299".into(),
                "#bcc0cc".into(),
            ],
        },
        Theme {
            // Unicorn — total black background, neon hot-pink cursor, mint
            // hacker-green foreground, and a full 16-slot ANSI palette of
            // vibrant neon colours. Keystroke FX NOT auto-enabled; that is a
            // separate user opt-in (Settings › Effects › Enable keystroke FX).
            name: "Unicorn".into(),
            background: "#000000".into(),
            foreground: "#B8FFC0".into(),
            cursor: "#FF1493".into(),    // hot pink (CSS deeppink)
            selection: "#3A1F4A".into(), // deep purple
            normal: [
                "#000000".into(), // black
                "#FF1493".into(), // red slot → hot pink
                "#B8FFC0".into(), // green slot → mint
                "#FFFF33".into(), // yellow slot → electric yellow
                "#BF00FF".into(), // blue slot → neon purple
                "#FF6EC7".into(), // magenta slot → candy pink
                "#00FFFF".into(), // cyan (also used for link/accent)
                "#E0E0FF".into(), // white slot → pale lavender white
            ],
            bright: [
                "#5A2A6A".into(), // bright black → dim purple
                "#FF69B4".into(), // bright red → hot pink bright
                "#C8FFD0".into(), // bright green → brighter mint
                "#FFFF80".into(), // bright yellow → bright yellow
                "#D070FF".into(), // bright blue → bright purple
                "#FF99D8".into(), // bright magenta → bright candy
                "#80FFFF".into(), // bright cyan
                "#FFFFFF".into(), // pure white
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex() {
        assert_eq!(parse_hex("#0d1017"), Some([0x0d, 0x10, 0x17]));
        assert_eq!(parse_hex("0d1017"), Some([0x0d, 0x10, 0x17]));
        assert_eq!(parse_hex("#zzzzzz"), None);
    }

    #[test]
    fn every_builtin_resolves() {
        for theme in builtin_themes() {
            let _ = theme.resolved();
        }
    }

    #[test]
    fn unicorn_theme_is_present() {
        assert!(
            builtin_themes().iter().any(|t| t.name == "Unicorn"),
            "Unicorn must be in builtin_themes()"
        );
    }

    #[test]
    fn unicorn_cursor_is_hot_pink() {
        let unicorn = builtin_themes()
            .into_iter()
            .find(|t| t.name == "Unicorn")
            .expect("Unicorn theme must exist");
        let resolved = unicorn.resolved();
        // Hot pink = #FF1493
        assert_eq!(resolved.cursor, [0xFF, 0x14, 0x93]);
        // Background is total black
        assert_eq!(resolved.background, [0x00, 0x00, 0x00]);
    }
}
