//! Literal diff search: prompt input, match finding, navigation, and highlight.
//!
//! `App` owns scroll and viewport state and feeds rendered lines in; this module
//! owns the query lifecycle, match coordinates, and the styled-span surgery that
//! highlights matches. The interface speaks in rendered `Line`s, scroll targets,
//! and render status, never exposing match coordinates to callers.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::accepts_text_input;
use crate::rows;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SearchMatch {
    row: usize,
    start: usize,
    end: usize,
}

#[derive(Debug, Default)]
pub(crate) struct Search {
    prompt_open: bool,
    input: String,
    query: String,
    matches: Vec<SearchMatch>,
    active_index: Option<usize>,
    match_file_id: Option<String>,
    scroll_pending: bool,
}

impl Search {
    pub(crate) fn active_query(&self) -> Option<&str> {
        (!self.query.is_empty()).then_some(self.query.as_str())
    }

    pub(crate) fn is_prompt_open(&self) -> bool {
        self.prompt_open
    }

    pub(crate) fn status(&self) -> Option<rows::SearchStatus<'_>> {
        if self.prompt_open {
            return Some(rows::SearchStatus::Prompt { input: &self.input });
        }

        self.active_query().map(|query| rows::SearchStatus::Active {
            query,
            active: self.active_index.map(|index| index + 1),
            total: self.matches.len(),
        })
    }

    pub(crate) fn open_prompt(&mut self) {
        self.input.clone_from(&self.query);
        self.prompt_open = true;
    }

    pub(crate) fn handle_prompt_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.clear_query(),
            KeyCode::Enter => self.apply_prompt(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(value) if accepts_text_input(key) => self.input.push(value),
            _ => {}
        }
    }

    pub(crate) fn clear_query(&mut self) {
        self.prompt_open = false;
        self.query.clear();
        self.input.clear();
        self.clear_rendered_matches();
        self.scroll_pending = false;
    }

    pub(crate) fn clear_rendered_matches(&mut self) {
        self.matches.clear();
        self.active_index = None;
        self.match_file_id = None;
    }

    pub(crate) fn invalidate_matches(&mut self) {
        self.clear_rendered_matches();
        if self.active_query().is_some() {
            self.scroll_pending = true;
        }
    }

    pub(crate) fn refresh_matches(&mut self, file_id: &str, lines: &[Line<'static>]) -> bool {
        let Some(query) = self.active_query() else {
            self.clear_rendered_matches();
            return false;
        };
        let previous_active_index = if self.match_file_id.as_deref() == Some(file_id) {
            self.active_index
        } else {
            None
        };

        self.matches = diff_search_matches(lines, query);
        self.match_file_id = Some(file_id.to_string());
        self.active_index = (!self.matches.is_empty()).then(|| {
            previous_active_index
                .unwrap_or(0)
                .min(self.matches.len() - 1)
        });

        let should_scroll = self.scroll_pending && self.active_index.is_some();
        self.scroll_pending = false;
        should_scroll
    }

    pub(crate) fn advance_match(&mut self, forward: bool) -> bool {
        if self.matches.is_empty() {
            self.active_index = None;
            return false;
        }

        let current = self.active_index.unwrap_or(0).min(self.matches.len() - 1);
        self.active_index = Some(if forward {
            (current + 1) % self.matches.len()
        } else {
            current.checked_sub(1).unwrap_or(self.matches.len() - 1)
        });

        self.active_match().is_some()
    }

    pub(crate) fn active_match_row(&self) -> Option<usize> {
        self.active_match().map(|search_match| search_match.row)
    }

    pub(crate) fn highlight(
        &self,
        lines: Vec<Line<'static>>,
        diff_scroll: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        if self.active_query().is_none() || self.matches.is_empty() {
            return lines;
        }

        lines
            .into_iter()
            .enumerate()
            .map(|(visible_index, line)| {
                let row = diff_scroll.saturating_add(visible_index);
                let matches = self.matches_on_row(row);
                highlight_line_search_matches(line, &matches, self.active_index, theme)
            })
            .collect()
    }

    fn apply_prompt(&mut self) {
        self.prompt_open = false;
        if self.input.is_empty() {
            self.clear_query();
            return;
        }

        self.query.clone_from(&self.input);
        self.invalidate_matches();
    }

    fn active_match(&self) -> Option<SearchMatch> {
        self.active_index
            .and_then(|index| self.matches.get(index).copied())
    }

    fn matches_on_row(&self, row: usize) -> Vec<(usize, SearchMatch)> {
        self.matches
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, search_match)| search_match.row == row)
            .collect()
    }
}

