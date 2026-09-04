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
tab_activity_spinner  = true   # animated spinner on a tab while it is busy
tab_spinner_idle_ms   = 800    # how long output keeps a tab spinning after it
                               # stops arriving (100-10000). This is what
                               # decides the spinner for programs that hold the
                               # terminal open - Claude Code, a REPL, vim, ssh -
                               # since shell integration reports those as one
                               # command from launch to exit. They animate while
                               # working and go silent while waiting, so the
                               # spinner follows that instead. Lower = clears
                               # sooner; raise it if a bursty tool makes it
                               # stutter. Plain commands are unaffected: they
                               # spin for as long as they run, output or not
```

See [`theming.md`](theming.md) to add your own themes.

### `[background_fx]`

```toml
[background_fx]
enabled              = false   # off by default — a continuous full-screen
                               # shader rendered behind the grid, distinct
                               # from any per-keystroke particle effect
style                = "aurora_plasma" # aurora_plasma | starfield | matrix |
                               # pixel_crt | none
intensity            = 0.35    # 0.0–1.0, kept modest so text stays readable
speed                = 1.0     # 0.1–5.0 animation speed multiplier
react_to_keystrokes  = true    # each keypress spawns its own animated band
pause_when_unfocused = true    # stop repainting the effect while the window
                               # doesn't have focus, so an idle background
                               # terminal costs no GPU
```

Purely cosmetic "wow" layer, off by default. `matrix_band_width`,
`matrix_fall_speed`, `max_emitters`, `band_lifetime_secs`, and `color1`/`color2`
tune the per-style look further — see **Settings → Appearance → Background
effect**.

### `[window]`

```toml
[window]
opacity          = 0.97        # 0.0–1.0
padding          = 8           # inner padding, px
scrollback_lines = 10000       # 0 disables scrollback; applied live
copy_on_select   = false       # copy to clipboard on selection
scroll_on_input  = true        # typing/pasting while scrolled up in history
                               # snaps the view back to the live prompt
scrollbar        = "auto"      # interactive scrollback scrollbar: drag the
                               # thumb, click the track to jump. auto = shown
                               # while scrolled or on right-edge hover;
                               # always | never
```

#### Session restore

```toml
[window]
restore_session         = "off"  # off | last_session
restore_working_dirs    = true   # reopen each pane in its last directory
restore_window_geometry = true   # position, size, monitor, Quake state
restore_all_windows     = true   # every window, not just the first
session_autosave_secs   = 15     # 0 = save on close only; else 5–3600
```

With `restore_session = "last_session"` the next launch reopens the layout you
left: every window, each with its tabs, splits, split ratios, tab groups,
focused pane, geometry and monitor. Running processes are never restored — each
pane starts a fresh shell (in its last directory, when
`restore_working_dirs` is on).

A window you close on its own and then keep working in is *not* brought back;
one closed as part of quitting is. Since quitting means closing one window after
another, a closed window stays in the snapshot for half a minute — long enough
to cover the whole gesture, short enough that a window left closed is gone from
the next save. `session_autosave_secs` is unrelated to that: it is how often the
snapshot is rewritten while you work, so a crash or power loss loses at most
that many seconds of layout. Set `restore_all_windows = false` to reopen only
the window you opened first.

All of it is in **Settings → Workspaces → Session restore**.

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

### `[quake]`

```toml
[quake]
animation     = "slide"   # none | slide | bounce | scale | fade
animation_ms  = 120       # show/hide animation duration, ms
easing        = "mirror"  # mirror | ease_out | ease_in_out | linear
animation_fps = 60        # animation repaint ceiling, 15-240
edge          = "off"     # off | top | bottom | left | right
display      = "current" # current | pointer | primary | { index = N }
size_percent = 0.5       # fraction of the monitor's perpendicular extent
                          # occupied when docked (edge != "off")
