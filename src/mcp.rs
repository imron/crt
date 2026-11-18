//! MCP adapter over stdio.
//!
//! This module is a thin protocol adapter: it speaks MCP JSON-RPC on stdin /
//! stdout and forwards supported tool calls to the shared reconnecting client.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::client::Client;
use crate::review_types::ConnectionContext;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

pub async fn run(client: Client, context: ConnectionContext) -> Result<()> {
    let result = run_loop(&client, &context).await;
    client.shutdown().await;
    result
}

async fn run_loop(client: &Client, context: &ConnectionContext) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .context("Failed to read MCP request")?;
        if n == 0 {
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(trimmed) {
            Ok(request) => request,
            Err(e) => {
                write_response(
                    &mut stdout,
                    json_rpc_error(Value::Null, -32700, format!("Parse error: {e}")),
                )
                .await?;
                continue;
            }
        };

        if request.get("id").is_none() {
            continue;
        }

        let response = handle_request(client, context, request).await;
        write_response(&mut stdout, response).await?;
    }

    Ok(())
}

async fn write_response(stdout: &mut tokio::io::Stdout, response: Value) -> Result<()> {
    let mut line = serde_json::to_string(&response).context("Failed to serialize MCP response")?;
    line.push('\n');
    stdout
        .write_all(line.as_bytes())
        .await
        .context("Failed to write MCP response")?;
    stdout
        .flush()
        .await
        .context("Failed to flush MCP response")?;
    Ok(())
}

async fn handle_request(client: &Client, context: &ConnectionContext, request: Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return json_rpc_error(id, -32600, "Invalid request: missing method");
    };

    match method {
        "initialize" => json_rpc_success(
            id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "crt",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),
        "tools/list" => json_rpc_success(id, json!({ "tools": tool_schemas() })),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            match handle_tool_call(client, context, params).await {
                Ok(result) => json_rpc_success(id, result),
                Err(e) => json_rpc_error(id, -32000, format!("{e:#}")),
            }
        }
        _ => json_rpc_error(id, -32601, format!("Method '{method}' not found")),
    }
}

async fn handle_tool_call(
    client: &Client,
    context: &ConnectionContext,
    params: Value,
) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .context("tools/call missing tool name")?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let value = match name {
        "list_changed_files" => serde_json::to_value(client.list_changed_files().await?)?,
        "get_file_diff" => {
            let file_path = required_string_arg(&arguments, "file_path")?;
            serde_json::to_value(client.get_file_diff(file_path).await?)?
        }
        "search_codebase" => {
            let pattern = required_string_arg(&arguments, "pattern")?;
            let scope = arguments
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or("all");
            serde_json::to_value(client.search_codebase(pattern, scope).await?)?
        }
        "find_definition" => {
            let symbol = required_string_arg(&arguments, "symbol")?;
            let context_file = arguments.get("context_file").and_then(Value::as_str);
            serde_json::to_value(client.find_definition(symbol, context_file).await?)?
        }
        "list_review_summary" => {
            let files = client.list_changed_files().await?;
            json!({
                "base_ref": context.base_ref,
                "head_ref": context.head_ref,
                "merge_base": context.merge_base,
                "total_files": files.files.len(),
                "reviewed_files": files.files.iter().filter(|entry| matches!(
                    entry.status,
                    crate::review_types::ReviewStatus::Reviewed { .. }
                )).count(),
                "unreviewed_files": files.files.iter().filter(|entry| matches!(
                    entry.status,
                    crate::review_types::ReviewStatus::Unreviewed
                        | crate::review_types::ReviewStatus::Changed { .. }
                )).count(),
                "files": files.files,
            })
        }
        _ => anyhow::bail!("Unknown tool '{name}'"),
    };

    Ok(json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&value)?,
        }],
        "isError": false,
    }))
}

fn required_string_arg<'a>(arguments: &'a Value, name: &str) -> Result<&'a str> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .with_context(|| format!("missing string argument '{name}'"))
}

fn json_rpc_success(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn json_rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into(),
        },
    })
}

fn tool_schemas() -> Value {
    json!([
        {
            "name": "list_changed_files",
            "description": "List files changed in the current review scope, including review status and diff metadata.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        },
        {
            "name": "get_file_diff",
            "description": "Return the diff for one changed file in the current review scope.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to a changed file, relative to the review worktree."
                    }
                },
                "required": ["file_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "search_codebase",
            "description": "Search the worktree or current diff for a regular expression.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regular expression to search for."
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["all", "diff"],
                        "description": "Use 'all' for the worktree or 'diff' for changed files only."
                    }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }
        },
        {
            "name": "find_definition",
            "description": "Find likely definitions for a symbol, optionally using a context file for ranking.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "Symbol name to locate."
                    },
                    "context_file": {
                        "type": "string",
                        "description": "Optional file path relative to the worktree."
                    }
                },
                "required": ["symbol"],
                "additionalProperties": false
            }
        },
        {
            "name": "list_review_summary",
            "description": "Summarize the current review scope with changed-file counts and per-file review status.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_schemas_only_advertise_implemented_tools() {
        let tools = tool_schemas();
        let names: Vec<_> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();

        assert_eq!(
            names,
            vec![
                "list_changed_files",
                "get_file_diff",
                "search_codebase",
                "find_definition",
                "list_review_summary"
            ]
        );
    }

    #[test]
    fn missing_required_string_arg_is_an_error() {
        let err = required_string_arg(&json!({}), "file_path").unwrap_err();

        assert!(err.to_string().contains("file_path"));
    }
}
