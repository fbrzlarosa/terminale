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
```

See [`plugins.md`](plugins.md) for the plugin API.

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
