//! MCP (Model Context Protocol) front-end for the control API.
//!
//! `terminale mcp` speaks MCP over stdio and forwards every tool call to the
//! running instance's control socket. It is a translator, not a second
//! automation surface: each tool is one [`ControlRequest`], the reply comes back
//! through [`crate::ipc::send_request`], and the permission gate that decides
//! what is allowed is the one already in [`crate::control::permits`] — inside
//! the app, where the config lives.
//!
//! Why this exists rather than "an agent can just run `terminale ctl`": an agent
//! that shells out has to be *told* the vocabulary, and it discovers refusals by
//! parsing prose. MCP is how agents are handed a typed tool list they can read
//! at connect time, with a schema per tool and a machine-readable error when one
//! is refused. That turns "read the terminal" from something the user has to
//! copy-paste into something the agent can ask for — which is the whole point of
//! the control API existing.
//!
//! # What it is not
//!
//! [`terminale_config::McpConfig::enabled`] is a convenience switch, not a
//! security boundary: anything that can spawn `terminale mcp` can also open the
//! socket itself, so turning MCP off refuses MCP *clients*, nothing more. The
//! boundary is `[integration.control_api]` — most of all `allow_submit`, which
//! stays off by default so an agent can compose a command at your prompt but
//! cannot run it.
//!
//! # Transport
//!
//! MCP's stdio transport is JSON-RPC 2.0, one message per line, requests in on
//! stdin and replies out on stdout. Nothing else may go to stdout — a stray
//! `println!` corrupts the stream — so diagnostics go to stderr, which the host
//! is free to log.

use std::io::{BufRead, Write};

use crate::control::{ControlReply, ControlRequest};

/// MCP revision this server implements.
///
/// Echoed back only when the client asks for a revision we know; a client that
/// asks for anything else is answered with this one and decides for itself
/// whether to continue, which is what the spec's version negotiation says to do.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Revisions whose message shapes this server is compatible with.
const KNOWN_PROTOCOL_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

// ── JSON-RPC error codes ─────────────────────────────────────────────────────

const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;

// ── Tool table ───────────────────────────────────────────────────────────────

/// One exposed tool: the MCP name, what it is for, and its input schema.
struct Tool {
    /// Tool name as the client sees it. Snake case, because that is what every
    /// published MCP server uses and agents pattern-match on it.
    name: &'static str,
    /// One-line description. Written for the agent, not for a manual: it has to
    /// convey *when* to reach for the tool.
    description: &'static str,
    /// JSON Schema for the arguments object.
    schema: fn() -> serde_json::Value,
}

