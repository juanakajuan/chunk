use std::ops::Range;
use std::str::Lines;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::model::{DiffFile, DiffHunk, DiffLine, DiffLineKind, SourceSnapshot};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;

use super::RAIL_MARKER;
use super::file_summary::{
    file_header_label, file_icon, file_icon_color, file_status_suffix, format_file_stats,
    padding_before_stats, push_stat_spans, stage_display, stats_width,
};
use super::intraline::{
    emphasize_spans, intraline_block_end, intraline_ranges_for_block, is_intraline_candidate,
};
use super::text::{
    color_style, display_width, expand_tabs, muted_line, wrap_line, wrap_styled_spans,
};

const DIFF_GUTTER_WIDTH: usize = 11;
const BINARY_FILE_CHANGED_MESSAGE: &str = "Binary file changed";
const FILE_CHANGED_WITHOUT_TEXTUAL_HUNKS_MESSAGE: &str = "File changed without textual hunks";

pub(crate) struct RenderedRows {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) complete: bool,
}

pub(crate) struct DiffLayoutCounts {
    pub(crate) total_rows: usize,
    pub(crate) hunk_offsets: Vec<usize>,
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

#[derive(Debug, Clone, Copy)]
struct HunkRenderOptions {
    content_width: usize,
    theme: Theme,
    can_stage: bool,
    is_selected: bool,
    target_rows: usize,
}

pub(crate) fn diff_lines_until(
    file: &DiffFile,
    content_width: usize,
    theme: Theme,
    can_stage: bool,
    selected_hunk_index: Option<usize>,
    target_rows: usize,
) -> RenderedRows {
    let mut lines = render_file_header_rows(file, content_width, can_stage, theme);

    if let Some(message) = file_body_message(file) {
        lines.extend(muted_body_rows(message, content_width, theme));
        return RenderedRows::complete(lines);
    }

    let mut old_highlighter =
        DiffSyntaxHighlighter::new(diff_old_path(file), &file.old_source, theme);
    let mut new_highlighter =
        DiffSyntaxHighlighter::new(diff_new_path(file), &file.new_source, theme);

    for (hunk_index, hunk) in file.hunks.iter().enumerate() {
        let options = HunkRenderOptions {
            content_width,
            theme,
            can_stage,
            is_selected: selected_hunk_index == Some(hunk_index),
            target_rows,
        };
        if !push_hunk_lines_until(
            &mut lines,
            hunk,
            &mut old_highlighter,
            &mut new_highlighter,
            options,
        ) {
            return RenderedRows::partial(lines);
        }
    }

    RenderedRows::complete(lines)
}

pub(crate) fn diff_layout_counts(
    file: &DiffFile,
    content_width: usize,
    theme: Theme,
    can_stage: bool,
) -> DiffLayoutCounts {
    let header_rows = render_file_header_rows(file, content_width, can_stage, theme).len();

    if let Some(message) = file_body_message(file) {
        return DiffLayoutCounts {
            total_rows: header_rows + muted_body_rows(message, content_width, theme).len(),
            hunk_offsets: Vec::new(),
        };
    }

    let mut total_rows = header_rows;
    let mut hunk_offsets = Vec::with_capacity(file.hunks.len());
    for hunk in &file.hunks {
        hunk_offsets.push(total_rows);
        total_rows += hunk_row_count(hunk, content_width, theme, can_stage);
    }

    DiffLayoutCounts {
        total_rows,
        hunk_offsets,
    }
}

pub(crate) fn selected_hunk_header_rows(
    hunk: &DiffHunk,
    content_width: usize,
    theme: Theme,
    can_stage: bool,
) -> Vec<Line<'static>> {
    wrap_line(
        hunk_header_line(hunk, theme, can_stage, true),
        content_width,
    )
}

fn push_hunk_lines_until(
    lines: &mut Vec<Line<'static>>,
    hunk: &DiffHunk,
    old_highlighter: &mut DiffSyntaxHighlighter<'_>,
    new_highlighter: &mut DiffSyntaxHighlighter<'_>,
    options: HunkRenderOptions,
) -> bool {
    old_highlighter.advance_to(hunk.old_start);
    new_highlighter.advance_to(hunk.new_start);

    lines.extend(wrap_line(
        hunk_header_line(hunk, options.theme, options.can_stage, options.is_selected),
        options.content_width,
    ));
    if lines.len() >= options.target_rows {
        return false;
    }

    let mut line_index = 0;
    while line_index < hunk.lines.len() {
        let line = &hunk.lines[line_index];
        if !is_intraline_candidate(line.kind) {
            if !push_diff_line_until(lines, line, &[], old_highlighter, new_highlighter, options) {
                return false;
            }
            line_index += 1;
            continue;
        }

        let block_end = intraline_block_end(&hunk.lines, line_index);
        let block = &hunk.lines[line_index..block_end];
        let intraline_ranges = intraline_ranges_for_block(block);
        for (line, ranges) in block.iter().zip(&intraline_ranges) {
            if !push_diff_line_until(
                lines,
                line,
                ranges,
                old_highlighter,
                new_highlighter,
                options,
            ) {
                return false;
            }
        }
        line_index = block_end;
    }

    true
}

