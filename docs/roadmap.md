# Roadmap

`terminale` is pre-release. This is the honest state of what works and what's
planned. Dates are intentionally omitted — items ship when they're ready and
tested.

## Working today

- Cross-platform PTY shell (ConPTY on Windows, `openpty` on Unix)
- GPU rendering via `wgpu` (Vulkan / Metal / DX12 / GL), glyph atlas, ligatures
- Multi-tab with reopen-closed-tab, live titles (OSC 0/2), tab groups, pinning
- Drag a tab out of the bar → it becomes its own window (and drag it back)
- Split panes (horizontal / vertical, arbitrarily nestable, focus + swap + zoom)
- Proactive AI command-suggestion bar
- Command palette and inline AI assistant (Claude / OpenAI / Ollama)
- Built-in SSH: hosts in config (secrets in the OS keychain), quick-connect
  button, and a "save this host?" prompt when you type `ssh …`
- Shell integration (OSC 133): prompt marks, command blocks with exit codes,
  jump-to-failed-command, "fix this command" on failure
- Inline images: Sixel, APC graphics, OSC 1337 `File=`
- 12 built-in themes with live switching + user themes
- Full-scrollback search
- Clickable links (OSC 8, auto-detected URLs, `file:line:col`)
- Quake drop-down mode and window snapping
- Shell profiles with auto-detection
- Live-applied TOML config + native settings window
- Configurable bell, bracketed paste, OSC 52 clipboard, OSC 7 cwd
- Copy mode (vim motions), quick-select, snippets, clipboard history,
  directory jump, broadcast input, zen mode, saved workspaces
- Lua plugin host (sandboxed): hooks, palette commands, keybindings,
  selection/scrollback reads behind a permission model
- Opt-in self-update from GitHub releases (SHA-256-verified, applies on next
  launch — never a forced restart)
- Control API (`terminale ctl`, Unix): list tabs/panes, read a pane, last
  command + exit code, run any action, type at the prompt, screenshot. Scoped
  permissions, with "may press Enter" off by default —
  see [`control-api.md`](control-api.md)
- MCP server (`terminale mcp`, Unix): the same commands as MCP tools, so an AI
  agent can read what your last command printed instead of asking you to paste
  it. Same permissions, no separate ones — see [`mcp.md`](mcp.md)

## Planned

Milestones match the placeholder crates in `Cargo.toml`; pre-1.0 minors may
include breaking changes.

### v1.0 — persistent sessions
- Persistent-session multiplexer (`terminale-ipc`): sessions survive a window
  crash / close and reattach on launch

### v1.5 — tmux integration
- tmux Control Mode (`tmux -CC`) integration (`terminale-tmux-cc`): native
  tabs/panes driving a remote tmux session

### v2.0 — sync & automation
- Cloud settings sync (`terminale-sync`)
- Control API: a Windows named-pipe transport (the command vocabulary is already
  platform-independent), and an event subscription so a client — `ctl` or MCP —
  can watch instead of poll
- Richer Lua plugin API: pane/tab queries, per-plugin permission scoping

## Releases

Installers for Windows, macOS, and Linux are produced automatically by the
release pipeline on every tag. Binaries are published unsigned.

Have a request? Open a [discussion](https://github.com/fbrzlarosa/terminale/discussions)
or an [issue](https://github.com/fbrzlarosa/terminale/issues).
