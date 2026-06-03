# Writing a plugin

`terminale` ships an embedded **Lua 5.4** plugin host. A plugin is a single
`.lua` file that the app loads at startup, runs inside a sandboxed Lua state, and
talks to through one injected global table: `terminale`.

Plugins are optional and isolated — a plugin that fails to parse, errors, or
misbehaves is logged and skipped; it can never prevent the terminal from
starting.

## Where plugins live

At startup the host loads **every `*.lua` file** in the plugins directory:

| OS | Default plugins directory |
|---|---|
| Linux | `$XDG_CONFIG_HOME/terminale/plugins/` (fallback `~/.config/terminale/plugins/`) |
| macOS | `~/Library/Application Support/terminale/plugins/` |
| Windows | `%APPDATA%\terminale\plugins\` |

Enable the host and (optionally) point it at a custom directory in `config.toml`:

```toml
[plugins]
enabled   = true
# directory = "/absolute/path/to/my/plugins"   # optional override

# Permissions (applied live; see "Permission model" below)
# allow_scrollback_read = false   # let plugins read terminal contents
# scrollback_read_cap   = 10000   # max lines per read
# allow_keybindings     = true    # let plugins register shortcuts
```

Files are loaded in directory order; each runs in the **same shared Lua state**,
so two plugins can see each other's globals. Name your globals defensively
(prefix them) to avoid collisions.

## A minimal plugin

Create `~/.config/terminale/plugins/hello.lua`:

```lua
-- Runs once, when the file is loaded.
terminale.log("hello plugin loaded")

-- Add an entry to the command palette (Ctrl+Shift+P).
terminale.register_command("Hello: greet me", function()
  terminale.notify("Hello", "Greetings from a Lua plugin!")
end)

-- React to a lifecycle event.
terminale.register_hook("command_end", function(ev)
  if ev.exit_code ~= 0 then
    terminale.notify("Command failed", ev.command .. " exited " .. ev.exit_code)
  end
end)
```

Restart `terminale` (plugins are loaded once at startup). You should see the
"Hello: greet me" command in the palette, and a desktop notification whenever a
shell command exits non-zero.

## The `terminale` API

All functions are namespaced under the global `terminale` table.

| Function | Effect |
|---|---|
| `terminale.log(msg)` | Write `msg` to the app log (target `terminale.plugin`). |
| `terminale.notify(title, body)` | Raise an OS desktop notification. |
| `terminale.set_tab_title(text)` | Rename the currently-active tab. |
| `terminale.open_tab()` | Open a new tab using the default profile. |
| `terminale.send_text(text)` | Write raw bytes to the focused pane's PTY. |
| `terminale.register_command(name, fn)` | Add `name` to the command palette; runs `fn` when chosen. |
| `terminale.register_hook(event, fn)` | Subscribe `fn` to a lifecycle `event` (see below). |
| `terminale.register_keybinding(combo, fn)` | Bind a shortcut (e.g. `"Ctrl+Shift+Y"`); runs `fn` when pressed. See [Permission model](#permission-model). |
| `terminale.get_selection()` | The focused pane's selection text; `""` when nothing is selected. |
| `terminale.get_scrollback(n)` | Array of the last `n` scrollback+screen lines (oldest-first). Gated; see below. |
| `terminale.get_visible_text()` | The visible screen joined with `\n`. Gated; see below. |

### How side effects are applied

Lua callbacks never mutate terminal state directly. Calls like `notify`,
`open_tab`, `set_tab_title`, and `send_text` **enqueue** a command that the host
applies on the main thread on the next tick. This keeps plugins free of
re-entrancy and borrow hazards — you just call the function and the effect lands
shortly after.

### Reading terminal state

The read APIs are synchronous and **copy-based**: once per tick, before any
hook fires, the app publishes a snapshot of the focused pane (selection,
scrollback, visible screen). `get_selection` / `get_scrollback` /
`get_visible_text` return copies from that snapshot — a plugin never holds a
live reference into terminal state, and the data is at most one tick old.

- `get_selection()` always works (a selection is content the user actively
  marked) and returns `""` — never `nil` — when nothing is selected.
- `get_scrollback(n)` returns up to `n` lines, newest-last; `n` omitted or
  `0` means "everything the snapshot carries". The snapshot itself is capped
  at `plugins.scrollback_read_cap` lines (default 10 000).
- `get_visible_text()` is the on-screen slice of the same content.

### Permission model

Two settings (Settings → Plugins, applied live — no restart) gate what
plugins may do:

| Setting | Default | Meaning |
|---|---|---|
| `plugins.allow_scrollback_read` | `false` | Master switch for `get_scrollback` / `get_visible_text`. Off = they return empty results. Off by default because terminal output can contain secrets. |
| `plugins.scrollback_read_cap` | `10000` | Upper bound on lines a plugin can read per call (max 200 000). |
| `plugins.allow_keybindings` | `true` | Master switch for `register_keybinding`. Off = registration is a logged no-op and existing plugin bindings stop firing. |

Plugin keybindings **extend** the keymap, they never override it: a combo
that collides with one of your `[shortcuts]` bindings or a
`[[keybinds.custom]]` entry is refused with a warning in the log. Precedence
is: your custom keybinds → your config shortcuts → plugin keybindings →
built-in fallbacks.

Example — grep the scrollback and act on the selection:

```lua
-- Ctrl+Shift+U: count how many visible lines mention "error".
terminale.register_keybinding("Ctrl+Shift+U", function()
  local n = 0
  for _, line in ipairs(terminale.get_scrollback(500)) do
    if line:lower():find("error", 1, true) then n = n + 1 end
  end
  terminale.notify("Scrollback scan", n .. " line(s) mention 'error'")
end)

