//! `zenmon mcp`: the read-only commands as Model Context Protocol tools, over
//! stdio, for agents that speak MCP rather than run shell commands.
//!
//! Each tool call runs this same binary as `zenmon <globals> --json <command>`
//! and hands back what it printed. That keeps one implementation of every
//! command — the MCP tools cannot drift from the CLI, its output contracts or
//! its bounds — at the cost of a process per call, which is noise next to a
//! Zenoh session open.
//!
//! Only commands that read are exposed. Publishing, replaying and serving
//! queries change the network being watched; an agent that needs them can
//! still run the CLI, where the user sees the command.
//!
//! The protocol is small enough to speak directly: newline-delimited
//! JSON-RPC 2.0 with `initialize`, `tools/list`, `tools/call` and `ping`.

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Map, Value};
use zenmon_core::error::ZenmonError;

/// Protocol revision answered when the client does not name one.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// How a tool parameter reaches the command line.
enum Arg {
    /// Appended as-is, in declaration order.
    Positional,
    /// `--flag value`, or just `--flag` for a boolean.
    Flag(&'static str),
}

struct Param {
    name: &'static str,
    /// JSON Schema type: `string`, `integer` or `boolean`.
    ty: &'static str,
    required: bool,
    description: &'static str,
    arg: Arg,
    /// Used when the caller omits it — how `sub` stays bounded.
    default: Option<&'static str>,
}

struct Tool {
    name: &'static str,
    description: &'static str,
    command: &'static [&'static str],
    params: &'static [Param],
}

const fn param(
    name: &'static str,
    ty: &'static str,
    required: bool,
    description: &'static str,
    arg: Arg,
) -> Param {
    Param {
        name,
        ty,
        required,
        description,
        arg,
        default: None,
    }
}

const KEY: Param = param(
    "key_expr",
    "string",
    true,
    "Zenoh key expression, e.g. robot/** or robot/*/pose",
    Arg::Positional,
);
const MAX_PAYLOAD: Param = param(
    "max_payload_bytes",
    "integer",
    false,
    "Cap each payload preview to this many bytes",
    Arg::Flag("--max-payload-bytes"),
);

