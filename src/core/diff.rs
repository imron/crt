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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn setup_test_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        run_git(path, &["init"]);
        run_git(path, &["config", "user.email", "test@test.com"]);
        run_git(path, &["config", "user.name", "Test"]);

        std::fs::write(
            path.join("hello.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        run_git(path, &["add", "-A"]);
        run_git(path, &["commit", "-m", "initial"]);
        run_git(path, &["tag", "base"]);

        std::fs::write(
            path.join("hello.rs"),
            "fn main() {\n    println!(\"hello world\");\n}\n",
        )
        .unwrap();
        run_git(path, &["add", "-A"]);
        run_git(path, &["commit", "-m", "committed change"]);

        std::fs::write(
            path.join("hello.rs"),
            "fn main() {\n    println!(\"hello world\");\n    run();\n}\n",
        )
        .unwrap();

        dir
    }

    fn run_git(path: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        if !out.status.success() {
            panic!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn resolve_diff_algorithm_prefers_configured_value() {
        assert_eq!(
            resolve_diff_algorithm("/path/that/does/not/exist", Some(DiffAlgorithm::Minimal)),
            DiffAlgorithm::Minimal
        );
    }

    #[test]
    fn resolve_diff_algorithm_reads_git_config() {
        let dir = setup_test_repo();
        run_git(dir.path(), &["config", "diff.algorithm", "histogram"]);

        assert_eq!(
            resolve_diff_algorithm(dir.path().to_str().unwrap(), None),
            DiffAlgorithm::Histogram
        );
    }

    #[test]
    fn resolve_diff_algorithm_defaults_when_repo_cannot_be_opened() {
        assert_eq!(
            resolve_diff_algorithm("/path/that/does/not/exist", None),
            DiffAlgorithm::Patience
        );
    }

    #[test]
    fn file_content_helpers_load_committed_and_workdir_content() {
        let dir = setup_test_repo();
        let worktree = dir.path().to_str().unwrap();

        let base = file_content(worktree, "base", "hello.rs").unwrap();
        let workdir = workdir_file_content(worktree, "hello.rs").unwrap();

        assert!(base.contains("hello"));
        assert!(!base.contains("run();"));
        assert!(workdir.contains("run();"));
    }

    #[test]
    fn blame_pair_returns_empty_when_disabled() {
        let (head, base) = blame_pair("/path/that/does/not/exist", "base", "hello.rs", false);

        assert!(head.is_empty());
        assert!(base.is_empty());
    }

    #[test]
    fn diff_with_fallback_uses_merge_base_when_primary_base_fails() {
        let dir = setup_test_repo();
        let worktree = dir.path().to_str().unwrap();

        let diff = diff_with_fallback(
            worktree,
            "missing-ref",
            "base",
            "hello.rs",
            DiffAlgorithm::Patience,
            false,
        )
        .unwrap();

        assert!(!diff.is_binary);
        assert!(!diff.hunks.is_empty());
    }
}
