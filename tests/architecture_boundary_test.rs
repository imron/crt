fn source(path: &str) -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    std::fs::read_to_string(format!("{root}/{path}")).unwrap()
}

fn assert_absent(source: &str, path: &str, patterns: &[&str]) {
    for pattern in patterns {
        assert!(
            !source.contains(pattern),
            "{path} must not contain `{pattern}`"
        );
    }
}

#[test]
fn tui_runtime_does_not_own_raw_client() {
    let path = "src/tui/runtime.rs";
    let runtime = source(path);

    assert_absent(
        &runtime,
        path,
        &[
            "crate::client",
            "Client::connect",
            "Client::connect_or_start",
            "Client::connect_http",
        ],
    );
}

#[test]
fn tui_runtime_does_not_execute_backend_workflows_directly() {
    let path = "src/tui/runtime.rs";
    let runtime = source(path);

    assert_absent(
        &runtime,
        path,
        &[
            ".list_changed_files(",
            ".list_file_statuses(",
            ".get_file_diff(",
            ".mark_reviewed(",
            ".unmark_reviewed(",
            ".search_codebase(",
            ".find_definition(",
            ".create_comment(",
            ".resolve_comment(",
            ".unresolve_comment(",
            ".delete_comment(",
        ],
    );
}

#[test]
fn tui_state_stays_terminal_presentation_state() {
    let path = "src/tui/state.rs";
    let state = source(path);

    assert_absent(
        &state,
        path,
        &[
            "crate::client",
            "ReviewActionResult",
            "SearchCodebaseResult",
            "FindDefinitionResult",
            "ListChangedFilesResult",
            "ListFileStatusesResult",
        ],
    );
}
