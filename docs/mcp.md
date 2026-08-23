# MCP server — `terminale mcp`

Let an AI agent see the terminal you are working in.

```bash
claude mcp add terminale -- terminale mcp
```

That is the whole setup. The agent now has ten tools: it can list your tabs,
read a pane, fetch the last command with its exit code, run a palette action,
compose a command at your prompt, and take a screenshot — under the permissions
you already set for [`terminale ctl`](control-api.md).

Unix only, like the control socket it forwards to.

---

## Why, when `ctl` already exists

`terminale ctl` is the human spelling: you know the vocabulary, you read the
refusals. An agent knows neither. MCP (Model Context Protocol) is how an agent is
handed a *typed* tool list at connect time — a schema per tool, and a
machine-readable error when one is refused — so "look at what my build printed"
stops being something you copy-paste and becomes something it can ask for.

Nothing new is exposed. `terminale mcp` is a translator: one tool call becomes
one control request on the same socket, checked by the same gate.

## The tools

| Tool | What it answers |
|---|---|
| `list_tabs` | What is open: index, title, cwd, size, active / crashed / unread |
| `list_panes` | The split panes of one tab, with their ids |
| `get_text` | A pane's text — visible screen, or the whole scrollback |
| `last_command` | The last finished command: input, output, exit code (needs OSC 133) |
| `list_actions` | Every action `run_action` accepts, with its key binding |
| `run_action` | Run one action by name (`SplitRight`, `NewTab`, …) |
| `send_text` | Type at a pane's prompt — **left there for you**, unless `allow_submit` |
| `send_keys` | Key presses (`ctrl+c`, `escape`, `down down enter`) |
| `screenshot` | PNG of the focused window to an absolute path |
| `version` | Running version + the permissions currently in effect |

`last_command` is the one worth telling an agent about: it is scoped to a single
command, so diagnosing a failure costs a fraction of the tokens that reading a
whole screen does.

## Permissions

There is no separate MCP permission model — that would be a second thing to keep
in sync, and a false sense of one. Every call goes through
`[integration.control_api]`:

```toml
[integration.control_api]
allow_read       = true    # get_text, last_command, titles and cwds
allow_input      = true    # send_text, send_keys, run_action
allow_submit     = false   # ← press Enter. OFF by default.
allow_screenshot = true

[integration.mcp]
enabled = true             # whether `terminale mcp` answers at all
```

With `allow_submit` off — the default — an agent can compose a command at your
prompt and cannot run it. That is the same contract the in-app AI features use:
you press Enter.

A refused tool comes back as a *failed tool call*, not a broken connection, and
the message names the setting to change:

```
`send-text` refused: set `integration.control_api.allow_input = true` to allow it
```

so the agent can tell you what to turn on instead of retrying.

`[integration.mcp].enabled` (Settings → Desktop integration → **Serve MCP to AI
agents**) decides whether this front-end answers. It is a convenience switch, not
a boundary: anything that can run `terminale mcp` can open the socket itself.
`[integration.control_api]` is the boundary.

## Registering it elsewhere

Any MCP client that speaks the stdio transport works — the command is always
`terminale mcp` with no arguments.

```jsonc
// e.g. an MCP client's config file
{
  "mcpServers": {
    "terminale": { "command": "terminale", "args": ["mcp"] }
  }
}
```

## Protocol notes

* stdio transport, JSON-RPC 2.0, one message per line. Stdout carries the
  protocol and nothing else; diagnostics go to stderr.
* Revisions `2024-11-05`, `2025-03-26` and `2025-06-18` are accepted; a client
  asking for one of those is answered with the same one.
* `tools/list` and `tools/call` are implemented, plus `initialize` and `ping`.
  Resources and prompts are not — there is nothing a terminal would put there
  that a tool does not already answer.
* No event subscription yet: a client polls. Watching instead of polling is on
  the [roadmap](roadmap.md).

## Troubleshooting

**"could not reach terminale"** — no instance is listening. Start terminale, and
check `integration.control_socket = true` (changing it needs a relaunch).

**The agent sees no tools** — `[integration.mcp].enabled` is off, or the client
never got past `initialize`. Run `terminale mcp` by hand and paste one line:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | terminale mcp
```

A tool list on stdout means the server is fine and the client's configuration is
what to look at.
