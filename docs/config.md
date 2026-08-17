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
theme                 = "Tokyo Night" # name of a built-in or user theme
tab_drop_merge        = true   # drop a dragged tab/pane onto a terminal body
                               # to merge it there as a split pane (a tinted
                               # zone previews the half it will occupy).
                               # false = body drops tear out a new window
tab_attention_on_bell = true   # light an amber dot on a background tab when
                               # its program rings the bell (e.g. Claude Code
                               # finished and is waiting); clears on focus
```

See [`theming.md`](theming.md) to add your own themes.

### `[window]`

```toml
[window]
opacity          = 0.97        # 0.0–1.0
padding          = 8           # inner padding, px
scrollback_lines = 10000       # 0 disables scrollback; applied live
copy_on_select   = false       # copy to clipboard on selection
scroll_on_input  = true        # typing/pasting while scrolled up in history
                               # snaps the view back to the live prompt
                               # (iTerm2 / Windows Terminal behaviour)
scrollbar        = "auto"      # interactive scrollback scrollbar: drag the
                               # thumb, click the track to jump. auto = shown
                               # while scrolled or on right-edge hover;
                               # always | never
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
file_enabled       = true      # rolling daily file in <config dir>/logs/
file_level         = "info"    # error | warn | info | debug | trace (or a
                               # tracing directive like "terminale=debug")
retention_days     = 7         # older files are pruned at startup (1–365)
slow_frame_warn_ms = 250       # freeze watchdog: warn when one render stalls
                               # longer than this (0 = off, else 16–60000)
```

The file exists so a freeze or crash leaves evidence even when terminale is
launched without a console. Enable/level apply on the next launch; the
console log (when launched from a shell) independently follows `--log-level`.

`slow_frame_warn_ms` is the **freeze watchdog**: when a single main-window
render takes longer than the threshold it logs a `WARN`, so transient stalls
(GPU reset / TDR, a blocking call on the UI thread) that recover on their own
still leave a timestamped trace. It applies live and is also exposed in
**Settings → About → Diagnostics**.

### `[terminal]`

```toml
[terminal]
ctrl_c_copies_selection = true # Ctrl+C with text selected copies it instead of
                               # interrupting (like Tabby / Windows Terminal);
                               # the selection clears on copy, so a second
                               # Ctrl+C interrupts as usual. false = always ^C
# Drag & drop: dropping files onto the window inserts their paths into the
# focused pane (like a paste) — drop an image onto Claude Code and it reads it.
drop_paths               = true # master toggle; false ignores dropped files
drop_path_quoting        = "auto" # auto = quote only when the path has spaces
                               # or shell-special chars; always | never
drop_path_trailing_space = true # append a space after each dropped path
```

#### Shell integration (OSC 133)

```toml
[terminal]
shell_integration  = true      # let terminale instrument the shell it launches
command_blocks     = true      # assemble command blocks from the marks
max_command_blocks = 200       # per pane; oldest are dropped
show_prompt_marks  = true      # status dot in the left margin at each prompt:
                               # green = exit 0, red = non-zero, neutral = unknown
```

A terminal cannot tell a prompt from output, or know that a command failed,
unless the shell tells it. `shell_integration` makes terminale inject a small
startup hook into the shell it launches so that it does:

* **`OSC 7`** — the working directory, which is what puts the folder in the tab
  title and lets `window.restore_working_dirs` restore it;
* **`OSC 133`** — prompt marks (`A`/`B`/`C`/`D;<exit>`), which is what powers
  command blocks, jump-to-next/previous-prompt, jump-to-failed-command, *"fix
  this command"*, copy-last-command-output, and
  [`terminale ctl last-command`](control-api.md).

Instrumented today: **bash** (via `--rcfile`, and the hook sources your own
`~/.bashrc` first, so your configuration is untouched) and **PowerShell** (cwd
only). zsh and fish are not instrumented yet — the features above work there only
if your prompt already emits the marks (starship does, for instance).

Injection is skipped whenever it would be wrong: a profile that passes its own
`-c`, `--rcfile`, `--norc`, or runs a login shell is launched exactly as written.
If the hook cannot be written to disk, the shell starts uninstrumented — you lose
the marks, never the session.

terminale also accepts the **`OSC 633`** spelling of the same protocol (VS Code's
variant, including `633;E` which reports the command line explicitly), so a shell
already set up for VS Code's shell integration works here as-is.

The generated hook lives under the data directory
(`~/.local/share/terminale/shell-integration/` on Linux) and is rewritten
whenever the binary is updated. It is a generated file — edit your own rc
instead.

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

### `[integration]` (Linux / BSD)

```toml
[integration]
desktop_entry           = true   # register a .desktop entry + icon on launch
linux_backend           = "auto" # auto | x11 | wayland — see below
control_socket          = true   # serve `terminale --toggle-quake` + `terminale ctl`
global_shortcuts_portal = true   # register the Quake hotkey with the desktop

