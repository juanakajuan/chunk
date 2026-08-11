//! Mouse text selection over rendered terminal rows.
//!
//! The app records the visible text surface after each draw. Mouse events then
//! use that surface for drag selection and copy text extraction.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

#[derive(Debug, Default)]
pub(crate) struct TextSelection {
    lines: Vec<SelectableLine>,
    drag: Option<SelectionDrag>,
    selected: Option<SelectionRange>,
    clipboard_request: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectableLine {
    surface: Rect,
    row: u16,
    start_col: u16,
    width: u16,
    text: String,
}

#[derive(Debug, Clone, Copy)]
struct SelectedLine<'a> {
    render_index: usize,
    line: &'a SelectableLine,
    columns: SelectedColumns,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionPoint {
    column: u16,
    row: u16,
}

impl SelectionPoint {
    fn new(column: u16, row: u16) -> Self {
        Self { column, row }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionDrag {
    surface: Rect,
    anchor: SelectionPoint,
    cursor: SelectionPoint,
    moved: bool,
}

impl SelectionDrag {
    fn new(surface: Rect, anchor: SelectionPoint) -> Self {
        Self {
            surface,
            anchor,
            cursor: anchor,
            moved: false,
        }
    }

    fn update_cursor(&mut self, cursor: SelectionPoint) {
        self.moved |= cursor != self.anchor;
        self.cursor = cursor;
    }

    fn range(self) -> Option<SelectionRange> {
        self.moved
            .then(|| SelectionRange::between(self.surface, self.anchor, self.cursor))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionRange {
    surface: Rect,
    start: SelectionPoint,
    end: SelectionPoint,
}

impl TextSelection {
    pub(crate) fn begin_frame(&mut self) {
        self.lines.clear();
    }

    pub(crate) fn clear(&mut self) {
        self.drag = None;
        self.selected = None;
        self.clipboard_request = None;
    }

    pub(crate) fn decorate_visible_lines(
        &mut self,
        area: Rect,
        lines: Vec<Line<'static>>,
        line_scroll: usize,
        visible_height: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        if area.width == 0 || area.height == 0 {
            return lines;
        }

        let visible_height = visible_height.min(usize::from(area.height));
        self.record_visible_lines(area, &lines, line_scroll, visible_height);
        self.highlight_visible_lines(area, lines, line_scroll, visible_height, theme)
    }

    pub(crate) fn begin_drag(&mut self, column: u16, row: u16) -> bool {
        let Some((surface, point)) = self.selectable_point_at(column, row) else {
            self.drag = None;
            self.selected = None;
            return false;
        };

        self.drag = Some(SelectionDrag::new(surface, point));
        self.selected = None;
        true
    }

    pub(crate) fn update_drag(&mut self, column: u16, row: u16) -> bool {
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };

        drag.update_cursor(SelectionPoint::new(column, row));
        true
    }

    pub(crate) fn finish_drag(&mut self, column: u16, row: u16) -> bool {
        let Some(mut drag) = self.drag.take() else {
            return false;
        };

        drag.update_cursor(SelectionPoint::new(column, row));
        let Some(range) = drag.range() else {
            self.selected = None;
            return false;
        };

        self.selected = Some(range);
        if let Some(text) = self.text_for_range(range) {
            self.clipboard_request = Some(text);
        }

        true
    }

    pub(crate) fn take_clipboard_request(&mut self) -> Option<String> {
        self.clipboard_request.take()
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        self.selected.and_then(|range| self.text_for_range(range))
    }

    fn record_visible_lines(
        &mut self,
        area: Rect,
        lines: &[Line<'static>],
        line_scroll: usize,
        visible_height: usize,
    ) {
        for (visible_row, line) in lines
            .iter()
            .skip(line_scroll)
            .take(visible_height)
            .enumerate()
        {
            self.lines.push(SelectableLine {
                surface: area,
                row: area.y.saturating_add(saturating_u16(visible_row)),
                start_col: area.x,
                width: area.width,
                text: line_text(line),
            });
        }
    }

    fn highlight_visible_lines(
        &self,
        area: Rect,
        mut lines: Vec<Line<'static>>,
        line_scroll: usize,
        visible_height: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        let Some(range) = self.active_range() else {
            return lines;
        };
        if range.surface != area {
            return lines;
        }

        for visible_row in 0..visible_height {
            let line_index = line_scroll.saturating_add(visible_row);
            let Some(line) = lines.get_mut(line_index) else {
                break;
            };

            let row = area.y.saturating_add(saturating_u16(visible_row));
            let Some(columns) = range
                .columns_for_row(row, area.x)
                .and_then(|columns| columns.relative_to(area.x, area.width))
            else {
                continue;
            };

            highlight_line_selection(line, columns, theme);
        }

        lines
    }

    fn active_range(&self) -> Option<SelectionRange> {
        self.drag.and_then(SelectionDrag::range).or(self.selected)
    }

    fn selectable_point_at(&self, column: u16, row: u16) -> Option<(Rect, SelectionPoint)> {
        self.lines
            .iter()
            .rev()
            .find(|line| line.contains(column, row))
            .map(|line| {
                (
                    line.surface,
                    SelectionPoint::new(column.max(line.start_col), row),
                )
            })
    }

    fn text_for_range(&self, range: SelectionRange) -> Option<String> {
        let mut text = String::new();
        let mut has_text = false;

        for row in range.start.row..=range.end.row {
            if row > range.start.row {
                text.push('\n');
            }

            if let Some(row_text) = self.selected_row_text(range, row) {
                has_text |= !row_text.is_empty();
                text.push_str(&row_text);
            }
        }

        has_text.then_some(text)
    }

    fn selected_row_text(&self, range: SelectionRange, row: u16) -> Option<String> {
        let mut text = String::new();
        let mut last_end_col = None;

        for selected_line in self.selected_lines_for_row(row, range) {
            let line = selected_line.line;
            let columns = selected_line.columns;
            if let Some(previous_end) = last_end_col {
                let gap = usize::from(line.start_col.saturating_sub(previous_end));
                text.extend(std::iter::repeat_n(' ', gap));
            }

            text.push_str(&line.slice(columns));
            last_end_col = Some(columns.end_col);
        }

        last_end_col.is_some().then_some(text)
    }

    fn selected_lines_for_row(&self, row: u16, range: SelectionRange) -> Vec<SelectedLine<'_>> {
        let candidates = self
            .lines
            .iter()
            .enumerate()
            .filter_map(|(render_index, line)| {
                if line.surface != range.surface || line.row != row {
                    return None;
                }

                range
                    .columns_for_line(row, line)
                    .map(|columns| SelectedLine {
                        render_index,
                        line,
                        columns,
                    })
            })
            .collect::<Vec<_>>();

        let mut visible_lines = candidates
            .iter()
            .copied()
            .filter(|candidate| {
                !candidates.iter().any(|later| {
                    later.render_index > candidate.render_index
                        && candidate.columns.overlaps(later.columns)
                })
            })
            .collect::<Vec<_>>();
        visible_lines.sort_by_key(|selected| selected.line.start_col);
        visible_lines
    }
}

impl SelectableLine {
    fn contains(&self, column: u16, row: u16) -> bool {
        row == self.row && column >= self.start_col && column < self.end_col()
    }

    fn end_col(&self) -> u16 {
        self.start_col.saturating_add(self.width)
    }

    fn slice(&self, columns: SelectedColumns) -> String {
        text_by_display_columns(&self.text, columns.offset_from(self.start_col))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectedColumns {
    start_col: u16,
    end_col: u16,
}

impl SelectedColumns {
    fn new(start_col: u16, end_col: u16) -> Option<Self> {
        (start_col < end_col).then_some(Self { start_col, end_col })
    }

    fn clipped_to(self, start_col: u16, end_col: u16) -> Option<Self> {
        Self::new(self.start_col.max(start_col), self.end_col.min(end_col))
    }

    fn relative_to(self, start_col: u16, width: u16) -> Option<Self> {
        self.clipped_to(start_col, start_col.saturating_add(width))
            .map(|columns| columns.offset_from(start_col))
    }

    fn offset_from(self, start_col: u16) -> Self {
        Self {
            start_col: self.start_col.saturating_sub(start_col),
            end_col: self.end_col.saturating_sub(start_col),
        }
    }

    fn overlaps(self, other: Self) -> bool {
        self.start_col < other.end_col && other.start_col < self.end_col
    }

    fn overlaps_display_char(self, character: DisplayChar) -> bool {
        character.start_col < usize::from(self.end_col)
            && character.end_col > usize::from(self.start_col)
    }
}

impl SelectionRange {
    fn between(surface: Rect, left: SelectionPoint, right: SelectionPoint) -> Self {
        let (start, end) = if (left.row, left.column) <= (right.row, right.column) {
            (left, right)
        } else {
            (right, left)
        };

        Self {
            surface,
            start,
            end,
        }
    }

    fn columns_for_row(self, row: u16, line_start_col: u16) -> Option<SelectedColumns> {
        if row < self.start.row || row > self.end.row {
            return None;
        }

        let start_col = if row == self.start.row {
            self.start.column
        } else {
            line_start_col
        };
        let end_col = if row == self.end.row {
            self.end.column.saturating_add(1)
        } else {
            u16::MAX
        };

        SelectedColumns::new(start_col, end_col)
    }

    fn columns_for_line(self, row: u16, line: &SelectableLine) -> Option<SelectedColumns> {
        self.columns_for_row(row, line.start_col)?
            .clipped_to(line.start_col, line.end_col())
    }
}

fn highlight_line_selection(line: &mut Line<'static>, columns: SelectedColumns, theme: Theme) {
    let mut spans = Vec::new();
    let mut display_col = 0;

    for span in std::mem::take(&mut line.spans) {
        for character in display_chars(span.content.as_ref(), &mut display_col) {
            let style = if columns.overlaps_display_char(character) {
                span.style
                    .bg(theme.accent)
                    .fg(theme.on_accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                span.style
            };
            push_styled_char(&mut spans, character.value, style);
        }
    }

    line.spans = spans;
}

fn text_by_display_columns(text: &str, columns: SelectedColumns) -> String {
    let mut output = String::new();
    let mut display_col = 0;

    for character in display_chars(text, &mut display_col) {
        if character.start_col >= usize::from(columns.end_col) {
            break;
        }

        if columns.overlaps_display_char(character) {
            output.push(character.value);
        }
    }

    output
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisplayChar {
    value: char,
    start_col: usize,
    end_col: usize,
}

fn display_chars<'a>(
    text: &'a str,
    display_col: &'a mut usize,
) -> impl Iterator<Item = DisplayChar> + 'a {
    text.chars().map(move |value| {
        let char_start = *display_col;
        let char_end = char_start + char_display_width(value).max(1);
        *display_col = char_end;
        DisplayChar {
            value,
            start_col: char_start,
            end_col: char_end,
        }
    })
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn push_styled_char(spans: &mut Vec<Span<'static>>, value: char, style: Style) {
    match spans.last_mut() {
        Some(span) if span.style == style => {
            span.content.to_mut().push(value);
        }
        _ => {
            spans.push(Span::styled(value.to_string(), style));
        }
    }
}

fn char_display_width(value: char) -> usize {
    Span::raw(value.to_string()).width()
}

fn saturating_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_selects_visible_text_with_line_breaks() {
        let mut selection = TextSelection::default();
        let theme = Theme::github_dark();
        selection.begin_frame();
        let _ = selection.decorate_visible_lines(
            Rect::new(5, 10, 20, 2),
            vec![Line::raw("alpha beta"), Line::raw("gamma delta")],
            0,
            2,
            theme,
        );

        assert!(selection.begin_drag(8, 10));
        assert!(selection.update_drag(9, 11));
        assert!(selection.finish_drag(9, 11));

        assert_eq!(
            selection.take_clipboard_request().as_deref(),
            Some("ha beta\ngamma")
        );
    }

    #[test]
    fn selection_highlights_visible_cells_only() {
        let mut selection = TextSelection::default();
        let theme = Theme::github_dark();
        selection.begin_frame();
        let _ = selection.decorate_visible_lines(
            Rect::new(0, 0, 8, 1),
            vec![Line::raw("abcdef")],
            0,
            1,
            theme,
        );

        assert!(selection.begin_drag(1, 0));
        assert!(selection.update_drag(3, 0));
        let lines = selection.decorate_visible_lines(
            Rect::new(0, 0, 8, 1),
            vec![Line::raw("abcdef")],
            0,
            1,
            theme,
        );

        let highlighted = lines[0]
            .spans
            .iter()
            .filter(|span| span.style.bg == Some(theme.accent))
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(highlighted, "bcd");
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.content.as_ref() == "a" && span.style.bg != Some(theme.accent))
        );
    }

    #[test]
    fn selection_highlight_tracks_columns_across_styled_spans() {
        let mut selection = TextSelection::default();
        let theme = Theme::github_dark();
        let line = Line::from(vec![
            Span::raw("ab"),
            Span::styled("cd", Style::default().fg(theme.accent)),
            Span::raw("ef"),
        ]);

        selection.begin_frame();
        let _ = selection.decorate_visible_lines(
            Rect::new(0, 0, 6, 1),
            vec![line.clone()],
            0,
            1,
            theme,
        );
        assert!(selection.begin_drag(1, 0));
        assert!(selection.update_drag(2, 0));

        let lines =
            selection.decorate_visible_lines(Rect::new(0, 0, 6, 1), vec![line], 0, 1, theme);

        assert_eq!(highlighted_text(&lines), "bc");
        assert!(selection.finish_drag(2, 0));
        assert_eq!(selection.take_clipboard_request().as_deref(), Some("bc"));
    }

    #[test]
    fn drag_highlight_stays_on_origin_surface() {
        let mut selection = TextSelection::default();
        let theme = Theme::github_dark();
        let sidebar = Rect::new(0, 0, 10, 2);
        let diff = Rect::new(12, 0, 16, 2);

        selection.begin_frame();
        let _ = selection.decorate_visible_lines(
            sidebar,
            vec![Line::raw("file one"), Line::raw("file two")],
            0,
            2,
            theme,
        );
        let _ = selection.decorate_visible_lines(
            diff,
            vec![Line::raw("diff one"), Line::raw("diff two")],
            0,
            2,
            theme,
        );

        assert!(selection.begin_drag(17, 0));
        assert!(selection.update_drag(15, 1));

        selection.begin_frame();
        let sidebar_lines = selection.decorate_visible_lines(
            sidebar,
            vec![Line::raw("file one"), Line::raw("file two")],
            0,
            2,
            theme,
        );
        let diff_lines = selection.decorate_visible_lines(
            diff,
            vec![Line::raw("diff one"), Line::raw("diff two")],
            0,
            2,
            theme,
        );

        assert!(highlighted_text(&sidebar_lines).is_empty());
        assert_eq!(highlighted_text(&diff_lines), "onediff");
        assert!(selection.finish_drag(15, 1));
        assert_eq!(
            selection.take_clipboard_request().as_deref(),
            Some("one\ndiff")
        );
    }

    fn highlighted_text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .filter(|span| span.style.bg.is_some())
            .map(|span| span.content.as_ref())
            .collect()
    }
}
