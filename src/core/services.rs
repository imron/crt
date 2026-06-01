//! Feature-oriented core service contracts.

use std::path::Path;

use anyhow::Result;

use crate::model;

#[derive(Debug, Clone)]
pub struct SessionInitRequest {
    pub worktree: String,
    pub base_ref: String,
}

#[derive(Debug, Clone)]
pub struct SessionInitResult {
    pub context: model::ConnectionContext,
}

pub trait SessionService {
    fn init_session(&self, request: &SessionInitRequest) -> Result<SessionInitResult>;
}

pub trait ReviewService {
    fn list_changed_files(&self) -> Result<model::ListChangedFilesResult>;
    fn mark_reviewed(&self, file_path: &str) -> Result<model::ReviewActionResult>;
    fn unmark_reviewed(&self, file_path: &str) -> Result<model::ReviewActionResult>;
    fn reset_reviews(&self) -> Result<model::ResetReviewsResult>;
}

pub trait DiffService {
    fn get_file_diff(&self, file_path: &str) -> Result<model::GetFileDiffResult>;
    fn get_file_content(
        &self,
        file_path: &str,
        version: model::FileVersion,
    ) -> Result<model::GetFileContentResult>;
    fn load_blame(&self, file_path: &str, rev: &str) -> Result<Vec<crate::git::BlameLine>>;
}

pub trait SearchService {
    fn search_codebase(&self, pattern: &str, scope: &str) -> Result<model::SearchCodebaseResult>;
    fn find_definition(
        &self,
        symbol: &str,
        context_file: Option<&str>,
    ) -> Result<model::FindDefinitionResult>;
}

pub trait NavigationService {
    fn map_new_to_old_line(&self, entry: &model::FileEntry, new_line: usize) -> usize;
    fn map_old_to_new_line(&self, entry: &model::FileEntry, old_line: usize) -> usize;
    fn estimate_line_from_display_row(
        &self,
        entry: &model::FileEntry,
        display_row: usize,
        hunk_starts: &[usize],
        hunk_ends: &[usize],
    ) -> usize;
}

pub trait RepoQueryService {
    fn repo_root(&self, path: &Path) -> Result<String>;
}
