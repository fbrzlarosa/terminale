# Configuration reference

`terminale` is configured from a single TOML file. Everything in it can also be
changed from the in-app **settings window**, and most options apply **live**
without a restart.

## File location

| OS | Path |
|---|---|
| Linux | `$XDG_CONFIG_HOME/terminale/config.toml` (fallback `~/.config/terminale/config.toml`) |
| macOS | `~/Library/Application Support/terminale/config.toml` |
| Windows | `%APPDATA%\terminale\config.toml` |

A missing or malformed file is non-fatal: `terminale` falls back to built-in
defaults and keeps running, so you can never lock yourself out by editing it.

Override the path on launch with `--config /path/to/config.toml`.

## Sections

### `[font]`

```toml
[font]
family    = "JetBrains Mono"   # any installed or bundled monospace family
size      = 14.0
ligatures = true               # enable programming ligatures
```

### `[appearance]`

```toml
[appearance]
theme = "Tokyo Night"          # name of a built-in or user theme
```

See [`theming.md`](theming.md) to add your own themes.

### `[window]`

```toml
[window]
opacity          = 0.97        # 0.0–1.0
padding          = 8           # inner padding, px
scrollback_lines = 10000       # 0 disables scrollback; applied live
copy_on_select   = false       # copy to clipboard on selection
```

### `[cursor]`

```toml
[cursor]
style         = "block"        # block | outline_block | underline | beam
blink         = true
blink_rate_ms = 530
```

### `[bell]`

```toml
[bell]
mode = "visual"                # visual | audio | both | none
```

### `[ai]`

```toml
[ai]
default_provider = "ollama"    # claude | openai | ollama
```

The AI assistant and the proactive command-suggestion bar share this provider
configuration. Provider credentials and the suggestion trigger (off / manual /
auto) are configured in **Settings → AI**.

### `[plugins]`

```toml
[plugins]
enabled   = true
# directory = "/absolute/path/to/plugins"   # optional override

# Permissions (applied live)
allow_scrollback_read = false  # let plugins read terminal contents (opt-in)
scrollback_read_cap   = 10000  # max lines a plugin can read per call
allow_keybindings     = true   # let plugins register shortcuts
```

See [`plugins.md`](plugins.md) for the plugin API and the permission model.

### `[logging]`

```toml
[logging]
file_enabled   = true          # rolling daily file in <config dir>/logs/
file_level     = "info"        # error | warn | info | debug | trace (or a
                               # tracing directive like "terminale=debug")
retention_days = 7             # older files are pruned at startup (1–365)
```

The file exists so a freeze or crash leaves evidence even when terminale is
launched without a console. Enable/level apply on the next launch; the
console log (when launched from a shell) independently follows `--log-level`.

### `[terminal]`

```toml
[terminal]
ctrl_c_copies_selection = true # Ctrl+C with text selected copies it instead of
                               # interrupting (like Tabby / Windows Terminal);
                               # the selection clears on copy, so a second
                               # Ctrl+C interrupts as usual. false = always ^C
```

### `[terminal.image_protocols]`

Inline images render out of the box — these toggles exist to *disable* a
protocol (e.g. when a runaway script floods the terminal with images).

```toml
[terminal.image_protocols]
sixel   = true                 # DCS Sixel graphics
osc1337 = true                 # OSC 1337 File= inline images
apc     = true                 # APC (ESC _G) graphics
```

Quick test: any Sixel-producing tool works (e.g. `img2sixel photo.jpg` from
libsixel), as do `imgcat`-style scripts that emit `OSC 1337 File=` payloads.

### `[keybinds]`

```toml
[keybinds]
quake = "Ctrl+`"               # global hotkey for the drop-down terminal

[keybinds.shortcuts]
new_tab           = "Ctrl+T"
command_palette   = "Ctrl+Shift+P"
ai_assistant      = "Ctrl+Shift+I"
explain_selection = "Ctrl+Shift+E"
# … every action is rebindable; see Settings → Keybinds for the full list.
```

## Settings window

Every option above has a control in the settings window, grouped by section
(Appearance, Window, Cursor, Bell, AI, Plugins, Keybinds, …). The project rule is
that **no setting is editable only by hand** — if behaviour is tunable, it has a
control. If you find a config field with no UI, that's a bug; please report it.