static TOOLS: &[Tool] = &[
    Tool {
        name: "nodes",
        description: "List the Zenoh nodes (routers, peers, clients) visible from this session.",
        command: &["nodes"],
        params: &[],
    },
    Tool {
        name: "discover",
        description: "List active key expressions (topics) seen on the network.",
        command: &["discover"],
        params: &[param(
            "key_expr",
            "string",
            false,
            "Restrict to keys matching this expression (default **)",
            Arg::Positional,
        )],
    },
    Tool {
        name: "subscribe",
        description: "Receive messages on a key expression for a bounded time and return them as NDJSON \
                      (one message per line, payloads decoded). Always bounded: by count and by duration.",
        command: &["sub"],
        params: &[
            KEY,
            Param {
                default: Some("20"),
                ..param("count", "integer", false, "Stop after this many messages (default 20)", Arg::Flag("--count"))
            },
            Param {
                default: Some("5s"),
                ..param("duration", "string", false, "Stop after this long, e.g. 5s or 500ms (default 5s)", Arg::Flag("--duration"))
            },
            MAX_PAYLOAD,
            param("qos", "boolean", false, "Include each message's QoS", Arg::Flag("--qos")),
            param(
                "detect_misses",
                "boolean",
                false,
                "Report samples lost in transit (only for publishers that number their samples)",
                Arg::Flag("--detect-misses"),
            ),
            param(
                "validate",
                "boolean",
                false,
                "Check payloads against the contract the server was started with",
                Arg::Flag("--validate"),
            ),
        ],
    },
    Tool {
        name: "query",
        description: "Send one Zenoh GET to queryables/storages on a key expression and return the replies.",
        command: &["query"],
        params: &[
            KEY,
            param("payload", "string", false, "Request payload (JSON text)", Arg::Flag("--payload")),
            param("timeout", "string", false, "Reply deadline, e.g. 5s (default 5s)", Arg::Flag("--timeout")),
            param("limit", "integer", false, "Return at most this many replies", Arg::Flag("--limit")),
            param(
                "since",
                "string",
                false,
                "History from storages: relative age (10m) or RFC3339 start",
                Arg::Flag("--since"),
            ),
            param("until", "string", false, "End of the history window", Arg::Flag("--until")),
            param(
                "consolidation",
                "string",
                false,
                "auto (one reply per key, default) or none (every queryable's reply)",
                Arg::Flag("--consolidation"),
            ),
        ],
    },
    Tool {
        name: "liveliness",
        description: "List liveliness tokens (which services declare themselves alive).",
        command: &["liveliness"],
        params: &[param(
            "key_expr",
            "string",
            false,
            "Restrict to tokens matching this expression (default **)",
            Arg::Positional,
        )],
    },
    Tool {
        name: "info",
        description: "This session's ID, mode, and the routers and peers it is connected to.",
        command: &["info"],
        params: &[],
    },
    Tool {
        name: "doctor",
        description: "Diagnose the connection: config, session, connectivity and admin-space checks.",
        command: &["doctor"],
        params: &[param("timeout", "string", false, "Overall deadline, e.g. 5s", Arg::Flag("--timeout"))],
    },
    Tool {
        name: "declarations",
        description: "Which subscribers, publishers, queryables and liveliness tokens routers know of on \
                      keys intersecting a key expression, and which sessions declared them. Answers \
                      'is anything subscribed to this?'. Needs a router.",
        command: &["admin", "decls"],
        params: &[
            param(
                "key_expr",
                "string",
                false,
                "Only declarations intersecting this (default **)",
                Arg::Positional,
            ),
            param(
                "kind",
                "string",
                false,
                "Only one kind: subscriber, publisher, queryable or token",
                Arg::Flag("--kind"),
            ),
        ],
    },
    Tool {
        name: "admin_get",
        description: "Raw Zenoh admin-space entries under a selector (starting with @/), payloads decoded.",
        command: &["admin", "get"],
        params: &[param(
            "selector",
            "string",
            false,
            "Admin-space selector (default @/*/router)",
            Arg::Positional,
        )],
    },
    Tool {
        name: "keyexpr_compare",
        description: "How two key expressions relate: intersect, include, equal. Pure, no network.",
        command: &["keyexpr"],
        params: &[
            param("a", "string", true, "First key expression", Arg::Positional),
            param("b", "string", true, "Second key expression", Arg::Positional),
        ],
    },
    Tool {
        name: "trace_read",
        description: "Read records from a capture store (zenmon capture --dir) without any network access.",
        command: &["trace", "read"],
        params: &[
            param("dir", "string", true, "Capture store directory", Arg::Positional),
            param("key", "string", false, "Key expression filter", Arg::Flag("--key")),
            param("since", "string", false, "Window start: relative (10m) or RFC3339", Arg::Flag("--since")),
            param("until", "string", false, "Window end: relative or RFC3339", Arg::Flag("--until")),
            Param {
                default: Some("100"),
                ..param("limit", "integer", false, "Return at most this many records (default 100)", Arg::Flag("--limit"))
            },
            param("last_per_key", "boolean", false, "Only the latest record per key", Arg::Flag("--last-per-key")),
            param("cursor", "string", false, "Continue from a previous page", Arg::Flag("--cursor")),
            MAX_PAYLOAD,
        ],
    },
    Tool {
        name: "trace_stats",
        description: "Per-topic rollup of a capture store: counts, rates, last values. No network access.",
        command: &["trace", "stats"],
        params: &[
            param("dir", "string", true, "Capture store directory", Arg::Positional),
            param("key", "string", false, "Key expression filter", Arg::Flag("--key")),
            param("since", "string", false, "Window start: relative (10m) or RFC3339", Arg::Flag("--since")),
            param("until", "string", false, "Window end: relative or RFC3339", Arg::Flag("--until")),
            param("top", "integer", false, "Only the N highest-volume topics", Arg::Flag("--top")),
            MAX_PAYLOAD,
        ],
    },
    Tool {
        name: "contract_show",
        description: "What the contract the server was started with says about a key: type, schema, endpoints.",
        command: &["contract", "show"],
        params: &[param("key", "string", true, "Observed or declared key expression", Arg::Positional)],
    },
];

fn tool_list() -> Value {
    let tools: Vec<Value> = TOOLS
        .iter()
        .map(|t| {
            let mut props = Map::new();
            let mut required = Vec::new();
            for p in t.params {
                props.insert(
                    p.name.to_string(),
                    json!({ "type": p.ty, "description": p.description }),
                );
                if p.required {
                    required.push(p.name);
                }
            }
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": {
                    "type": "object",
                    "properties": props,
                    "required": required,
                    "additionalProperties": false,
                },
                "annotations": { "readOnlyHint": true, "openWorldHint": true },
            })
        })
        .collect();
    json!({ "tools": tools })
}

/// The command line for a tool call, after the global flags.
fn tool_argv(tool: &Tool, args: &Map<String, Value>) -> Result<Vec<String>, String> {
    for name in args.keys() {
        if !tool.params.iter().any(|p| p.name == name) {
            return Err(format!("unknown argument '{name}' for {}", tool.name));
        }
    }
    let mut argv: Vec<String> = tool.command.iter().map(|s| s.to_string()).collect();
    for p in tool.params {
        let value = match args.get(p.name) {
            Some(Value::Null) | None => match p.default {
                Some(d) => Value::String(d.to_string()),
                None if p.required => {
                    return Err(format!("missing required argument '{}'", p.name))
                }
                None => continue,
            },
            Some(v) => v.clone(),
        };
        let text = match (&value, p.ty) {
            (Value::Bool(b), "boolean") => {
                if let (true, Arg::Flag(flag)) = (*b, &p.arg) {
                    argv.push(flag.to_string());
                }
                continue;
            }
            (Value::String(s), "string") => s.clone(),
            (Value::String(s), "integer") => s.clone(),
            (Value::Number(n), "integer") if n.is_u64() => n.to_string(),
            _ => return Err(format!("argument '{}' must be a {}", p.name, p.ty)),
        };
        match &p.arg {
            Arg::Positional => argv.push(text),
            Arg::Flag(flag) => {
                argv.push(flag.to_string());
                argv.push(text);
            }
        }
    }
    Ok(argv)
}