[integration.control_api]        # what `terminale ctl` may do — see control-api.md
enabled          = true          # serve the automation commands at all
allow_read       = true          # get-text, last-command, tab titles + cwds
allow_input      = true          # send-text, send-keys, palette actions
allow_submit     = false         # may press Enter, i.e. actually run commands
allow_screenshot = true          # render the window to a PNG file
```

`[integration.control_api]` scopes the **control API** — the `terminale ctl`
commands that let a script (or an AI coding agent) read a pane, fetch the last
command with its exit code, run any palette action, or type at your prompt. Full
reference: [`control-api.md`](control-api.md).

The split worth understanding is `allow_input` versus `allow_submit`: input may
*type*, submit may *press Enter*. `allow_submit` is **off by default**, so out of
the box an automation tool can compose a command at your prompt and you decide
whether to run it — the same contract as the AI assistant's *Inject* button.
Every refusal names the setting to flip. All five apply immediately; the socket
itself still needs a relaunch when you toggle `control_socket`.

**`linux_backend` is the setting that makes window placement work.** Wayland
deliberately gives clients no control over their own window position, so on a
native Wayland surface every feature built on explicit geometry silently does
nothing: Quake edge docking, the Snap Top/Bottom/Left/Right actions,
`window.startup_position`, cursor-anchored menus and dialogs, and tab tear-out.
X11 supports all of them, and every mainstream Wayland session ships XWayland —
so `auto` (the default) asks for X11 whenever `$DISPLAY` names a reachable
server and falls back to Wayland otherwise. Choose `wayland` explicitly if you
would rather have a native surface and can live without window positioning.
The backend is chosen once, when the event loop is built, so changing it needs
a restart.

Docked and snapped windows respect the desktop's **work area** (`_NET_WORKAREA`)
on X11, so a `top`-docked Quake window sits under the GNOME/KDE panel instead of
behind it. On Linux/X11 `quake.show_on_all_desktops` and the `fade` Quake
animation are implemented through EWMH (`_NET_WM_DESKTOP` and
`_NET_WM_WINDOW_OPACITY`); both are no-ops on a native Wayland surface.

#### The Quake hotkey under Wayland

No Wayland compositor lets an application grab keys globally, so terminale's own
hotkey never fires there — the X11 grab it uses everywhere else only reaches it
while an X11 window happens to have focus. There are two supported replacements,
both on by default:

* **The desktop's global-shortcuts portal**
  (`org.freedesktop.portal.GlobalShortcuts`, GNOME 48+ / KDE Plasma 6+). The
  desktop confirms the binding with you once, then delivers it whatever has
  focus. It identifies callers by *application id*, which a process only carries
  when the desktop launched it from its `.desktop` entry — so this path works
  when you start terminale from the application menu, not from a shell.
* **A desktop keybinding that runs `terminale --toggle-quake`**, served by the
  control socket (a per-user socket under `$XDG_RUNTIME_DIR`). This has no
  application-id requirement and works on every desktop and window manager
  (GNOME, KDE, sway, i3, Hyprland). On GNOME,
  **Settings → Desktop integration → "Register in GNOME"** sets it up in one
  click, using the key from Shortcuts → Quake toggle; elsewhere, bind the
  command by hand.

## Settings window

Every option above has a control in the settings window, grouped by section
(Appearance, Window, Cursor, Bell, AI, Plugins, Keybinds, …). The project rule is
that **no setting is editable only by hand** — if behaviour is tunable, it has a
control. If you find a config field with no UI, that's a bug; please report it.
