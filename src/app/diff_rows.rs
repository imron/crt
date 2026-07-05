use crate::review_types::{CommentAnchorSide, DiffHunk, DiffLine, LineKind};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinearDiffRows {
    pub rows: Vec<LinearDiffRow>,
    pub hunk_starts: Vec<usize>,
    pub hunk_ends: Vec<usize>,
    pub hunk_first_changes: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearDiffRow {
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    pub content: String,
    pub kind: LineKind,
    pub paired_content: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SideBySideDiffRows {
    pub rows: Vec<SideBySideRow>,
    pub hunk_starts: Vec<usize>,
    pub hunk_ends: Vec<usize>,
    pub hunk_first_changes: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideBySideRow {
    pub base: Option<SideBySideCell>,
    pub head: Option<SideBySideCell>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideBySideCell {
    pub side: CommentAnchorSide,
    pub line_number: u32,
    pub content: String,
    pub kind: LineKind,
}

pub fn inline_diff_rows(hunks: &[DiffHunk], head_content: Option<&str>) -> LinearDiffRows {
    let head_lines: Vec<&str> = head_content
        .map(|content| content.lines().collect())
        .unwrap_or_default();

    if hunks.is_empty() {
        return LinearDiffRows {
            rows: head_file_linear_rows(&head_lines),
            hunk_starts: Vec::new(),
            hunk_ends: Vec::new(),
            hunk_first_changes: Vec::new(),
        };
    }

    let mut rows = Vec::new();
    let mut hunk_starts = Vec::new();
    let mut hunk_ends = Vec::new();
    let mut hunk_first_changes = Vec::new();
    let mut old_cursor = 1;
    let mut new_cursor = 1;

    for hunk in hunks {
        while (old_cursor < hunk.old_start || new_cursor < hunk.new_start)
            && (new_cursor as usize) <= head_lines.len()
        {
            let old_lineno = (old_cursor < hunk.old_start).then_some(old_cursor);
            let new_lineno = (new_cursor < hunk.new_start).then_some(new_cursor);
            rows.push(linear_row(
                old_lineno,
                new_lineno,
                new_lineno
                    .and_then(|lineno| head_lines.get((lineno - 1) as usize).copied())
                    .unwrap_or(""),
                LineKind::Context,
                None,
            ));
            if old_lineno.is_some() {
                old_cursor = old_cursor.saturating_add(1);
            }
            if new_lineno.is_some() {
                new_cursor = new_cursor.saturating_add(1);
            }
        }

        hunk_starts.push(rows.len());
        hunk_first_changes.push(rows.len() + leading_context_len(hunk));
        push_inline_hunk_rows(&mut rows, &mut old_cursor, &mut new_cursor, hunk);
        hunk_ends.push(rows.len());
    }

    while (new_cursor as usize) <= head_lines.len() {
        rows.push(linear_row(
            Some(old_cursor),
            Some(new_cursor),
            head_lines
                .get((new_cursor - 1) as usize)
                .copied()
                .unwrap_or(""),
            LineKind::Context,
            None,
        ));
        old_cursor = old_cursor.saturating_add(1);
        new_cursor = new_cursor.saturating_add(1);
    }

    LinearDiffRows {
        rows,
        hunk_starts,
        hunk_ends,
        hunk_first_changes,
    }
}

pub fn full_file_head_rows(hunks: &[DiffHunk], head_content: Option<&str>) -> LinearDiffRows {
    let Some(content) = head_content else {
        return LinearDiffRows::default();
    };
    let file_lines: Vec<&str> = content.lines().collect();
    if file_lines.is_empty() {
        return LinearDiffRows::default();
    }

    let addition_lines: std::collections::HashSet<u32> = hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter(|line| line.kind == LineKind::Addition)
        .filter_map(|line| line.new_lineno)
        .collect();
    let hunk_ranges: Vec<(u32, u32)> = hunks
        .iter()
        .map(|hunk| (hunk.new_start, hunk.new_start + hunk.new_lines))
        .collect();

    full_file_rows_for_side(
        &file_lines,
        &addition_lines,
        &hunk_ranges,
        CommentAnchorSide::Head,
    )
}

pub fn full_file_base_rows(hunks: &[DiffHunk], base_content: Option<&str>) -> LinearDiffRows {
    let Some(content) = base_content else {
        return LinearDiffRows::default();
    };
    let file_lines: Vec<&str> = content.lines().collect();
    if file_lines.is_empty() {
        return LinearDiffRows::default();
    }

    let deletion_lines: std::collections::HashSet<u32> = hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter(|line| line.kind == LineKind::Deletion)
        .filter_map(|line| line.old_lineno)
        .collect();
    let hunk_ranges: Vec<(u32, u32)> = hunks
        .iter()
        .map(|hunk| (hunk.old_start, hunk.old_start + hunk.old_lines))
        .collect();

    full_file_rows_for_side(
        &file_lines,
        &deletion_lines,
        &hunk_ranges,
        CommentAnchorSide::Base,
    )
}

pub fn side_by_side_diff_rows(
    hunks: &[DiffHunk],
    base_content: Option<&str>,
    head_content: Option<&str>,
) -> SideBySideDiffRows {
    let base_lines: Vec<&str> = base_content
        .map(|content| content.lines().collect())
        .unwrap_or_default();
    let head_lines: Vec<&str> = head_content
        .map(|content| content.lines().collect())
        .unwrap_or_default();

    if hunks.is_empty() {
        return SideBySideDiffRows {
            rows: head_file_rows(&head_lines),
            hunk_starts: Vec::new(),
            hunk_ends: Vec::new(),
            hunk_first_changes: Vec::new(),
        };
    }

    let mut rows = Vec::new();
    let mut hunk_starts = Vec::new();
    let mut hunk_ends = Vec::new();
    let mut hunk_first_changes = Vec::new();
    let mut old_cursor = 1;
    let mut new_cursor = 1;

    for hunk in hunks {
        push_gap_rows(
            &mut rows,
            &mut old_cursor,
            &mut new_cursor,
            hunk.old_start,
            hunk.new_start,
            &base_lines,
            &head_lines,
        );

        hunk_starts.push(rows.len());
        hunk_first_changes.push(rows.len() + leading_context_len(hunk));
        push_hunk_rows(&mut rows, &mut old_cursor, &mut new_cursor, hunk);
        hunk_ends.push(rows.len());
    }

    push_remaining_rows(
        &mut rows,
        &mut old_cursor,
        &mut new_cursor,
        &base_lines,
        &head_lines,
    );

    SideBySideDiffRows {
        rows,
        hunk_starts,
        hunk_ends,
        hunk_first_changes,
    }
}

fn head_file_linear_rows(head_lines: &[&str]) -> Vec<LinearDiffRow> {
    head_lines
        .iter()
        .enumerate()
        .map(|(idx, content)| {
            linear_row(
                Some((idx + 1) as u32),
                Some((idx + 1) as u32),
                content,
                LineKind::Context,
                None,
            )
        })
        .collect()
}

fn push_inline_hunk_rows(
    rows: &mut Vec<LinearDiffRow>,
    old_cursor: &mut u32,
    new_cursor: &mut u32,
    hunk: &DiffHunk,
) {
    let mut li = 0;
    while li < hunk.lines.len() {
        let line = &hunk.lines[li];
        if line.kind == LineKind::Context {
            rows.push(linear_row(
                line.old_lineno,
                line.new_lineno,
                line.content.trim_end_matches('\n'),
                LineKind::Context,
                None,
            ));
            *old_cursor = old_cursor.saturating_add(1);
            *new_cursor = new_cursor.saturating_add(1);
            li += 1;
            continue;
        }

        let (del_end, add_end) = paired_change_bounds(&hunk.lines, li);
        let deletions = &hunk.lines[li..del_end];
        let additions = &hunk.lines[del_end..add_end];
        let pair_count = deletions.len().min(additions.len());

        for (idx, line) in deletions.iter().enumerate() {
            rows.push(linear_row(
                line.old_lineno,
                line.new_lineno,
                line.content.trim_end_matches('\n'),
                LineKind::Deletion,
                additions
                    .get(idx)
                    .filter(|_| idx < pair_count)
                    .map(|paired| paired.content.trim_end_matches('\n').to_string()),
            ));
            *old_cursor = old_cursor.saturating_add(1);
        }

        for (idx, line) in additions.iter().enumerate() {
            rows.push(linear_row(
                line.old_lineno,
                line.new_lineno,
                line.content.trim_end_matches('\n'),
                LineKind::Addition,
                deletions
                    .get(idx)
                    .filter(|_| idx < pair_count)
                    .map(|paired| paired.content.trim_end_matches('\n').to_string()),
            ));
            *new_cursor = new_cursor.saturating_add(1);
        }

        li = add_end;
    }
}

fn full_file_rows_for_side(
    file_lines: &[&str],
    changed_lines: &std::collections::HashSet<u32>,
    hunk_ranges: &[(u32, u32)],
    side: CommentAnchorSide,
) -> LinearDiffRows {
    let mut rows = Vec::new();
    let mut hunk_starts = Vec::new();
    let mut hunk_ends = Vec::new();
    let mut hunk_first_changes = Vec::new();
    let mut current_hunk_first_change_recorded = false;

    for (idx, content) in file_lines.iter().enumerate() {
        let lineno = (idx + 1) as u32;
        for &(start, end) in hunk_ranges {
            if lineno == start {
                hunk_starts.push(rows.len());
                current_hunk_first_change_recorded = false;
            }
            if lineno == end {
                hunk_ends.push(rows.len());
            }
        }

        let changed = changed_lines.contains(&lineno);
        if changed && !current_hunk_first_change_recorded {
            hunk_first_changes.push(rows.len());
            current_hunk_first_change_recorded = true;
        }
        let kind = match (side, changed) {
            (CommentAnchorSide::Head, true) => LineKind::Addition,
            (CommentAnchorSide::Base, true) => LineKind::Deletion,
            _ => LineKind::Context,
        };
        rows.push(match side {
            CommentAnchorSide::Base => linear_row(Some(lineno), None, content, kind, None),
            CommentAnchorSide::Head => linear_row(None, Some(lineno), content, kind, None),
        });
    }

    while hunk_ends.len() < hunk_starts.len() {
        hunk_ends.push(rows.len());
    }
    while hunk_first_changes.len() < hunk_starts.len() {
        hunk_first_changes.push(*hunk_starts.last().unwrap_or(&0));
    }

    LinearDiffRows {
        rows,
        hunk_starts,
        hunk_ends,
        hunk_first_changes,
    }
}

fn linear_row(
    old_lineno: Option<u32>,
    new_lineno: Option<u32>,
    content: &str,
    kind: LineKind,
    paired_content: Option<String>,
) -> LinearDiffRow {
    LinearDiffRow {
        old_lineno,
        new_lineno,
        content: content.to_string(),
        kind,
        paired_content,
    }
}

fn head_file_rows(head_lines: &[&str]) -> Vec<SideBySideRow> {
    head_lines
        .iter()
        .enumerate()
        .map(|(idx, content)| SideBySideRow {
            base: None,
            head: Some(cell(
                CommentAnchorSide::Head,
                (idx + 1) as u32,
                content,
                LineKind::Context,
            )),
        })
        .collect()
}

fn leading_context_len(hunk: &DiffHunk) -> usize {
    hunk.lines
        .iter()
        .take_while(|line| line.kind == LineKind::Context)
        .count()
}

fn push_gap_rows(
    rows: &mut Vec<SideBySideRow>,
    old_cursor: &mut u32,
    new_cursor: &mut u32,
    hunk_old_start: u32,
    hunk_new_start: u32,
    base_lines: &[&str],
    head_lines: &[&str],
) {
    while *old_cursor < hunk_old_start || *new_cursor < hunk_new_start {
        push_context_row(
            rows,
            (*old_cursor < hunk_old_start).then_some(*old_cursor),
            (*new_cursor < hunk_new_start).then_some(*new_cursor),
            base_lines,
            head_lines,
        );
        if *old_cursor < hunk_old_start {
            *old_cursor = (*old_cursor).saturating_add(1);
        }
        if *new_cursor < hunk_new_start {
            *new_cursor = (*new_cursor).saturating_add(1);
        }
    }
}

fn push_remaining_rows(
    rows: &mut Vec<SideBySideRow>,
    old_cursor: &mut u32,
    new_cursor: &mut u32,
    base_lines: &[&str],
    head_lines: &[&str],
) {
    while (*old_cursor as usize) <= base_lines.len() || (*new_cursor as usize) <= head_lines.len() {
        let old_lineno = ((*old_cursor as usize) <= base_lines.len()).then_some(*old_cursor);
        let new_lineno = ((*new_cursor as usize) <= head_lines.len()).then_some(*new_cursor);
        push_context_row(rows, old_lineno, new_lineno, base_lines, head_lines);
        if old_lineno.is_some() {
            *old_cursor = (*old_cursor).saturating_add(1);
        }
        if new_lineno.is_some() {
            *new_cursor = (*new_cursor).saturating_add(1);
        }
    }
}

fn push_context_row(
    rows: &mut Vec<SideBySideRow>,
    old_cursor: Option<u32>,
    new_cursor: Option<u32>,
    base_lines: &[&str],
    head_lines: &[&str],
) {
    let row = context_row(old_cursor, new_cursor, base_lines, head_lines);
    if row.base.is_some() || row.head.is_some() {
        rows.push(row);
    }
}

fn context_row(
    old_cursor: Option<u32>,
    new_cursor: Option<u32>,
    base_lines: &[&str],
    head_lines: &[&str],
) -> SideBySideRow {
    SideBySideRow {
        base: old_cursor.and_then(|lineno| {
            source_line(base_lines, lineno)
                .map(|content| cell(CommentAnchorSide::Base, lineno, content, LineKind::Context))
        }),
        head: new_cursor.and_then(|lineno| {
            source_line(head_lines, lineno)
                .map(|content| cell(CommentAnchorSide::Head, lineno, content, LineKind::Context))
        }),
    }
}

fn source_line<'a>(lines: &'a [&str], lineno: u32) -> Option<&'a str> {
    let index = lineno.checked_sub(1)? as usize;
    lines.get(index).copied()
}

fn push_hunk_rows(
    rows: &mut Vec<SideBySideRow>,
    old_cursor: &mut u32,
    new_cursor: &mut u32,
    hunk: &DiffHunk,
) {
    let mut li = 0;
    let hunk_lines = &hunk.lines;
    while li < hunk_lines.len() {
        let line = &hunk_lines[li];
        if line.kind == LineKind::Context {
            rows.push(hunk_context_row(line, *old_cursor, *new_cursor));
            *old_cursor = (*old_cursor).saturating_add(1);
            *new_cursor = (*new_cursor).saturating_add(1);
            li += 1;
            continue;
        }

        let (del_end, add_end) = paired_change_bounds(hunk_lines, li);
        push_paired_change_rows(
            rows,
            old_cursor,
            new_cursor,
            &hunk_lines[li..del_end],
            &hunk_lines[del_end..add_end],
        );
        li = add_end;
    }
}

fn hunk_context_row(line: &DiffLine, old_cursor: u32, new_cursor: u32) -> SideBySideRow {
    let content = line.content.trim_end_matches('\n');
    SideBySideRow {
        base: Some(cell(
            CommentAnchorSide::Base,
            line.old_lineno.unwrap_or(old_cursor),
            content,
            LineKind::Context,
        )),
        head: Some(cell(
            CommentAnchorSide::Head,
            line.new_lineno.unwrap_or(new_cursor),
            content,
            LineKind::Context,
        )),
    }
}

fn paired_change_bounds(lines: &[DiffLine], start: usize) -> (usize, usize) {
    let mut del_end = start;
    while del_end < lines.len() && lines[del_end].kind == LineKind::Deletion {
        del_end += 1;
    }
    let mut add_end = del_end;
    while add_end < lines.len() && lines[add_end].kind == LineKind::Addition {
        add_end += 1;
    }
    (del_end, add_end)
}

fn push_paired_change_rows(
    rows: &mut Vec<SideBySideRow>,
    old_cursor: &mut u32,
    new_cursor: &mut u32,
    deletions: &[DiffLine],
    additions: &[DiffLine],
) {
    for idx in 0..deletions.len().max(additions.len()) {
        rows.push(paired_change_row(
            deletions.get(idx),
            additions.get(idx),
            *old_cursor,
            *new_cursor,
        ));
        if deletions.get(idx).is_some() {
            *old_cursor = (*old_cursor).saturating_add(1);
        }
        if additions.get(idx).is_some() {
            *new_cursor = (*new_cursor).saturating_add(1);
        }
    }
}

fn paired_change_row(
    deletion: Option<&DiffLine>,
    addition: Option<&DiffLine>,
    old_cursor: u32,
    new_cursor: u32,
) -> SideBySideRow {
    SideBySideRow {
        base: deletion.map(|line| {
            cell(
                CommentAnchorSide::Base,
                line.old_lineno.unwrap_or(old_cursor),
                line.content.trim_end_matches('\n'),
                LineKind::Deletion,
            )
        }),
        head: addition.map(|line| {
            cell(
                CommentAnchorSide::Head,
                line.new_lineno.unwrap_or(new_cursor),
                line.content.trim_end_matches('\n'),
                LineKind::Addition,
            )
        }),
    }
}

fn cell(
    side: CommentAnchorSide,
    line_number: u32,
    content: &str,
    kind: LineKind,
) -> SideBySideCell {
    SideBySideCell {
        side,
        line_number,
        content: content.to_string(),
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff_line(
        kind: LineKind,
        content: &str,
        old_lineno: Option<u32>,
        new_lineno: Option<u32>,
    ) -> DiffLine {
        DiffLine {
            kind,
            content: content.to_string(),
            old_lineno,
            new_lineno,
        }
    }

    #[test]
    fn side_by_side_rows_match_render_rows_for_offset_replacement() {
        let hunk = DiffHunk {
            old_start: 26,
            old_lines: 1,
            new_start: 27,
            new_lines: 2,
            header: "@@ -26 +27,2 @@".to_string(),
            lines: vec![
                diff_line(LineKind::Deletion, "old line", Some(26), None),
                diff_line(LineKind::Addition, "new line one", None, Some(27)),
                diff_line(LineKind::Addition, "new line two", None, Some(28)),
            ],
        };
        let base_owned: Vec<String> = (1..=32).map(|n| format!("base {n}")).collect();
        let head_owned: Vec<String> = (1..=34).map(|n| format!("head {n}")).collect();
        let base_content = format!("{}\n", base_owned.join("\n"));
        let head_content = format!("{}\n", head_owned.join("\n"));

        let layout = side_by_side_diff_rows(&[hunk], Some(&base_content), Some(&head_content));

        assert_eq!(layout.rows.len(), 34);
        assert_eq!(layout.hunk_starts, vec![26]);
        assert_eq!(layout.hunk_first_changes, vec![26]);
        assert_eq!(layout.hunk_ends, vec![28]);

        let replacement_start = &layout.rows[26];
        let base = replacement_start.base.as_ref().expect("base cell");
        let head = replacement_start.head.as_ref().expect("head cell");
        assert_eq!(base.line_number, 26);
        assert_eq!(base.content, "old line");
        assert_eq!(head.line_number, 27);
        assert_eq!(head.content, "new line one");

        let replacement_tail = &layout.rows[27];
        assert!(replacement_tail.base.is_none());
        let head = replacement_tail.head.as_ref().expect("head cell");
        assert_eq!(head.line_number, 28);
        assert_eq!(head.content, "new line two");
    }

    #[test]
    fn side_by_side_rows_render_shared_tail_after_offset_hunk() {
        let mut hunk_lines = vec![
            diff_line(LineKind::Deletion, "old changed", Some(18), None),
            diff_line(LineKind::Addition, "new changed", None, Some(20)),
        ];
        for offset in 0..8 {
            let old_lineno = 19 + offset;
            let new_lineno = 21 + offset;
            hunk_lines.push(diff_line(
                LineKind::Context,
                &format!("context {old_lineno}/{new_lineno}"),
                Some(old_lineno),
                Some(new_lineno),
            ));
        }
        hunk_lines.push(diff_line(
            LineKind::Addition,
            "new trailing addition",
            None,
            Some(29),
        ));
        let hunk = DiffHunk {
            old_start: 18,
            old_lines: 9,
            new_start: 20,
            new_lines: 10,
            header: "@@ -18,9 +20,10 @@".to_string(),
            lines: hunk_lines,
        };
        let mut base_lines: Vec<String> = (1..=90).map(|n| format!("base {n}")).collect();
        let mut head_lines: Vec<String> = (1..=93).map(|n| format!("head {n}")).collect();
        for base_lineno in 27..=90 {
            let head_lineno = base_lineno + 3;
            let content = format!("shared tail {base_lineno}/{head_lineno}");
            base_lines[(base_lineno - 1) as usize] = content.clone();
            head_lines[(head_lineno - 1) as usize] = content;
        }
        let base_content = format!("{}\n", base_lines.join("\n"));
        let head_content = format!("{}\n", head_lines.join("\n"));

        let layout = side_by_side_diff_rows(&[hunk], Some(&base_content), Some(&head_content));

        let first_tail = layout
            .rows
            .iter()
            .find(|row| {
                row.base.as_ref().map(|cell| cell.line_number) == Some(27)
                    && row.head.as_ref().map(|cell| cell.line_number) == Some(30)
            })
            .expect("base 27 should be paired with head 30");
        assert_eq!(
            first_tail.base.as_ref().map(|cell| cell.content.as_str()),
            Some("shared tail 27/30")
        );
        assert_eq!(
            first_tail.head.as_ref().map(|cell| cell.content.as_str()),
            Some("shared tail 27/30")
        );

        let last_tail = layout
            .rows
            .iter()
            .find(|row| {
                row.base.as_ref().map(|cell| cell.line_number) == Some(90)
                    && row.head.as_ref().map(|cell| cell.line_number) == Some(93)
            })
            .expect("base 90 should be paired with head 93");
        assert_eq!(
            last_tail.base.as_ref().map(|cell| cell.content.as_str()),
            Some("shared tail 90/93")
        );
        assert_eq!(
            last_tail.head.as_ref().map(|cell| cell.content.as_str()),
            Some("shared tail 90/93")
        );
    }
}
