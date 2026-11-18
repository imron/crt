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
            | Command::FindDefinition { .. }),
        ) => {
            update.request_command(cmd);
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
            update.set_comments_visible(show);
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
                    "Unknown option. Use: blame, noblame, comments, nocomments, whitespace, nowhitespace",
                );
            } else {
                update.set_status(format!("Unknown command: {name}"));
            }
        }
    }
}
