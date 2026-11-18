//! Git operations via git2: changed files, diffs, blobs, worktree resolution.
//!
//! This module exposes a clean public API that hides git2 types from callers.
//! Core data types (`FileChange`, `DiffContent`, etc.) are defined in
//! [`crate::review_types`] and re-exported here for convenience.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

// Re-export model types so existing callers (e.g. `git::ChangeKind`) still work.
pub use crate::review_types::{
    ChangeKind, DiffContent, DiffHunk, DiffLine, FileChange, FileVersion, LineKind,
};

/// Diff preferences read from the user's git config.
#[derive(Debug, Clone, Default)]
pub struct GitDiffConfig {
    /// `diff.algorithm` setting, if set.
    pub algorithm: Option<crate::config::DiffAlgorithm>,
}

/// Per-line blame information.
#[derive(Debug, Clone)]
pub struct BlameLine {
    /// Short commit hash (7 chars).
    pub hash: String,
    /// Author name.
    pub author: String,
    /// Commit date (YYYY-MM-DD).
    pub date: String,
}

/// A full commit object ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitId(String);

impl CommitId {
    pub fn new(oid: impl Into<String>) -> Self {
        Self(oid.into())
    }
}

impl AsRef<str> for CommitId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<CommitId> for String {
    fn from(commit: CommitId) -> Self {
        commit.0
    }
}

impl std::fmt::Display for CommitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A named ref class that can be used as a long-lived review base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedRefKind {
    Branch,
    RemoteBranch,
    Tag,
    Other,
}

/// The review base ref provided by the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewBase {
    /// A branch, remote branch, tag, or other named Git ref.
    Named {
        input: String,
        resolved_commit: CommitId,
        kind: NamedRefKind,
    },
    /// An explicit commit hash or rev expression.
    Anonymous {
        input: String,
        resolved_commit: CommitId,
    },
    /// The initial commit reachable from HEAD, reviewed against the empty tree.
    Root { resolved_commit: CommitId },
}

impl ReviewBase {
    pub fn input(&self) -> &str {
        match self {
            Self::Named { input, .. } | Self::Anonymous { input, .. } => input,
            Self::Root { .. } => crate::review_types::ROOT_REVIEW_BASE_REF,
        }
    }

    pub fn resolved_commit(&self) -> &CommitId {
        match self {
            Self::Named {
                resolved_commit, ..
            }
            | Self::Anonymous {
                resolved_commit, ..
            }
            | Self::Root { resolved_commit } => resolved_commit,
        }
    }