-- Ctrl+Shift+M: shout the current selection back into the pane.
terminale.register_keybinding("Ctrl+Shift+M", function()
  local sel = terminale.get_selection()
  if sel ~= "" then terminale.send_text(sel:upper()) end
end)
```

(Remember: `get_scrollback` needs `plugins.allow_scrollback_read = true`.)

## Hooks

Subscribe with `terminale.register_hook(event, handler)`. The handler receives a
single table argument with fields specific to the event. A handler that raises
an error is logged and **dropped** (so a buggy plugin won't spam the log), but
the host stays healthy.

| Event | Payload fields | Fires when |
|---|---|---|
| `"tick"` | *(none)* | Once per main-loop tick. Keep the work tiny. |
| `"session_start"` | `pane_id`, `program` | A shell/program starts in a pane. |
| `"session_exit"` | `pane_id`, `exit_code` | The program in a pane exits. |
| `"tab_open"` | `tab_id`, `title` | A new tab is opened. |
| `"tab_close"` | `tab_id` | A tab is closed. |
| `"pane_focus"` | `pane_id` | Focus moves to a pane. |
| `"command_end"` | `exit_code`, `command`, `cwd` | A shell command finishes (requires shell integration / OSC 133). |
| `"config_reload"` | *(none)* | The config file is reloaded. |

Example — auto-title a tab from the running program:

```lua
terminale.register_hook("session_start", function(ev)
  terminale.set_tab_title(ev.program .. "  [pane " .. ev.pane_id .. "]")
end)
```

## Sandbox

The host strips the parts of the Lua stdlib that could touch the filesystem,
spawn processes, or load native code. The following are **removed** and will be
`nil`:

- Globals: `io`, `package`, `debug`, `dofile`, `loadfile`, `load`, `require`
- From `os`: `execute`, `exit`, `remove`, `rename`, `tmpname`, `getenv`,
  `setlocale`

Everything else in the standard library stays available — notably `math`,
`string`, `table`, and the safe parts of `os` (e.g. `os.time`, `os.date`,
`os.clock`). All terminal interaction must go through the `terminale` table.

## Tips

- **Plugins load once, at startup.** Restart the app (or trigger a config
  reload) after editing a plugin.
- **`tick` runs very often** — never block or do heavy work in it.
- **Prefix your globals** (`myplugin_state = {}`) since the Lua state is shared.
- **Errors are non-fatal** — check the app log for `plugin load failed` /
  `lua hook failed` messages while developing.

## Roadmap

Selection/scrollback reads and keybinding registration shipped (see the API
table above). Planned next: richer pane/tab queries and per-plugin
permission scoping. See [`roadmap.md`](roadmap.md).
