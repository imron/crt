//! Git operations via git2: changed files, diffs, blobs, worktree resolution.
//!
//! This module exposes a clean public API that hides git2 types from callers.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Public types (no git2 types leak through these)
// ---------------------------------------------------------------------------

/// How a file changed between the base and head commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Deleted,
    Modified,
    Renamed,
}

/// A file that changed between base and HEAD.
#[derive(Debug, Clone)]
pub struct FileChange {
    /// Path in the new (HEAD) tree. For deletions this is the old path.
    pub path: String,
    /// For renames, the path in the old (base) tree.
    pub old_path: Option<String>,
    pub kind: ChangeKind,
}

/// A single line within a diff hunk.
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
    /// Line number in the old file (None for additions).
    pub old_lineno: Option<u32>,
    /// Line number in the new file (None for deletions).
    pub new_lineno: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Addition,
    Deletion,
}

/// A single hunk within a diff.
#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// The complete diff for a single file.
#[derive(Debug, Clone)]
pub struct DiffContent {
    pub hunks: Vec<DiffHunk>,
    pub is_binary: bool,
    /// Stable SHA-256 hash of the diff content.
    pub diff_hash: String,
}

/// Which version of a file to read.
#[derive(Debug, Clone, Copy)]
pub enum FileVersion {
    /// The version at the base commit.
    Base,
    /// The version at HEAD.
    Head,
}

/// Resolved repository context from a working directory path.
#[derive(Debug, Clone)]
pub struct RepoContext {
    /// The main repository root (not the worktree).
    pub repo_root: PathBuf,
    /// The worktree path where the command was run.
    pub worktree: PathBuf,
    /// The branch name at HEAD (or short hash if detached).
    pub head_ref: String,
    /// Whether HEAD is detached.
    pub is_detached: bool,
}

// ---------------------------------------------------------------------------
// Repository handle — wraps git2::Repository
// ---------------------------------------------------------------------------

/// Opaque handle to a git repository. All git operations go through this.
pub struct Repo {
    inner: git2::Repository,
}

impl Repo {
    /// Open a repository by discovering it from `path` (searches parents).
    pub fn open(path: &Path) -> Result<Self> {
        let inner = git2::Repository::discover(path)
            .context("Not a git repository (or any parent up to mount point)")?;
        Ok(Self { inner })
    }

    /// Resolve the full repository context (repo root, worktree, head ref).
    pub fn context(&self) -> Result<RepoContext> {
        let repo_root = if self.inner.is_worktree() {
            let common = self.inner.commondir().to_path_buf();
            common.parent().map(|p| p.to_path_buf()).unwrap_or(common)
        } else {
            self.inner
                .workdir()
                .context("Bare repositories are not supported")?
                .to_path_buf()
        };

        let worktree = self
            .inner
            .workdir()
            .context("Bare repositories are not supported")?
            .to_path_buf();

        let (head_ref, is_detached) = match self.inner.head() {
            Ok(head) => {
                if head.is_branch() {
                    let name = head.shorthand().unwrap_or("HEAD").to_string();
                    (name, false)
                } else {
                    let oid = head.target().context("HEAD has no target")?;
                    (oid.to_string()[..8].to_string(), true)
                }
            }
            Err(e) => {
                if e.code() == git2::ErrorCode::UnbornBranch {
                    bail!(
                        "Repository has no commits yet. \
                         Make an initial commit before running crt."
                    );
                }
                bail!("Failed to read HEAD: {}", e);
            }
        };

        Ok(RepoContext {
            repo_root,
            worktree,
            head_ref,
            is_detached,
        })
    }

    /// Resolve a ref string (branch name, tag, commit hash) to a commit OID string.
    pub fn resolve_commit(&self, refspec: &str) -> Result<String> {
        let obj = self.inner.revparse_single(refspec).with_context(|| {
            format!(
                "Could not resolve ref '{}'. Is it a valid branch, tag, or commit?",
                refspec
            )
        })?;
        let commit = obj
            .peel_to_commit()
            .with_context(|| format!("Ref '{}' does not point to a commit", refspec))?;
        Ok(commit.id().to_string())
    }

