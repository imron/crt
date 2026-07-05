//! TUI-side application of core effects.

use super::state::TuiState;
use crate::app::App;
use crate::core::{CommentsPanelEffect, CoreEffect, PromptKind};

pub fn apply_core_effects(
    app: &mut App,
    tui_state: &mut TuiState,
    effects: Vec<CoreEffect>,
) -> bool {
    let mut handled = false;
    let mut app_effects = Vec::new();

    for effect in effects {
        handled = true;
        match effect {
            CoreEffect::RequestPrompt(prompt) => match prompt.kind {
                PromptKind::CommandLine => {
                    tui_state.open_command_prompt(prompt.id, prompt.initial_value);
                }
                PromptKind::Search => {
                    tui_state.open_diff_search_prompt(prompt.id, prompt.initial_value);
                }
                PromptKind::Comment => {
                    tui_state.open_comment_prompt_with_title(
                        prompt.id,
                        prompt.title,
                        prompt.initial_value,
                    );
                }
                PromptKind::Custom(_) => {}
            },
            CoreEffect::ClearPrompt { id } => {
                if tui_state.active_core_prompt == Some(id) {
                    tui_state.clear_prompt();
                }
            }
            CoreEffect::ShowHelp => {
                tui_state.show_help = true;
            }
            CoreEffect::DismissHelp => {
                tui_state.show_help = false;
            }
            effect => app_effects.push(effect),
        }
    }

    if opens_comments_panel_for_navigation(app, &app_effects) {
        tui_state.project_comments_panel_open();
    }

    let app_output = app.apply_core_effects(tui_state, app_effects);
    let app_handled = app_output.handled;
    tui_state.apply_app_output(app_output);
    app_handled || handled
}

fn opens_comments_panel_for_navigation(app: &App, effects: &[CoreEffect]) -> bool {
    !app.state.show_comments_panel
        && effects.iter().any(|effect| {
            matches!(
                effect,
                CoreEffect::NavigateUnresolvedComment(_)
                    | CoreEffect::CommentsPanel(
                        CommentsPanelEffect::NavigateNextComment
                            | CommentsPanelEffect::NavigatePreviousComment
                    )
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::core::navigation::Direction;
    use crate::review_types::{
        AnchorAggregateStatus, AnchorMatchMethod, AnchorPlacementStatus, AnchorStatus, ChangeKind,
        Comment, CommentAnchor, CommentAnchorSegment, CommentAnchorSide, ConnectionContext,
        DiffContent, FileChange, FileEntry, ReviewStatus,
    };
    use ratatui::layout::Rect;

    #[test]
    fn comment_jump_uses_projected_diff_height_when_panel_opens() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut comment = stored_comment(7, "src/main.rs");
        move_comment_head_range(&mut comment, 16, 24);
        app.state.comments = vec![comment];
        app.state.head_content = Some(
            (1..=60)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.show_comments_panel = false;

        let mut tui_state = TuiState::default();
        tui_state.diff_area = Rect::new(0, 0, 100, 30);
        tui_state.diff_content_height = 60;
        tui_state.diff_view_height = 28;
        tui_state.diff_rendered_text = (1..=60).map(|n| n.to_string()).collect();

        apply_core_effects(
            &mut app,
            &mut tui_state,
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigateNextComment,
            )],
        );

        assert!(app.state.show_comments_panel);
        assert_eq!(app.state.selected_comment_id, Some(7));
        assert_eq!(app.state.diff_line_cursor, 15);
        assert_eq!(app.state.diff_scroll, 7);
    }

    #[test]
    fn unresolved_comment_jump_uses_projected_diff_height_when_panel_opens() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut comment = stored_comment(7, "src/main.rs");
        move_comment_head_range(&mut comment, 16, 24);
        app.state.comments = vec![comment];
        app.state.head_content = Some(
            (1..=60)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.show_comments_panel = false;

        let mut tui_state = TuiState::default();
        tui_state.diff_area = Rect::new(0, 0, 100, 30);
        tui_state.diff_content_height = 60;
        tui_state.diff_view_height = 28;
        tui_state.diff_rendered_text = (1..=60).map(|n| n.to_string()).collect();

        apply_core_effects(
            &mut app,
            &mut tui_state,
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );

        assert!(app.state.show_comments_panel);
        assert_eq!(app.state.selected_comment_id, Some(7));
        assert_eq!(app.state.diff_line_cursor, 15);
        assert_eq!(app.state.diff_scroll, 7);
    }

    fn test_context() -> ConnectionContext {
        ConnectionContext {
            repo_root: "/repo".to_string(),
            worktree: "/repo".to_string(),
            base_ref: "main".to_string(),
            head_ref: "feature".to_string(),
            merge_base: "abc123".to_string(),
        }
    }

    fn test_file(path: &str) -> FileEntry {
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
            },
        }
    }

    fn stored_comment(id: i64, file_path: &str) -> Comment {
        Comment::new(crate::review_types::CommentInit {
            id,
            merge_base: "abc123".to_string(),
            head_ref: "feature".to_string(),
            created_head_commit: "head-commit".to_string(),
            anchor: CommentAnchor {
                segments: vec![CommentAnchorSegment {
                    side: CommentAnchorSide::Head,
                    file_path: file_path.to_string(),
                    line_start: 2,
                    line_end: 2,
                    char_start: None,
                    char_end: None,
                    anchor_text: "anchor".to_string(),
                    context_before: String::new(),
                    context_after: String::new(),
                    placement_status: AnchorPlacementStatus::Anchored,
                    match_method: AnchorMatchMethod::ExactAtLine,
                }],
                aggregate_status: AnchorAggregateStatus::Anchored,
            },
            body: "comment".to_string(),
            resolved: false,
            created_at: "2026-06-28T00:00:00+10:00".to_string(),
            updated_at: "2026-06-28T00:00:00+10:00".to_string(),
            anchor_status: AnchorStatus::Anchored,
        })
        .expect("test comment anchor should be valid")
    }

    fn move_comment_head_range(comment: &mut Comment, line_start: i64, line_end: i64) {
        let file_path = comment.file_path().to_string();
        let anchor_text = comment.anchor_text().to_string();
        comment
            .replace_anchor(CommentAnchor {
                segments: vec![CommentAnchorSegment {
                    side: CommentAnchorSide::Head,
                    file_path,
                    line_start,
                    line_end,
                    char_start: None,
                    char_end: None,
                    anchor_text,
                    context_before: String::new(),
                    context_after: String::new(),
                    placement_status: AnchorPlacementStatus::Anchored,
                    match_method: AnchorMatchMethod::ExactAtLine,
                }],
                aggregate_status: AnchorAggregateStatus::Anchored,
            })
            .expect("test comment anchor should be valid");
    }
}
