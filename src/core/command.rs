//! Command parsing and typed command requests shared by UI adapters.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Quit,
    SearchAll { pattern: String },
    SearchDiff { pattern: String },
    FindDefinition { symbol: String },
    ViewFile { path: String, line_number: u32 },
    SetBlame(bool),
    SetComments(bool),
    SetWhitespaceIgnored(bool),
    Unknown { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandParse {
    Empty,
    Parsed(Command),
    NeedsArgument { usage: String },
    NeedsWord { usage: String },
}

pub fn parse_command(input: &str, fallback_word: Option<&str>) -> CommandParse {
    let input = input.trim();
    if input.is_empty() {
        return CommandParse::Empty;
    }

    let (name, args) = match input.split_once(char::is_whitespace) {
        Some((name, args)) => (name, args.trim()),
        None => (input, ""),
    };

    match name {
        "q" | "quit" => CommandParse::Parsed(Command::Quit),
        "gr" => parse_required_arg(args, "Usage: :gr <regex>", |pattern| Command::SearchAll {
            pattern,
        }),
        "grd" => parse_required_arg(args, "Usage: :grd <regex>", |pattern| Command::SearchDiff {
            pattern,
        }),
        "gd" => {
            if !args.is_empty() {
                CommandParse::Parsed(Command::FindDefinition {
                    symbol: args.to_string(),
                })
            } else if let Some(word) = fallback_word.filter(|word| !word.is_empty()) {
                CommandParse::Parsed(Command::FindDefinition {
                    symbol: word.to_string(),
                })
            } else {
                CommandParse::NeedsWord {
                    usage: "Usage: :gd <symbol>".to_string(),
                }
            }
        }
        "set" => match args {
            "blame" => CommandParse::Parsed(Command::SetBlame(true)),
            "noblame" => CommandParse::Parsed(Command::SetBlame(false)),
            "comments" => CommandParse::Parsed(Command::SetComments(true)),
            "nocomments" => CommandParse::Parsed(Command::SetComments(false)),
            "whitespace" => CommandParse::Parsed(Command::SetWhitespaceIgnored(false)),
            "nowhitespace" => CommandParse::Parsed(Command::SetWhitespaceIgnored(true)),
            _ => CommandParse::Parsed(Command::Unknown {
                name: if args.is_empty() {
                    "set".to_string()
                } else {
                    format!("set {args}")
                },
            }),
        },
        _ => CommandParse::Parsed(Command::Unknown {
            name: name.to_string(),
        }),
    }
}

fn parse_required_arg(
    args: &str,
    usage: &str,
    build: impl FnOnce(String) -> Command,
) -> CommandParse {
    if args.is_empty() {
        CommandParse::NeedsArgument {
            usage: usage.to_string(),
        }
    } else {
        CommandParse::Parsed(build(args.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_commands() {
        assert_eq!(
            parse_command("gr needle", None),
            CommandParse::Parsed(Command::SearchAll {
                pattern: "needle".to_string()
            })
        );
        assert_eq!(
            parse_command("grd needle", None),
            CommandParse::Parsed(Command::SearchDiff {
                pattern: "needle".to_string()
            })
        );
    }

    #[test]
    fn reports_missing_search_argument() {
        assert_eq!(
            parse_command("gr", None),
            CommandParse::NeedsArgument {
                usage: "Usage: :gr <regex>".to_string()
            }
        );
    }

    #[test]
    fn parses_gd_with_argument_or_fallback_word() {
        assert_eq!(
            parse_command("gd thing", None),
            CommandParse::Parsed(Command::FindDefinition {
                symbol: "thing".to_string()
            })
        );
        assert_eq!(
            parse_command("gd", Some("cursor_word")),
            CommandParse::Parsed(Command::FindDefinition {
                symbol: "cursor_word".to_string()
            })
        );
    }

    #[test]
    fn reports_missing_gd_word() {
        assert_eq!(
            parse_command("gd", None),
            CommandParse::NeedsWord {
                usage: "Usage: :gd <symbol>".to_string()
            }
        );
    }

    #[test]
    fn parses_local_toggle_commands() {
        assert_eq!(
            parse_command("set noblame", None),
            CommandParse::Parsed(Command::SetBlame(false))
        );
        assert_eq!(
            parse_command("set comments", None),
            CommandParse::Parsed(Command::SetComments(true))
        );
        assert_eq!(
            parse_command("set nowhitespace", None),
            CommandParse::Parsed(Command::SetWhitespaceIgnored(true))
        );
    }
}
