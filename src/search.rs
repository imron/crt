// Codebase search (:gr) and go-to-definition (Ctrl-]).
// Implemented in Stage 11.

use std::path::Path;

use anyhow::Result;

use crate::review_types::{
    DefinitionLocation, FindDefinitionResult, SearchCodebaseResult, SearchMatch,
};

/// Maximum number of search results to return (prevents UI overload).
const MAX_SEARCH_RESULTS: usize = 500;

/// Maximum file size (in bytes) to search. Skips very large files.
const MAX_FILE_SIZE: u64 = 2 * 1024 * 1024; // 2 MiB

// ---------------------------------------------------------------------------
// Codebase search
// ---------------------------------------------------------------------------

/// Search the codebase for lines matching a regex pattern.
///
/// Uses `ignore::WalkBuilder` to respect `.gitignore` rules.
/// When `diff_files` is provided, only those files are searched.
pub fn search_codebase(
    repo_root: &Path,
    pattern: &str,
    diff_files: Option<&[String]>,
) -> Result<SearchCodebaseResult> {
    let re = regex::Regex::new(pattern).map_err(|e| anyhow::anyhow!("Invalid regex: {e}"))?;

    let mut matches = Vec::new();

    if let Some(files) = diff_files {
        // Search only the specified files.
        for file_path in files {
            let abs = repo_root.join(file_path);
            if !abs.is_file() {
                continue;
            }
            if let Ok(meta) = abs.metadata() {
                if meta.len() > MAX_FILE_SIZE {
                    continue;
                }
            }
            if let Ok(content) = std::fs::read_to_string(&abs) {
                search_file_content(&re, file_path, &content, &mut matches);
            }
            if matches.len() >= MAX_SEARCH_RESULTS {
                break;
            }
        }
    } else {
        // Walk the entire repository, respecting .gitignore.
        let walker = ignore::WalkBuilder::new(repo_root)
            .hidden(false) // don't skip dotfiles that aren't gitignored
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map_or(false, |ft| ft.is_file()) {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                if meta.len() > MAX_FILE_SIZE {
                    continue;
                }
            }
            let abs_path = entry.path();
            let rel_path = match abs_path.strip_prefix(repo_root) {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(_) => continue,
            };

            // Skip binary-looking files.
            if is_likely_binary(abs_path) {
                continue;
            }

            if let Ok(content) = std::fs::read_to_string(abs_path) {
                search_file_content(&re, &rel_path, &content, &mut matches);
            }
            if matches.len() >= MAX_SEARCH_RESULTS {
                break;
            }
        }
    }

    matches.truncate(MAX_SEARCH_RESULTS);
    Ok(SearchCodebaseResult { matches })
}

/// Search a single file's content for regex matches, appending to `results`.
fn search_file_content(
    re: &regex::Regex,
    file_path: &str,
    content: &str,
    results: &mut Vec<SearchMatch>,
) {
    for (line_idx, line) in content.lines().enumerate() {
        if re.is_match(line) {
            results.push(SearchMatch {
                file_path: file_path.to_string(),
                line_number: (line_idx + 1) as u32,
                line_content: line.to_string(),
            });
            if results.len() >= MAX_SEARCH_RESULTS {
                return;
            }
        }
    }
}

