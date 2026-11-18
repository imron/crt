//! MCP adapter over stdio.
//!
//! This module is a thin protocol adapter: it speaks MCP JSON-RPC on stdin /
//! stdout and forwards supported tool calls to the shared reconnecting client.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::client::Client;
use crate::review_types::{ActiveReviewSession, ConnectionContext};

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

pub async fn run(client: Client, context: Option<ConnectionContext>) -> Result<()> {
    let result = run_loop(&client, context).await;
    client.shutdown().await;
    result
}

async fn run_loop(client: &Client, mut context: Option<ConnectionContext>) -> Result<()> {
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

        let response = handle_request(client, &mut context, request).await;
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

async fn handle_request(
    client: &Client,
    context: &mut Option<ConnectionContext>,
    request: Value,
) -> Value {
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
    context: &mut Option<ConnectionContext>,
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
        "list_review_sessions" => serde_json::to_value(client.list_repos().await?)?,
        "select_review_session" => {
            let sessions = client.list_repos().await?.sessions;
            let selected = select_session(&arguments, &sessions)?;
            let init = client.init(&selected.worktree, &selected.base_ref).await?;
            *context = Some(init.clone());
            serde_json::to_value(init)?
        }
        "list_changed_files" => {
            selected_context(context)?;
            serde_json::to_value(client.list_changed_files().await?)?
        }
        "get_file_diff" => {
            selected_context(context)?;
            let file_path = required_string_arg(&arguments, "file_path")?;
            serde_json::to_value(client.get_file_diff(file_path).await?)?
        }
        "search_codebase" => {
            selected_context(context)?;
            let pattern = required_string_arg(&arguments, "pattern")?;
            let scope = arguments
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or("all");
            serde_json::to_value(client.search_codebase(pattern, scope).await?)?
        }
        "find_definition" => {
            selected_context(context)?;
            let symbol = required_string_arg(&arguments, "symbol")?;
            let context_file = arguments.get("context_file").and_then(Value::as_str);
            serde_json::to_value(client.find_definition(symbol, context_file).await?)?
        }
        "list_review_summary" => {
            let context = selected_context(context)?;
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

fn selected_context(context: &Option<ConnectionContext>) -> Result<&ConnectionContext> {
    context.as_ref().context(
        "No review session selected. Call list_review_sessions, then select_review_session.",
    )
}

fn select_session(
    arguments: &Value,
    sessions: &[ActiveReviewSession],
) -> Result<ActiveReviewSession> {
    if sessions.is_empty() {
        anyhow::bail!("No active review sessions are registered with the crt server");
    }

    if let Some(index) = arguments.get("index").and_then(Value::as_u64) {
        return sessions
            .get(index as usize)
            .cloned()
            .with_context(|| format!("session index {index} is out of range"));
    }

    let worktree = arguments.get("worktree").and_then(Value::as_str);
    let base_ref = arguments.get("base_ref").and_then(Value::as_str);
    let merge_base = arguments.get("merge_base").and_then(Value::as_str);

    let matches = sessions
        .iter()
        .filter(|session| {
            worktree.is_none_or(|value| session.worktree == value)
                && base_ref.is_none_or(|value| session.base_ref == value)
                && merge_base.is_none_or(|value| session.merge_base == value)
        })
        .cloned()
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [session] => Ok(session.clone()),
        [] => anyhow::bail!("No active review session matched the selection arguments"),
        _ => anyhow::bail!(
            "Multiple active review sessions matched; pass index or merge_base to disambiguate"
        ),
    }
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
            "name": "list_review_sessions",
            "description": "List active crt review sessions registered by connected clients. Use this before selecting a session for scoped review tools.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        },
        {
            "name": "select_review_session",
            "description": "Select an active crt review session by index or identifying fields. Scoped tools use the selected session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "index": {
                        "type": "integer",
                        "description": "Zero-based index from list_review_sessions."
                    },
                    "worktree": {
                        "type": "string",
                        "description": "Worktree path from list_review_sessions."
                    },
                    "base_ref": {
                        "type": "string",
                        "description": "Base ref from list_review_sessions."
                    },
                    "merge_base": {
                        "type": "string",
                        "description": "Merge-base hash from list_review_sessions."
                    }
                },
                "additionalProperties": false
            }
        },
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
                "list_review_sessions",
                "select_review_session",
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

    #[test]
    fn select_session_can_use_index() {
        let sessions = vec![session("repo-a", "main"), session("repo-b", "main")];

        let selected = select_session(&json!({ "index": 1 }), &sessions).unwrap();

        assert_eq!(selected.repo_root, "repo-b");
    }

    #[test]
    fn select_session_requires_disambiguation_for_multiple_matches() {
        let sessions = vec![session("repo", "main"), session("repo", "feature")];

        let err = select_session(&json!({ "worktree": "/tmp/repo" }), &sessions).unwrap_err();

        assert!(err.to_string().contains("Multiple active review sessions"));
    }

    fn session(repo_root: &str, base_ref: &str) -> ActiveReviewSession {
        ActiveReviewSession {
            repo_root: repo_root.to_string(),
            worktree: "/tmp/repo".to_string(),
            base_ref: base_ref.to_string(),
            head_ref: "HEAD".to_string(),
            merge_base: format!("{repo_root}-{base_ref}"),
            client_count: 1,
        }
    }
}