```

`display` decides which monitor a docked drop-down appears on:

* `current` (default) — the monitor it was last visible on. Drag it to another
  monitor to re-anchor it there. Deliberately *not* pointer-following: a window
  that relocates on its own is disorienting once you have parked it somewhere.
  When there is no history at all — the first reveal after
  `integration.autostart` started it hidden — this falls back to the monitor the
  pointer is on, which beats picking whichever screen the OS happens to list
  first.
* `pointer` — the monitor the mouse is on, every time. What most people mean by
  a drop-down terminal: it opens where you are looking. Needs X11; Wayland does
  not tell an application where the pointer is, and there it behaves as
  `current`.
* `primary` — always the OS primary monitor.
* `{ index = N }` — pinned to the N-th enumerated monitor.

`easing` shapes the timing curve, which is what decides whether the drop-down
*feels* snappy at a given `animation_ms` — a duration alone does not:

* `mirror` (default) — the open eases out and the close eases in, so a close is
  the open played backwards. The previous behaviour eased *both* directions out,
  which meant a close collapsed almost at once and then crept the last handful
  of pixels for the rest of the duration: at half of a 350 ms close the window
  was already down to 14 % of its height, and the remaining 175 ms were spent
  animating something too small to see.
* `ease_out` — the old behaviour, kept for anyone who preferred it.
* `ease_in_out` — gentle at both ends.
* `linear` — constant speed.

`animation_fps` caps how often the animation repaints. It is a **ceiling, not a
target**: the animation is driven from the event loop's idle callback, which
fires on every event batch rather than on a timer, so the window-resize events
the animation itself generates wake the loop straight back up. Without the cap
the animation repainted as fast as the machine allowed — a single 350 ms close
was measured at 114 frames (327 fps), half of them re-applying a rect identical
to the previous frame's — and a compositor that cannot keep up with a window
resizing itself hundreds of times a second makes the animation look *slower*,
not smoother. Raise it on a high-refresh display, lower it to spend less GPU per
toggle.

The global toggle hotkey lives in `[keybinds] quake = "Ctrl+\`"`, not here.
`edge = "off"` (the default) is a free-floating window that restores its exact
last geometry on show/hide; picking an edge instead docks it there, sized by
`size_percent` and inset by `margin_px`. See **Settings → Quake** for
`display` (which monitor), `hide_on_focus_loss`, and `show_on_all_desktops`.

### `[gpu]`

```toml
[gpu]
backend          = "auto" # auto | vulkan | dx12 | metal | gl | software
power_preference = "auto" # auto | low | high
```

`software` requests a CPU fallback adapter, effectively disabling hardware GPU
acceleration — useful when a driver misbehaves or a remote/VM display only
exposes a software adapter. The explicit API variants force one backend
(e.g. `vulkan` on a Linux box that otherwise picks Wayland's default).

### `[profiles]`

```toml
[profiles]
default = "Zsh"           # launched when no --profile flag is passed

[[profiles.profiles]]
name    = "Zsh"
command = "/bin/zsh"
args    = []
icon    = "🐚"             # optional, shown in pickers and tabs
```

Profiles are auto-detected on first launch (every shell found on `$PATH`, plus
`$SHELL` first on Unix, PowerShell/cmd/Git Bash/WSL on Windows). Add, edit, or
reorder them here or from **Settings → Profiles**; each can also set `env` and
`cwd`.

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
                               # interrupting; the selection clears on copy, so a second
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

### `[quick_select]`

```toml
[quick_select]
alphabet    = "asdfjklqwerzxcvghtybnuiopm" # label characters, shortest-used first
overlay_dim = 0.45     # 0.0–1.0 dimming behind the label badges
# patterns  = [...]    # regexes scanned for matches (URLs, paths, git SHAs,
                        # IPv4s, hex colours, UUIDs by default) — edit the
                        # full list from Settings → Quick Select
```

Label-hint mode: overlays short keyboard labels on every regex match in the
visible screen + scrollback so you can copy one without touching the mouse.
The same label renderer powers pane-select mode.

### `[status_bar]`

```toml
[status_bar]
enabled            = false    # off by default
position           = "bottom" # top | bottom
update_interval_ms = 1000     # 200–60000

[[status_bar.left_segments]]
type = "cwd"

[[status_bar.right_segments]]
type = "profile"

[[status_bar.right_segments]]
type   = "clock"
format = "%H:%M"
```

A thin strip at the top or bottom of the terminal. Segment kinds: `cwd`,
`clock` (`strftime` format), `profile`, `tab_index`, `user_var` (an OSC 1337
`SetUserVar`), `literal` text, and `spacer`.

### `[clipboard_history]`

```toml
[clipboard_history]
enabled       = true  # in-memory ring buffer only — nothing touches disk
size          = 20    # 1–500 entries retained
capture_osc52 = false # off by default: OSC 52 (programmatic clipboard set)
                       # payloads often carry tokens or passwords
