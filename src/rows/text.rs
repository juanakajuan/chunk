use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

pub(super) fn muted_line(text: impl Into<String>, theme: Theme) -> Line<'static> {
    Line::styled(text.into(), color_style(theme.muted, theme.background))
}

pub(super) fn wrap_line(line: Line<'static>, content_width: usize) -> Vec<Line<'static>> {
    let style = line.style;
    let alignment = line.alignment;
    wrap_styled_spans(line.spans, content_width.max(1))
        .into_iter()
        .map(|spans| Line {
            style,
            alignment,
            spans,
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct StyledChar {
    pub(super) value: char,
    pub(super) style: Style,
    width: usize,
}

pub(super) fn wrap_styled_spans(
    spans: Vec<Span<'static>>,
    max_width: usize,
) -> Vec<Vec<Span<'static>>> {
    let max_width = max_width.max(1);
    let chars = styled_chars(spans);
    if chars.is_empty() {
        return vec![Vec::new()];
    }

    let mut rows = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = wrapped_row_end(&chars, start, max_width);
        rows.push(chars_to_spans(&chars[start..end]));
        start = end;
    }

    rows
}

pub(super) fn styled_chars(spans: Vec<Span<'static>>) -> Vec<StyledChar> {
    let mut chars = Vec::new();
    for span in spans {
        chars.extend(span.content.chars().map(|value| StyledChar {
            value,
            style: span.style,
            width: char_display_width(value),
        }));
    }
    chars
}

fn wrapped_row_end(chars: &[StyledChar], start: usize, max_width: usize) -> usize {
    let mut width = 0;
    let mut index = start;
    let mut last_break = None;

    while index < chars.len() {
        let char_width = chars[index].width;
        if width > 0 && width + char_width > max_width {
            break;
        }
        if width == 0 && char_width > max_width {
            return index + 1;
        }

        width += char_width;
        index += 1;

        if chars[index - 1].value.is_whitespace() {
            last_break = Some(index);
        }

        if width >= max_width {
            break;
        }
    }

    if index >= chars.len() {
        return chars.len();
    }

    last_break
        .filter(|break_index| *break_index > start)
        .unwrap_or(index.max(start + 1))
}

pub(super) fn chars_to_spans(chars: &[StyledChar]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();

    for character in chars {
        match spans.last_mut() {
            Some(span) if span.style == character.style => {
                span.content.to_mut().push(character.value);
            }
            _ => spans.push(Span::styled(character.value.to_string(), character.style)),
        }
    }

    spans
}

fn char_display_width(value: char) -> usize {
    Span::raw(value.to_string()).width()
}

pub(super) fn expand_tabs(text: &str) -> String {
    text.replace('\t', "  ")
}

pub(super) fn display_width(text: &str) -> usize {
    Span::raw(text.to_string()).width()
}

pub(super) fn color_style(foreground: Color, background: Color) -> Style {
    Style::default().fg(foreground).bg(background)
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Style};
    use ratatui::text::Span;

    use super::*;

    #[test]
    fn wrapped_spans_preserve_styles_across_rows() {
        let red = Style::default().fg(Color::Red);
        let blue = Style::default().fg(Color::Blue);
        let rows = wrap_styled_spans(
            vec![Span::styled("abcdef", red), Span::styled("gh", blue)],
            4,
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(spans_text(&rows[0]), "abcd");
        assert_eq!(spans_text(&rows[1]), "efgh");
        assert_eq!(rows[1][0].style, red);
        assert_eq!(rows[1][1].style, blue);
    }

    fn spans_text(spans: &[Span<'_>]) -> String {
        spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }
}
