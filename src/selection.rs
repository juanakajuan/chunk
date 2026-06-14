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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionPoint {
    column: u16,
    row: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionDrag {
    surface: Rect,
    anchor: SelectionPoint,
    cursor: SelectionPoint,
    moved: bool,
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

        let visible_height = visible_height.min(area.height as usize);
        self.record_visible_lines(area, &lines, line_scroll, visible_height);
        self.highlight_visible_lines(area, lines, line_scroll, visible_height, theme)
    }

    pub(crate) fn begin_drag(&mut self, column: u16, row: u16) -> bool {
        let Some((surface, point)) = self.selectable_point_at(column, row) else {
            self.drag = None;
            self.selected = None;
            return false;
        };

        self.drag = Some(SelectionDrag {
            surface,
            anchor: point,
            cursor: point,
            moved: false,
        });
        self.selected = None;
        true
    }

    pub(crate) fn update_drag(&mut self, column: u16, row: u16) -> bool {
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };

        let cursor = SelectionPoint { column, row };
        drag.moved |= cursor != drag.anchor;
        drag.cursor = cursor;
        true
    }

    pub(crate) fn finish_drag(&mut self, column: u16, row: u16) -> bool {
        let Some(mut drag) = self.drag.take() else {
            return false;
        };

        let cursor = SelectionPoint { column, row };
        drag.moved |= cursor != drag.anchor;
        drag.cursor = cursor;
        if !drag.moved {
            self.selected = None;
            return false;
        }

        let range = SelectionRange::between(drag.surface, drag.anchor, drag.cursor);
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

            *line = highlight_line_selection(line.clone(), columns, theme);
        }

        lines
    }

    fn active_range(&self) -> Option<SelectionRange> {
        self.drag
            .filter(|drag| drag.moved)
            .map(|drag| SelectionRange::between(drag.surface, drag.anchor, drag.cursor))
            .or(self.selected)
    }

    fn selectable_point_at(&self, column: u16, row: u16) -> Option<(Rect, SelectionPoint)> {
        self.lines
            .iter()
            .rev()
            .find(|line| line.contains(column, row))
            .map(|line| {
                (
                    line.surface,
                    SelectionPoint {
                        column: column.max(line.start_col),
                        row,
                    },
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

        for line in self.visible_lines_for_row(row, range) {
            let columns = range.columns_for_line(row, line)?;
            if let Some(previous_end) = last_end_col {
                let gap = line.start_col.saturating_sub(previous_end) as usize;
                text.extend(std::iter::repeat_n(' ', gap));
            }

            text.push_str(&line.slice(columns));
            last_end_col = Some(columns.end_col);
        }

        last_end_col.is_some().then_some(text)
    }

    fn visible_lines_for_row(&self, row: u16, range: SelectionRange) -> Vec<&SelectableLine> {
        let mut lines = self
            .lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                if line.surface != range.surface || line.row != row {
                    return None;
                }

                range
                    .columns_for_line(row, line)
                    .map(|columns| (index, line, columns))
            })
            .filter(|(index, line, columns)| !self.has_later_overlap(*index, line, range, *columns))
            .map(|(_, line, _)| line)
            .collect::<Vec<_>>();
        lines.sort_by_key(|line| line.start_col);
        lines
    }

    fn has_later_overlap(
        &self,
        index: usize,
        line: &SelectableLine,
        range: SelectionRange,
        columns: SelectedColumns,
    ) -> bool {
        self.lines
            .iter()
            .skip(index + 1)
            .filter(|later| later.surface == line.surface && later.row == line.row)
            .any(|later| {
                range
                    .columns_for_line(later.row, later)
                    .is_some_and(|later_columns| columns.overlaps(later_columns))
            })
    }
}

impl SelectableLine {
    fn contains(&self, column: u16, row: u16) -> bool {
        row == self.row
            && column >= self.start_col
            && column < self.start_col.saturating_add(self.width)
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

    fn overlaps_display_range(self, start_col: usize, end_col: usize) -> bool {
        start_col < usize::from(self.end_col) && end_col > usize::from(self.start_col)
    }
}

impl SelectionRange {
    fn between(surface: Rect, left: SelectionPoint, right: SelectionPoint) -> Self {
        if (left.row, left.column) <= (right.row, right.column) {
            Self {
                surface,
                start: left,
                end: right,
            }
        } else {
            Self {
                surface,
                start: right,
                end: left,
            }
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

fn highlight_line_selection(
    line: Line<'static>,
    columns: SelectedColumns,
    theme: Theme,
) -> Line<'static> {
    let style = line.style;
    let alignment = line.alignment;
    let mut display_col = 0;
    let mut spans = Vec::new();

    for span in line.spans {
        for value in span.content.chars() {
            let char_start = display_col;
            let char_end = display_col + char_display_width(value).max(1);
            display_col = char_end;

            let style = if columns.overlaps_display_range(char_start, char_end) {
                span.style
                    .bg(theme.accent)
                    .fg(theme.on_accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                span.style
            };
            push_styled_char(&mut spans, value, style);
        }
    }

    Line {
        spans,
        style,
        alignment,
    }
}

fn text_by_display_columns(text: &str, columns: SelectedColumns) -> String {
    let mut output = String::new();
    let mut display_col = 0;

    for character in text.chars() {
        let width = char_display_width(character).max(1);
        let char_start = display_col;
        let char_end = display_col + width;
        display_col = char_end;

        if char_start >= usize::from(columns.end_col) {
            break;
        }

        if columns.overlaps_display_range(char_start, char_end) {
            output.push(character);
        }
    }

    output
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
    value.min(u16::MAX as usize) as u16
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