```

### `[directory_jump]`

```toml
[directory_jump]
enabled     = true
max_tracked = 200   # 1–2000 directories, ranked by frecency
persist     = true  # survive restarts (<data dir>/dir_history.toml)
```

Tracks every directory visited via OSC 7 cwd reports and ranks them by a
frequency + recency score; the picker jumps the active shell there with
`cd <path>`. Works with any OSC-7-capable shell, no third-party tool required.

### `[[snippets]]`

```toml
[[snippets]]
name        = "Git status"
body        = "git status\n"          # \n \r \t \e \\ \xNN are decoded
description = "Show the working-tree status" # optional
```

Named text bodies inserted into the focused pane via the snippet picker.
Default: empty (add your own here or from **Settings → Snippets**).

### `[[context_rules]]`

```toml
[[context_rules]]
name      = "Production"
host_glob = "*prod*"      # matched against the tab's SSH host name
# cwd_glob = "/srv/production/*" # or matched against the working directory
tab_color = [200, 50, 50]
badge     = "PROD"
```

Auto-tints a tab chip and/or overlays a badge when its SSH host or working
directory matches a glob — the primary use case is a safety cue for
production hosts. Rules are evaluated in order; the first match wins.

### `[editor]`

```toml
[editor]
command = "code -g {file}:{line}:{column}" # empty = OS default file handler
```

External editor launched on Ctrl+click of a `file:line:col` reference.
Supports `{file}`, `{line}`, `{column}` tokens — e.g. `"vim +{line} {file}"`
or `"subl {file}:{line}:{column}"`.

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

#### `[[keybinds.custom]]`, `[[keybinds.key_tables]]`, `[[keybinds.mouse]]`

```toml
# A combo bound to an ordered list of actions — named built-ins and/or a
# literal "send:" byte payload, run in sequence. Takes priority over
# [keybinds.shortcuts].
[[keybinds.custom]]
keys    = "Ctrl+Alt+G"
actions = ["NewTab", "send:git status\n"]

# A named modal key-table (tmux-style prefix key): the leader combo enters
# the mode, the next key dispatches its own action list, and the mode exits
# after `timeout_ms` or Esc.
[[keybinds.key_tables]]
name       = "pane"
leader     = "Ctrl+A"
timeout_ms = 1500   # 100–30000

[[keybinds.key_tables.bindings]]
key     = "V"
actions = ["SplitRight"]

# A (button + modifiers + click-count) combination bound to an action list.
[[keybinds.mouse]]
button  = "Middle"     # Left | Right | Middle | Back | Forward
mods    = "Alt"        # "+"-separated Ctrl/Shift/Alt/Meta, or "" for none
count   = 1            # 1–3: single/double/triple click
actions = ["Paste"]
```

All three default to empty and only add behaviour — nothing here can shadow
`[keybinds.shortcuts]` or take away built-in mouse behaviour unless a rule
actually matches.

### `[ssh]` and `[[ssh_hosts]]`

```toml
[ssh]
host_key_policy = "accept_new" # accept_new (TOFU, default) | strict | off
# known_hosts = "~/.ssh/known_hosts"

[[ssh_hosts]]
name = "prod"
host = "10.0.0.5"
user = "deploy"
auth = "agent"                  # agent (default) | key | password
# key_path = "/home/me/.ssh/id_ed25519" # only used when auth = "key"
```

Each `[[ssh_hosts]]` entry surfaces as `SSH: <name>` in the command palette
and in the "New SSH tab" picker; selecting one opens an interactive remote
shell tab. Passwords and key passphrases are **never** written here — they
live in the OS keychain, looked up by the host's stable id. `host_key_policy`
defaults to trust-on-first-use (pin on first connect, refuse a later change);
`import_openssh_config` can pull hosts from `~/.ssh/config` once or on every
reload — see **Settings → SSH**.

### `[integration]` (Linux / BSD)

```toml
[integration]
desktop_entry           = true   # register a .desktop entry + icon on launch
linux_backend           = "auto" # auto | x11 | wayland — see below
control_socket          = true   # serve `terminale --toggle-quake` + `terminale ctl`
global_shortcuts_portal = true   # register the Quake hotkey with the desktop
quake_launch_on_demand  = true   # the Quake hotkey may START terminale, not just toggle it
autostart               = false  # start hidden at login so the first press is instant
quake_desktop_entry     = false  # also install terminale.Quake.desktop for a shell extension

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