/// Heuristic: check the first 512 bytes for NUL characters.
fn is_likely_binary(path: &Path) -> bool {
    if let Ok(data) = std::fs::read(path) {
        let check_len = data.len().min(512);
        data[..check_len].contains(&0)
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Definition finding
// ---------------------------------------------------------------------------

/// Trait for finding definitions (allows future LSP/tree-sitter replacement).
pub trait DefinitionFinder {
    fn find_definition(
        &self,
        symbol: &str,
        context_file: Option<&str>,
        repo_root: &Path,
    ) -> Result<FindDefinitionResult>;
}

/// Regex-based definition finder. Scans the codebase for common definition
/// patterns (fn, struct, class, def, const, etc.) matching the symbol name.
pub struct RegexDefinitionFinder;

impl DefinitionFinder for RegexDefinitionFinder {
    fn find_definition(
        &self,
        symbol: &str,
        _context_file: Option<&str>,
        repo_root: &Path,
    ) -> Result<FindDefinitionResult> {
        // Build regex patterns for common definition forms across languages.
        let patterns = definition_patterns(symbol);
        let regexes: Vec<regex::Regex> = patterns
            .iter()
            .filter_map(|p| regex::Regex::new(p).ok())
            .collect();

        if regexes.is_empty() {
            return Ok(FindDefinitionResult {
                definitions: Vec::new(),
            });
        }

        let mut definitions = Vec::new();

        let walker = ignore::WalkBuilder::new(repo_root)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map_or(false, |ft| ft.is_file()) {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                if meta.len() > MAX_FILE_SIZE {
                    continue;
                }
            }
            let abs_path = entry.path();
            let rel_path = match abs_path.strip_prefix(repo_root) {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(_) => continue,
            };

            if is_likely_binary(abs_path) {
                continue;
            }

            if let Ok(content) = std::fs::read_to_string(abs_path) {
                for (line_idx, line) in content.lines().enumerate() {
                    for re in &regexes {
                        if re.is_match(line) {
                            definitions.push(DefinitionLocation {
                                file_path: rel_path.clone(),
                                line_number: (line_idx + 1) as u32,
                                line_content: line.to_string(),
                            });
                            break; // one match per line is enough
                        }
                    }
                }
            }
            if definitions.len() >= 50 {
                break;
            }
        }

        Ok(FindDefinitionResult { definitions })
    }
}

/// Generate regex patterns that match common definition forms for a symbol.
fn definition_patterns(symbol: &str) -> Vec<String> {
    let sym = regex::escape(symbol);
    vec![
        // Rust: fn, struct, enum, trait, type, const, static, mod, impl, macro_rules
        format!(r"(?:pub\s+(?:\(crate\)\s+)?)?fn\s+{sym}\b"),
        format!(r"(?:pub\s+(?:\(crate\)\s+)?)?struct\s+{sym}\b"),
        format!(r"(?:pub\s+(?:\(crate\)\s+)?)?enum\s+{sym}\b"),
        format!(r"(?:pub\s+(?:\(crate\)\s+)?)?trait\s+{sym}\b"),
        format!(r"(?:pub\s+(?:\(crate\)\s+)?)?type\s+{sym}\b"),
        format!(r"(?:pub\s+(?:\(crate\)\s+)?)?const\s+{sym}\b"),
        format!(r"(?:pub\s+(?:\(crate\)\s+)?)?static\s+{sym}\b"),
        format!(r"(?:pub\s+(?:\(crate\)\s+)?)?mod\s+{sym}\b"),
        format!(r"macro_rules!\s+{sym}\b"),
        // Go: func, type, var, const
        format!(r"func\s+(?:\([^)]*\)\s+)?{sym}\b"),
        format!(r"type\s+{sym}\b"),
        format!(r"var\s+{sym}\b"),
        // Python: def, class
        format!(r"def\s+{sym}\b"),
        format!(r"class\s+{sym}\b"),
        // JavaScript/TypeScript: function, class, const, let, var, interface, type
        format!(r"(?:export\s+(?:default\s+)?)?function\s+{sym}\b"),
        format!(r"(?:export\s+(?:default\s+)?)?class\s+{sym}\b"),
        format!(r"(?:export\s+)?(?:const|let|var)\s+{sym}\b"),
        format!(r"(?:export\s+)?interface\s+{sym}\b"),
        format!(r"(?:export\s+)?type\s+{sym}\b"),
        // C/C++: simple function/class/struct/enum/typedef patterns
        format!(r"(?:class|struct|enum|union)\s+{sym}\b"),
        format!(r"typedef\s+.*\b{sym}\s*;"),
        format!(r"#define\s+{sym}\b"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definition_patterns_rust_fn() {
        let patterns = definition_patterns("foo_bar");
        let re = regex::Regex::new(&patterns[0]).unwrap();
        assert!(re.is_match("pub fn foo_bar(x: i32) -> bool {"));
        assert!(re.is_match("fn foo_bar() {"));
        assert!(re.is_match("pub(crate) fn foo_bar() {"));
        assert!(!re.is_match("fn foo_baz() {"));
    }

    #[test]
    fn test_definition_patterns_python() {
        let patterns = definition_patterns("MyClass");
        // class pattern
        let class_re = patterns
            .iter()
            .find(|p| p.contains("class") && p.contains("MyClass"))
            .expect("should have class pattern");
        let re = regex::Regex::new(class_re).unwrap();
        assert!(re.is_match("class MyClass:"));
        assert!(re.is_match("class MyClass(Base):"));
    }

    #[test]
    fn test_is_likely_binary() {
        // Non-existent file is treated as binary (safe default).
        assert!(!is_likely_binary(Path::new("/dev/null")));
    }
}
