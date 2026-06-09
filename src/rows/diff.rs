use std::ops::Range;
use std::str::Lines;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::model::{DiffFile, DiffHunk, DiffLine, DiffLineKind, SourceSnapshot};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;

use super::RAIL_MARKER;
use super::file_summary::{
    file_header_label, file_status_suffix, format_file_stats, padding_before_stats,
    push_stat_spans, stage_display, stats_width,
};
use super::intraline::{
    emphasize_spans, intraline_block_end, intraline_ranges_for_block, is_intraline_candidate,
};
use super::text::{
    color_style, display_width, expand_tabs, muted_line, wrap_line, wrap_styled_spans,
};

const DIFF_GUTTER_WIDTH: usize = 11;

pub(crate) struct RenderedRows {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) complete: bool,
}

impl RenderedRows {
    fn complete(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            complete: true,
        }
    }

    fn partial(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            complete: false,
        }
    }
}

struct DiffSyntaxHighlighter<'a> {
    highlighter: SyntaxHighlighter,
    source_lines: Option<Lines<'a>>,
    next_line: u32,
}

pub(crate) fn diff_lines_until(
    file: &DiffFile,
    content_width: usize,
    theme: Theme,
    can_stage: bool,
    target_rows: usize,
) -> RenderedRows {
    let mut lines = wrap_line(
        render_file_header(file, content_width, can_stage, theme),
        content_width,
    );

    if file.binary {
        lines.extend(wrap_line(
            muted_line("Binary file changed", theme),
            content_width,
        ));
        return RenderedRows::complete(lines);
    }

    if file.hunks.is_empty() {
        lines.extend(wrap_line(
            muted_line("File changed without textual hunks", theme),
            content_width,
        ));
        return RenderedRows::complete(lines);
    }

    let mut old_highlighter =
        DiffSyntaxHighlighter::new(diff_old_path(file), &file.old_source, theme);
    let mut new_highlighter =
        DiffSyntaxHighlighter::new(diff_new_path(file), &file.new_source, theme);

    for hunk in &file.hunks {
        if !push_hunk_lines_until(
            &mut lines,
            hunk,
            &mut old_highlighter,
            &mut new_highlighter,
            content_width,
            theme,
            target_rows,
        ) {
            return RenderedRows::partial(lines);
        }
    }

    RenderedRows::complete(lines)
}

pub(crate) fn hunk_offsets(
    file: &DiffFile,
    content_width: usize,
    theme: Theme,
    can_stage: bool,
) -> Vec<usize> {
    if file.binary || file.hunks.is_empty() {
        return Vec::new();
    }

    let mut offset = wrap_line(
        render_file_header(file, content_width, can_stage, theme),
        content_width,
    )
    .len();
    let mut offsets = Vec::with_capacity(file.hunks.len());

    for hunk in &file.hunks {
        offsets.push(offset);
        offset += hunk_row_count(hunk, content_width, theme);
    }

    offsets
}

fn push_hunk_lines_until(
    lines: &mut Vec<Line<'static>>,
    hunk: &DiffHunk,
    old_highlighter: &mut DiffSyntaxHighlighter<'_>,
    new_highlighter: &mut DiffSyntaxHighlighter<'_>,
    content_width: usize,
    theme: Theme,
    target_rows: usize,
) -> bool {
    old_highlighter.advance_to(hunk.old_start);
    new_highlighter.advance_to(hunk.new_start);

    lines.extend(wrap_line(
        hunk_header_line(&hunk.header, theme),
        content_width,
    ));
    if lines.len() >= target_rows {
        return false;
    }

    let mut push_diff_line = |line: &DiffLine, intraline_ranges: &[Range<usize>]| {
        lines.extend(diff_line(
            line,
            intraline_ranges,
            old_highlighter,
            new_highlighter,
            content_width,
            theme,
        ));
        lines.len() < target_rows
    };

    let mut line_index = 0;
    while line_index < hunk.lines.len() {
        let line = &hunk.lines[line_index];
        if !is_intraline_candidate(line.kind) {
            if !push_diff_line(line, &[]) {
                return false;
            }
            line_index += 1;
            continue;
        }

        let block_end = intraline_block_end(&hunk.lines, line_index);
        let block = &hunk.lines[line_index..block_end];
        let intraline_ranges = intraline_ranges_for_block(block);
        for (line, ranges) in block.iter().zip(&intraline_ranges) {
            if !push_diff_line(line, ranges) {
                return false;
            }
        }
        line_index = block_end;
    }

    true
}

