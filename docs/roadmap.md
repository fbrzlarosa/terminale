# Roadmap

`terminale` is pre-release. This is the honest state of what works and what's
planned. Dates are intentionally omitted — items ship when they're ready and
tested.

## Working today

- Cross-platform PTY shell (ConPTY on Windows, `openpty` on Unix)
- GPU rendering via `wgpu` (Vulkan / Metal / DX12 / GL), glyph atlas, ligatures
- Multi-tab with reopen-closed-tab and live titles (OSC 0/2)
- Split panes (horizontal / vertical, arbitrarily nestable, focus + swap)
- Proactive AI command-suggestion bar
- Command palette and inline AI assistant (Claude / OpenAI / Ollama)
- 12 built-in themes with live switching + user themes
- Full-scrollback search
- Clickable links (OSC 8, auto-detected URLs, `file:line:col`)
- Quake drop-down mode and window snapping
- Shell profiles with auto-detection
- Live-applied TOML config + native settings window
- Configurable bell, bracketed paste, OSC 52 clipboard, OSC 7 cwd
- Lua plugin host (sandboxed)

## Planned

### Near term
- Drag-out tab → new window
- Wire the existing SSH client (`terminale-ssh`) into the UI
- Shell integration (OSC 133): prompt marks, "fix this command" on failure

### Medium term
- Persistent-session multiplexer and `tmux -CC` control-mode integration
- Inline image protocols: OSC 1337, APC graphics, Sixel
- Expanded Lua plugin API (selection/scrollback reads, keybinding registration)

### Longer term
- Cloud settings sync
- Opt-in auto-update
- RPC API for external automation

## Releases

Installers for Windows, macOS, and Linux are produced automatically by the
release pipeline on every tag. Binaries are published unsigned.

Have a request? Open a [discussion](https://github.com/fbrzlarosa/terminale/discussions)
or an [issue](https://github.com/fbrzlarosa/terminale/issues).