/// Every tool this server exposes, in the order `tools/list` reports them.
///
/// Deliberately not the whole [`ControlRequest`] vocabulary: `ping` and
/// `toggle-quake` are liveness and window management, which an agent has no use
/// for and which would only add noise to a tool list it has to read on every
/// connect.
const TOOLS: &[Tool] = &[
    Tool {
        name: "list_tabs",
        description: "List every window's tabs: index, title, working directory, size, and \
                      whether a tab is active, crashed, or has unread output. Start here to \
                      find out what is open.",
        schema: || serde_json::json!({ "type": "object", "properties": {} }),
    },
    Tool {
        name: "list_panes",
        description: "List the split panes of one tab, with their ids and sizes. Needed only \
                      when a tab is split and you must address a specific pane.",
        schema: || {
            serde_json::json!({
                "type": "object",
                "properties": { "tab": tab_arg() }
            })
        },
    },
    Tool {
        name: "get_text",
        description: "Read a pane's text — the visible screen by default, the whole \
                      scrollback with `scrollback: true`. This is how you see what a command \
                      printed instead of asking the user to paste it.",
        schema: || {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "tab": tab_arg(),
                    "pane": pane_arg(),
                    "scrollback": {
                        "type": "boolean",
                        "description": "Include scrollback history, not just the visible screen."
                    }
                }
            })
        },
    },
    Tool {
        name: "last_command",
        description: "The most recent finished command in a pane: what was typed, what it \
                      printed, and its exit code. Prefer this over get_text when diagnosing \
                      a failure — it is scoped to one command. Needs shell integration \
                      (OSC 133) in the running shell.",
        schema: || {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "tab": tab_arg(),
                    "pane": pane_arg(),
                    "max_lines": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Cap on returned output lines; the tail is kept and the \
                                        reply says whether it truncated."
                    }
                }
            })
        },
    },
    Tool {
        name: "list_actions",
        description: "List every action run_action accepts, with its label and current key \
                      binding. Call this before run_action rather than guessing a name.",
        schema: || serde_json::json!({ "type": "object", "properties": {} }),
    },
    Tool {
        name: "run_action",
        description: "Run one terminale action by name — anything in the command palette, \
                      e.g. SplitRight, NewTab, ToggleZenMode. Names come from list_actions \
                      and are case-insensitive.",
        schema: || {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Action name, e.g. `SplitRight`." }
                },
                "required": ["name"]
            })
        },
    },
    Tool {
        name: "send_text",
        description: "Type text at a pane's prompt, exactly as if the user had typed it. It \
                      is left at the prompt for the user to read and run: submitting needs \
                      `submit: true` *and* `integration.control_api.allow_submit`, which is \
                      off by default.",
        schema: || {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The text to type." },
                    "tab": tab_arg(),
                    "pane": pane_arg(),
                    "submit": {
                        "type": "boolean",
                        "description": "Also press Enter, actually running the command. \
                                        Refused unless the user enabled allow_submit."
                    }
                },
                "required": ["text"]
            })
        },
    },
    Tool {
        name: "send_keys",
        description: "Send key presses — \"ctrl+c\", \"escape\", \"down down enter\" — encoded \
                      for the pane's current modes. Use it to answer a prompt or interrupt a \
                      running program, not to type text.",
        schema: || {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "keys": {
                        "type": "string",
                        "description": "Space-separated key specs, e.g. `ctrl+c` or `down enter`."
                    },
                    "tab": tab_arg(),
                    "pane": pane_arg()
                },
                "required": ["keys"]
            })
        },
    },
    Tool {
        name: "screenshot",
        description: "Write a PNG of the focused window's next frame to an absolute path. For \
                      when the question is about what the terminal *looks* like — colours, \
                      layout, a rendering glitch — rather than what it says.",
        schema: || {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute destination path for the PNG."
                    }
                },
                "required": ["path"]
            })
        },
    },
    Tool {
        name: "version",
        description: "The running terminale's version and which control-API permissions are \
                      in effect. Call it when a tool is refused, to report what the user \
                      would have to turn on.",
        schema: || serde_json::json!({ "type": "object", "properties": {} }),
    },
];

/// Schema fragment for the optional `tab` argument, which every pane-addressed
/// tool shares.
fn tab_arg() -> serde_json::Value {
    serde_json::json!({
        "type": "integer",
        "minimum": 0,
        "description": "Tab index from list_tabs. Omit for the active tab."
    })
}

/// Schema fragment for the optional `pane` argument.
fn pane_arg() -> serde_json::Value {
    serde_json::json!({
        "type": "integer",
        "minimum": 0,
        "description": "Pane id from list_panes. Omit for the tab's focused pane."
    })
}

// ── Argument extraction ──────────────────────────────────────────────────────

/// Read an optional `usize` argument, rejecting a value of the wrong type
/// rather than silently ignoring it — a tool that quietly used the active tab
/// when the agent asked for tab 3 would be worse than an error.
fn opt_usize(args: &serde_json::Value, key: &str) -> Result<Option<usize>, String> {
    match args.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a non-negative integer")),
    }
}

/// Read an optional `u32` argument (pane ids).
fn opt_u32(args: &serde_json::Value, key: &str) -> Result<Option<u32>, String> {
    match args.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a non-negative integer")),
    }
}

