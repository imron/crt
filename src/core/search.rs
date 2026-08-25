//! Search and definition result semantics shared by UI adapters.

use crate::review_types::{
    DefinitionLocation, FileEntry, FindDefinitionResult, SearchCodebaseResult, SearchMatch,
};

#[derive(Debug, Clone)]
pub enum SearchOutcome {
    NoMatches,
    ShowResults {
        query: String,
        diff_only: bool,
        matches: Vec<SearchMatch>,
    },
}

#[derive(Debug, Clone)]
pub enum DefinitionOutcome {
    NoDefinitions,
    Navigate(LocationTarget),
    ShowResults {
        symbol: String,
        definitions: Vec<DefinitionLocation>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocationTarget {
    InDiff { file_index: usize, line_number: u32 },
    External { file_path: String, line_number: u32 },
}

pub fn search_outcome(query: &str, diff_only: bool, result: SearchCodebaseResult) -> SearchOutcome {
    if result.matches.is_empty() {
        SearchOutcome::NoMatches
    } else {
        SearchOutcome::ShowResults {
            query: query.to_string(),
            diff_only,
            matches: result.matches,
        }
    }
}

pub fn definition_outcome(
    symbol: &str,
    result: FindDefinitionResult,
    files: &[FileEntry],
) -> DefinitionOutcome {
    match result.definitions.as_slice() {
        [] => DefinitionOutcome::NoDefinitions,
        [definition] => DefinitionOutcome::Navigate(resolve_definition_target(files, definition)),
        _ => DefinitionOutcome::ShowResults {
            symbol: symbol.to_string(),
            definitions: result.definitions,
        },
    }
}

pub fn resolve_search_target(files: &[FileEntry], search_match: &SearchMatch) -> LocationTarget {
    resolve_location(files, &search_match.file_path, search_match.line_number)
}

pub fn resolve_definition_target(
    files: &[FileEntry],
    definition: &DefinitionLocation,
) -> LocationTarget {
    resolve_location(files, &definition.file_path, definition.line_number)
}

fn resolve_location(files: &[FileEntry], file_path: &str, line_number: u32) -> LocationTarget {
    if let Some(file_index) = files.iter().position(|f| f.change.path == file_path) {
        LocationTarget::InDiff {
            file_index,
            line_number,
        }
    } else {
        LocationTarget::External {
            file_path: file_path.to_string(),
            line_number,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_types::{ChangeKind, DiffContent, FileChange, ReviewStatus};

    fn entry(path: &str) -> FileEntry {
        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: Vec::new(),
                is_binary: false,
                diff_hash: format!("hash-{path}"),
                content_id: String::new(),
            },
        }
    }

    fn search_match(path: &str, line: u32) -> SearchMatch {
        SearchMatch {
            file_path: path.to_string(),
            line_number: line,
            line_content: "match".to_string(),
        }
    }

    fn definition(path: &str, line: u32) -> DefinitionLocation {
        DefinitionLocation {
            file_path: path.to_string(),
            line_number: line,
            line_content: "def".to_string(),
        }
    }

    #[test]
    fn search_outcome_distinguishes_empty_results() {
        let outcome = search_outcome(
            "needle",
            false,
            SearchCodebaseResult {
                matches: Vec::new(),
            },
        );

        assert!(matches!(outcome, SearchOutcome::NoMatches));
    }

    #[test]
    fn search_outcome_shapes_overlay_results() {
        let outcome = search_outcome(
            "needle",
            true,
            SearchCodebaseResult {
                matches: vec![search_match("a.rs", 10)],
            },
        );

        match outcome {
            SearchOutcome::ShowResults {
                query,
                diff_only,
                matches,
            } => {
                assert_eq!(query, "needle");
                assert!(diff_only);
                assert_eq!(matches.len(), 1);
            }
            SearchOutcome::NoMatches => panic!("expected search results"),
        }
    }

    #[test]
    fn definition_outcome_resolves_single_result_to_diff_target() {
        let files = vec![entry("a.rs")];
        let outcome = definition_outcome(
            "thing",
            FindDefinitionResult {
                definitions: vec![definition("a.rs", 42)],
            },
            &files,
        );

        match outcome {
            DefinitionOutcome::Navigate(target) => {
                assert_eq!(
                    target,
                    LocationTarget::InDiff {
                        file_index: 0,
                        line_number: 42
                    }
                );
            }
            _ => panic!("expected navigation target"),
        }
    }

    #[test]
    fn definition_outcome_shapes_multiple_results_for_overlay() {
        let outcome = definition_outcome(
            "thing",
            FindDefinitionResult {
                definitions: vec![definition("a.rs", 1), definition("b.rs", 2)],
            },
            &[],
        );

        match outcome {
            DefinitionOutcome::ShowResults {
                symbol,
                definitions,
            } => {
                assert_eq!(symbol, "thing");
                assert_eq!(definitions.len(), 2);
            }
            _ => panic!("expected definition picker"),
        }
    }

    #[test]
    fn resolves_external_location_when_file_is_not_in_diff() {
        assert_eq!(
            resolve_search_target(&[entry("a.rs")], &search_match("b.rs", 7)),
            LocationTarget::External {
                file_path: "b.rs".to_string(),
                line_number: 7
            }
        );
    }
}