/// The global flags this server was started with (everything before `mcp`),
/// forwarded to every tool call so they all reach the same network.
fn global_args() -> Vec<String> {
    std::env::args()
        .skip(1)
        .take_while(|a| a != "mcp")
        .filter(|a| a != "--json")
        .collect()
}

fn call_tool(globals: &[String], params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let empty = Map::new();
    let args = params
        .get("arguments")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let Some(tool) = TOOLS.iter().find(|t| t.name == name) else {
        return tool_result(format!("unknown tool '{name}'"), true);
    };
    let argv = match tool_argv(tool, args) {
        Ok(argv) => argv,
        Err(e) => return tool_result(e, true),
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return tool_result(format!("cannot locate zenmon: {e}"), true),
    };
    let output = Command::new(exe)
        .args(globals)
        .arg("--json")
        .args(&argv)
        .stdin(Stdio::null())
        .output();
    match output {
        Ok(out) if out.status.success() => {
            tool_result(String::from_utf8_lossy(&out.stdout).into_owned(), false)
        }
        // `--json` errors are one structured line on stderr.
        Ok(out) => tool_result(String::from_utf8_lossy(&out.stderr).into_owned(), true),
        Err(e) => tool_result(format!("cannot run zenmon: {e}"), true),
    }
}

fn tool_result(text: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

/// Answer one JSON-RPC message; `None` for a notification.
fn handle(globals: &[String], msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "zenmon", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Read-only Zenoh network inspection. Start with `doctor` or `nodes` \
                             to confirm connectivity, `discover` to see which keys exist, then \
                             `subscribe` (bounded) or `query` for values.",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tool_list()),
        "tools/call" => Ok(call_tool(globals, &params)),
        _ => Err(json!({ "code": -32601, "message": format!("method not found: {method}") })),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

/// Serve MCP on stdin/stdout until stdin closes. Logs, if any, go to stderr,
/// which MCP clients keep apart from the protocol stream.
pub async fn run() -> Result<(), ZenmonError> {
    tokio::task::spawn_blocking(|| {
        let globals = global_args();
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        for line in stdin.lock().lines() {
            let line = line.map_err(|e| ZenmonError::internal(format!("stdin: {e}")))?;
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(&line) {
                Ok(msg) => handle(&globals, &msg),
                Err(e) => Some(json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32700, "message": format!("parse error: {e}") },
                })),
            };
            if let Some(reply) = reply {
                writeln!(stdout, "{reply}")
                    .and_then(|()| stdout.flush())
                    .map_err(|e| ZenmonError::internal(format!("stdout: {e}")))?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| ZenmonError::internal(format!("mcp server task: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> &'static Tool {
        TOOLS.iter().find(|t| t.name == name).unwrap()
    }

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn subscribe_is_always_bounded() {
        let argv = tool_argv(tool("subscribe"), &args(json!({ "key_expr": "a/**" }))).unwrap();
        assert_eq!(argv, ["sub", "a/**", "--count", "20", "--duration", "5s"]);
    }

    #[test]
    fn flags_booleans_and_integers_map_to_the_command_line() {
        let argv = tool_argv(
            tool("subscribe"),
            &args(json!({ "key_expr": "a", "count": 3, "qos": true, "validate": false })),
        )
        .unwrap();
        assert_eq!(
            argv,
            ["sub", "a", "--count", "3", "--duration", "5s", "--qos"]
        );
    }

    #[test]
    fn bad_arguments_are_refused_before_running_anything() {
        let t = tool("query");
        assert!(tool_argv(t, &args(json!({})))
            .unwrap_err()
            .contains("missing"));
        assert!(tool_argv(t, &args(json!({ "key_expr": "a", "x": 1 })))
            .unwrap_err()
            .contains("unknown argument"));
        assert!(tool_argv(t, &args(json!({ "key_expr": 5 })))
            .unwrap_err()
            .contains("must be a string"));
    }

    #[test]
    fn only_reading_commands_are_exposed() {
        for t in TOOLS {
            assert!(
                ![
                    "pub",
                    "replay",
                    "queryable",
                    "capture",
                    "scenario",
                    "update",
                    "remote"
                ]
                .contains(&t.command[0]),
                "{} writes",
                t.name
            );
        }
    }

    #[test]
    fn notifications_get_no_reply_and_unknown_methods_an_error() {
        assert!(handle(
            &[],
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
        )
        .is_none());
        let r = handle(&[], &json!({ "jsonrpc": "2.0", "id": 1, "method": "nope" })).unwrap();
        assert_eq!(r["error"]["code"], -32601);
        let r = handle(
            &[],
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .unwrap();
        assert_eq!(r["result"]["tools"].as_array().unwrap().len(), TOOLS.len());
    }
}