/// Read an optional boolean argument, defaulting to `false`.
fn opt_bool(args: &serde_json::Value, key: &str) -> Result<bool, String> {
    match args.get(key) {
        None | Some(serde_json::Value::Null) => Ok(false),
        Some(v) => v
            .as_bool()
            .ok_or_else(|| format!("`{key}` must be a boolean")),
    }
}

/// Read a required string argument.
fn req_str(args: &serde_json::Value, key: &str) -> Result<String, String> {
    match args.get(key) {
        Some(serde_json::Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(format!("`{key}` must be a string")),
        None => Err(format!("`{key}` is required")),
    }
}

/// Translate one `tools/call` into the control request it stands for.
///
/// Kept a pure function so the whole mapping — including every argument
/// rejection — is unit-testable without a socket or a running instance.
fn tool_to_request(name: &str, args: &serde_json::Value) -> Result<ControlRequest, String> {
    Ok(match name {
        "list_tabs" => ControlRequest::ListTabs,
        "list_panes" => ControlRequest::ListPanes {
            tab: opt_usize(args, "tab")?,
        },
        "get_text" => ControlRequest::GetText {
            tab: opt_usize(args, "tab")?,
            pane: opt_u32(args, "pane")?,
            scrollback: opt_bool(args, "scrollback")?,
        },
        "last_command" => ControlRequest::LastCommand {
            tab: opt_usize(args, "tab")?,
            pane: opt_u32(args, "pane")?,
            max_lines: opt_usize(args, "max_lines")?,
        },
        "list_actions" => ControlRequest::ListActions,
        "run_action" => ControlRequest::Action {
            name: req_str(args, "name")?,
        },
        "send_text" => ControlRequest::SendText {
            text: req_str(args, "text")?,
            tab: opt_usize(args, "tab")?,
            pane: opt_u32(args, "pane")?,
            submit: opt_bool(args, "submit")?,
        },
        "send_keys" => ControlRequest::SendKeys {
            keys: req_str(args, "keys")?,
            tab: opt_usize(args, "tab")?,
            pane: opt_u32(args, "pane")?,
        },
        "screenshot" => {
            // The running instance's working directory is not the caller's, and
            // an agent's idea of "here" is a third place again — so a relative
            // path is refused rather than resolved against a directory nobody
            // meant.
            let path = std::path::PathBuf::from(req_str(args, "path")?);
            if !path.is_absolute() {
                return Err("`path` must be absolute".into());
            }
            ControlRequest::Screenshot { path }
        }
        "version" => ControlRequest::Version,
        other => return Err(format!("unknown tool `{other}`")),
    })
}

// ── JSON-RPC plumbing ────────────────────────────────────────────────────────

/// A successful JSON-RPC response.
fn rpc_result(id: serde_json::Value, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// A JSON-RPC error response — for protocol-level failures only. A tool that
/// ran and was refused is a *successful* call carrying `isError`, per MCP: the
/// agent is meant to read the refusal and adapt, not treat it as a broken
/// connection.
fn rpc_error(id: serde_json::Value, code: i32, message: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() }
    })
}

/// Shape a control reply as an MCP tool result.
///
/// Payloads go out as pretty JSON text: MCP content blocks are text, and an
/// agent reads indented JSON more reliably than one long line. `get_text` is
/// special-cased to return the pane's text itself — wrapping a screen of
/// terminal output in a JSON string quotes every newline and doubles the tokens
/// for no gain.
fn tool_result(reply: &ControlReply) -> serde_json::Value {
    let text = if let Some(err) = &reply.error {
        err.clone()
    } else {
        match &reply.data {
            Some(serde_json::Value::Object(map)) if map.len() == 1 => match map.get("text") {
                Some(serde_json::Value::String(s)) => s.clone(),
                _ => pretty(&reply.data),
            },
            Some(_) => pretty(&reply.data),
            None => "ok".to_string(),
        }
    };
    serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "isError": !reply.ok
    })
}

