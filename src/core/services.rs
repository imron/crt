//! Feature-oriented core service contracts.

use std::path::Path;

use anyhow::Result;

use crate::review_types;

#[derive(Debug, Clone)]
pub struct SessionInitRequest {
    pub worktree: String,
    pub base_ref: String,
}

#[derive(Debug, Clone)]
pub struct SessionInitResult {
    pub context: review_types::ConnectionContext,
}

pub trait SessionService {
    fn init_session(&self, request: &SessionInitRequest) -> Result<SessionInitResult>;
}

pub trait ReviewService {
    fn list_changed_files(&self) -> Result<review_types::ListChangedFilesResult>;
    fn mark_reviewed(&self, file_path: &str) -> Result<review_types::ReviewActionResult>;
    fn unmark_reviewed(&self, file_path: &str) -> Result<review_types::ReviewActionResult>;
    fn reset_reviews(&self) -> Result<review_types::ResetReviewsResult>;
}

pub trait DiffService {
    fn get_file_diff(&self, file_path: &str) -> Result<review_types::GetFileDiffResult>;
    fn get_file_content(
        &self,
        file_path: &str,
        version: review_types::FileVersion,
    ) -> Result<review_types::GetFileContentResult>;
    fn load_blame(&self, file_path: &str, rev: &str) -> Result<Vec<crate::git::BlameLine>>;
}

pub trait SearchService {
    fn search_codebase(
        &self,
        pattern: &str,
        scope: &str,
    ) -> Result<review_types::SearchCodebaseResult>;
    fn find_definition(
        &self,
        symbol: &str,
        context_file: Option<&str>,
    ) -> Result<review_types::FindDefinitionResult>;
}

pub trait RepoQueryService {
    fn repo_root(&self, path: &Path) -> Result<String>;
}