fn diff_old_path(file: &DiffFile) -> &str {
    path_or_display(&file.old_path, file)
}

fn diff_new_path(file: &DiffFile) -> &str {
    path_or_display(&file.path, file)
}

fn path_or_display<'a>(path: &'a str, file: &'a DiffFile) -> &'a str {
    if path.is_empty() {
        file.display_path()
    } else {
        path
    }
}

fn hunk_row_count(hunk: &DiffHunk, content_width: usize, theme: Theme) -> usize {
    wrap_line(hunk_header_line(&hunk.header, theme), content_width).len()
        + hunk
            .lines
            .iter()
            .map(|line| diff_line_row_count(line, content_width))
            .sum::<usize>()
}

fn diff_line_row_count(line: &DiffLine, content_width: usize) -> usize {
    wrap_styled_spans(
        vec![Span::raw(expand_tabs(&line.content))],
        diff_content_width(content_width),
    )
    .len()
}

fn hunk_header_line(header: &str, theme: Theme) -> Line<'static> {
    Line::styled(
        format!(" {header}"),
        color_style(theme.muted, theme.background_alt).add_modifier(Modifier::BOLD),
    )
}

fn diff_line(
    line: &DiffLine,
    intraline_ranges: &[Range<usize>],
    old_highlighter: &mut DiffSyntaxHighlighter<'_>,
    new_highlighter: &mut DiffSyntaxHighlighter<'_>,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let style = diff_line_style(line.kind, theme);
    let number_style = color_style(theme.line_number_fg, theme.line_number_bg);
    let content_spans = highlight_diff_content(
        line.kind,
        &line.content,
        style.content,
        intraline_ranges,
        old_highlighter,
        new_highlighter,
    );

    wrap_diff_line(
        line.old_line,
        line.new_line,
        content_spans,
        content_width,
        style,
        number_style,
    )
}

fn wrap_diff_line(
    old_line: Option<u32>,
    new_line: Option<u32>,
    content_spans: Vec<Span<'static>>,
    content_width: usize,
    style: DiffLineStyle,
    number_style: Style,
) -> Vec<Line<'static>> {
    let content_rows = wrap_styled_spans(content_spans, diff_content_width(content_width));
    content_rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let mut spans = if index == 0 {
                diff_gutter_spans(old_line, new_line, style, number_style)
            } else {
                continuation_gutter_spans(style, number_style)
            };
            spans.extend(row);
            Line::from(spans)
        })
        .collect()
}

fn diff_gutter_spans(
    old_line: Option<u32>,
    new_line: Option<u32>,
    style: DiffLineStyle,
    number_style: Style,
) -> Vec<Span<'static>> {
    vec![
        Span::styled(RAIL_MARKER, style.rail),
        Span::styled(format_line_number(old_line), number_style),
        Span::styled(" ", number_style),
        Span::styled(format_line_number(new_line), number_style),
        Span::styled(" ", number_style),
        Span::styled(style.marker, style.content),
        Span::styled(" ", style.content),
    ]
}

fn continuation_gutter_spans(style: DiffLineStyle, number_style: Style) -> Vec<Span<'static>> {
    vec![
        Span::styled(RAIL_MARKER, style.rail),
        Span::styled("   ", number_style),
        Span::styled(" ", number_style),
        Span::styled("   ", number_style),
        Span::styled(" ", number_style),
        Span::styled(" ", style.content),
        Span::styled(" ", style.content),
    ]
}

