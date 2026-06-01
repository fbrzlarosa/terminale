# Writing a theme

A theme in `terminale` is a small TOML file describing one colour palette:
a background, a foreground, a cursor and selection colour, and the 16 ANSI
colours (8 normal + 8 bright). `terminale` ships a dozen hand-tuned built-ins
and lets you drop in as many of your own as you like.

## Where themes live

User themes are loaded from the themes directory:

| OS | Default themes directory |
|---|---|
| Linux | `$XDG_CONFIG_HOME/terminale/themes/` (fallback `~/.config/terminale/themes/`) |
| macOS | `~/Library/Application Support/terminale/themes/` |
| Windows | `%APPDATA%\terminale\themes\` |

Each `*.toml` file in that directory contributes one theme, listed in the theme
picker alongside the built-ins. You can also import a theme from anywhere via
**Settings → Appearance → Import theme…**, which copies the chosen file into the
themes directory and selects it.

## Theme file format

Every colour is a `#rrggbb` hex string. All fields are required.

```toml
name       = "My Theme"        # shown in the picker; selected via appearance.theme
background = "#0d1017"         # window / cell background
foreground = "#a9b1d6"         # default text colour
cursor     = "#7da6ff"         # cursor block colour
selection  = "#33467c"         # selection highlight

# 8 normal ANSI colours, in this exact order:
# black, red, green, yellow, blue, magenta, cyan, white
normal = [
  "#1a1b26", "#f7768e", "#9ece6a", "#e0af68",
  "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
]

# 8 bright ANSI colours, same order:
bright = [
  "#414868", "#ff757f", "#b9f27c", "#ff9e64",
  "#7da6ff", "#bb9af7", "#0db9d7", "#c0caf5",
]
```

### Field reference

| Field | Type | Meaning |
|---|---|---|
| `name` | string | Display name. Must match `appearance.theme` to select it. A user theme whose name duplicates a built-in does **not** override the built-in. |
| `background` | `#rrggbb` | Default window and cell background. |
| `foreground` | `#rrggbb` | Default text colour. |
| `cursor` | `#rrggbb` | Cursor colour. |
| `selection` | `#rrggbb` | Selection highlight colour. |
| `normal` | `[#rrggbb; 8]` | ANSI 0–7: black, red, green, yellow, blue, magenta, cyan, white. |
| `bright` | `[#rrggbb; 8]` | ANSI 8–15, same order as `normal`. |

The leading `#` is optional (`"0d1017"` works too). Any unparseable colour falls
back to a sane default rather than failing the whole theme.

## Selecting a theme

```toml
[appearance]
theme = "My Theme"
```

Or pick it live from the command palette (`Ctrl+Shift+P` → "Theme: …") or
**Settings → Appearance → Theme**. Theme changes apply immediately.

## Tips

- Start from an existing palette: copy one of the built-ins (e.g. Tokyo Night,
  Dracula, Gruvbox Dark) and tweak it.
- Keep `background` and `foreground` at a comfortable contrast ratio — terminals
  are read for hours.
- The first `normal`/`bright` entry (ANSI black) is often used for dim text;
  don't make it identical to the background or it disappears.
- Provide a light variant if you work in bright environments — `terminale` has no
  problem with light palettes (see the built-in **Catppuccin Latte**).

## Sharing a theme

A theme is just a `.toml` file — share it directly, and others drop it into their
themes directory or import it from Settings. If you'd like it bundled as a
built-in, open a pull request adding it to the catalog.
