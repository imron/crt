//! MCP adapter over stdio.
//!
//! This module is a thin protocol adapter. `rmcp` owns the external MCP
//! handshake, tool discovery, schemas, and stdio transport; tool methods
//! forward to the shared reconnecting crt client.

use std::collections::BTreeMap;
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

use crate::client::{Client, CommentScope};
use crate::review_types::{
    ActiveReviewSession, AnchorStatus, Comment, ConnectionContext, ListReposResult,
};

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
pub struct ListReviewCommentsParams {
    #[schemars(description = "Optional changed-file path to filter comments by")]
    pub file_path: Option<String>,
    #[serde(default)]
    #[schemars(description = "Include resolved comments as well as unresolved comments")]
    pub include_resolved: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CommentIdParams {
    #[schemars(description = "Review comment id returned by list_review_comments")]
    pub id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FilePathParams {
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

#[derive(Debug, Serialize)]
struct ReviewCommentSummary {
    id: i64,
    file_path: String,
    line_start: i64,
    line_end: i64,
    body: String,
    resolved: bool,
    anchor_status: AnchorStatus,
}

#[derive(Debug, Serialize)]
struct ListReviewCommentsSummary {
    comments: Vec<ReviewCommentSummary>,
}

#[derive(Debug, Serialize)]
struct ReviewCommentSummaryResult {
    comment: ReviewCommentSummary,
}

#[derive(Clone)]
pub struct CrtMcp {
    client: ClientProvider,
    selected: Arc<Mutex<Option<ConnectionContext>>>,
    tool_router: ToolRouter<Self>,
}

#[derive(Clone)]
struct ClientProvider {
    client: Arc<Mutex<Option<Arc<Client>>>>,
}

impl ClientProvider {
    fn new(client: Option<Client>) -> Self {
        Self {
            client: Arc::new(Mutex::new(client.map(Arc::new))),
        }
    }

    async fn get(&self) -> Result<Arc<Client>> {
        if let Some(client) = self.client.lock().await.as_ref().cloned() {
            return Ok(client);
        }

        let client = Arc::new(Client::connect_http_from_env().await.with_context(|| {
            format!(
                "No crt server is available at {}:{}. Start `crt server` or a crt TUI session first.",
                std::env::var(crate::server::ENV_HTTP_HOST)
                    .unwrap_or_else(|_| crate::server::DEFAULT_HTTP_HOST.to_string()),
                std::env::var(crate::server::ENV_HTTP_PORT)
                    .unwrap_or_else(|_| crate::server::DEFAULT_HTTP_PORT.to_string())
            )
        })?);
        *self.client.lock().await = Some(Arc::clone(&client));
        Ok(client)
    }

    async fn shutdown(&self) {
        let client = self.client.lock().await.take();
        if let Some(client) = client {
            client.shutdown().await;
        }
    }
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
        let client = match self.client().await {
            Ok(client) => client,
            Err(_) => {
                return to_json(&ListReposResult {
                    repos: Vec::new(),
                    sessions: Vec::new(),
                });
            }
        };
        match client.list_repos().await {
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
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return format!("Error listing review sessions: {e:#}"),
        };
        let sessions = match client.list_repos().await {
            Ok(result) => result.sessions,
            Err(e) => return format!("Error listing review sessions: {e:#}"),
        };
        let selected = match select_session(&params, &sessions) {
            Ok(selected) => selected,
            Err(e) => return format!("Error selecting review session: {e:#}"),
        };
        match client.init(&selected.worktree, &selected.base_ref).await {
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
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return format!("Error listing changed files: {e:#}"),
        };
        match client.list_changed_files().await {
            Ok(result) => to_json(&result),
            Err(e) => format!("Error listing changed files: {e:#}"),
        }
    }

    #[tool(
        description = "List changed files with review status and compact diff stats, without full diff hunks. Use this to find files still needing review."
    )]
    async fn list_file_statuses(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        if let Err(e) = self.require_selected().await {
            return format!("Error: {e:#}");
        }
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return format!("Error listing file statuses: {e:#}"),
        };
        match client.list_file_statuses().await {
            Ok(result) => to_json(&result),
            Err(e) => format!("Error listing file statuses: {e:#}"),
        }
    }

    #[tool(description = "Return the diff for one changed file in the selected review scope.")]
    async fn get_file_diff(&self, Parameters(params): Parameters<GetFileDiffParams>) -> String {
        if let Err(e) = self.require_selected().await {
            return format!("Error: {e:#}");
        }
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return format!("Error getting file diff: {e:#}"),
        };
        match client.get_file_diff(&params.file_path).await {
            Ok(result) => to_json(&result),
            Err(e) => format!("Error getting file diff: {e:#}"),
        }
    }

    #[tool(
        description = "List compact review comment summaries in the selected review scope: id, file_path, line_start, line_end, body, resolved, and anchor_status. Call select_review_session first. By default only unresolved comments are returned; pass include_resolved=true to include resolved comments. Optionally pass file_path to return comments for one changed file. Use get_comment_detail only when full anchor/context data is needed."
    )]
    async fn list_review_comments(
        &self,
        Parameters(params): Parameters<ListReviewCommentsParams>,
    ) -> String {
        if let Err(e) = self.require_selected().await {
            return format!("Error: {e:#}");
        }
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return format!("Error listing review comments: {e:#}"),
        };
        let scope = if params.include_resolved {
            CommentScope::CurrentWithResolved
        } else {
            CommentScope::CurrentUnresolved
        };
        match client
            .list_comments(params.file_path.as_deref(), scope)
            .await
        {
            Ok(result) => to_json(&summarize_comments(result.comments)),
            Err(e) => format!("Error listing review comments: {e:#}"),
        }
    }

    #[tool(
        description = "Return full detail for one review comment in the selected review scope, including line range, character range, anchor text, surrounding context, resolved status, timestamps, and anchor status. Call select_review_session first."
    )]
    async fn get_comment_detail(&self, Parameters(params): Parameters<CommentIdParams>) -> String {
        if let Err(e) = self.require_selected().await {
            return format!("Error: {e:#}");
        }
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return format!("Error getting review comment: {e:#}"),
        };
        match client.get_comment(params.id).await {
            Ok(result) => to_json(&result),
            Err(e) => format!("Error getting review comment: {e:#}"),
        }
    }

    #[tool(
        description = "Mark one review comment resolved in the selected review scope. Use the id returned by list_review_comments. Call select_review_session first."
    )]
    async fn resolve_comment(&self, Parameters(params): Parameters<CommentIdParams>) -> String {
        if let Err(e) = self.require_selected().await {
            return format!("Error: {e:#}");
        }
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return format!("Error resolving review comment: {e:#}"),
        };
        match client.resolve_comment(params.id).await {
            Ok(result) => to_json(&summarize_comment_result(result.comment)),
            Err(e) => format!("Error resolving review comment: {e:#}"),
        }
    }

    #[tool(
        description = "Mark one resolved review comment unresolved in the selected review scope. Use the id returned by list_review_comments with include_resolved=true. Call select_review_session first."
    )]
    async fn unresolve_comment(&self, Parameters(params): Parameters<CommentIdParams>) -> String {
        if let Err(e) = self.require_selected().await {
            return format!("Error: {e:#}");
        }
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return format!("Error unresolving review comment: {e:#}"),
        };
        match client.unresolve_comment(params.id).await {
            Ok(result) => to_json(&summarize_comment_result(result.comment)),
            Err(e) => format!("Error unresolving review comment: {e:#}"),
        }
    }

    #[tool(
        description = "Mark a changed file reviewed in the selected review scope. Use a file_path returned by list_changed_files. Call select_review_session first."
    )]
    async fn mark_file_reviewed(&self, Parameters(params): Parameters<FilePathParams>) -> String {
        if let Err(e) = self.require_selected().await {
            return format!("Error: {e:#}");
        }
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return format!("Error marking file reviewed: {e:#}"),
        };
        match client.mark_reviewed(&params.file_path).await {
            Ok(result) => to_json(&result),
            Err(e) => format!("Error marking file reviewed: {e:#}"),
        }
    }

    #[tool(
        description = "Clear a changed file's reviewed state in the selected review scope. Use a file_path returned by list_changed_files. Call select_review_session first."
    )]
    async fn unmark_file_reviewed(&self, Parameters(params): Parameters<FilePathParams>) -> String {
        if let Err(e) = self.require_selected().await {
            return format!("Error: {e:#}");
        }
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return format!("Error unmarking file reviewed: {e:#}"),
        };
        match client.unmark_reviewed(&params.file_path).await {
            Ok(result) => to_json(&result),
            Err(e) => format!("Error unmarking file reviewed: {e:#}"),
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
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return format!("Error searching codebase: {e:#}"),
        };
        match client.search_codebase(&params.pattern, scope).await {
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
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return format!("Error finding definition: {e:#}"),
        };
        match client
            .find_definition(&params.symbol, params.context_file.as_deref())
            .await
        {
            Ok(result) => to_json(&result),
            Err(e) => format!("Error finding definition: {e:#}"),
        }
    }

    #[tool(
        description = "Summarize the selected review scope with changed-file counts, review status, and review comment counts. Call select_review_session first."
    )]
    async fn list_review_summary(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        let context = match self.require_selected().await {
            Ok(context) => context,
            Err(e) => return format!("Error: {e:#}"),
        };
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return format!("Error summarizing review: {e:#}"),
        };
        let files = match client.list_file_statuses().await {
            Ok(files) => files,
            Err(e) => return format!("Error summarizing review: {e:#}"),
        };
        let comments = match client
            .list_comments(None, CommentScope::CurrentWithResolved)
            .await
        {
            Ok(comments) => comments,
            Err(e) => return format!("Error summarizing review comments: {e:#}"),
        };
        let mut per_file_comments = BTreeMap::<String, serde_json::Value>::new();
        for comment in &comments.comments {
            let entry = per_file_comments
                .entry(comment.file_path.clone())
                .or_insert_with(|| {
                    serde_json::json!({
                        "total": 0,
                        "resolved": 0,
                        "unresolved": 0,
                    })
                });
            entry["total"] = serde_json::json!(entry["total"].as_u64().unwrap_or(0) + 1);
            if comment.resolved {
                entry["resolved"] = serde_json::json!(entry["resolved"].as_u64().unwrap_or(0) + 1);
            } else {
                entry["unresolved"] =
                    serde_json::json!(entry["unresolved"].as_u64().unwrap_or(0) + 1);
            }
        }
        let value = serde_json::json!({
            "base_ref": context.base_ref,
            "head_ref": context.head_ref,
            "merge_base": context.merge_base,
            "total_files": files.total_files,
            "reviewed_files": files.reviewed_files,
            "unreviewed_files": files.unreviewed_files + files.changed_files,
            "changed_files": files.changed_files,
            "total_comments": comments.comments.len(),
            "resolved_comments": comments.comments.iter().filter(|comment| comment.resolved).count(),
            "unresolved_comments": comments.comments.iter().filter(|comment| !comment.resolved).count(),
            "comments_by_file": per_file_comments,
            "files": files.files,
        });
        to_json(&value)
    }
}