fn highlight_diff_content(
    kind: DiffLineKind,
    content: &str,
    content_style: Style,
    intraline_ranges: &[Range<usize>],
    old_highlighter: &mut DiffSyntaxHighlighter<'_>,
    new_highlighter: &mut DiffSyntaxHighlighter<'_>,
) -> Vec<Span<'static>> {
    match kind {
        DiffLineKind::Added => emphasize_spans(
            new_highlighter.highlight_line(content, content_style),
            intraline_ranges,
        ),
        DiffLineKind::Removed => emphasize_spans(
            old_highlighter.highlight_line(content, content_style),
            intraline_ranges,
        ),
        DiffLineKind::Context => {
            highlight_context_content(content, content_style, old_highlighter, new_highlighter)
        }
        DiffLineKind::Meta => vec![Span::styled(expand_tabs(content), content_style)],
    }
}

fn highlight_context_content(
    content: &str,
    content_style: Style,
    old_highlighter: &mut DiffSyntaxHighlighter<'_>,
    new_highlighter: &mut DiffSyntaxHighlighter<'_>,
) -> Vec<Span<'static>> {
    if new_highlighter.is_enabled() {
        let spans = new_highlighter.highlight_line(content, content_style);
        old_highlighter.advance_line(content);
        return spans;
    }

    let spans = old_highlighter.highlight_line(content, content_style);
    new_highlighter.advance_line(content);
    spans
}

impl<'a> DiffSyntaxHighlighter<'a> {
    fn new(path: &str, source: &'a SourceSnapshot, theme: Theme) -> Self {
        let highlighter = match source {
            SourceSnapshot::Unavailable => SyntaxHighlighter::disabled(),
            SourceSnapshot::Loaded(_) | SourceSnapshot::Unloaded => {
                SyntaxHighlighter::for_path(path, theme.syntax)
            }
        };

        Self {
            highlighter,
            source_lines: source.as_str().map(str::lines),
            next_line: 1,
        }
    }

    fn is_enabled(&self) -> bool {
        self.highlighter.is_enabled()
    }

    fn advance_to(&mut self, target_line: u32) {
        while self.next_line < target_line && self.advance_source_line() {}

        if self.next_line < target_line {
            self.next_line = target_line;
        }
    }

    fn highlight_line(&mut self, content: &str, base_style: Style) -> Vec<Span<'static>> {
        let expanded_content = expand_tabs(content);
        let spans = self
            .highlighter
            .highlight_line(&expanded_content, base_style);
        self.advance_position();
        spans
    }

    fn advance_line(&mut self, content: &str) {
        self.advance_highlighter_line(content);
        self.advance_position();
    }

    fn advance_source_line(&mut self) -> bool {
        let Some(content) = self.take_source_line() else {
            return false;
        };

        self.advance_highlighter_line(content);
        self.next_line += 1;
        true
    }

    fn advance_highlighter_line(&mut self, content: &str) {
        let expanded_content = expand_tabs(content);
        self.highlighter.advance_line(&expanded_content);
    }

    fn advance_position(&mut self) {
        let _ = self.take_source_line();
        self.next_line += 1;
    }

    fn take_source_line(&mut self) -> Option<&'a str> {
        self.source_lines.as_mut().and_then(Iterator::next)
    }
}

#[derive(Debug, Clone, Copy)]
struct DiffLineStyle {
    marker: &'static str,
    content: Style,
    rail: Style,
}

fn diff_line_style(kind: DiffLineKind, theme: Theme) -> DiffLineStyle {
    let (marker, text_color, background, rail_color) = match kind {
        DiffLineKind::Context => (" ", theme.text, theme.context_bg, theme.line_number_fg),
        DiffLineKind::Added => ("+", theme.added, theme.added_bg, theme.added),
        DiffLineKind::Removed => ("-", theme.removed, theme.removed_bg, theme.removed),
        DiffLineKind::Meta => (" ", theme.muted, theme.context_bg, theme.line_number_fg),
    };

    DiffLineStyle {
        marker,
        content: color_style(text_color, background),
        rail: color_style(rail_color, background),
    }
}

fn diff_content_width(content_width: usize) -> usize {
    content_width.saturating_sub(DIFF_GUTTER_WIDTH).max(1)
}

fn format_line_number(line: Option<u32>) -> String {
    line.map_or_else(|| "   ".to_string(), |line| format!("{line:<3}"))
}