fn push_diff_line_until(
    lines: &mut Vec<Line<'static>>,
    line: &DiffLine,
    intraline_ranges: &[Range<usize>],
    old_highlighter: &mut DiffSyntaxHighlighter<'_>,
    new_highlighter: &mut DiffSyntaxHighlighter<'_>,
    options: HunkRenderOptions,
) -> bool {
    lines.extend(diff_line(
        line,
        intraline_ranges,
        old_highlighter,
        new_highlighter,
        options.content_width,
        options.theme,
    ));
    lines.len() < options.target_rows
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

fn hunk_row_count(hunk: &DiffHunk, content_width: usize, theme: Theme, can_stage: bool) -> usize {
    let header_rows = wrap_line(
        hunk_header_line(hunk, theme, can_stage, false),
        content_width,
    )
    .len();
    let content_rows = hunk
        .lines
        .iter()
        .map(|line| diff_line_row_count(line, content_width))
        .sum::<usize>();

    header_rows + content_rows
}

fn diff_line_row_count(line: &DiffLine, content_width: usize) -> usize {
    wrap_styled_spans(
        vec![Span::raw(expand_tabs(&line.content))],
        diff_content_width(content_width),
    )
    .len()
}

fn hunk_header_line(
    hunk: &DiffHunk,
    theme: Theme,
    can_stage: bool,
    selected: bool,
) -> Line<'static> {
    let background = if selected {
        theme.selected
    } else {
        theme.background_alt
    };
    let foreground = if selected { theme.accent } else { theme.muted };
    let style = color_style(foreground, background).add_modifier(Modifier::BOLD);
    let marker = if selected { ">" } else { " " };
    let mut spans = vec![Span::styled(format!("{marker} {}", hunk.header), style)];

    if can_stage {
        let stage = stage_display(hunk.stage, background, theme);
        spans.push(Span::styled(stage.suffix.to_string(), stage.style));
    }

    Line::from(spans)
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
            let mut gutter_spans = if index == 0 {
                diff_gutter_spans(old_line, new_line, style, number_style)
            } else {
                continuation_gutter_spans(style, number_style)
            };
            gutter_spans.extend(row);
            Line::from(gutter_spans)
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
    let spans = match kind {
        DiffLineKind::Added => new_highlighter.highlight_line(content, content_style),
        DiffLineKind::Removed => old_highlighter.highlight_line(content, content_style),
        DiffLineKind::Context => {
            return highlight_context_content(
                content,
                content_style,
                old_highlighter,
                new_highlighter,
            );
        }
        DiffLineKind::Meta => return vec![Span::styled(expand_tabs(content), content_style)],
    };

    emphasize_spans(spans, intraline_ranges)
}

fn highlight_context_content(
    content: &str,
    content_style: Style,
    old_highlighter: &mut DiffSyntaxHighlighter<'_>,
    new_highlighter: &mut DiffSyntaxHighlighter<'_>,
) -> Vec<Span<'static>> {
    let use_new_highlighter = new_highlighter.is_enabled();
    let spans = if use_new_highlighter {
        new_highlighter.highlight_line(content, content_style)
    } else {
        old_highlighter.highlight_line(content, content_style)
    };

    if use_new_highlighter {
        old_highlighter.advance_line(content);
    } else {
        new_highlighter.advance_line(content);
    }

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
        while self.next_line < target_line {
            if !self.advance_source_line() {
                self.next_line = target_line;
                break;
            }
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

fn file_body_message(file: &DiffFile) -> Option<&'static str> {
    if file.binary {
        Some(BINARY_FILE_CHANGED_MESSAGE)
    } else if file.hunks.is_empty() {
        Some(FILE_CHANGED_WITHOUT_TEXTUAL_HUNKS_MESSAGE)
    } else {
        None
    }
}

fn muted_body_rows(
    message: &'static str,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    wrap_line(muted_line(message, theme), content_width)
}

fn render_file_header_rows(
    file: &DiffFile,
    content_width: usize,
    can_stage: bool,
    theme: Theme,
) -> Vec<Line<'static>> {
    wrap_line(
        render_file_header(file, content_width, can_stage, theme),
        content_width,
    )
}