/// Pretty-print a payload, falling back to its compact form.
fn pretty(data: &Option<serde_json::Value>) -> String {
    let Some(data) = data else {
        return "ok".to_string();
    };
    serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string())
}

/// Handle one incoming line and return the line to write back, if any.
///
/// `send` is the transport to the running instance, injected so every branch
/// here is testable against a stub. Returns `None` for a notification, which
/// JSON-RPC says must not be answered.
fn handle_line(
    line: &str,
    send: &mut dyn FnMut(&ControlRequest) -> std::io::Result<ControlReply>,
) -> Option<String> {
    let msg: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(rpc_error(serde_json::Value::Null, PARSE_ERROR, e.to_string()).to_string())
        }
    };
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(|m| m.as_str()) else {
        // No method at all: if it carried an id it was a malformed request, and
        // if it did not it is a response to something we never sent — ignore.
        let id = id?;
        return Some(rpc_error(id, INVALID_REQUEST, "missing `method`").to_string());
    };
    // A message without an id is a notification: act on it, answer nothing.
    let id = id?;
    let params = msg
        .get("params")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    let response = match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or(PROTOCOL_VERSION);
            let version = if KNOWN_PROTOCOL_VERSIONS.contains(&asked) {
                asked
            } else {
                PROTOCOL_VERSION
            };
            rpc_result(
                id,
                serde_json::json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "terminale",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "instructions":
                        "Tools drive the terminale terminal the user is working in. \
                         `list_tabs` first to see what is open; `last_command` to find out \
                         why something failed; `send_text` leaves a command at the prompt \
                         for the user to run unless they enabled allow_submit."
                }),
            )
        }
        "ping" => rpc_result(id, serde_json::json!({})),
        "tools/list" => rpc_result(
            id,
            serde_json::json!({
                "tools": TOOLS
                    .iter()
                    .map(|t| serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": (t.schema)(),
                    }))
                    .collect::<Vec<_>>()
            }),
        ),
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            match tool_to_request(name, &args) {
                // A bad tool name is a protocol error; bad arguments are a
                // failed call the agent can correct and retry.
                Err(e) if e.starts_with("unknown tool") => rpc_error(id, METHOD_NOT_FOUND, e),
                Err(e) => rpc_error(id, INVALID_PARAMS, e),
                Ok(req) => match send(&req) {
                    Ok(reply) => rpc_result(id, tool_result(&reply)),
                    // The instance is gone or wedged. Report it as a tool
                    // failure with the reason, so the agent can tell the user
                    // "terminale isn't running" instead of retrying forever.
                    Err(e) => rpc_result(
                        id,
                        tool_result(&ControlReply::err(format!(
                            "could not reach terminale: {e}. Is it running, and is \
                             `integration.control_socket` on?"
                        ))),
                    ),
                },
            }
        }
        other => rpc_error(id, METHOD_NOT_FOUND, format!("unknown method `{other}`")),
    };
    Some(response.to_string())
}