`--toggle-quake` talks to a *running* terminale, so on the first press after
logging in there is nothing on the other end. `quake_launch_on_demand = true`
(the default) makes that first press start terminale instead of failing, and
every press after it is a plain toggle — which is what makes a desktop-owned
hotkey work from a fresh login without anything in your autostart. Only a
*missing* socket triggers it: an instance that is running but not answering is
not helped by starting a second one.

#### Making the first press instant

`quake_launch_on_demand` keeps the hotkey from doing nothing, but a press that
has to *start* terminale cannot feel instant however fast terminale is: the
process, the GPU surface and the shell all come up first, and only then does a
window appear.

`autostart = true` removes that cost by moving it to login. It writes an entry
under `$XDG_CONFIG_HOME/autostart` that launches `terminale --start-hidden`: the
window is built, its surface is live and its shell is running, and the only thing
that has not happened is the map. The first press of the hotkey is then a reveal,
like every press after it. Turning the setting off removes the entry again.
**Settings → Desktop integration → "Start hidden when you log in"** is the same
switch.

`--start-hidden` is also usable on its own, from a session script or a unit of
your own making; it applies to the first window only, so a window opened later is
an ordinary window.

#### Letting a shell extension own the drop-down

On GNOME under Wayland an application may not grab a global key, may not place
its own window, and may not animate it onto the screen — which is, between them,
everything a drop-down terminal is made of. A GNOME Shell extension can do all
three, and that is why a terminal with a genuinely good drop-down on this
desktop is being driven by one rather than doing it itself.

`quake_desktop_entry = true` installs a second launcher entry,
`terminale.Quake.desktop`, for exactly that. Such an extension launches an app
by desktop-entry id and then finds its window by application id, so the
drop-down needs both of its own: the entry passes `--class=terminale.Quake` and
declares the matching `StartupWMClass`. Two consequences are the point of the
whole arrangement:

* the extension drives **that** window and never the terminale you were working
  in — a shared id would let it grab either;
* the entry does **not** pass `--quake`, so terminale comes up as an ordinary
  window and leaves the geometry, the always-on-top and the show/hide animation
  to the extension. Doing both at once is what makes a drop-down look like it is
  fighting the desktop, because it is.

Because the extension starts the app itself, the hotkey works from a fresh login
with nothing running.

**Settings → Desktop integration → "Drop-down via shell extension"** installs
the entry, reports which application the extension is currently driving, and has
a one-click **"Point it at terminale"** that rewrites just that one key —
leaving the extension's own hotkey, size and animation exactly as you tuned
them. The same thing from the CLI:

```console
$ terminale --install-quake-launcher
$ gsettings set org.gnome.shell.extensions.quake-terminal terminal-id \
    'terminale.Quake.desktop'
```

A key held by an extension is also the answer to a puzzling symptom: **recording
that same combination in Settings appears to do nothing.** A grabbed shortcut is
consumed by the desktop before any window sees it, so the recorder genuinely
never receives the keypress. Terminale releases its *own* grab while a recorder
is armed, but a binding owned by the compositor or by an extension is not
terminale's to release — and if the extension is driving the drop-down, there is
nothing to bind in terminale at all.

### `[resource_indicators]`

```toml
[resource_indicators]
enabled = true # pixel-art CPU/RAM/GPU strip in a reserved band at the very
                # bottom of the window (below the terminal grid, so it never
                # overlaps content)
```

### `[updates]`

```toml
[updates]
check_on_startup = true  # check GitHub releases in the background on launch
auto_install     = false # notify-only by default; true downloads, verifies
                          # (SHA-256), and stages the new binary automatically
```

The built-in self-updater never interrupts the running session — even with
`auto_install = true` the new version only takes effect on the next launch,
and the download is checksum-verified before anything on disk is replaced.

## Settings window

Every option above has a control in the settings window, grouped by section
(Appearance, Window, Cursor, Bell, AI, Plugins, Keybinds, …). The project rule is
that **no setting is editable only by hand** — if behaviour is tunable, it has a
control. If you find a config field with no UI, that's a bug; please report it.