fn render_file_header(
    file: &DiffFile,
    content_width: usize,
    can_stage: bool,
    theme: Theme,
) -> Line<'static> {
    let display_path = file.display_path();
    let icon = file_icon(display_path);
    let label = format!(" {}", file_header_label(file));
    let suffix = file_status_suffix(file.status);
    let stage_affordance = can_stage.then(|| stage_display(file.stage, theme.background, theme));
    let stats = format_file_stats(file);
    let stats_width = stats_width(&stats);
    let stage_affordance_width = stage_affordance
        .as_ref()
        .map_or(0, |stage_affordance| display_width(stage_affordance.suffix));
    let used_width = display_width(icon)
        + display_width(&label)
        + display_width(suffix)
        + stage_affordance_width
        + stats_width;
    let padding = padding_before_stats(content_width, used_width, stats_width);
    let style = color_style(theme.text, theme.background);
    let muted_style = color_style(theme.muted, theme.background);
    let icon_style = color_style(file_icon_color(display_path, theme), theme.background);

    let mut spans = vec![
        Span::styled(icon, icon_style),
        Span::styled(label, style),
        Span::styled(suffix.to_string(), muted_style),
    ];
    if let Some(stage_affordance) = stage_affordance {
        spans.push(Span::styled(
            stage_affordance.suffix.to_string(),
            stage_affordance.style,
        ));
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
            let lines = diff_lines_until(
                &file,
                content_width,
                Theme::github_dark(),
                true,
                None,
                usize::MAX,
            )
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
    fn diff_line_count_matches_fully_rendered_rows() {
        let file = diff_file_with_line(
            DiffLineKind::Added,
            "alpha beta gamma delta epsilon zeta eta theta iota",
        );
        let content_width = DIFF_GUTTER_WIDTH + 12;
        let theme = Theme::github_dark();
        let lines = diff_lines_until(&file, content_width, theme, true, None, usize::MAX).lines;

        assert_eq!(
            diff_layout_counts(&file, content_width, theme, true).total_rows,
            lines.len()
        );
    }

    #[test]
    fn continuation_rows_align_under_diff_content() {
        let file = diff_file_with_line(
            DiffLineKind::Added,
            "alpha beta gamma delta epsilon zeta eta theta iota",
        );
        let content_width = DIFF_GUTTER_WIDTH + 12;
        let lines = diff_lines_until(
            &file,
            content_width,
            Theme::github_dark(),
            true,
            None,
            usize::MAX,
        )
        .lines;
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

        let offsets = diff_layout_counts(&file, content_width, theme, true).hunk_offsets;
        let lines = diff_lines_until(&file, content_width, theme, true, None, usize::MAX).lines;

        assert_eq!(offsets.len(), 2);
        assert!(line_text(&lines[offsets[0]]).starts_with("  @@ -1 +1 @@"));
        assert!(line_text(&lines[offsets[1]]).starts_with("  @@ -10 +10 @@"));
        assert!(offsets[1] > offsets[0] + 2);
    }

    #[test]
    fn selected_hunk_header_gets_marker() {
        let theme = Theme::github_dark();
        let file = diff_file_with_hunks(vec![
            ("@@ -1 +1 @@", vec![added_diff_line("one")]),
            ("@@ -10 +10 @@", vec![added_diff_line("two")]),
        ]);
        let content_width = DIFF_GUTTER_WIDTH + 40;

        let offsets = diff_layout_counts(&file, content_width, theme, true).hunk_offsets;
        let lines = diff_lines_until(&file, content_width, theme, true, Some(1), usize::MAX).lines;

        assert!(line_text(&lines[offsets[0]]).starts_with("  @@ -1 +1 @@"));
        assert!(line_text(&lines[offsets[1]]).starts_with("> @@ -10 +10 @@"));
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
            None,
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
            None,
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
            None,
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
                stage: FileStage::Unstaged,
                lines: vec![DiffLine {
                    kind: DiffLineKind::Added,
                    old_line: None,
                    new_line: Some(4),
                    content: changed_line.to_string(),
                }],
            }],
            binary: false,
        };

        let lines =
            diff_lines_until(&file, DIFF_GUTTER_WIDTH + 80, theme, true, None, usize::MAX).lines;
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

    #[test]
    fn diff_header_icon_uses_file_type_color() {
        let theme = Theme::github_dark();
        let mut file = diff_file_with_line(DiffLineKind::Context, "short");
        file.old_path = "src/main.rs".to_string();
        file.path = "src/main.rs".to_string();
        let header = render_file_header(&file, 80, false, theme);

        let icon = file_icon("src/main.rs");
        let icon_span = header
            .spans
            .iter()
            .find(|span| span.content.as_ref() == icon)
            .expect("type icon span should render");
        assert_eq!(
            icon_span.style.fg,
            Some(file_icon_color("src/main.rs", theme))
        );
    }

    #[test]
    fn review_mode_omits_hunk_staging_affordances() {
        let file = diff_file_with_line(DiffLineKind::Context, "short");
        let lines =
            diff_lines_until(&file, 80, Theme::github_dark(), false, None, usize::MAX).lines;

        assert!(
            lines
                .iter()
                .all(|line| !line_text(line).contains("[unstaged]"))
        );
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
                stage: FileStage::Unstaged,
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
                stage: FileStage::Unstaged,
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
                stage: FileStage::Unstaged,
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
