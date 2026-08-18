# Control API — `terminale ctl`

Drive a running `terminale` from a script, an editor, or an AI coding agent.

```bash
terminale ctl list-tabs
terminale ctl last-command            # command + output + exit code
terminale ctl send-text "cargo test"  # types it at the prompt; does NOT run it
terminale ctl action SplitRight
terminale ctl screenshot ~/shot.png
```

Unix only for now (the transport is a per-user Unix socket under
`$XDG_RUNTIME_DIR`). A Windows named-pipe transport is planned; the command
vocabulary is already platform-independent.

---

## Why

Two unrelated needs met in the same place.

A Wayland desktop keybinding can only *run a command*, so `terminale
--toggle-quake` has to reach the already-running instance — that is the socket's
original job, and it is unchanged.

The second is that a terminal is no longer driven only by a human. An agent
working in your repo wants to know what the last command printed and whether it
succeeded. The terminal already has that: the grid, the scrollback, and — via
OSC 133 shell integration — the exact boundaries and exit code of every command.
Exposing it beats having something scrape a screenshot or re-run your build.

## Enabling it

On by default, except for the one switch that matters:

```toml
[integration]
control_socket = true            # the channel itself; needs a relaunch to change

[integration.control_api]
enabled          = true
allow_read       = true          # read terminal content
allow_input      = true          # type into a shell / run palette actions
allow_submit     = false         # ← press Enter. OFF by default.
allow_screenshot = true
```

All of it is also in **Settings → Desktop integration → Automation & AI
control**, and the four scopes apply immediately.

`terminale ctl version` prints what is currently in effect:

```json
{
  "version": "0.1.42",
  "windows": 1,
  "permissions": { "enabled": true, "read": true, "input": true,
                   "submit": false, "screenshot": true }
}
```

## Commands

Every command takes `--tab <n>` and `--pane <id>` where it makes sense; both
default to whatever is focused. Output is JSON on stdout, so it pipes into `jq`.
A refusal or failure prints to stderr and exits `1`.

| Command | Scope | What it does |
|---|---|---|
| `ping` | always | Is an instance listening? Answered without touching the UI thread, so it works even while the app is busy. |
| `version` | always | Version + effective permissions. |
| `toggle-quake` | always | Show/hide the Quake drop-down. |
| `list-actions` | metadata | Every dispatchable action with its label and current key binding. |
| `action <name>` | input | Run one of them. Case-insensitive. |
| `list-tabs` | read | Every window's tabs: title, cwd, size, `active`, `unread`, `attention`, `alt_screen`, `crashed`. |
| `list-panes [--tab n]` | read | The split panes of a tab: id, title, cwd, `focused`, `zoomed`, size. |
| `get-text [--scrollback]` | read | A pane's text. Prints the text itself, not JSON. |
| `last-command [--max-lines n]` | read | The last command: what was typed, its output, `exit_code`, `cwd`, and `running`. |
| `send-text <text> [--submit]` | input / submit | Type at the prompt. |
| `send-keys <keys>` | input / submit | Key presses: `ctrl+c`, `escape`, `"down down enter"`. |
| `screenshot <path>` | screenshot | PNG of the focused window's next frame. Waits for the file, so the exit status is meaningful. |

### `send-keys` grammar

Whitespace separates presses, `+` separates modifiers from the key:

```bash
terminale ctl send-keys ctrl+c
terminale ctl send-keys "down down enter"
terminale ctl send-keys shift+tab
```

Modifiers: `ctrl`/`control`, `alt`/`opt`/`option`/`meta`, `shift`,
`super`/`cmd`/`win`. Keys: any single character, or `enter`, `tab`, `escape`,
`space`, `backspace`, `up`/`down`/`left`/`right`, `home`, `end`, `pageup`,
`pagedown`, `insert`, `delete`, `f1`–`f12`, and spelled-out punctuation
(`plus`, `minus`, `slash`, …). Names are case-insensitive.

Presses are encoded for the pane's **current modes**, using the same encoders
the live keyboard uses: application cursor-key mode (DECCKM) changes what
`up` sends, and if the program in that pane has engaged the kitty keyboard
protocol, `shift+enter` arrives as `CSI 13;2u` — distinguishable from a plain
`Enter`, which is the whole point.

`send-keys` writes to the **shell**, not to terminale's own UI. So it cannot
dismiss an overlay: with the command palette or the find bar open, `send-keys
escape` goes to the program in the pane, which is not what closes the palette.
Drive the UI with `action` instead (`action CommandPalette`, `action Find`, …).

### Typing vs running

`send-text` strips a trailing newline unless you pass `--submit`, and `--submit`
is refused unless `allow_submit = true`. `send-keys enter` counts as submitting
too, and so does a newline smuggled inside a `send-text` payload — the gate is on
the *effect*, not the spelling.

So the default posture is: **an automation tool may draft a command at your
prompt; you press Enter.** For scripted or CI use, turn `allow_submit` on
deliberately.

---

## Recipes

### Give an AI agent the failing command

```bash
terminale ctl last-command | jq -r '.command, .exit_code, .output'
```

`last-command` needs OSC 133 marks, which come from
`terminal.shell_integration = true` (the default) plus
`terminal.command_blocks = true`. Without them the command says so instead of
guessing from the grid.

### Wait for a long build, then react

```bash
while [ "$(terminale ctl last-command | jq -r .running)" = true ]; do sleep 2; done
terminale ctl last-command | jq -r 'if .exit_code == 0 then "ok" else .output end'
```

### Watch for a tab that wants attention

`list-tabs` exposes `attention`, set when a program in a background tab rings the
bell — which is how a finished agent turn announces itself:

```bash
terminale ctl list-tabs | jq -r '.windows[].tabs[] | select(.attention) | .title'
```

### Draft a command for review

```bash
terminale ctl send-text "git rebase -i HEAD~3"   # sits at the prompt
```

### Capture the UI

```bash
terminale ctl screenshot /tmp/terminale.png
```

The frame is rendered and read back by terminale itself, so it needs no
compositor screenshot permission and works the same on X11 and Wayland.

---

## Talking to the socket directly

The CLI is a convenience; the wire is one JSON request per connection, one JSON
reply, newline-terminated:

```bash
SOCK="${XDG_RUNTIME_DIR:-/tmp}/terminale.sock"
echo '{"cmd":"list-tabs"}' | socat - "UNIX-CONNECT:$SOCK"
```

Requests are the `cmd`-tagged form of each command above
(`{"cmd":"send-text","text":"ls","submit":false}`). Replies are
`{"ok":true,"data":{…}}` or `{"ok":false,"error":"…"}`.

Two exceptions for backward compatibility: the bare words `ping` and
`toggle-quake` are still accepted, and still answered with a bare `ok`, so
existing keybindings and one-liners keep working.

## Security notes

- The socket is per-user, mode `0600`, in `$XDG_RUNTIME_DIR` (which is itself
  `0700` and cleared at logout). Only processes running as you can reach it.
- "Running as you" is a real bar but not the same as your intent — plugins and
  agents run as you too. That is what the four scopes are for.
- `allow_read` is the privacy-relevant one: scrollback contains whatever your
  commands printed, tokens included. `allow_screenshot` leaks the same content as
  an image, which is why it is separate.
- Nothing here is a network service. There is no port, no remote access, and no
  authentication token to leak — reachability *is* the authorization.
- `control_socket = false` removes the whole surface, including the Quake toggle.