    /// Compute the merge-base (common ancestor) of two refs.
    ///
    /// This is the "true" fork point — the commit where the branch diverged
    /// from the base. Used as the stable scope key for reviews, because it
    /// doesn't change when the base ref advances (only when the branch is
    /// rebased).
    pub fn merge_base(&self, ref_a: &str, ref_b: &str) -> Result<String> {
        let oid_a = self
            .inner
            .revparse_single(ref_a)
            .with_context(|| format!("Could not resolve ref '{ref_a}'"))?
            .peel_to_commit()
            .with_context(|| format!("Ref '{ref_a}' does not point to a commit"))?
            .id();

        let oid_b = self
            .inner
            .revparse_single(ref_b)
            .with_context(|| format!("Could not resolve ref '{ref_b}'"))?
            .peel_to_commit()
            .with_context(|| format!("Ref '{ref_b}' does not point to a commit"))?
            .id();

        let mb = self
            .inner
            .merge_base(oid_a, oid_b)
            .with_context(|| format!("No common ancestor between '{ref_a}' and '{ref_b}'"))?;

        Ok(mb.to_string())
    }

    /// List all files changed between `base_ref` and `head_ref`.
    pub fn list_changed_files(&self, base_ref: &str, head_ref: &str) -> Result<Vec<FileChange>> {
        let base_tree = self.resolve_tree(base_ref)?;
        let head_tree = self.resolve_tree(head_ref)?;

        let mut diff_opts = git2::DiffOptions::new();
        diff_opts.patience(true);

        let diff = self
            .inner
            .diff_tree_to_tree(Some(&base_tree), Some(&head_tree), Some(&mut diff_opts))
            .context("Failed to compute diff between base and HEAD")?;

        // Enable rename detection
        let mut find_opts = git2::DiffFindOptions::new();
        find_opts.renames(true);
        let mut diff = diff;
        diff.find_similar(Some(&mut find_opts))
            .context("Failed to detect renames")?;

        let mut changes = Vec::new();

        for delta in diff.deltas() {
            let kind = match delta.status() {
                git2::Delta::Added => ChangeKind::Added,
                git2::Delta::Deleted => ChangeKind::Deleted,
                git2::Delta::Modified => ChangeKind::Modified,
                git2::Delta::Renamed => ChangeKind::Renamed,
                git2::Delta::Copied => ChangeKind::Added,
                // Treat typechange, unmodified, etc. as modified
                _ => ChangeKind::Modified,
            };

            let new_path = delta
                .new_file()
                .path()
                .map(|p| p.to_string_lossy().into_owned());
            let old_path = delta
                .old_file()
                .path()
                .map(|p| p.to_string_lossy().into_owned());

            let path = match kind {
                ChangeKind::Deleted => old_path.clone().unwrap_or_default(),
                _ => new_path.unwrap_or_default(),
            };

            let old_path = if kind == ChangeKind::Renamed {
                old_path
            } else {
                None
            };

            changes.push(FileChange {
                path,
                old_path,
                kind,
            });
        }

        // Sort alphabetically by path
        changes.sort_by(|a, b| a.path.cmp(&b.path));

        Ok(changes)
    }

