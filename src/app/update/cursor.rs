use super::viewport::AppViewport;
use crate::app::AppState;
use crate::core::DiffCursorEffect;

pub(super) fn apply_diff_cursor_effect(
    state: &mut AppState,
    view: &impl AppViewport,
    effect: DiffCursorEffect,
) {
    match effect {
        DiffCursorEffect::MoveTo { line, column } => {
            state.diff_line_cursor = line;
            state.diff_col_cursor = column;
            clamp_cursor_and_scroll(state, view);
            clamp_col_cursor(state, view);
        }
        DiffCursorEffect::LineDown => {
            state.diff_line_cursor = state.diff_line_cursor.saturating_add(1);
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::LineUp => {
            state.diff_line_cursor = state.diff_line_cursor.saturating_sub(1);
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::PageDown => {
            let delta = view.diff_view_height();
            state.diff_line_cursor = state.diff_line_cursor.saturating_add(delta);
            state.diff_scroll = state.diff_scroll.saturating_add(delta);
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::PageUp => {
            let delta = view.diff_view_height();
            state.diff_line_cursor = state.diff_line_cursor.saturating_sub(delta);
            state.diff_scroll = state.diff_scroll.saturating_sub(delta);
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::HalfPageDown => {
            let delta = view.diff_view_height() / 2;
            state.diff_line_cursor = state.diff_line_cursor.saturating_add(delta);
            state.diff_scroll = state.diff_scroll.saturating_add(delta);
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::HalfPageUp => {
            let delta = view.diff_view_height() / 2;
            state.diff_line_cursor = state.diff_line_cursor.saturating_sub(delta);
            state.diff_scroll = state.diff_scroll.saturating_sub(delta);
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::ScrollDown => {
            state.diff_scroll = state.diff_scroll.saturating_add(1);
            clamp_diff_scroll(state, view);
            if state.diff_line_cursor < state.diff_scroll {
                state.diff_line_cursor = state.diff_scroll;
                state.diff_col_cursor = 0;
            }
        }
        DiffCursorEffect::ScrollUp => {
            state.diff_scroll = state.diff_scroll.saturating_sub(1);
            if view.diff_view_height() > 0
                && state.diff_line_cursor >= state.diff_scroll + view.diff_view_height()
            {
                state.diff_line_cursor = state.diff_scroll + view.diff_view_height() - 1;
                state.diff_col_cursor = 0;
            }
        }
        DiffCursorEffect::WheelDown => {
            state.diff_scroll = state.diff_scroll.saturating_add(3);
            clamp_diff_scroll(state, view);
            if state.diff_line_cursor < state.diff_scroll {
                state.diff_line_cursor = state.diff_scroll;
            }
        }
        DiffCursorEffect::WheelUp => {
            state.diff_scroll = state.diff_scroll.saturating_sub(3);
            let bottom = state
                .diff_scroll
                .saturating_add(view.diff_view_height().saturating_sub(1));
            if state.diff_line_cursor > bottom {
                state.diff_line_cursor = bottom;
            }
        }
        DiffCursorEffect::Top => {
            state.diff_line_cursor = 0;
            state.diff_scroll = 0;
            state.diff_col_cursor = 0;
        }
        DiffCursorEffect::Bottom => {
            state.diff_line_cursor = view.max_diff_scroll();
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::ViewTop => {
            state.diff_line_cursor = state.diff_scroll;
            state.diff_col_cursor = 0;
        }
        DiffCursorEffect::ViewMiddle => {
            let mid = view.diff_view_height() / 2;
            state.diff_line_cursor = state.diff_scroll + mid;
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::ViewBottom => {
            let bottom = view.diff_view_height().saturating_sub(1);
            state.diff_line_cursor = state.diff_scroll + bottom;
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::CharLeft => {
            state.diff_col_cursor = state.diff_col_cursor.saturating_sub(1);
        }
        DiffCursorEffect::CharRight => {
            let max = view.current_line_text_len(state).saturating_sub(1);
            if state.diff_col_cursor < max {
                state.diff_col_cursor += 1;
            }
        }
        DiffCursorEffect::LineStart => {
            state.diff_col_cursor = 0;
        }
        DiffCursorEffect::LineEnd => {
            state.diff_col_cursor = view.current_line_text_len(state).saturating_sub(1);
        }
        DiffCursorEffect::WordForward => {
            word_forward(state, view);
        }
        DiffCursorEffect::WordBackward => {
            word_backward(state, view);
        }
        DiffCursorEffect::BigWordForward => {
            bigword_forward(state, view);
        }
        DiffCursorEffect::BigWordBackward => {
            bigword_backward(state, view);
        }
    }
    state.mark_model_changed();
}

fn clamp_diff_scroll(state: &mut AppState, view: &impl AppViewport) {
    state.diff_scroll = state.diff_scroll.min(view.max_diff_scroll());
}

pub(super) fn clamp_cursor_and_scroll(state: &mut AppState, view: &impl AppViewport) {
    let max = view.max_diff_scroll();
    state.diff_line_cursor = state.diff_line_cursor.min(max);
    if state.diff_line_cursor < state.diff_scroll {
        state.diff_scroll = state.diff_line_cursor;
    }
    if view.diff_view_height() > 0
        && state.diff_line_cursor >= state.diff_scroll + view.diff_view_height()
    {
        state.diff_scroll = state
            .diff_line_cursor
            .saturating_sub(view.diff_view_height() - 1);
    }
    clamp_diff_scroll(state, view);
}

fn clamp_col_cursor(state: &mut AppState, view: &impl AppViewport) {
    let max = view.current_line_text_len(state).saturating_sub(1);
    state.diff_col_cursor = state.diff_col_cursor.min(max);
}

fn char_class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        0
    } else if c.is_whitespace() {
        1
    } else {
        2
    }
}

fn word_forward(state: &mut AppState, view: &impl AppViewport) {
    let content = view.line_content_trimmed(state.diff_line_cursor);
    let chars: Vec<char> = content.chars().collect();
    let text_len = chars.len();

    if text_len == 0 || state.diff_col_cursor >= text_len.saturating_sub(1) {
        let max_line = view.diff_content_height().saturating_sub(1);
        if state.diff_line_cursor < max_line {
            state.diff_line_cursor += 1;
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
            let new_content = view.line_content_trimmed(state.diff_line_cursor);
            let new_chars: Vec<char> = new_content.chars().collect();
            let mut pos = 0;
            while pos < new_chars.len() && new_chars[pos].is_whitespace() {
                pos += 1;
            }
            state.diff_col_cursor = pos.min(new_chars.len().saturating_sub(1));
        }
        return;
    }

    let mut pos = state.diff_col_cursor;
    let start_class = char_class(chars[pos]);
    while pos < text_len && char_class(chars[pos]) == start_class {
        pos += 1;
    }
    while pos < text_len && chars[pos].is_whitespace() {
        pos += 1;
    }
    if pos >= text_len {
        let max_line = view.diff_content_height().saturating_sub(1);
        if state.diff_line_cursor < max_line {
            state.diff_line_cursor += 1;
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
            let new_content = view.line_content_trimmed(state.diff_line_cursor);
            let new_chars: Vec<char> = new_content.chars().collect();
            let mut p = 0;
            while p < new_chars.len() && new_chars[p].is_whitespace() {
                p += 1;
            }
            state.diff_col_cursor = p.min(new_chars.len().saturating_sub(1));
        } else {
            state.diff_col_cursor = text_len.saturating_sub(1);
        }
        return;
    }
    state.diff_col_cursor = pos;
}

fn word_backward(state: &mut AppState, view: &impl AppViewport) {
    if state.diff_col_cursor == 0 {
        if state.diff_line_cursor > 0 {
            state.diff_line_cursor -= 1;
            clamp_cursor_and_scroll(state, view);
            let content = view.line_content_trimmed(state.diff_line_cursor);
            let chars: Vec<char> = content.chars().collect();
            if chars.is_empty() {
                state.diff_col_cursor = 0;
            } else {
                let end = chars.len() - 1;
                let mut pos = end;
                while pos > 0 && chars[pos].is_whitespace() {
                    pos -= 1;
                }
                let target_class = char_class(chars[pos]);
                while pos > 0 && char_class(chars[pos - 1]) == target_class {
                    pos -= 1;
                }
                state.diff_col_cursor = pos;
            }
        }
        return;
    }

    let content = view.line_content_trimmed(state.diff_line_cursor);
    let chars: Vec<char> = content.chars().collect();
    if chars.is_empty() {
        return;
    }
    let mut pos = state.diff_col_cursor;
    pos -= 1;
    while pos > 0 && chars[pos].is_whitespace() {
        pos -= 1;
    }
    let target_class = char_class(chars[pos]);
    while pos > 0 && char_class(chars[pos - 1]) == target_class {
        pos -= 1;
    }
    state.diff_col_cursor = pos;
}

fn bigword_forward(state: &mut AppState, view: &impl AppViewport) {
    let content = view.line_content_trimmed(state.diff_line_cursor);
    let chars: Vec<char> = content.chars().collect();
    let text_len = chars.len();

    if text_len == 0 || state.diff_col_cursor >= text_len.saturating_sub(1) {
        let max_line = view.diff_content_height().saturating_sub(1);
        if state.diff_line_cursor < max_line {
            state.diff_line_cursor += 1;
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
            let new_content = view.line_content_trimmed(state.diff_line_cursor);
            let new_chars: Vec<char> = new_content.chars().collect();
            let mut pos = 0;
            while pos < new_chars.len() && new_chars[pos].is_whitespace() {
                pos += 1;
            }
            state.diff_col_cursor = pos.min(new_chars.len().saturating_sub(1));
        }
        return;
    }

    let mut pos = state.diff_col_cursor;
    while pos < text_len && !chars[pos].is_whitespace() {
        pos += 1;
    }
    while pos < text_len && chars[pos].is_whitespace() {
        pos += 1;
    }
    if pos >= text_len {
        let max_line = view.diff_content_height().saturating_sub(1);
        if state.diff_line_cursor < max_line {
            state.diff_line_cursor += 1;
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
            let new_content = view.line_content_trimmed(state.diff_line_cursor);
            let new_chars: Vec<char> = new_content.chars().collect();
            let mut p = 0;
            while p < new_chars.len() && new_chars[p].is_whitespace() {
                p += 1;
            }
            state.diff_col_cursor = p.min(new_chars.len().saturating_sub(1));
        } else {
            state.diff_col_cursor = text_len.saturating_sub(1);
        }
        return;
    }
    state.diff_col_cursor = pos;
}

fn bigword_backward(state: &mut AppState, view: &impl AppViewport) {
    if state.diff_col_cursor == 0 {
        if state.diff_line_cursor > 0 {
            state.diff_line_cursor -= 1;
            clamp_cursor_and_scroll(state, view);
            let content = view.line_content_trimmed(state.diff_line_cursor);
            let chars: Vec<char> = content.chars().collect();
            if chars.is_empty() {
                state.diff_col_cursor = 0;
            } else {
                let mut pos = chars.len() - 1;
                while pos > 0 && chars[pos].is_whitespace() {
                    pos -= 1;
                }
                while pos > 0 && !chars[pos - 1].is_whitespace() {
                    pos -= 1;
                }
                state.diff_col_cursor = pos;
            }
        }
        return;
    }

    let content = view.line_content_trimmed(state.diff_line_cursor);
    let chars: Vec<char> = content.chars().collect();
    if chars.is_empty() {
        return;
    }
    let mut pos = state.diff_col_cursor - 1;
    while pos > 0 && chars[pos].is_whitespace() {
        pos -= 1;
    }
    while pos > 0 && !chars[pos - 1].is_whitespace() {
        pos -= 1;
    }
    state.diff_col_cursor = pos;
}
