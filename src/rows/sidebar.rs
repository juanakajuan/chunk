use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::model::DiffFile;
use crate::theme::Theme;

use super::RAIL_MARKER;
use super::file_summary::{
    format_file_stats, padding_before_stats, push_stat_spans, sidebar_file_label, stage_display,
    stats_width, status_color,
};
use super::text::{color_style, display_width, muted_line, wrap_line, wrap_styled_spans};

const SIDEBAR_STAGE_GUTTER_WIDTH: usize = 8;
const SIDEBAR_REVIEW_GUTTER_WIDTH: usize = 4;

pub(crate) struct SidebarRowsInput<'a> {
    pub(crate) files: &'a [DiffFile],
    pub(crate) empty_message: &'static str,
    pub(crate) can_stage: bool,
    pub(crate) selected_file_index: usize,
    pub(crate) sidebar_scroll: usize,
    pub(crate) row_counts: &'a [usize],
    pub(crate) content_width: usize,
    pub(crate) visible_height: usize,
    pub(crate) theme: Theme,
}

pub(crate) struct RenderedSidebarRows {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) row_records: Vec<SidebarRowRecord>,
    pub(crate) sidebar_scroll: usize,
}

pub(crate) struct SidebarRowRecord {
    pub(crate) index: usize,
    pub(crate) row_count: usize,
}

pub(crate) fn sidebar_rows(input: SidebarRowsInput<'_>) -> RenderedSidebarRows {
    if input.files.is_empty() {
        return RenderedSidebarRows {
            lines: vec![muted_line(input.empty_message, input.theme)],
            row_records: Vec::new(),
            sidebar_scroll: 0,
        };
    }

    let sidebar_scroll = visible_sidebar_scroll(
        input.row_counts,
        input.sidebar_scroll,
        input.selected_file_index,
        input.files.len(),
        input.visible_height,
    );
    let selected_file_index = input.selected_file_index.min(input.files.len() - 1);

    let mut lines = Vec::new();
    let mut row_records = Vec::new();
    for (index, file) in input.files.iter().enumerate().skip(sidebar_scroll) {
        let remaining_height = input.visible_height.saturating_sub(lines.len());
        if remaining_height == 0 {
            break;
        }

        let entry_lines = render_file_entry(
            index,
            file,
            selected_file_index,
            input.content_width,
            input.can_stage,
            input.theme,
        );
        let visible_rows = entry_lines.len().min(remaining_height);

        row_records.push(SidebarRowRecord {
            index,
            row_count: visible_rows,
        });
        lines.extend(entry_lines.into_iter().take(visible_rows));
        if lines.len() >= input.visible_height {
            break;
        }
    }

    RenderedSidebarRows {
        lines,
        row_records,
        sidebar_scroll,
    }
}

pub(crate) fn sidebar_row_counts(
    files: &[DiffFile],
    content_width: usize,
    can_stage: bool,
    theme: Theme,
) -> Vec<usize> {
    files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            render_file_entry(index, file, usize::MAX, content_width, can_stage, theme).len()
        })
        .collect()
}

fn visible_sidebar_scroll(
    row_counts: &[usize],
    sidebar_scroll: usize,
    selected_file_index: usize,
    file_count: usize,
    visible_height: usize,
) -> usize {
    if file_count == 0 {
        return 0;
    }

    let selected_index = selected_file_index.min(file_count - 1);
    let clamped_scroll = sidebar_scroll.min(file_count - 1);

    if selected_index < clamped_scroll {
        return selected_index;
    }

    if sidebar_selection_visible(row_counts, clamped_scroll, selected_index, visible_height) {
        clamped_scroll
    } else {
        sidebar_scroll_for_selected(row_counts, selected_index, visible_height)
    }
}

fn sidebar_selection_visible(
    row_counts: &[usize],
    scroll: usize,
    selected_index: usize,
    visible_height: usize,
) -> bool {
    let Some(selected_row_count) = row_counts.get(selected_index).copied() else {
        return false;
    };

    if selected_index < scroll {
        return false;
    }

    let visible_height = visible_height.max(1);
    let rows_before_selected: usize = row_counts[scroll..selected_index].iter().sum();
    if rows_before_selected >= visible_height {
        return false;
    }

    rows_before_selected == 0 || rows_before_selected + selected_row_count <= visible_height
}

fn sidebar_scroll_for_selected(
    row_counts: &[usize],
    selected_index: usize,
    visible_height: usize,
) -> usize {
    let visible_height = visible_height.max(1);
    let mut scroll = selected_index;
    let mut row_total = row_counts.get(selected_index).copied().unwrap_or(1);

    while scroll > 0 {
        let previous_rows = row_counts.get(scroll - 1).copied().unwrap_or(1);
        if row_total + previous_rows > visible_height {
            break;
        }

        scroll -= 1;
        row_total += previous_rows;
    }

    scroll
}

