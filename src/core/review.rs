//! Review workflow rules shared by UI adapters.
//!
//! This module owns review-list ordering and selection behavior. UI layers
//! provide transport and rendering; these helpers decide how review state
//! changes affect the canonical file list.

use crate::review_types::{FileEntry, ReviewActionResult, ReviewStatus};

pub fn is_reviewed(status: &ReviewStatus) -> bool {
    matches!(status, ReviewStatus::Reviewed { .. })
}

pub fn sort_files(files: &mut [FileEntry]) {
    files.sort_by(|a, b| {
        is_reviewed(&a.status)
            .cmp(&is_reviewed(&b.status))
            .then(a.change.path.cmp(&b.change.path))
    });
}

pub fn unreviewed_count(files: &[FileEntry]) -> usize {
    files.iter().filter(|f| !is_reviewed(&f.status)).count()
}

pub fn selected_path(files: &[FileEntry], selected_file: usize) -> Option<String> {
    files.get(selected_file).map(|f| f.change.path.clone())
}

pub fn restore_selection_by_path(files: &[FileEntry], path: Option<&str>) -> usize {
    path.and_then(|p| files.iter().position(|f| f.change.path == p))
        .unwrap_or(0)
}

pub fn effective_diff_base<'a>(
    entry: Option<&'a FileEntry>,
    merge_base: &'a str,
    show_merge_base: bool,
) -> &'a str {
    if show_merge_base {
        return merge_base;
    }

    let reviewed_commit = entry.and_then(|entry| match &entry.status {
        ReviewStatus::Reviewed {
            reviewed_commit, ..
        }
        | ReviewStatus::Changed {
            reviewed_commit, ..
        } => reviewed_commit.as_deref(),
        ReviewStatus::Unreviewed => None,
    });

    reviewed_commit.unwrap_or(merge_base)
}

pub fn apply_review_result(
    files: &mut [FileEntry],
    selected_file: usize,
    result: &ReviewActionResult,
) -> usize {
    let was_marking = is_reviewed(&result.status);

    let advance_target = if was_marking {
        files
            .iter()
            .skip(selected_file + 1)
            .find(|f| !is_reviewed(&f.status))
            .map(|f| f.change.path.clone())
    } else {
        None
    };

    if let Some(entry) = files.iter_mut().find(|f| f.change.path == result.file_path) {
        entry.status = result.status.clone();
    }

    sort_files(files);

    if was_marking {
        if unreviewed_count(files) > 0 {
            return restore_selection_by_path(files, advance_target.as_deref());
        }
    }

    restore_selection_by_path(files, Some(&result.file_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_types::{ChangeKind, DiffContent, FileChange};

    fn entry(path: &str, status: ReviewStatus) -> FileEntry {
        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status,
            diff: DiffContent {
                hunks: Vec::new(),
                is_binary: false,
                diff_hash: format!("hash-{path}"),
                content_id: String::new(),
            },
        }
    }

    fn reviewed() -> ReviewStatus {
        ReviewStatus::Reviewed {
            at: "2026-06-08T00:00:00Z".to_string(),
            reviewed_commit: Some("reviewed-head".to_string()),
        }
    }

    #[test]
    fn sort_files_groups_unreviewed_then_reviewed_by_path() {
        let mut files = vec![
            entry("z.rs", reviewed()),
            entry("b.rs", ReviewStatus::Unreviewed),
            entry(
                "a.rs",
                ReviewStatus::Changed {
                    at: "2026-06-08T00:00:00Z".to_string(),
                    reviewed_commit: None,
                },
            ),
            entry("c.rs", reviewed()),
        ];

        sort_files(&mut files);

        let paths: Vec<_> = files.iter().map(|f| f.change.path.as_str()).collect();
        assert_eq!(paths, vec!["a.rs", "b.rs", "c.rs", "z.rs"]);
        assert_eq!(unreviewed_count(&files), 2);
    }

    #[test]
    fn apply_review_result_advances_to_next_unreviewed_file() {
        let mut files = vec![
            entry("a.rs", ReviewStatus::Unreviewed),
            entry("b.rs", ReviewStatus::Unreviewed),
            entry("c.rs", ReviewStatus::Unreviewed),
        ];

        let selected = apply_review_result(
            &mut files,
            0,
            &ReviewActionResult {
                file_path: "a.rs".to_string(),
                status: reviewed(),
            },
        );

        assert_eq!(files[selected].change.path, "b.rs");
    }

    #[test]
    fn apply_review_result_wraps_to_first_unreviewed_file() {
        let mut files = vec![
            entry("a.rs", ReviewStatus::Unreviewed),
            entry("b.rs", ReviewStatus::Unreviewed),
            entry("c.rs", ReviewStatus::Unreviewed),
        ];

        let selected = apply_review_result(
            &mut files,
            2,
            &ReviewActionResult {
                file_path: "c.rs".to_string(),
                status: reviewed(),
            },
        );

        assert_eq!(files[selected].change.path, "a.rs");
    }

    #[test]
    fn apply_review_result_stays_on_file_when_all_reviewed() {
        let mut files = vec![
            entry("a.rs", reviewed()),
            entry("b.rs", ReviewStatus::Unreviewed),
        ];
        sort_files(&mut files);

        let selected = apply_review_result(
            &mut files,
            0,
            &ReviewActionResult {
                file_path: "b.rs".to_string(),
                status: reviewed(),
            },
        );

        assert_eq!(files[selected].change.path, "b.rs");
        assert_eq!(unreviewed_count(&files), 0);
    }

    #[test]
    fn apply_review_result_stays_on_file_when_unmarking() {
        let mut files = vec![entry("a.rs", reviewed()), entry("b.rs", reviewed())];

        let selected = apply_review_result(
            &mut files,
            1,
            &ReviewActionResult {
                file_path: "b.rs".to_string(),
                status: ReviewStatus::Unreviewed,
            },
        );

        assert_eq!(files[selected].change.path, "b.rs");
        assert!(!is_reviewed(&files[selected].status));
    }

    #[test]
    fn effective_diff_base_uses_reviewed_commit_when_available() {
        let file = entry("a.rs", reviewed());

        assert_eq!(
            effective_diff_base(Some(&file), "merge-base", false),
            "reviewed-head"
        );
        assert_eq!(
            effective_diff_base(Some(&file), "merge-base", true),
            "merge-base"
        );
    }
}