    pub fn is_migration_eligible(&self) -> bool {
        matches!(self, Self::Named { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffBase<'a> {
    Commit(&'a str),
    EmptyTree,
}

impl<'a> DiffBase<'a> {
    fn cli_ref(self) -> &'a str {
        const EMPTY_TREE_OID: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
        match self {
            DiffBase::Commit(refspec) => refspec,
            DiffBase::EmptyTree => EMPTY_TREE_OID,
        }
    }
}

/// The identity of HEAD when a session is initialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadIdentity {
    Branch {
        name: String,
        resolved_commit: CommitId,
    },
    Detached {
        commit: CommitId,
    },
}

impl HeadIdentity {
    pub fn display_name(&self) -> String {
        match self {
            Self::Branch { name, .. } => name.clone(),
            Self::Detached { commit } => short_hash(commit.as_ref()),
        }
    }

    pub fn scope_key(&self) -> String {
        self.display_name()
    }

    pub fn resolved_commit(&self) -> &CommitId {
        match self {
            Self::Branch {
                resolved_commit, ..
            } => resolved_commit,
            Self::Detached { commit } => commit,
        }
    }
}

/// Resolved repository context from a working directory path.
#[derive(Debug, Clone)]
pub struct RepoContext {
    /// The main repository root (not the worktree).
    pub repo_root: PathBuf,
    /// The worktree path where the command was run.
    pub worktree: PathBuf,
    /// The resolved HEAD identity.
    pub head: HeadIdentity,
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

        let head = match self.inner.head() {
            Ok(head) => {
                let oid = head.target().context("HEAD has no target")?;
                let commit = CommitId::new(oid.to_string());
                if head.is_branch() {
                    let name = head.shorthand().unwrap_or("HEAD").to_string();
                    HeadIdentity::Branch {
                        name,
                        resolved_commit: commit,
                    }
                } else {
                    HeadIdentity::Detached { commit }
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
            head,
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

    /// Resolve a user-provided review base and classify how it was written.
    ///
    /// Explicit commits and rev expressions like `HEAD~1` are valid refs for
    /// diffing, but they should not be treated like long-lived review bases
    /// for review-scope migration.
    pub fn resolve_review_base(&self, refspec: &str) -> Result<ReviewBase> {
        let resolved_commit = CommitId::new(self.resolve_commit(refspec)?);

        if let Some(kind) = self.named_ref_kind(refspec) {
            return Ok(ReviewBase::Named {
                input: refspec.to_string(),
                resolved_commit,
                kind,
            });
        }

        Ok(ReviewBase::Anonymous {
            input: refspec.to_string(),
            resolved_commit,
        })
    }

    pub fn resolve_root_review_base(&self) -> Result<ReviewBase> {
        Ok(ReviewBase::Root {
            resolved_commit: self.root_commit()?,
        })
    }

    pub fn root_commit(&self) -> Result<CommitId> {
        let mut revwalk = self.inner.revwalk().context("Failed to create revwalk")?;
        revwalk
            .push_head()
            .context("Failed to add HEAD to revwalk")?;

        let mut roots = Vec::new();
        for oid in revwalk {
            let oid = oid.context("Failed to walk repository history")?;
            let commit = self
                .inner
                .find_commit(oid)
                .context("Failed to read commit while finding root")?;
            if commit.parent_count() == 0 {
                roots.push(CommitId::new(oid.to_string()));
            }
        }
        roots.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));

        match roots.len() {
            0 => bail!("No root commit is reachable from HEAD"),
            1 => Ok(roots.remove(0)),
            _ => bail!(
                "Multiple root commits are reachable from HEAD; \
                 --root requires a single-root history"
            ),
        }
    }

    fn named_ref_kind(&self, refspec: &str) -> Option<NamedRefKind> {
        let candidates = [
            (refspec.to_string(), NamedRefKind::Other),
            (format!("refs/heads/{refspec}"), NamedRefKind::Branch),
            (
                format!("refs/remotes/{refspec}"),
                NamedRefKind::RemoteBranch,
            ),
            (format!("refs/tags/{refspec}"), NamedRefKind::Tag),
        ];

        for (candidate, kind) in &candidates {
            if self.inner.find_reference(candidate).is_ok() {
                return Some(*kind);
            }
        }

        self.inner
            .revparse_ext(refspec)
            .ok()
            .and_then(|(_, reference)| reference)
            .map(|reference| {
                if reference.is_branch() {
                    NamedRefKind::Branch
                } else if reference.is_remote() {
                    NamedRefKind::RemoteBranch
                } else if reference.is_tag() {
                    NamedRefKind::Tag
                } else {
                    NamedRefKind::Other
                }
            })
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
        self.list_changed_files_for_base(DiffBase::Commit(base_ref), head_ref)
    }

    pub fn list_changed_files_for_base(
        &self,
        base: DiffBase<'_>,
        head_ref: &str,
    ) -> Result<Vec<FileChange>> {
        let base_tree = self.resolve_diff_base_tree(base)?;
        let head_tree = self.resolve_tree(head_ref)?;

        let mut diff_opts = git2::DiffOptions::new();
        diff_opts.patience(true);

        let diff = self
            .inner
            .diff_tree_to_tree(base_tree.as_ref(), Some(&head_tree), Some(&mut diff_opts))
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

    /// List files changed between a base ref and the working tree.
    /// Similar to `git diff <base_ref>` (includes both staged and unstaged).
    pub fn list_changed_files_workdir(&self, base_ref: &str) -> Result<Vec<FileChange>> {
        self.list_changed_files_workdir_for_base(DiffBase::Commit(base_ref))
    }

    pub fn list_changed_files_workdir_for_base(
        &self,
        base: DiffBase<'_>,
    ) -> Result<Vec<FileChange>> {
        let base_tree = self.resolve_diff_base_tree(base)?;

        let mut diff_opts = git2::DiffOptions::new();
        diff_opts.patience(true);

        // Compare base tree to the working directory (via the index, so
        // both staged and unstaged changes are included).
        let diff = self
            .inner
            .diff_tree_to_workdir_with_index(base_tree.as_ref(), Some(&mut diff_opts))
            .context("Failed to compute diff between base and working tree")?;

        // Enable rename detection.
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
                git2::Delta::Untracked => continue, // skip untracked files
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

        changes.sort_by(|a, b| a.path.cmp(&b.path));

        Ok(changes)
    }

    /// Compute structured workdir diffs for every changed file in a single
    /// tree-to-workdir pass.
    ///
    /// When `include_hunk_lines` is false, hunk line bodies are omitted from
    /// the returned `DiffContent` (hash and binary flag remain accurate).
    pub fn diff_all_files_workdir_for_base(
        &self,
        base: DiffBase<'_>,
        algorithm: crate::config::DiffAlgorithm,
        ignore_whitespace: bool,
        include_hunk_lines: bool,
    ) -> Result<Vec<(FileChange, DiffContent)>> {
        // Histogram is only available via the git CLI and is handled one file
        // at a time so hashing stays consistent with single-file diffs.
        if algorithm == crate::config::DiffAlgorithm::Histogram {
            let changes = self.list_changed_files_workdir_for_base(base)?;
            let mut files = Vec::with_capacity(changes.len());
            for change in changes {
                let mut diff = self.diff_file_workdir_opts_for_base(
                    base,
                    &change.path,
                    algorithm,
                    ignore_whitespace,
                )?;
                if !include_hunk_lines {
                    diff.hunks.clear();
                }
                files.push((change, diff));
            }
            return Ok(files);
        }

        let base_tree = self.resolve_diff_base_tree(base)?;

        let mut diff_opts = git2::DiffOptions::new();
        match algorithm {
            crate::config::DiffAlgorithm::Myers => {}
            crate::config::DiffAlgorithm::Patience => {
                diff_opts.patience(true);
            }
            crate::config::DiffAlgorithm::Minimal => {
                diff_opts.minimal(true);
            }
            crate::config::DiffAlgorithm::Histogram => unreachable!(),
        }
        if ignore_whitespace {
            diff_opts.ignore_whitespace(true);
        }

        let diff = self
            .inner
            .diff_tree_to_workdir_with_index(base_tree.as_ref(), Some(&mut diff_opts))
            .context("Failed to compute bulk diff between base and working tree")?;

        let mut find_opts = git2::DiffFindOptions::new();
        find_opts.renames(true);
        let mut diff = diff;
        diff.find_similar(Some(&mut find_opts))
            .context("Failed to detect renames in bulk workdir diff")?;

        let mut files = Vec::new();
        let delta_count = diff.deltas().len();
        for idx in 0..delta_count {
            let Some(delta) = diff.get_delta(idx) else {
                continue;
            };
            if delta.status() == git2::Delta::Untracked {
                continue;
            }

            let Some(change) = file_change_from_delta(&delta) else {
                continue;
            };

            let is_binary = self.delta_is_binary(&delta);
            if is_binary {
                files.push((
                    change,
                    DiffContent {
                        hunks: Vec::new(),
                        is_binary: true,
                        diff_hash: hash_bytes(b"<binary>"),
                    },
                ));
                continue;
            }

            let mut patch = git2::Patch::from_diff(&diff, idx)
                .context("Failed to build patch from bulk workdir diff")?
                .context("Missing patch for bulk workdir diff delta")?;
            let mut content = parse_patch(&mut patch, include_hunk_lines)?.content;
            content.is_binary = false;
            files.push((change, content));
        }

        files.sort_by(|a, b| a.0.path.cmp(&b.0.path));
        Ok(files)
    }

    /// Like [`Self::diff_all_files_workdir_for_base`], but returns compact
    /// per-file summaries without allocating hunk line bodies.
    pub fn summarize_all_files_workdir_for_base(
        &self,
        base: DiffBase<'_>,
        algorithm: crate::config::DiffAlgorithm,
        ignore_whitespace: bool,
    ) -> Result<Vec<(FileChange, crate::review_types::DiffSummary)>> {
        if algorithm == crate::config::DiffAlgorithm::Histogram {
            let changes = self.list_changed_files_workdir_for_base(base)?;
            let mut files = Vec::with_capacity(changes.len());
            for change in changes {
                let diff = self.diff_file_workdir_opts_for_base(
                    base,
                    &change.path,
                    algorithm,
                    ignore_whitespace,
                )?;
                files.push((change, summarize_diff_content(&diff)));
            }
            return Ok(files);
        }

        let base_tree = self.resolve_diff_base_tree(base)?;

        let mut diff_opts = git2::DiffOptions::new();
        match algorithm {
            crate::config::DiffAlgorithm::Myers => {}
            crate::config::DiffAlgorithm::Patience => {
                diff_opts.patience(true);
            }
            crate::config::DiffAlgorithm::Minimal => {
                diff_opts.minimal(true);
            }
            crate::config::DiffAlgorithm::Histogram => unreachable!(),
        }
        if ignore_whitespace {
            diff_opts.ignore_whitespace(true);
        }

        let diff = self
            .inner
            .diff_tree_to_workdir_with_index(base_tree.as_ref(), Some(&mut diff_opts))
            .context("Failed to compute bulk diff between base and working tree")?;

        let mut find_opts = git2::DiffFindOptions::new();
        find_opts.renames(true);
        let mut diff = diff;
        diff.find_similar(Some(&mut find_opts))
            .context("Failed to detect renames in bulk workdir diff")?;

        let mut files = Vec::new();
        let delta_count = diff.deltas().len();
        for idx in 0..delta_count {
            let Some(delta) = diff.get_delta(idx) else {
                continue;
            };
            if delta.status() == git2::Delta::Untracked {
                continue;
            }

            let Some(change) = file_change_from_delta(&delta) else {
                continue;
            };

            if self.delta_is_binary(&delta) {
                files.push((
                    change,
                    crate::review_types::DiffSummary {
                        hunks: 0,
                        additions: 0,
                        deletions: 0,
                        is_binary: true,
                        diff_hash: hash_bytes(b"<binary>"),
                    },
                ));
                continue;
            }

            let mut patch = git2::Patch::from_diff(&diff, idx)
                .context("Failed to build patch from bulk workdir diff")?
                .context("Missing patch for bulk workdir diff delta")?;
            let parsed = parse_patch(&mut patch, false)?;
            files.push((
                change,
                crate::review_types::DiffSummary {
                    hunks: parsed.hunk_count,
                    additions: parsed.additions,
                    deletions: parsed.deletions,
                    is_binary: false,
                    diff_hash: parsed.content.diff_hash,
                },
            ));
        }

        files.sort_by(|a, b| a.0.path.cmp(&b.0.path));
        Ok(files)
    }

    fn delta_is_binary(&self, delta: &git2::DiffDelta<'_>) -> bool {
        if delta.flags().contains(git2::DiffFlags::BINARY) {
            return true;
        }
        let old_binary = if delta.old_file().id().is_zero() {
            false
        } else {
            self.inner
                .find_blob(delta.old_file().id())
                .map(|b| b.is_binary())
                .unwrap_or(false)
        };
        let new_binary = if delta.new_file().id().is_zero() {
            if let Some(path) = delta.new_file().path() {
                let worktree = self.inner.workdir().unwrap_or_else(|| self.inner.path());
                is_likely_binary_file(&worktree.join(path))
            } else {
                false
            }
        } else {
            self.inner
                .find_blob(delta.new_file().id())
                .map(|b| b.is_binary())
                .unwrap_or(false)
        };
        old_binary || new_binary
    }

    /// Compute the structured diff for a single file between base and HEAD.
    pub fn diff_file(
        &self,
        base_ref: &str,
        head_ref: &str,
        file_path: &str,
    ) -> Result<DiffContent> {
        self.diff_file_opts(
            base_ref,
            head_ref,
            file_path,
            crate::config::DiffAlgorithm::Patience,
            false,
        )
    }

    /// Compute the structured diff for a single file between base and working tree.
    pub fn diff_file_workdir(&self, base_ref: &str, file_path: &str) -> Result<DiffContent> {
        self.diff_file_workdir_opts(
            base_ref,
            file_path,
            crate::config::DiffAlgorithm::Patience,
            false,
        )
    }

    /// Compute the structured diff with configurable algorithm and whitespace.
    pub fn diff_file_opts(
        &self,
        base_ref: &str,
        head_ref: &str,
        file_path: &str,
        algorithm: crate::config::DiffAlgorithm,
        ignore_whitespace: bool,
    ) -> Result<DiffContent> {
        self.diff_file_opts_for_base(
            DiffBase::Commit(base_ref),
            head_ref,
            file_path,
            algorithm,
            ignore_whitespace,
        )
    }

    pub fn diff_file_opts_for_base(
        &self,
        base: DiffBase<'_>,
        head_ref: &str,
        file_path: &str,
        algorithm: crate::config::DiffAlgorithm,
        ignore_whitespace: bool,
    ) -> Result<DiffContent> {
        let base_tree = self.resolve_diff_base_tree(base)?;
        let head_tree = self.resolve_tree(head_ref)?;

        // Histogram requires shelling out to git CLI.
        if algorithm == crate::config::DiffAlgorithm::Histogram {
            return self.diff_file_git_cli_for_base(
                base,
                head_ref,
                file_path,
                "histogram",
                ignore_whitespace,
            );
        }

        let mut diff_opts = git2::DiffOptions::new();
        match algorithm {
            crate::config::DiffAlgorithm::Myers => {} // default
            crate::config::DiffAlgorithm::Patience => {
                diff_opts.patience(true);
            }
            crate::config::DiffAlgorithm::Minimal => {
                diff_opts.minimal(true);
            }
            crate::config::DiffAlgorithm::Histogram => unreachable!(),
        }
        diff_opts.pathspec(file_path);
        if ignore_whitespace {
            diff_opts.ignore_whitespace(true);
        }

        let diff = self
            .inner
            .diff_tree_to_tree(base_tree.as_ref(), Some(&head_tree), Some(&mut diff_opts))
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

        let mut result = parse_diff(&diff)?;
        result.is_binary = is_binary;
        Ok(result)
    }

    /// Compute the structured diff for a single file between base and working
    /// tree, with configurable algorithm and whitespace settings.
    pub fn diff_file_workdir_opts(
        &self,
        base_ref: &str,
        file_path: &str,
        algorithm: crate::config::DiffAlgorithm,
        ignore_whitespace: bool,
    ) -> Result<DiffContent> {
        self.diff_file_workdir_opts_for_base(
            DiffBase::Commit(base_ref),
            file_path,
            algorithm,
            ignore_whitespace,
        )
    }

    pub fn diff_file_workdir_opts_for_base(
        &self,
        base: DiffBase<'_>,
        file_path: &str,
        algorithm: crate::config::DiffAlgorithm,
        ignore_whitespace: bool,
    ) -> Result<DiffContent> {
        let base_tree = self.resolve_diff_base_tree(base)?;

        // Histogram requires shelling out to git CLI.
        if algorithm == crate::config::DiffAlgorithm::Histogram {
            return self.diff_file_workdir_git_cli_for_base(
                base,
                file_path,
                "histogram",
                ignore_whitespace,
            );
        }

        let mut diff_opts = git2::DiffOptions::new();
        match algorithm {
            crate::config::DiffAlgorithm::Myers => {}
            crate::config::DiffAlgorithm::Patience => {
                diff_opts.patience(true);
            }
            crate::config::DiffAlgorithm::Minimal => {
                diff_opts.minimal(true);
            }
            crate::config::DiffAlgorithm::Histogram => unreachable!(),
        }
        diff_opts.pathspec(file_path);
        if ignore_whitespace {
            diff_opts.ignore_whitespace(true);
        }

        let diff = self
            .inner
            .diff_tree_to_workdir_with_index(base_tree.as_ref(), Some(&mut diff_opts))
            .context("Failed to compute diff against working tree")?;

        // Check for binary. For workdir diffs, the new file OID may be zero
        // (file on disk, not in ODB). Check the old blob and also inspect
        // the diff flags.
        let is_binary = diff.deltas().next().is_some_and(|d| {
            // Check diff delta flags first (git2 sets BINARY after content inspection).
            if d.flags().contains(git2::DiffFlags::BINARY) {
                return true;
            }
            let old_binary = if d.old_file().id().is_zero() {
                false
            } else {
                self.inner
                    .find_blob(d.old_file().id())
                    .map(|b| b.is_binary())
                    .unwrap_or(false)
            };
            // For the new file, check if the blob is in the ODB; if not,
            // read from disk and check for NUL bytes.
            let new_binary = if d.new_file().id().is_zero() {
                if let Some(path) = d.new_file().path() {
                    let worktree = self.inner.workdir().unwrap_or_else(|| self.inner.path());
                    is_likely_binary_file(&worktree.join(path))
                } else {
                    false
                }
            } else {
                self.inner
                    .find_blob(d.new_file().id())
                    .map(|b| b.is_binary())
                    .unwrap_or(false)
            };
            old_binary || new_binary
        });

        if is_binary {
            return Ok(DiffContent {
                hunks: Vec::new(),
                is_binary: true,
                diff_hash: hash_bytes(b"<binary>"),
            });
        }

        let mut result = parse_diff(&diff)?;
        result.is_binary = is_binary;
        Ok(result)
    }

    /// Compute a workdir diff by shelling out to `git` CLI (e.g. histogram).
    fn diff_file_workdir_git_cli_for_base(
        &self,
        base: DiffBase<'_>,
        file_path: &str,
        algorithm: &str,
        ignore_whitespace: bool,
    ) -> Result<DiffContent> {
        let worktree = self
            .inner
            .workdir()
            .unwrap_or_else(|| self.inner.path())
            .to_path_buf();

        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(&worktree);
        cmd.args(["diff", &format!("--diff-algorithm={algorithm}")]);
        if ignore_whitespace {
            cmd.arg("-w");
        }
        // No second ref — compares base to working tree.
        cmd.args([base.cli_ref(), "--", file_path]);

        let output = cmd
            .output()
            .context("Failed to run git diff (is git on PATH?)")?;

        if !output.status.success() && output.stdout.is_empty() {
            return Ok(DiffContent {
                hunks: Vec::new(),
                is_binary: false,
                diff_hash: hash_bytes(b""),
            });
        }

        let stdout_str = String::from_utf8_lossy(&output.stdout);
        if stdout_str.contains("Binary files") && stdout_str.contains("differ") {
            return Ok(DiffContent {
                hunks: Vec::new(),
                is_binary: true,
                diff_hash: hash_bytes(b"<binary>"),
            });
        }

        let diff =
            git2::Diff::from_buffer(&output.stdout).context("Failed to parse git diff output")?;

        parse_diff(&diff)
    }

    /// Read the content of a file from the working tree on disk.
    pub fn file_content_workdir(&self, file_path: &str) -> Result<Option<String>> {
        let worktree = self.inner.workdir().unwrap_or_else(|| self.inner.path());
        let full_path = worktree.join(file_path);
        match std::fs::read(&full_path) {
            Ok(bytes) => {
                // Check for binary (NUL byte in the first 8KB).
                if bytes.iter().take(8192).any(|&b| b == 0) {
                    return Ok(None);
                }
                Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Compute a diff by shelling out to the `git` CLI for algorithms
    /// not supported by libgit2 (e.g. histogram). The output is parsed
    /// back via `git2::Diff::from_buffer`.
    fn diff_file_git_cli_for_base(
        &self,
        base: DiffBase<'_>,
        head_ref: &str,
        file_path: &str,
        algorithm: &str,
        ignore_whitespace: bool,
    ) -> Result<DiffContent> {
        let worktree = self
            .inner
            .workdir()
            .unwrap_or_else(|| self.inner.path())
            .to_path_buf();

        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(&worktree);
        cmd.args(["diff", &format!("--diff-algorithm={algorithm}")]);
        if ignore_whitespace {
            cmd.arg("-w");
        }
        cmd.args([base.cli_ref(), head_ref, "--", file_path]);

        let output = cmd
            .output()
            .context("Failed to run git diff (is git on PATH?)")?;

        if !output.status.success() && output.stdout.is_empty() {
            // Non-zero exit with no output typically means no diff.
            return Ok(DiffContent {
                hunks: Vec::new(),
                is_binary: false,
                diff_hash: hash_bytes(b""),
            });
        }

        // Check for binary.
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        if stdout_str.contains("Binary files") && stdout_str.contains("differ") {
            return Ok(DiffContent {
                hunks: Vec::new(),
                is_binary: true,
                diff_hash: hash_bytes(b"<binary>"),
            });
        }

        // Parse the unified diff output via git2.
        let diff =
            git2::Diff::from_buffer(&output.stdout).context("Failed to parse git diff output")?;

        parse_diff(&diff)
    }

    /// Read the full content of a file at a given ref.
    pub fn file_content(&self, refspec: &str, file_path: &str) -> Result<Option<String>> {
        self.file_content_for_base(DiffBase::Commit(refspec), file_path)
    }

    pub fn file_content_for_base(
        &self,
        base: DiffBase<'_>,
        file_path: &str,
    ) -> Result<Option<String>> {
        let DiffBase::Commit(refspec) = base else {
            return Ok(None);
        };
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
    // Object queries (for rebase migration)
    // -----------------------------------------------------------------------

    /// Get the blob OID (hex string) of a file at a given commit.
    ///
    /// Returns `Ok(None)` if the file doesn't exist at that commit.
    /// Returns `Err` if the commit itself can't be resolved (e.g. GC'd).
    pub fn file_blob_hash(&self, commit_ref: &str, file_path: &str) -> Result<Option<String>> {
        let tree = self.resolve_tree(commit_ref)?;
        match tree.get_path(Path::new(file_path)) {
            Ok(entry) => Ok(Some(entry.id().to_string())),
            Err(_) => Ok(None), // File doesn't exist at this commit
        }
    }

    /// Check whether a commit OID exists in the object store.
    ///
    /// Returns `true` if the commit is resolvable (even if dangling),
    /// `false` if it has been garbage collected.
    pub fn commit_exists(&self, oid: &str) -> bool {
        self.inner.revparse_single(oid).is_ok()
    }

    // -----------------------------------------------------------------------
    // Blame
    // -----------------------------------------------------------------------

    /// Compute blame for a file at a given ref.
    /// Returns a Vec with one entry per line (1-indexed line numbers).
    pub fn blame_file(&self, refspec: &str, file_path: &str) -> Result<Vec<BlameLine>> {
        self.blame_file_for_base(DiffBase::Commit(refspec), file_path)
    }

    pub fn blame_file_for_base(
        &self,
        base: DiffBase<'_>,
        file_path: &str,
    ) -> Result<Vec<BlameLine>> {
        let DiffBase::Commit(refspec) = base else {
            return Ok(Vec::new());
        };
        let commit = self
            .inner
            .revparse_single(refspec)
            .with_context(|| format!("Could not resolve ref '{}'", refspec))?
            .peel_to_commit()
            .with_context(|| format!("Ref '{}' does not point to a commit", refspec))?;

        let mut blame_opts = git2::BlameOptions::new();
        blame_opts.newest_commit(commit.id());

        let blame = self
            .inner
            .blame_file(std::path::Path::new(file_path), Some(&mut blame_opts))
            .with_context(|| format!("Failed to blame {file_path}"))?;

        // Count total lines by finding max line in the last hunk.
        let total_lines = blame
            .iter()
            .map(|h| h.final_start_line() + h.lines_in_hunk() - 1)
            .max()
            .unwrap_or(0);

        let mut result = Vec::with_capacity(total_lines);
        for line_no in 1..=total_lines {
            if let Some(hunk) = blame.get_line(line_no) {
                let oid = hunk.final_commit_id();
                let hash = format!("{}", oid)[..7.min(format!("{}", oid).len())].to_string();
                let sig = hunk.final_signature();
                let author = sig.name().unwrap_or("?").to_string();
                let date = sig
                    .when()
                    .seconds()
                    .try_into()
                    .ok()
                    .and_then(|secs| {
                        chrono::DateTime::from_timestamp(secs, 0)
                            .map(|dt| dt.format("%Y-%m-%d").to_string())
                    })
                    .unwrap_or_else(|| "??????????".to_string());
                result.push(BlameLine { hash, author, date });
            } else {
                result.push(BlameLine {
                    hash: "???????".to_string(),
                    author: "?".to_string(),
                    date: "??????????".to_string(),
                });
            }
        }

        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Read diff preferences from the user's git config
    /// (`diff.algorithm`), resolved with system → global → local precedence.
    pub fn diff_config(&self) -> GitDiffConfig {
        let config = match self.inner.config() {
            Ok(c) => c,
            Err(_) => return GitDiffConfig::default(),
        };

        let algorithm = config.get_string("diff.algorithm").ok().and_then(|s| {
            match s.to_lowercase().as_str() {
                "myers" | "default" => Some(crate::config::DiffAlgorithm::Myers),
                "patience" => Some(crate::config::DiffAlgorithm::Patience),
                "minimal" => Some(crate::config::DiffAlgorithm::Minimal),
                "histogram" => Some(crate::config::DiffAlgorithm::Histogram),
                _ => None,
            }
        });

        GitDiffConfig { algorithm }
    }

    fn resolve_tree(&self, refspec: &str) -> Result<git2::Tree<'_>> {
        let obj = self
            .inner
            .revparse_single(refspec)
            .with_context(|| format!("Could not resolve ref '{}'", refspec))?;
        obj.peel_to_tree()
            .with_context(|| format!("Ref '{}' does not point to a tree", refspec))
    }

    fn resolve_diff_base_tree(&self, base: DiffBase<'_>) -> Result<Option<git2::Tree<'_>>> {
        match base {
            DiffBase::Commit(refspec) => self.resolve_tree(refspec).map(Some),
            DiffBase::EmptyTree => Ok(None),
        }
    }
}

fn file_change_from_delta(delta: &git2::DiffDelta<'_>) -> Option<FileChange> {
    let kind = match delta.status() {
        git2::Delta::Added | git2::Delta::Copied => ChangeKind::Added,
        git2::Delta::Deleted => ChangeKind::Deleted,
        git2::Delta::Modified => ChangeKind::Modified,
        git2::Delta::Renamed => ChangeKind::Renamed,
        git2::Delta::Untracked => return None,
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
    if path.is_empty() {
        return None;
    }

    let old_path = if kind == ChangeKind::Renamed {
        old_path
    } else {
        None
    };

    Some(FileChange {
        path,
        old_path,
        kind,
    })
}

struct ParsedDiff {
    content: DiffContent,
    hunk_count: usize,
    additions: usize,
    deletions: usize,
}

/// Parse a `git2::Diff` into our `DiffContent` structure.
fn parse_diff(diff: &git2::Diff<'_>) -> Result<DiffContent> {
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut hasher = Sha256::new();
    let mut hunk_count = 0usize;
    let mut additions = 0usize;
    let mut deletions = 0usize;

    diff.print(git2::DiffFormat::Patch, |_delta, hunk, line| {
        accumulate_diff_line(
            hunk.as_ref(),
            &line,
            true,
            &mut hunks,
            &mut hasher,
            &mut hunk_count,
            &mut additions,
            &mut deletions,
        );
        true
    })
    .context("Failed to print diff")?;

    let _ = (hunk_count, additions, deletions);
    Ok(DiffContent {
        hunks,
        is_binary: false,
        diff_hash: format!("{:x}", hasher.finalize()),
    })
}

/// Parse a single-file `git2::Patch` into `DiffContent` plus stats.
///
/// When `include_hunk_lines` is false, line bodies are hashed but not stored.
fn parse_patch(patch: &mut git2::Patch<'_>, include_hunk_lines: bool) -> Result<ParsedDiff> {
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut hasher = Sha256::new();
    let mut hunk_count = 0usize;
    let mut additions = 0usize;
    let mut deletions = 0usize;

    patch
        .print(&mut |_delta, hunk, line| {
            accumulate_diff_line(
                hunk.as_ref(),
                &line,
                include_hunk_lines,
                &mut hunks,
                &mut hasher,
                &mut hunk_count,
                &mut additions,
                &mut deletions,
            );
            true
        })
        .context("Failed to print patch")?;

    Ok(ParsedDiff {
        content: DiffContent {
            hunks,
            is_binary: false,
            diff_hash: format!("{:x}", hasher.finalize()),
        },
        hunk_count,
        additions,
        deletions,
    })
}

fn accumulate_diff_line(
    hunk: Option<&git2::DiffHunk<'_>>,
    line: &git2::DiffLine<'_>,
    include_hunk_lines: bool,
    hunks: &mut Vec<DiffHunk>,
    hasher: &mut Sha256,
    hunk_count: &mut usize,
    additions: &mut usize,
    deletions: &mut usize,
) {
    match line.origin() {
        '+' | '-' | ' ' => {
            // Hash only the semantic diff content (origin + text).
            hasher.update([line.origin() as u8]);
            hasher.update(line.content());

            match line.origin() {
                '+' => *additions += 1,
                '-' => *deletions += 1,
                _ => {}
            }

            if !include_hunk_lines {
                return;
            }

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

            if let Some(current_hunk) = hunks.last_mut() {
                current_hunk.lines.push(diff_line);
            }
        }
        'H' => {
            // Hunk headers are part of the semantic content.
            hasher.update([line.origin() as u8]);
            hasher.update(line.content());
            *hunk_count += 1;

            if let Some(h) = hunk {
                if include_hunk_lines {
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
        }
        _ => {
            // File headers, "No newline at end of file", etc.
            // NOT included in the hash — they contain metadata
            // (blob OIDs, mode bits) that can vary without the
            // actual diff content changing.
        }
    }
}

fn summarize_diff_content(diff: &DiffContent) -> crate::review_types::DiffSummary {
    let mut additions = 0usize;
    let mut deletions = 0usize;
    for hunk in &diff.hunks {
        for line in &hunk.lines {
            match line.kind {
                LineKind::Addition => additions += 1,
                LineKind::Deletion => deletions += 1,
                LineKind::Context => {}
            }
        }
    }
    crate::review_types::DiffSummary {
        hunks: diff.hunks.len(),
        additions,
        deletions,
        is_binary: diff.is_binary,
        diff_hash: diff.diff_hash.clone(),
    }
}

fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Check if a file on disk is likely binary (contains NUL in first 512 bytes).
fn is_likely_binary_file(path: &Path) -> bool {
    if let Ok(data) = std::fs::read(path) {
        let check_len = data.len().min(512);
        data[..check_len].contains(&0)
    } else {
        false
    }
}

/// Truncate a hex hash to a short display form, safely.
pub fn short_hash(hash: &str) -> String {
    const SHORT_HASH_LEN: usize = 12;
    hash.get(..SHORT_HASH_LEN).unwrap_or(hash).to_string()
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

    fn git_output(path: &Path, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&out.stdout).trim().to_string()
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
    fn bulk_workdir_diff_matches_per_file_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        run_git(path, &["init"]);
        run_git(path, &["config", "user.email", "test@test.com"]);
        run_git(path, &["config", "user.name", "Test"]);
        std::fs::write(path.join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(path.join("b.rs"), "fn b() {}\n").unwrap();
        run_git(path, &["add", "."]);
        run_git(path, &["commit", "-m", "base"]);
        run_git(path, &["tag", "base"]);
        std::fs::write(path.join("a.rs"), "fn a() { changed }\n").unwrap();
        std::fs::write(path.join("b.rs"), "fn b() { changed }\n").unwrap();
        std::fs::write(path.join("c.rs"), "fn c() {}\n").unwrap();
        // Staged+unstaged workdir diffs ignore pure untracked files.
        run_git(path, &["add", "c.rs"]);

        let repo = Repo::open(path).unwrap();
        let bulk = repo
            .diff_all_files_workdir_for_base(
                DiffBase::Commit("base"),
                crate::config::DiffAlgorithm::Patience,
                false,
                true,
            )
            .unwrap();
        assert_eq!(bulk.len(), 3);

        for (change, diff) in &bulk {
            let single = repo
                .diff_file_workdir_opts_for_base(
                    DiffBase::Commit("base"),
                    &change.path,
                    crate::config::DiffAlgorithm::Patience,
                    false,
                )
                .unwrap();
            assert_eq!(
                diff.diff_hash, single.diff_hash,
                "hash mismatch for {}",
                change.path
            );
            assert_eq!(diff.hunks.len(), single.hunks.len());
        }

        let summary_only = repo
            .diff_all_files_workdir_for_base(
                DiffBase::Commit("base"),
                crate::config::DiffAlgorithm::Patience,
                false,
                false,
            )
            .unwrap();
        assert_eq!(summary_only.len(), 3);
        for ((_, full), (_, summary)) in bulk.iter().zip(summary_only.iter()) {
            assert_eq!(full.diff_hash, summary.diff_hash);
            assert!(summary.hunks.is_empty());
        }
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

        assert!(!ctx.head.display_name().is_empty());
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
    fn test_resolve_review_base_classifies_named_and_anonymous_refs() {
        let (_dir, repo) = setup_test_repo();
        let oid = repo.resolve_commit("base").unwrap();

        match repo.resolve_review_base("base").unwrap() {
            ReviewBase::Named {
                input,
                resolved_commit,
                kind,
            } => {
                assert_eq!(input, "base");
                assert_eq!(resolved_commit.as_ref(), oid);
                assert_eq!(kind, NamedRefKind::Tag);
            }
            ReviewBase::Anonymous { .. } => panic!("tag should be a named review base"),
            ReviewBase::Root { .. } => panic!("tag should not be a root review base"),
        }

        match repo.resolve_review_base(&oid).unwrap() {
            ReviewBase::Anonymous {
                input,
                resolved_commit,
            } => {
                assert_eq!(input, oid);
                assert_eq!(resolved_commit.as_ref(), oid);
            }
            ReviewBase::Named { .. } => panic!("commit hash should be anonymous"),
            ReviewBase::Root { .. } => panic!("commit hash should not be a root review base"),
        }

        assert!(matches!(
            repo.resolve_review_base("HEAD~1").unwrap(),
            ReviewBase::Anonymous { .. }
        ));
    }

    #[test]
    fn root_review_base_uses_initial_commit_and_empty_tree_diff() {
        let (dir, repo) = setup_test_repo();
        let root = git_output(dir.path(), &["rev-list", "--max-parents=0", "HEAD"]);

        match repo.resolve_root_review_base().unwrap() {
            ReviewBase::Root { resolved_commit } => {
                assert_eq!(resolved_commit.as_ref(), root);
            }
            _ => panic!("root review should resolve to root base"),
        }

        let changes = repo
            .list_changed_files_for_base(DiffBase::EmptyTree, "HEAD")
            .unwrap();
        let paths: Vec<&str> = changes.iter().map(|change| change.path.as_str()).collect();
        assert!(paths.contains(&"hello.rs"));
        assert!(paths.contains(&"new_file.rs"));

        let diff = repo
            .diff_file_opts_for_base(
                DiffBase::EmptyTree,
                "HEAD",
                "hello.rs",
                crate::config::DiffAlgorithm::Patience,
                false,
            )
            .unwrap();
        assert!(
            diff.hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .all(|line| line.kind == LineKind::Addition)
        );
        assert!(
            repo.file_content_for_base(DiffBase::EmptyTree, "hello.rs")
                .unwrap()
                .is_none()
        );
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

    #[test]
    fn test_file_blob_hash() {
        let (_dir, repo) = setup_test_repo();

        // File exists at HEAD.
        let hash = repo.file_blob_hash("HEAD", "hello.rs").unwrap();
        assert!(hash.is_some());
        assert_eq!(hash.as_ref().unwrap().len(), 40, "should be full SHA-1");

        // File does not exist at HEAD (was deleted).
        let hash = repo.file_blob_hash("HEAD", "lib.rs").unwrap();
        assert!(hash.is_none());

        // File exists at base.
        let hash = repo.file_blob_hash("base", "lib.rs").unwrap();
        assert!(hash.is_some());

        // Same file at same ref should produce the same blob hash.
        let h1 = repo.file_blob_hash("HEAD", "hello.rs").unwrap();
        let h2 = repo.file_blob_hash("HEAD", "hello.rs").unwrap();
        assert_eq!(h1, h2);

        // Different content at different refs should produce different hashes.
        let base_hash = repo.file_blob_hash("base", "hello.rs").unwrap();
        let head_hash = repo.file_blob_hash("HEAD", "hello.rs").unwrap();
        assert_ne!(base_hash, head_hash);
    }

    #[test]
    fn test_commit_exists() {
        let (_dir, repo) = setup_test_repo();

        let head_oid = repo.resolve_commit("HEAD").unwrap();
        assert!(repo.commit_exists(&head_oid));

        let base_oid = repo.resolve_commit("base").unwrap();
        assert!(repo.commit_exists(&base_oid));

        // Nonexistent commit.
        assert!(!repo.commit_exists("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"));
    }
}
