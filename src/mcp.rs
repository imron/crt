//! MCP adapter over stdio.
//!
//! This module is a thin protocol adapter. `rmcp` owns the external MCP
//! handshake, tool discovery, schemas, and stdio transport; tool methods
//! forward to the shared reconnecting crt client.

use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, ServiceExt, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::client::Client;
use crate::review_types::{ActiveReviewSession, ConnectionContext};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EmptyParams {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SelectReviewSessionParams {
    #[schemars(description = "Zero-based index from list_review_sessions")]
    pub index: Option<usize>,
    #[schemars(description = "Worktree path from list_review_sessions")]
    pub worktree: Option<String>,
    #[schemars(description = "Base ref from list_review_sessions")]
    pub base_ref: Option<String>,
    #[schemars(description = "Merge-base hash from list_review_sessions")]
    pub merge_base: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetFileDiffParams {
    #[schemars(description = "Path to a changed file, relative to the review worktree")]
    pub file_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchCodebaseParams {
    #[schemars(description = "Regular expression to search for")]
    pub pattern: String,
    #[schemars(description = "Use 'all' for the worktree or 'diff' for changed files only")]
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindDefinitionParams {
    #[schemars(description = "Symbol name to locate")]
    pub symbol: String,
    #[schemars(description = "Optional file path relative to the worktree")]
    pub context_file: Option<String>,
}

#[derive(Clone)]
pub struct CrtMcp {
    client: Arc<Client>,
    selected: Arc<Mutex<Option<ConnectionContext>>>,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for CrtMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrtMcp")
            .field("tools", &self.list_tool_names())
            .finish()
    }
}

#[tool_router]
impl CrtMcp {
    #[tool(
        description = "List active crt review sessions registered by connected clients. Use this before selecting a session for scoped review tools."
    )]
    async fn list_review_sessions(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        match self.client.list_repos().await {
            Ok(result) => to_json(&result),
            Err(e) => format!("Error listing review sessions: {e:#}"),
        }
    }

    #[tool(
        description = "Select an active crt review session by index or identifying fields. Scoped tools use the selected session."
    )]
    async fn select_review_session(
        &self,
        Parameters(params): Parameters<SelectReviewSessionParams>,
    ) -> String {
        let sessions = match self.client.list_repos().await {
            Ok(result) => result.sessions,
            Err(e) => return format!("Error listing review sessions: {e:#}"),
        };
        let selected = match select_session(&params, &sessions) {
            Ok(selected) => selected,
            Err(e) => return format!("Error selecting review session: {e:#}"),
        };
        match self
            .client
            .init(&selected.worktree, &selected.base_ref)
            .await
        {
            Ok(context) => {
                *self.selected.lock().await = Some(context.clone());
                to_json(&context)
            }
            Err(e) => format!("Error initializing selected review session: {e:#}"),
        }
    }

    #[tool(
        description = "List files changed in the selected review scope, including review status and diff metadata."
    )]
    async fn list_changed_files(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        if let Err(e) = self.require_selected().await {
            return format!("Error: {e:#}");
        }
        match self.client.list_changed_files().await {
            Ok(result) => to_json(&result),
            Err(e) => format!("Error listing changed files: {e:#}"),
        }
    }

    #[tool(description = "Return the diff for one changed file in the selected review scope.")]
    async fn get_file_diff(&self, Parameters(params): Parameters<GetFileDiffParams>) -> String {
        if let Err(e) = self.require_selected().await {
            return format!("Error: {e:#}");
        }
        match self.client.get_file_diff(&params.file_path).await {
            Ok(result) => to_json(&result),
            Err(e) => format!("Error getting file diff: {e:#}"),
        }
    }

    #[tool(
        description = "Search the selected review worktree or current diff for a regular expression."
    )]
    async fn search_codebase(
        &self,
        Parameters(params): Parameters<SearchCodebaseParams>,
    ) -> String {
        if let Err(e) = self.require_selected().await {
            return format!("Error: {e:#}");
        }
        let scope = params.scope.as_deref().unwrap_or("all");
        match self.client.search_codebase(&params.pattern, scope).await {
            Ok(result) => to_json(&result),
            Err(e) => format!("Error searching codebase: {e:#}"),
        }
    }

    #[tool(description = "Find likely definitions for a symbol in the selected review worktree.")]
    async fn find_definition(
        &self,
        Parameters(params): Parameters<FindDefinitionParams>,
    ) -> String {
        if let Err(e) = self.require_selected().await {
            return format!("Error: {e:#}");
        }
        match self
            .client
            .find_definition(&params.symbol, params.context_file.as_deref())
            .await
        {
            Ok(result) => to_json(&result),
            Err(e) => format!("Error finding definition: {e:#}"),
        }
    }

    #[tool(
        description = "Summarize the selected review scope with changed-file counts and per-file review status."
    )]
    async fn list_review_summary(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        let context = match self.require_selected().await {
            Ok(context) => context,
            Err(e) => return format!("Error: {e:#}"),
        };
        match self.client.list_changed_files().await {
            Ok(files) => {
                let value = serde_json::json!({
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
                });
                to_json(&value)
            }
            Err(e) => format!("Error summarizing review: {e:#}"),
        }
    }
}

