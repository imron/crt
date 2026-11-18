//! Diff and content loading rules shared by UI adapters.

use std::path::Path;

use crate::config::DiffAlgorithm;
use crate::git::{BlameLine, Repo};
use crate::model::DiffContent;

pub fn resolve_diff_algorithm(worktree: &str, configured: Option<DiffAlgorithm>) -> DiffAlgorithm {
    configured.unwrap_or_else(|| {
        Repo::open(Path::new(worktree))
            .ok()
            .and_then(|repo| repo.diff_config().algorithm)
            .unwrap_or(DiffAlgorithm::Patience)
    })
}

pub fn file_content(worktree: &str, rev: &str, path: &str) -> Option<String> {
    Repo::open(Path::new(worktree))
        .ok()
        .and_then(|repo| repo.file_content(rev, path).ok().flatten())
}

pub fn workdir_file_content(worktree: &str, path: &str) -> Option<String> {
    Repo::open(Path::new(worktree))
        .ok()
        .and_then(|repo| repo.file_content_workdir(path).ok().flatten())
}

pub fn blame_pair(
    worktree: &str,
    merge_base: &str,
    path: &str,
    enabled: bool,
) -> (Vec<BlameLine>, Vec<BlameLine>) {
    if !enabled {
        return (Vec::new(), Vec::new());
    }

    let Ok(repo) = Repo::open(Path::new(worktree)) else {
        return (Vec::new(), Vec::new());
    };

    let head = repo.blame_file("HEAD", path).unwrap_or_default();
    let base = repo.blame_file(merge_base, path).unwrap_or_default();
    (head, base)
}

pub fn diff_with_fallback(
    worktree: &str,
    diff_base: &str,
    merge_base: &str,
    path: &str,
    algorithm: DiffAlgorithm,
    ignore_whitespace: bool,
) -> Option<DiffContent> {
    let repo = Repo::open(Path::new(worktree)).ok()?;
    repo.diff_file_workdir_opts(diff_base, path, algorithm, ignore_whitespace)
        .or_else(|_| {
            if diff_base != merge_base {
                repo.diff_file_workdir_opts(merge_base, path, algorithm, ignore_whitespace)
            } else {
                Err(anyhow::anyhow!("diff failed"))
            }
        })
        .ok()
}
