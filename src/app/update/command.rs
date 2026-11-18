use super::output::AppOutput;
use crate::app::AppState;
use crate::core::command::{Command, CommandParse};

/// Apply a parsed command emitted by the core interaction engine.
pub fn apply_command(state: &mut AppState, update: &mut AppOutput, command: CommandParse) {
    match command {
        CommandParse::Empty => {}
        CommandParse::NeedsArgument { usage } | CommandParse::NeedsWord { usage } => {
            update.set_status(usage);
        }
        CommandParse::Parsed(Command::Quit) => {
            update.request_quit();
        }
        CommandParse::Parsed(
            cmd @ (Command::SearchAll { .. }
            | Command::SearchDiff { .. }
            | Command::FindDefinition { .. }
            | Command::SetMergeBase(Some(_))),
        ) => {
            update.request_command(cmd);
        }
        CommandParse::Parsed(Command::SetMergeBase(None)) => {
            update.set_status(format!("Merge base: {}", state.context.merge_base));
        }
        CommandParse::Parsed(Command::SetBlame(true)) => {
            state.show_blame = true;
            state.load_blame();
            update.set_status("Blame: shown");
        }
        CommandParse::Parsed(Command::SetBlame(false)) => {
            state.show_blame = false;
            state.load_blame();
            update.set_status("Blame: hidden");
        }
        CommandParse::Parsed(Command::SetComments(show)) => {
            state.show_comments_panel = show;
            state.mark_model_changed();
            let status = if show {
                "Comments: shown"
            } else {
                "Comments: hidden"
            };
            update.set_status(status);
        }
        CommandParse::Parsed(Command::SetWhitespaceIgnored(false)) => {
            state.ignore_whitespace = false;
            state.reload_current_diff();
            update.set_status("Whitespace: shown");
        }
        CommandParse::Parsed(Command::SetWhitespaceIgnored(true)) => {
            state.ignore_whitespace = true;
            state.reload_current_diff();
            update.set_status("Whitespace: ignored");
        }
        CommandParse::Parsed(Command::ViewFile { .. }) => {
            update.set_status("Unsupported command from prompt");
        }
        CommandParse::Parsed(Command::Unknown { name }) => {
            if name == "set" || name.starts_with("set ") {
                update.set_status(
                    "Unknown option. Use: blame, noblame, comments, nocomments, whitespace, nowhitespace, mergebase, mb",
                );
            } else {
                update.set_status(format!("Unknown command: {name}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::update::output::StatusUpdate;
    use crate::review_types::ConnectionContext;

    fn test_state() -> AppState {
        AppState::new(
            crate::config::DiffAlgorithm::Myers,
            ConnectionContext {
                repo_root: "/repo".to_string(),
                worktree: "/repo".to_string(),
                base_ref: "main".to_string(),
                head_ref: "feature".to_string(),
                merge_base: "0123456789abcdef0123456789abcdef01234567".to_string(),
            },
            Vec::new(),
            40,
        )
    }

    #[test]
    fn set_mergebase_without_value_displays_current_merge_base() {
        let mut state = test_state();
        let mut output = AppOutput::default();

        apply_command(
            &mut state,
            &mut output,
            CommandParse::Parsed(Command::SetMergeBase(None)),
        );

        assert_eq!(
            output.status,
            Some(StatusUpdate::Set(
                "Merge base: 0123456789abcdef0123456789abcdef01234567".to_string()
            ))
        );
        assert_eq!(output.take_pending_command(), None);
    }

    #[test]
    fn set_mergebase_with_value_requests_pending_command() {
        let mut state = test_state();
        let mut output = AppOutput::default();

        apply_command(
            &mut state,
            &mut output,
            CommandParse::Parsed(Command::SetMergeBase(Some("HEAD".to_string()))),
        );

        assert_eq!(
            output.take_pending_command(),
            Some(Command::SetMergeBase(Some("HEAD".to_string())))
        );
        assert_eq!(output.status, None);
    }
}