impl CrtMcp {
    pub fn new(client: Arc<Client>, selected: Option<ConnectionContext>) -> Self {
        Self {
            client,
            selected: Arc::new(Mutex::new(selected)),
            tool_router: Self::tool_router(),
        }
    }

    pub fn list_tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    async fn require_selected(&self) -> Result<ConnectionContext> {
        self.selected.lock().await.clone().context(
            "No review session selected. Call list_review_sessions, then select_review_session.",
        )
    }
}

impl ServerHandler for CrtMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            concat!(
                "crt MCP Server. Start with list_review_sessions to discover active review ",
                "sessions registered by connected crt clients, then call select_review_session ",
                "before scoped tools such as list_changed_files or get_file_diff."
            )
            .to_string(),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        Ok(ListToolsResult {
            tools: self.tool_router.list_all(),
            meta: None,
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }
}

pub async fn run(client: Client, context: Option<ConnectionContext>) -> Result<()> {
    let client = Arc::new(client);
    let handler = CrtMcp::new(Arc::clone(&client), context);
    let service = handler.serve(rmcp::transport::stdio()).await?;
    let result = service.waiting().await;
    client.shutdown().await;
    result?;
    Ok(())
}

fn select_session(
    params: &SelectReviewSessionParams,
    sessions: &[ActiveReviewSession],
) -> Result<ActiveReviewSession> {
    if sessions.is_empty() {
        anyhow::bail!("No active review sessions are registered with the crt server");
    }

    if let Some(index) = params.index {
        return sessions
            .get(index)
            .cloned()
            .with_context(|| format!("session index {index} is out of range"));
    }

    let matches = sessions
        .iter()
        .filter(|session| {
            params
                .worktree
                .as_ref()
                .is_none_or(|value| session.worktree == *value)
                && params
                    .base_ref
                    .as_ref()
                    .is_none_or(|value| session.base_ref == *value)
                && params
                    .merge_base
                    .as_ref()
                    .is_none_or(|value| session.merge_base == *value)
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

fn to_json(value: &impl Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_session_can_use_index() {
        let sessions = vec![session("repo-a", "main"), session("repo-b", "main")];
        let params = SelectReviewSessionParams {
            index: Some(1),
            worktree: None,
            base_ref: None,
            merge_base: None,
        };

        let selected = select_session(&params, &sessions).unwrap();

        assert_eq!(selected.repo_root, "repo-b");
    }

    #[test]
    fn select_session_requires_disambiguation_for_multiple_matches() {
        let sessions = vec![session("repo", "main"), session("repo", "feature")];
        let params = SelectReviewSessionParams {
            index: None,
            worktree: Some("/tmp/repo".to_string()),
            base_ref: None,
            merge_base: None,
        };

        let err = select_session(&params, &sessions).unwrap_err();

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