#[cfg(test)]
impl Search {
    pub(crate) fn match_count(&self) -> usize {
        self.matches.len()
    }

    pub(crate) fn active_index(&self) -> Option<usize> {
        self.active_index
    }
}

fn diff_search_matches(lines: &[Line<'static>], query: &str) -> Vec<SearchMatch> {
    lines
        .iter()
        .enumerate()
        .flat_map(|(row, line)| {
            search_matches_in_text(&line_text(line), query)
                .into_iter()
                .map(move |(start, end)| SearchMatch { row, start, end })
        })
        .collect()
}

fn search_matches_in_text(text: &str, query: &str) -> Vec<(usize, usize)> {
    let haystack: Vec<char> = text.chars().collect();
    let needle: Vec<char> = query.chars().collect();
    let needle_len = needle.len();
    if needle_len == 0 || haystack.len() < needle_len {
        return Vec::new();
    }

    (0..=haystack.len() - needle_len)
        .filter(|start| {
            haystack[*start..*start + needle_len]
                .iter()
                .zip(&needle)
                .all(|(left, right)| search_chars_match(*left, *right))
        })
        .map(|start| (start, start + needle_len))
        .collect()
}

fn search_chars_match(left: char, right: char) -> bool {
    left == right || (left.is_ascii() && right.is_ascii() && left.eq_ignore_ascii_case(&right))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchMatchKind {
    Active,
    Inactive,
}

#[derive(Debug, Clone, Copy)]
struct SearchStyledChar {
    value: char,
    style: Style,
}

fn highlight_line_search_matches(
    line: Line<'static>,
    matches: &[(usize, SearchMatch)],
    active_index: Option<usize>,
    theme: Theme,
) -> Line<'static> {
    if matches.is_empty() {
        return line;
    }

    let style = line.style;
    let alignment = line.alignment;
    let chars = line_search_chars(line.spans);
    let spans = chars_to_search_spans(chars.into_iter().enumerate().map(|(index, character)| {
        SearchStyledChar {
            value: character.value,
            style: search_style_for_char(index, character.style, matches, active_index, theme),
        }
    }));

    Line {
        spans,
        style,
        alignment,
    }
}

fn line_search_chars(spans: Vec<Span<'static>>) -> Vec<SearchStyledChar> {
    let mut chars = Vec::new();

    for span in spans {
        let style = span.style;
        for value in span.content.chars() {
            chars.push(SearchStyledChar { value, style });
        }
    }

    chars
}

fn search_style_for_char(
    index: usize,
    base_style: Style,
    matches: &[(usize, SearchMatch)],
    active_index: Option<usize>,
    theme: Theme,
) -> Style {
    match search_match_kind_at(index, matches, active_index) {
        Some(SearchMatchKind::Active) => base_style
            .fg(theme.on_accent)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD),
        Some(SearchMatchKind::Inactive) => {
            base_style.bg(theme.selected).add_modifier(Modifier::BOLD)
        }
        None => base_style,
    }
}

fn search_match_kind_at(
    index: usize,
    matches: &[(usize, SearchMatch)],
    active_index: Option<usize>,
) -> Option<SearchMatchKind> {
    let mut found_inactive_match = false;

    for (match_index, search_match) in matches {
        if !search_match_contains(*search_match, index) {
            continue;
        }

        if Some(*match_index) == active_index {
            return Some(SearchMatchKind::Active);
        }

        found_inactive_match = true;
    }

    found_inactive_match.then_some(SearchMatchKind::Inactive)
}

fn search_match_contains(search_match: SearchMatch, index: usize) -> bool {
    index >= search_match.start && index < search_match.end
}

fn chars_to_search_spans(chars: impl IntoIterator<Item = SearchStyledChar>) -> Vec<Span<'static>> {
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

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}