fn render_file_entry(
    index: usize,
    file: &DiffFile,
    selected_index: usize,
    content_width: usize,
    can_stage: bool,
    theme: Theme,
) -> Vec<Line<'static>> {
    let is_selected = index == selected_index;
    let row_background = if is_selected {
        theme.selected
    } else {
        theme.background
    };
    let base_style = color_style(theme.text, row_background);
    let marker_style = if is_selected {
        color_style(theme.accent, row_background)
    } else {
        base_style
    };
    let status_style = color_style(status_color(file.status, theme), row_background);
    let file_label = sidebar_file_label(file);
    let stats = format_file_stats(file);
    let stats_width = stats_width(&stats);
    let gutter_width = sidebar_gutter_width(can_stage);
    let used_width = gutter_width + display_width(&file_label) + stats_width;
    let padding = padding_before_stats(content_width, used_width, stats_width);
    let rail = if is_selected { RAIL_MARKER } else { " " };

    let mut line_prefix = vec![
        Span::styled(rail, marker_style),
        Span::styled(" ", base_style),
    ];
    if can_stage {
        let stage_marker = stage_display(file.stage, row_background, theme);
        line_prefix.push(Span::styled(stage_marker.checkbox, stage_marker.style));
        line_prefix.push(Span::styled(" ", base_style));
    }
    line_prefix.push(Span::styled(file.status.marker().to_string(), status_style));
    line_prefix.push(Span::styled(" ", base_style));
    let mut content_spans = vec![
        Span::styled(file_label, base_style),
        Span::styled(padding, base_style),
    ];
    push_stat_spans(&mut content_spans, file, row_background, theme);

    if content_width <= gutter_width {
        let mut spans = line_prefix;
        spans.extend(content_spans);
        return wrap_line(Line::from(spans), content_width);
    }

    wrap_sidebar_content(
        line_prefix,
        continuation_prefix(rail, marker_style, base_style, gutter_width),
        content_spans,
        content_width,
        gutter_width,
    )
}

fn wrap_sidebar_content(
    first_prefix: Vec<Span<'static>>,
    continuation_prefix: Vec<Span<'static>>,
    content_spans: Vec<Span<'static>>,
    content_width: usize,
    gutter_width: usize,
) -> Vec<Line<'static>> {
    wrap_styled_spans(content_spans, content_width.saturating_sub(gutter_width))
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let mut line_spans = if index == 0 {
                first_prefix.clone()
            } else {
                continuation_prefix.clone()
            };
            line_spans.extend(row);
            Line::from(line_spans)
        })
        .collect()
}

fn continuation_prefix(
    rail: &str,
    marker_style: Style,
    base_style: Style,
    gutter_width: usize,
) -> Vec<Span<'static>> {
    vec![
        Span::styled(rail.to_string(), marker_style),
        Span::styled(" ".repeat(gutter_width.saturating_sub(1)), base_style),
    ]
}

fn sidebar_gutter_width(can_stage: bool) -> usize {
    if can_stage {
        SIDEBAR_STAGE_GUTTER_WIDTH
    } else {
        SIDEBAR_REVIEW_GUTTER_WIDTH
    }
}

#[cfg(test)]
mod tests {
    use ratatui::text::Line;

    use crate::model::{
        DiffFile, DiffHunk, DiffLine, DiffLineKind, FileStage, FileStatus, SourceSnapshot,
    };

    use super::*;

    #[test]
    fn sidebar_entries_wrap_to_content_width() {
        let files = vec![diff_file_with_path(
            "src/components/extremely_long_file_name_component.rs",
        )];
        let content_width = 16;
        let row_counts = sidebar_row_counts(&files, content_width, true, Theme::github_dark());
        let rows = sidebar_rows(SidebarRowsInput {
            files: &files,
            empty_message: "No tracked changes",
            can_stage: true,
            selected_file_index: 0,
            sidebar_scroll: 0,
            row_counts: &row_counts,
            content_width,
            visible_height: 8,
            theme: Theme::github_dark(),
        });

        assert!(rows.lines.len() > 1);
        assert!(rows.lines.iter().all(|line| line.width() <= content_width));
        assert_eq!(rows.row_records.len(), 1);
        assert_eq!(rows.row_records[0].index, 0);
        assert_eq!(rows.row_records[0].row_count, rows.lines.len());
    }

    #[test]
    fn sidebar_scroll_accounts_for_wrapped_rows() {
        let files = vec![
            diff_file_with_path("first_extremely_long_file_name.rs"),
            diff_file_with_path("second_extremely_long_file_name.rs"),
        ];
        let row_counts = sidebar_row_counts(&files, 14, true, Theme::github_dark());
        let rows = sidebar_rows(SidebarRowsInput {
            files: &files,
            empty_message: "No tracked changes",
            can_stage: true,
            selected_file_index: 1,
            sidebar_scroll: 0,
            row_counts: &row_counts,
            content_width: 14,
            visible_height: 2,
            theme: Theme::github_dark(),
        });

        assert_eq!(rows.sidebar_scroll, 1);
        assert_eq!(rows.lines.len(), 2);
        assert_eq!(rows.row_records.len(), 1);
        assert_eq!(rows.row_records[0].index, 1);
        assert_eq!(rows.row_records[0].row_count, 2);
    }

    #[test]
    fn review_mode_omits_sidebar_staging_affordances() {
        let file = diff_file_with_path("src/main.rs");
        let sidebar = render_file_entry(0, &file, 0, 80, false, Theme::github_dark());

        assert!(!line_text(&sidebar[0]).contains("[ ]"));
    }

    fn diff_file_with_path(path: &str) -> DiffFile {
        DiffFile {
            id: path.to_string(),
            old_path: path.to_string(),
            path: path.to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions: 12,
            deletions: 3,
            hunks: vec![DiffHunk {
                header: "@@ -1 +1 @@".to_string(),
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                stage: FileStage::Unstaged,
                lines: vec![DiffLine {
                    kind: DiffLineKind::Context,
                    old_line: Some(1),
                    new_line: Some(1),
                    content: "short".to_string(),
                }],
            }],
            binary: false,
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }
}