    /// Compute the structured diff for a single file between base and HEAD.
    pub fn diff_file(
        &self,
        base_ref: &str,
        head_ref: &str,
        file_path: &str,
    ) -> Result<DiffContent> {
        let base_tree = self.resolve_tree(base_ref)?;
        let head_tree = self.resolve_tree(head_ref)?;

        let mut diff_opts = git2::DiffOptions::new();
        diff_opts.patience(true);
        diff_opts.pathspec(file_path);

        let diff = self
            .inner
            .diff_tree_to_tree(Some(&base_tree), Some(&head_tree), Some(&mut diff_opts))
            .context("Failed to compute diff")?;

        // Check for binary — need to inspect the actual file content.
        // The delta flags may not be set until the diff is examined, so we
        // check the blob content directly.
        let is_binary = diff.deltas().next().is_some_and(|d| {
            let new_binary = if d.new_file().id().is_zero() {
                false
            } else {
                self.inner
                    .find_blob(d.new_file().id())
                    .map(|b| b.is_binary())
                    .unwrap_or(false)
            };
            let old_binary = if d.old_file().id().is_zero() {
                false
            } else {
                self.inner
                    .find_blob(d.old_file().id())
                    .map(|b| b.is_binary())
                    .unwrap_or(false)
            };
            new_binary || old_binary
        });

        if is_binary {
            return Ok(DiffContent {
                hunks: Vec::new(),
                is_binary: true,
                diff_hash: hash_bytes(b"<binary>"),
            });
        }

        // Collect hunks and lines
        let mut hunks: Vec<DiffHunk> = Vec::new();
        let mut hasher = Sha256::new();

        diff.print(git2::DiffFormat::Patch, |_delta, hunk, line| {
            // Feed everything to the hasher for a stable hash
            hasher.update([line.origin() as u8]);
            hasher.update(line.content());

            match line.origin() {
                '+' | '-' | ' ' => {
                    let diff_line = DiffLine {
                        kind: match line.origin() {
                            '+' => LineKind::Addition,
                            '-' => LineKind::Deletion,
                            _ => LineKind::Context,
                        },
                        content: String::from_utf8_lossy(line.content()).into_owned(),
                        old_lineno: line.old_lineno(),
                        new_lineno: line.new_lineno(),
                    };

                    // If we have a current hunk, add the line to it
                    if let Some(current_hunk) = hunks.last_mut() {
                        current_hunk.lines.push(diff_line);
                    }
                }
                'H' => {
                    // Hunk header
                    if let Some(h) = hunk {
                        hunks.push(DiffHunk {
                            old_start: h.old_start(),
                            old_lines: h.old_lines(),
                            new_start: h.new_start(),
                            new_lines: h.new_lines(),
                            header: String::from_utf8_lossy(line.content())
                                .trim_end()
                                .to_string(),
                            lines: Vec::new(),
                        });
                    }
                }
                _ => {
                    // File header lines, "No newline at end of file", etc.
                }
            }
            true
        })
        .context("Failed to print diff")?;

        let hash = format!("{:x}", hasher.finalize());

        Ok(DiffContent {
            hunks,
            is_binary,
            diff_hash: hash,
        })
    }

    /// Read the full content of a file at a given ref.
    pub fn file_content(&self, refspec: &str, file_path: &str) -> Result<Option<String>> {
        let tree = self.resolve_tree(refspec)?;

        let entry = match tree.get_path(Path::new(file_path)) {
            Ok(entry) => entry,
            Err(_) => return Ok(None), // File doesn't exist at this ref
        };

        let obj = entry
            .to_object(&self.inner)
            .context("Failed to read file object")?;

        let blob = obj.as_blob().context("Path does not point to a file")?;

        if blob.is_binary() {
            return Ok(None);
        }

        Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn resolve_tree(&self, refspec: &str) -> Result<git2::Tree<'_>> {
        let obj = self
            .inner
            .revparse_single(refspec)
            .with_context(|| format!("Could not resolve ref '{}'", refspec))?;
        obj.peel_to_tree()
            .with_context(|| format!("Ref '{}' does not point to a tree", refspec))
    }
}

fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Create a temporary git repo with some commits for testing.
    fn setup_test_repo() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        // Init repo
        run_git(path, &["init"]);
        run_git(path, &["config", "user.email", "test@test.com"]);
        run_git(path, &["config", "user.name", "Test"]);

        // Initial commit with a file
        std::fs::write(
            path.join("hello.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        std::fs::write(
            path.join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )
        .unwrap();
        run_git(path, &["add", "-A"]);
        run_git(path, &["commit", "-m", "initial"]);

        // Tag the base
        run_git(path, &["tag", "base"]);

        // Second commit: modify, add, delete
        std::fs::write(
            path.join("hello.rs"),
            "fn main() {\n    println!(\"hello world\");\n    run();\n}\n",
        )
        .unwrap();
        std::fs::write(path.join("new_file.rs"), "// new file\n").unwrap();
        std::fs::remove_file(path.join("lib.rs")).unwrap();
        run_git(path, &["add", "-A"]);
        run_git(path, &["commit", "-m", "changes"]);

        let repo = Repo::open(path).unwrap();
        (dir, repo)
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
    fn test_list_changed_files() {
        let (_dir, repo) = setup_test_repo();
        let changes = repo.list_changed_files("base", "HEAD").unwrap();

        let paths: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
        assert!(paths.contains(&"hello.rs"), "should contain modified file");
        assert!(paths.contains(&"new_file.rs"), "should contain added file");
        assert!(paths.contains(&"lib.rs"), "should contain deleted file");

        let hello = changes.iter().find(|c| c.path == "hello.rs").unwrap();
        assert_eq!(hello.kind, ChangeKind::Modified);

        let new_file = changes.iter().find(|c| c.path == "new_file.rs").unwrap();
        assert_eq!(new_file.kind, ChangeKind::Added);

        let lib = changes.iter().find(|c| c.path == "lib.rs").unwrap();
        assert_eq!(lib.kind, ChangeKind::Deleted);
    }

    #[test]
    fn test_diff_file_modified() {
        let (_dir, repo) = setup_test_repo();
        let diff = repo.diff_file("base", "HEAD", "hello.rs").unwrap();

        assert!(!diff.is_binary);
        assert!(!diff.hunks.is_empty(), "should have at least one hunk");
        assert!(!diff.diff_hash.is_empty());

        // Should have both additions and deletions
        let has_add = diff
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .any(|l| l.kind == LineKind::Addition);
        let has_del = diff
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .any(|l| l.kind == LineKind::Deletion);
        assert!(has_add, "modified file diff should have additions");
        assert!(has_del, "modified file diff should have deletions");

        // Lines should have line numbers
        for hunk in &diff.hunks {
            for line in &hunk.lines {
                match line.kind {
                    LineKind::Context => {
                        assert!(line.old_lineno.is_some());
                        assert!(line.new_lineno.is_some());
                    }
                    LineKind::Addition => {
                        assert!(line.new_lineno.is_some());
                    }
                    LineKind::Deletion => {
                        assert!(line.old_lineno.is_some());
                    }
                }
            }
        }
    }

    #[test]
    fn test_diff_file_added() {
        let (_dir, repo) = setup_test_repo();
        let diff = repo.diff_file("base", "HEAD", "new_file.rs").unwrap();

        assert!(!diff.is_binary);
        // All lines should be additions
        let all_additions = diff
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .all(|l| l.kind == LineKind::Addition);
        assert!(all_additions, "added file should have only additions");
    }

    #[test]
    fn test_diff_file_deleted() {
        let (_dir, repo) = setup_test_repo();
        let diff = repo.diff_file("base", "HEAD", "lib.rs").unwrap();

        assert!(!diff.is_binary);
        let all_deletions = diff
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .all(|l| l.kind == LineKind::Deletion);
        assert!(all_deletions, "deleted file should have only deletions");
    }

    #[test]
    fn test_diff_hash_deterministic() {
        let (_dir, repo) = setup_test_repo();
        let diff1 = repo.diff_file("base", "HEAD", "hello.rs").unwrap();
        let diff2 = repo.diff_file("base", "HEAD", "hello.rs").unwrap();
        assert_eq!(diff1.diff_hash, diff2.diff_hash);
    }

    #[test]
    fn test_file_content() {
        let (_dir, repo) = setup_test_repo();

        // Read base version
        let base_content = repo.file_content("base", "hello.rs").unwrap().unwrap();
        assert!(base_content.contains("println!(\"hello\")"));
        assert!(!base_content.contains("hello world"));

        // Read HEAD version
        let head_content = repo.file_content("HEAD", "hello.rs").unwrap().unwrap();
        assert!(head_content.contains("hello world"));

        // File that doesn't exist at a given ref
        let missing = repo.file_content("base", "new_file.rs").unwrap();
        assert!(missing.is_none());

        // File that was deleted at HEAD
        let deleted = repo.file_content("HEAD", "lib.rs").unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_repo_context() {
        let (_dir, repo) = setup_test_repo();
        let ctx = repo.context().unwrap();

        assert!(!ctx.head_ref.is_empty());
        assert!(ctx.repo_root.exists());
        assert!(ctx.worktree.exists());
    }

    #[test]
    fn test_resolve_commit() {
        let (_dir, repo) = setup_test_repo();
        let oid = repo.resolve_commit("base").unwrap();
        assert_eq!(oid.len(), 40, "should be a full SHA-1 hex string");

        let err = repo.resolve_commit("nonexistent");
        assert!(err.is_err());
    }

    #[test]
    fn test_merge_base() {
        let (_dir, repo) = setup_test_repo();

        // merge-base of "base" tag and HEAD should be the "base" commit
        let mb = repo.merge_base("base", "HEAD").unwrap();
        let base_oid = repo.resolve_commit("base").unwrap();
        assert_eq!(mb, base_oid);

        // merge-base of HEAD with itself should be HEAD
        let mb_self = repo.merge_base("HEAD", "HEAD").unwrap();
        let head_oid = repo.resolve_commit("HEAD").unwrap();
        assert_eq!(mb_self, head_oid);
    }

    #[test]
    fn test_merge_base_stable_after_base_advances() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        run_git(path, &["init"]);
        run_git(path, &["config", "user.email", "test@test.com"]);
        run_git(path, &["config", "user.name", "Test"]);

        // Create initial commit on main
        std::fs::write(path.join("file.txt"), "initial\n").unwrap();
        run_git(path, &["add", "-A"]);
        run_git(path, &["commit", "-m", "M1"]);

        // Create feature branch
        run_git(path, &["checkout", "-b", "feature"]);
        std::fs::write(path.join("feature.txt"), "feature work\n").unwrap();
        run_git(path, &["add", "-A"]);
        run_git(path, &["commit", "-m", "F1"]);

        // Record merge-base before main advances
        let repo = Repo::open(path).unwrap();
        let mb_before = repo.merge_base("main", "feature").unwrap();

        // Switch to main and advance it
        run_git(path, &["checkout", "main"]);
        std::fs::write(path.join("main_new.txt"), "main work\n").unwrap();
        run_git(path, &["add", "-A"]);
        run_git(path, &["commit", "-m", "M2"]);

        // merge-base should be the same (M1) — main advancing doesn't change it
        let repo = Repo::open(path).unwrap();
        let mb_after = repo.merge_base("main", "feature").unwrap();
        assert_eq!(
            mb_before, mb_after,
            "merge-base should be stable when main advances"
        );
    }

    #[test]
    fn test_rename_detection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        run_git(path, &["init"]);
        run_git(path, &["config", "user.email", "test@test.com"]);
        run_git(path, &["config", "user.name", "Test"]);

        std::fs::write(
            path.join("old_name.rs"),
            "fn foo() {}\nfn bar() {}\nfn baz() {}\n",
        )
        .unwrap();
        run_git(path, &["add", "-A"]);
        run_git(path, &["commit", "-m", "initial"]);
        run_git(path, &["tag", "base"]);

        // Rename with minor change (should still be detected as rename)
        std::fs::remove_file(path.join("old_name.rs")).unwrap();
        std::fs::write(
            path.join("new_name.rs"),
            "fn foo() {}\nfn bar() {}\nfn baz() {}\n",
        )
        .unwrap();
        run_git(path, &["add", "-A"]);
        run_git(path, &["commit", "-m", "rename"]);

        let repo = Repo::open(path).unwrap();
        let changes = repo.list_changed_files("base", "HEAD").unwrap();

        let rename = changes.iter().find(|c| c.kind == ChangeKind::Renamed);
        assert!(rename.is_some(), "should detect rename");
        let rename = rename.unwrap();
        assert_eq!(rename.path, "new_name.rs");
        assert_eq!(rename.old_path.as_deref(), Some("old_name.rs"));
    }

    #[test]
    fn test_binary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        run_git(path, &["init"]);
        run_git(path, &["config", "user.email", "test@test.com"]);
        run_git(path, &["config", "user.name", "Test"]);

        std::fs::write(path.join("readme.txt"), "hello\n").unwrap();
        run_git(path, &["add", "-A"]);
        run_git(path, &["commit", "-m", "initial"]);
        run_git(path, &["tag", "base"]);

        // Add a binary file (random bytes)
        let binary_data: Vec<u8> = (0..256).map(|i| i as u8).collect();
        std::fs::write(path.join("image.bin"), &binary_data).unwrap();
        run_git(path, &["add", "-A"]);
        run_git(path, &["commit", "-m", "add binary"]);

        let repo = Repo::open(path).unwrap();
        let diff = repo.diff_file("base", "HEAD", "image.bin").unwrap();
        assert!(diff.is_binary, "should detect binary file");
        assert!(diff.hunks.is_empty(), "binary diff should have no hunks");
    }
}