/// Serve MCP on stdin/stdout until the client closes the stream, then exit.
///
/// # Errors
///
/// Only a stdout write failure, which means the host is gone; a malformed
/// request is answered on the wire rather than ending the session.
pub(crate) fn serve() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut send = |req: &ControlRequest| crate::ipc::send_request(req);
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_line(&line, &mut send) {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transport stub: records what was asked, answers with a fixed reply.
    fn stub(
        seen: &mut Vec<ControlRequest>,
        reply: ControlReply,
    ) -> impl FnMut(&ControlRequest) -> std::io::Result<ControlReply> + '_ {
        move |req| {
            seen.push(req.clone());
            Ok(reply.clone())
        }
    }

    fn call(name: &str, args: serde_json::Value) -> String {
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": args }
        })
        .to_string()
    }

    /// `initialize` must answer with a protocol version, the tools capability
    /// and a server identity, or no client will proceed to `tools/list`.
    #[test]
    fn initialize_advertises_tools() {
        let mut seen = Vec::new();
        let mut send = stub(&mut seen, ControlReply::ok());
        let out = handle_line(
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-03-26" }
            })
            .to_string(),
            &mut send,
        )
        .expect("initialize is a request, so it must be answered");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        // A version the client asked for and we know is echoed back verbatim.
        assert_eq!(v["result"]["protocolVersion"], "2025-03-26");
        assert!(v["result"]["capabilities"]["tools"].is_object());
        assert_eq!(v["result"]["serverInfo"]["name"], "terminale");
    }

    /// A revision we do not know must not be echoed: the client has to be told
    /// what it is actually talking to.
    #[test]
    fn unknown_protocol_version_is_answered_with_ours() {
        let mut seen = Vec::new();
        let mut send = stub(&mut seen, ControlReply::ok());
        let out = handle_line(
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "1999-01-01" }
            })
            .to_string(),
            &mut send,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    /// Every tool must carry a name, a description and an object schema — a
    /// client that gets a tool without them may drop it silently.
    #[test]
    fn every_tool_is_fully_described() {
        let mut seen = Vec::new();
        let mut send = stub(&mut seen, ControlReply::ok());
        let out = handle_line(
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#,
            &mut send,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let tools = v["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), TOOLS.len());
        for t in tools {
            assert!(t["name"].as_str().is_some_and(|s| !s.is_empty()));
            assert!(t["description"].as_str().is_some_and(|s| s.len() > 20));
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }

    /// The tool list is a contract: renaming one silently breaks every agent
    /// configuration that referenced it.
    #[test]
    fn tool_names_are_stable() {
        let names: Vec<_> = TOOLS.iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            vec![
                "list_tabs",
                "list_panes",
                "get_text",
                "last_command",
                "list_actions",
                "run_action",
                "send_text",
                "send_keys",
                "screenshot",
                "version",
            ]
        );
    }

    /// Arguments must reach the control request unchanged — this is the whole
    /// job of the translator.
    #[test]
    fn arguments_map_onto_the_control_request() {
        let mut seen = Vec::new();
        {
            let mut send = stub(&mut seen, ControlReply::ok());
            handle_line(
                &call(
                    "get_text",
                    serde_json::json!({ "tab": 2, "pane": 5, "scrollback": true }),
                ),
                &mut send,
            );
        }
        assert_eq!(
            seen,
            vec![ControlRequest::GetText {
                tab: Some(2),
                pane: Some(5),
                scrollback: true
            }]
        );
    }

    /// Omitted optional arguments mean "whatever is focused", not zero.
    #[test]
    fn omitted_arguments_stay_none() {
        let mut seen = Vec::new();
        {
            let mut send = stub(&mut seen, ControlReply::ok());
            handle_line(&call("get_text", serde_json::json!({})), &mut send);
        }
        assert_eq!(
            seen,
            vec![ControlRequest::GetText {
                tab: None,
                pane: None,
                scrollback: false
            }]
        );
    }

    /// A `tab` of the wrong type must fail loudly. Ignoring it would send the
    /// request to the active tab, which is not what was asked for.
    #[test]
    fn a_mistyped_argument_is_refused_not_ignored() {
        let mut seen = Vec::new();
        let out = {
            let mut send = stub(&mut seen, ControlReply::ok());
            handle_line(
                &call("get_text", serde_json::json!({ "tab": "two" })),
                &mut send,
            )
            .unwrap()
        };
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], INVALID_PARAMS);
        assert!(seen.is_empty(), "nothing may reach the instance");
    }

    /// A missing required argument is the agent's mistake to correct, so it
    /// comes back as invalid params rather than a dropped connection.
    #[test]
    fn a_missing_required_argument_is_invalid_params() {
        let mut seen = Vec::new();
        let mut send = stub(&mut seen, ControlReply::ok());
        let out = handle_line(&call("run_action", serde_json::json!({})), &mut send).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], INVALID_PARAMS);
    }

    /// A relative screenshot path would land in whichever directory the
    /// instance happens to be in, so it is refused before it is sent.
    #[test]
    fn a_relative_screenshot_path_is_refused() {
        let mut seen = Vec::new();
        let out = {
            let mut send = stub(&mut seen, ControlReply::ok());
            handle_line(
                &call("screenshot", serde_json::json!({ "path": "shot.png" })),
                &mut send,
            )
            .unwrap()
        };
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], INVALID_PARAMS);
        assert!(seen.is_empty());
    }

    /// A refusal from the permission gate is a *successful* call with
    /// `isError`, carrying the message that names the setting to change — that
    /// is what lets an agent tell the user what to turn on.
    #[test]
    fn a_refusal_becomes_an_error_result_not_a_protocol_error() {
        let mut seen = Vec::new();
        let refusal = ControlReply::err(
            "`send-text` refused: set `integration.control_api.allow_input = true` to allow it",
        );
        let mut send = stub(&mut seen, refusal);
        let out = handle_line(
            &call("send_text", serde_json::json!({ "text": "ls" })),
            &mut send,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["error"].is_null(), "not a protocol error");
        assert_eq!(v["result"]["isError"], true);
        assert!(v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("allow_input"));
    }

    /// Pane text comes back as text. Wrapping a screenful of output in a JSON
    /// string escapes every newline and doubles what the agent has to read.
    #[test]
    fn pane_text_is_returned_verbatim() {
        let mut seen = Vec::new();
        let mut send = stub(
            &mut seen,
            ControlReply::with(serde_json::json!({ "text": "line one\nline two" })),
        );
        let out = handle_line(&call("get_text", serde_json::json!({})), &mut send).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["result"]["content"][0]["text"], "line one\nline two");
        assert_eq!(v["result"]["isError"], false);
    }

    /// An unknown tool is a protocol error: there is nothing for the agent to
    /// fix in its arguments.
    #[test]
    fn an_unknown_tool_is_method_not_found() {
        let mut seen = Vec::new();
        let mut send = stub(&mut seen, ControlReply::ok());
        let out = handle_line(&call("rm_rf", serde_json::json!({})), &mut send).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], METHOD_NOT_FOUND);
    }

    /// An unknown method likewise, and it must not take the session down.
    #[test]
    fn an_unknown_method_is_answered_not_fatal() {
        let mut seen = Vec::new();
        let mut send = stub(&mut seen, ControlReply::ok());
        let out = handle_line(
            r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#,
            &mut send,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], METHOD_NOT_FOUND);
    }

    /// Notifications carry no id and must not be answered — a reply to one is
    /// a protocol violation that some clients treat as fatal.
    #[test]
    fn notifications_are_not_answered() {
        let mut seen = Vec::new();
        let mut send = stub(&mut seen, ControlReply::ok());
        assert!(handle_line(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &mut send
        )
        .is_none());
    }

    /// Garbage in must produce a parse error with a null id, not a panic and
    /// not silence.
    #[test]
    fn malformed_json_gets_a_parse_error() {
        let mut seen = Vec::new();
        let mut send = stub(&mut seen, ControlReply::ok());
        let out = handle_line("{not json", &mut send).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], PARSE_ERROR);
        assert!(v["id"].is_null());
    }

    /// With no instance listening the call fails as a tool error naming the
    /// cause, so the agent stops retrying and says what is wrong.
    #[test]
    fn an_unreachable_instance_is_an_error_result() {
        let mut send = |_: &ControlRequest| {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no such file",
            ))
        };
        let out = handle_line(&call("list_tabs", serde_json::json!({})), &mut send).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["result"]["isError"], true);
        assert!(v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("could not reach terminale"));
    }
}