impl CrtMcp {
    pub fn new(client: Option<Client>, selected: Option<ConnectionContext>) -> Self {
        Self {
            client: ClientProvider::new(client),
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

    async fn client(&self) -> Result<Arc<Client>> {
        self.client.get().await
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

pub async fn run(client: Option<Client>, context: Option<ConnectionContext>) -> Result<()> {
    let handler = CrtMcp::new(client, context);
    let client_provider = handler.client.clone();
    let service = handler.serve(rmcp::transport::stdio()).await?;
    let result = service.waiting().await;
    client_provider.shutdown().await;
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

fn summarize_comments(comments: Vec<Comment>) -> ListReviewCommentsSummary {
    ListReviewCommentsSummary {
        comments: comments.into_iter().map(summarize_comment).collect(),
    }
}

fn summarize_comment_result(comment: Comment) -> ReviewCommentSummaryResult {
    ReviewCommentSummaryResult {
        comment: summarize_comment(comment),
    }
}

fn summarize_comment(comment: Comment) -> ReviewCommentSummary {
    ReviewCommentSummary {
        id: comment.id,
        file_path: comment.file_path,
        line_start: comment.line_start,
        line_end: comment.line_end,
        body: comment.body,
        resolved: comment.resolved,
        anchor_status: comment.anchor_status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

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

    #[test]
    fn comment_summary_omits_anchor_context_and_scope_fields() {
        let summary = summarize_comments(vec![comment()]);
        let value = serde_json::to_value(summary).unwrap();
        let comment = &value["comments"][0];

        assert_eq!(comment["id"], 7);
        assert_eq!(comment["file_path"], "src/lib.rs");
        assert_eq!(comment["line_start"], 12);
        assert_eq!(comment["line_end"], 14);
        assert_eq!(comment["body"], "Please simplify this branch.");
        assert_eq!(comment["resolved"], false);
        assert_eq!(comment["anchor_status"], "approximate");
        assert!(comment.get("anchor_text").is_none());
        assert!(comment.get("context_before").is_none());
        assert!(comment.get("context_after").is_none());
        assert!(comment.get("merge_base").is_none());
        assert!(comment.get("head_ref").is_none());
        assert!(comment.get("created_at").is_none());
        assert!(comment.get("updated_at").is_none());
    }

    #[tokio::test]
    async fn tool_router_advertises_review_comment_tools() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("server.sock");
        let _listener = UnixListener::bind(&socket_path).unwrap();
        let client = Client::connect(&socket_path).await.unwrap();
        let mcp = CrtMcp::new(Some(client), None);
        let mut names = mcp.list_tool_names();
        names.sort();

        assert_eq!(
            names,
            vec![
                "find_definition",
                "get_comment_detail",
                "get_file_diff",
                "list_changed_files",
                "list_file_statuses",
                "list_review_comments",
                "list_review_sessions",
                "list_review_summary",
                "mark_file_reviewed",
                "resolve_comment",
                "search_codebase",
                "select_review_session",
                "unmark_file_reviewed",
                "unresolve_comment",
            ]
        );
    }

    #[tokio::test]
    async fn list_sessions_returns_empty_when_crt_server_is_unavailable() {
        let port = unused_local_port();
        unsafe {
            std::env::set_var(crate::server::ENV_HTTP_PORT, port.to_string());
        }
        let mcp = CrtMcp::new(None, None);

        let result = mcp.list_review_sessions(Parameters(EmptyParams {})).await;

        unsafe {
            std::env::remove_var(crate::server::ENV_HTTP_PORT);
        }
        let parsed: ListReposResult = serde_json::from_str(&result).unwrap();
        assert!(parsed.repos.is_empty());
        assert!(parsed.sessions.is_empty());
    }

    fn unused_local_port() -> u16 {
        for port in 30_000..40_000 {
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return port;
            }
        }
        panic!("could not find an unused localhost port");
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

    fn comment() -> Comment {
        Comment {
            id: 7,
            merge_base: "merge-base".to_string(),
            head_ref: "HEAD".to_string(),
            file_path: "src/lib.rs".to_string(),
            line_start: 12,
            line_end: 14,
            char_start: Some(2),
            char_end: Some(8),
            anchor_text: "if condition {\n    do_work();\n}".to_string(),
            context_before: "fn example() {".to_string(),
            context_after: "}".to_string(),
            body: "Please simplify this branch.".to_string(),
            resolved: false,
            created_at: "2026-07-01T00:00:00+10:00".to_string(),
            updated_at: "2026-07-01T00:01:00+10:00".to_string(),
            anchor_status: AnchorStatus::Approximate,
        }
    }
}