fn render_file_header(
    file: &DiffFile,
    content_width: usize,
    can_stage: bool,
    theme: Theme,
) -> Line<'static> {
    let label = file_header_label(file);
    let suffix = file_status_suffix(file.status);
    let stage = can_stage.then(|| stage_display(file.stage, theme.background, theme));
    let stats = format_file_stats(file);
    let stats_width = stats_width(&stats);
    let stage_width = stage
        .as_ref()
        .map_or(0, |stage| display_width(stage.suffix));
    let used_width = display_width(&label) + display_width(suffix) + stage_width + stats_width;
    let padding = padding_before_stats(content_width, used_width, stats_width);
    let style = color_style(theme.text, theme.background);
    let muted_style = color_style(theme.muted, theme.background);

    let mut spans = vec![
        Span::styled(label, style),
        Span::styled(suffix.to_string(), muted_style),
    ];
    if let Some(stage) = stage {
        spans.push(Span::styled(stage.suffix.to_string(), stage.style));
    }
    spans.push(Span::styled(padding, style));
    push_stat_spans(&mut spans, file, theme.background, theme);
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span};

    use crate::model::{DiffHunk, FileStage, FileStatus};

    use super::*;

    #[test]
    fn wraps_added_removed_context_and_meta_content() {
        for kind in [
            DiffLineKind::Added,
            DiffLineKind::Removed,
            DiffLineKind::Context,
            DiffLineKind::Meta,
        ] {
            let file =
                diff_file_with_line(kind, "alpha beta gamma delta epsilon zeta eta theta iota");
            let content_width = DIFF_GUTTER_WIDTH + 12;
            let lines =
                diff_lines_until(&file, content_width, Theme::github_dark(), true, usize::MAX)
                    .lines;
            let diff_rows: Vec<&Line<'_>> = lines
                .iter()
                .filter(|line| line_text(line).starts_with(RAIL_MARKER))
                .collect();

            assert!(diff_rows.len() > 1, "{kind:?} did not wrap");
            assert!(
                diff_rows.iter().all(|line| line.width() <= content_width),
                "{kind:?} rendered wider than the diff pane"
            );
        }
    }

    #[test]
    fn continuation_rows_align_under_diff_content() {
        let file = diff_file_with_line(
            DiffLineKind::Added,
            "alpha beta gamma delta epsilon zeta eta theta iota",
        );
        let content_width = DIFF_GUTTER_WIDTH + 12;
        let lines =
            diff_lines_until(&file, content_width, Theme::github_dark(), true, usize::MAX).lines;
        let diff_rows: Vec<&Line<'_>> = lines
            .iter()
            .filter(|line| line_text(line).starts_with(RAIL_MARKER))
            .collect();

        let first_prefix = line_prefix(diff_rows[0], DIFF_GUTTER_WIDTH);
        assert!(first_prefix.contains('+'));

        let continuation_prefix = line_prefix(diff_rows[1], DIFF_GUTTER_WIDTH);
        assert_eq!(
            continuation_prefix,
            format!("{RAIL_MARKER}{}", " ".repeat(DIFF_GUTTER_WIDTH - 1))
        );

        let continuation_content = line_suffix(diff_rows[1], DIFF_GUTTER_WIDTH);
        assert!(!continuation_content.is_empty());
        assert!(!continuation_content.starts_with('+'));
    }

    #[test]
    fn hunk_offsets_match_wrapped_rendered_rows() {
        let theme = Theme::github_dark();
        let file = diff_file_with_hunks(vec![
            (
                "@@ -1 +1 @@",
                vec![added_diff_line(
                    "alpha beta gamma delta epsilon zeta eta theta iota",
                )],
            ),
            ("@@ -10 +10 @@", vec![added_diff_line("short")]),
        ]);
        let content_width = DIFF_GUTTER_WIDTH + 12;

        let offsets = hunk_offsets(&file, content_width, theme, true);
        let lines = diff_lines_until(&file, content_width, theme, true, usize::MAX).lines;

        assert_eq!(offsets.len(), 2);
        assert_eq!(line_text(&lines[offsets[0]]), " @@ -1 +1 @@");
        assert_eq!(line_text(&lines[offsets[1]]), " @@ -10 +10 @@");
        assert!(offsets[1] > offsets[0] + 2);
    }

    #[test]
    fn intraline_highlights_inserted_spans() {
        let removed = r#"let title = format!("{} +{}", path, additions);"#;
        let added = r#"let title = format!("{} +{} -{}", path, additions, deletions);"#;
        let file = diff_file_with_lines(vec![removed_diff_line(removed), added_diff_line(added)]);
        let lines = diff_lines_until(
            &file,
            DIFF_GUTTER_WIDTH + 120,
            Theme::github_dark(),
            true,
            usize::MAX,
        )
        .lines;
        let added_line = rendered_line_with_text(&lines, added);

        assert_bold_span(added_line, " -{}");
        assert_bold_span(added_line, ", deletions");
    }

    #[test]
    fn intraline_highlights_replaced_spans_on_both_sides() {
        let removed = "let total = add(left, right);";
        let added = "let total = add(left, extra);";
        let file = diff_file_with_lines(vec![removed_diff_line(removed), added_diff_line(added)]);
        let lines = diff_lines_until(
            &file,
            DIFF_GUTTER_WIDTH + 80,
            Theme::github_dark(),
            true,
            usize::MAX,
        )
        .lines;

        assert_bold_span(rendered_line_with_text(&lines, removed), "right");
        assert_bold_span(rendered_line_with_text(&lines, added), "extra");
    }

    #[test]
    fn unrelated_changed_lines_skip_intraline_highlighting() {
        let removed = "fn alpha() -> bool { true }";
        let added = "struct Beta { value: usize }";
        let file = diff_file_with_lines(vec![removed_diff_line(removed), added_diff_line(added)]);
        let lines = diff_lines_until(
            &file,
            DIFF_GUTTER_WIDTH + 80,
            Theme::github_dark(),
            true,
            usize::MAX,
        )
        .lines;

        assert_no_bold_spans(rendered_line_with_text(&lines, removed));
        assert_no_bold_spans(rendered_line_with_text(&lines, added));
    }

    #[test]
    fn vue_hunks_use_source_context_before_the_hunk() {
        let theme = Theme::github_dark();
        let changed_line = "const getCurrentCursorPosition = () =>";
        let file = DiffFile {
            id: "0".to_string(),
            old_path: "src/App.vue".to_string(),
            path: "src/App.vue".to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::loaded(
                "<template>\n</template>\n<script setup lang=\"ts\">\n".to_string(),
            ),
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions: 1,
            deletions: 0,
            hunks: vec![DiffHunk {
                header: "@@ -4 +4 @@".to_string(),
                old_start: 4,
                old_lines: 0,
                new_start: 4,
                new_lines: 1,
                lines: vec![DiffLine {
                    kind: DiffLineKind::Added,
                    old_line: None,
                    new_line: Some(4),
                    content: changed_line.to_string(),
                }],
            }],
            binary: false,
        };

        let lines = diff_lines_until(&file, DIFF_GUTTER_WIDTH + 80, theme, true, usize::MAX).lines;
        let added_line = lines
            .iter()
            .find(|line| line_text(line).contains(changed_line))
            .expect("changed Vue script line should render");

        assert!(
            added_line
                .spans
                .iter()
                .any(|span| span.content.contains("const")
                    && span.style.fg == Some(theme.syntax.keyword)),
            "{:?}",
            added_line.spans
        );
    }

    #[test]
    fn review_mode_omits_diff_header_staging_affordance() {
        let file = diff_file_with_line(DiffLineKind::Context, "short");
        let header = render_file_header(&file, 80, false, Theme::github_dark());

        assert!(!line_text(&header).contains("[unstaged]"));
    }

    fn diff_file_with_line(kind: DiffLineKind, content: &str) -> DiffFile {
        DiffFile {
            id: "0".to_string(),
            old_path: "sample.unknown".to_string(),
            path: "sample.unknown".to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions: usize::from(matches!(kind, DiffLineKind::Added)),
            deletions: usize::from(matches!(kind, DiffLineKind::Removed)),
            hunks: vec![DiffHunk {
                header: "@@ -1 +1 @@".to_string(),
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                lines: vec![DiffLine {
                    kind,
                    old_line: (!matches!(kind, DiffLineKind::Added)).then_some(1),
                    new_line: (!matches!(kind, DiffLineKind::Removed)).then_some(1),
                    content: content.to_string(),
                }],
            }],
            binary: false,
        }
    }

    fn diff_file_with_lines(lines: Vec<DiffLine>) -> DiffFile {
        let additions = lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Added)
            .count();
        let deletions = lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Removed)
            .count();
        let old_lines = lines
            .iter()
            .filter(|line| !matches!(line.kind, DiffLineKind::Added | DiffLineKind::Meta))
            .count() as u32;
        let new_lines = lines
            .iter()
            .filter(|line| !matches!(line.kind, DiffLineKind::Removed | DiffLineKind::Meta))
            .count() as u32;

        DiffFile {
            id: "0".to_string(),
            old_path: "sample.unknown".to_string(),
            path: "sample.unknown".to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions,
            deletions,
            hunks: vec![DiffHunk {
                header: "@@ -1 +1 @@".to_string(),
                old_start: 1,
                old_lines,
                new_start: 1,
                new_lines,
                lines,
            }],
            binary: false,
        }
    }

    fn diff_file_with_hunks(hunks: Vec<(&str, Vec<DiffLine>)>) -> DiffFile {
        let additions = hunks
            .iter()
            .flat_map(|(_, lines)| lines)
            .filter(|line| line.kind == DiffLineKind::Added)
            .count();
        let deletions = hunks
            .iter()
            .flat_map(|(_, lines)| lines)
            .filter(|line| line.kind == DiffLineKind::Removed)
            .count();
        let hunks = hunks
            .into_iter()
            .enumerate()
            .map(|(index, (header, lines))| DiffHunk {
                header: header.to_string(),
                old_start: index as u32 + 1,
                old_lines: 1,
                new_start: index as u32 + 1,
                new_lines: 1,
                lines,
            })
            .collect();

        DiffFile {
            id: "0".to_string(),
            old_path: "sample.unknown".to_string(),
            path: "sample.unknown".to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions,
            deletions,
            hunks,
            binary: false,
        }
    }

    fn removed_diff_line(content: &str) -> DiffLine {
        DiffLine {
            kind: DiffLineKind::Removed,
            old_line: Some(1),
            new_line: None,
            content: content.to_string(),
        }
    }

    fn added_diff_line(content: &str) -> DiffLine {
        DiffLine {
            kind: DiffLineKind::Added,
            old_line: None,
            new_line: Some(1),
            content: content.to_string(),
        }
    }

    fn line_prefix(line: &Line<'_>, width: usize) -> String {
        line_text(line).chars().take(width).collect()
    }

    fn line_suffix(line: &Line<'_>, width: usize) -> String {
        line_text(line).chars().skip(width).collect()
    }

    fn rendered_line_with_text<'a>(lines: &'a [Line<'static>], text: &str) -> &'a Line<'static> {
        lines
            .iter()
            .find(|line| line_text(line).contains(text))
            .expect("rendered diff line should exist")
    }

    fn assert_bold_span(line: &Line<'_>, text: &str) {
        assert!(
            line.spans.iter().any(|span| {
                span.content.as_ref() == text && span.style.add_modifier.contains(Modifier::BOLD)
            }),
            "expected bold span {text:?} in {:?}",
            line.spans
        );
    }

    fn assert_no_bold_spans(line: &Line<'_>) {
        assert!(
            !line
                .spans
                .iter()
                .any(|span| span.style.add_modifier.contains(Modifier::BOLD)),
            "expected no bold spans in {:?}",
            line.spans
        );
    }

    fn line_text(line: &Line<'_>) -> String {
        spans_text(&line.spans)
    }

    fn spans_text(spans: &[Span<'_>]) -> String {
        spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }
}
