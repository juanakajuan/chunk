use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

use super::text::{color_style, wrap_line};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ListState {
    next: Option<u64>,
}

#[derive(Debug)]
struct MarkdownRenderer {
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<ListState>,
    quote_depth: usize,
    continuation_prefix: Option<String>,
    in_code_block: bool,
    first_table_cell: bool,
    content_width: usize,
    theme: Theme,
}

pub(super) fn markdown_lines(
    markdown: &str,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    if markdown.is_empty() {
        return wrap_line(
            Line::styled("(empty)", color_style(theme.muted, theme.background)),
            content_width,
        );
    }

    let mut options = Options::empty();
    options.insert(Options::ENABLE_GFM);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_STRIKETHROUGH);

    let mut renderer = MarkdownRenderer::new(content_width, theme);
    for event in Parser::new_ext(markdown, options) {
        renderer.handle_event(event);
    }
    renderer.finish()
}

impl MarkdownRenderer {
    fn new(content_width: usize, theme: Theme) -> Self {
        Self {
            lines: Vec::new(),
            current: Vec::new(),
            styles: vec![color_style(theme.text, theme.background)],
            lists: Vec::new(),
            quote_depth: 0,
            continuation_prefix: None,
            in_code_block: false,
            first_table_cell: true,
            content_width,
            theme,
        }
    }

    fn handle_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.append_text(text.as_ref()),
            Event::Code(code) => self.append_inline_code(code.as_ref()),
            Event::InlineMath(math) => self.append_inline_code(math.as_ref()),
            Event::DisplayMath(math) => self.append_code_text(math.as_ref()),
            Event::Html(html) | Event::InlineHtml(html) => self.append_text(html.as_ref()),
            Event::FootnoteReference(label) => self.append_text(&format!("[^{label}]")),
            Event::SoftBreak => self.append_text(" "),
            Event::HardBreak => self.flush_current(),
            Event::Rule => self.push_rule(),
            Event::TaskListMarker(checked) => {
                self.append_styled_text(
                    if checked { "[x] " } else { "[ ] " },
                    self.list_marker_style(),
                );
            }
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => self.start_heading(level),
            Tag::BlockQuote(_) => self.quote_depth = self.quote_depth.saturating_add(1),
            Tag::CodeBlock(kind) => self.start_code_block(kind),
            Tag::List(start) => self.lists.push(ListState { next: start }),
            Tag::Item => self.start_list_item(),
            Tag::Emphasis => self.push_style(self.current_style().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(self.current_style().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.push_style(self.current_style().add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { .. } => self.push_style(
                color_style(self.theme.syntax.link, self.theme.background)
                    .add_modifier(Modifier::UNDERLINED),
            ),
            Tag::Image { dest_url, .. } => {
                self.append_styled_text("[image: ", self.muted_style());
                self.append_styled_text(dest_url.as_ref(), self.link_style());
                self.append_styled_text("] ", self.muted_style());
                self.push_style(self.current_style());
            }
            Tag::Table(_) | Tag::TableHead | Tag::TableRow => {
                self.first_table_cell = true;
            }
            Tag::TableCell => self.start_table_cell(),
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => {
                self.flush_current();
                self.push_blank();
                if matches!(tag, TagEnd::Heading(_)) {
                    self.pop_style();
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush_current();
                self.quote_depth = self.quote_depth.saturating_sub(1);
                if self.quote_depth == 0 {
                    self.push_blank();
                }
            }
            TagEnd::CodeBlock => {
                self.in_code_block = false;
                self.push_blank();
            }
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.push_blank();
                }
            }
            TagEnd::Item => {
                self.flush_current();
                self.continuation_prefix = None;
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image => self.pop_style(),
            TagEnd::TableRow => self.flush_current(),
            TagEnd::TableCell => {}
            TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::MetadataBlock(_) => {}
        }
    }

    fn start_heading(&mut self, level: HeadingLevel) {
        self.ensure_line_prefix();
        let marker = format!("{} ", "#".repeat(heading_depth(level)));
        self.current
            .push(Span::styled(marker, self.heading_marker_style()));
        self.push_style(self.heading_text_style());
    }

    fn start_code_block(&mut self, kind: CodeBlockKind<'_>) {
        self.flush_current();
        self.in_code_block = true;
        if let CodeBlockKind::Fenced(language) = kind
            && !language.is_empty()
        {
            self.push_wrapped_line(Line::styled(
                format!("```{language}"),
                self.code_fence_style(),
            ));
        }
    }

    fn start_list_item(&mut self) {
        let marker = self.next_list_marker();
        let prefix_len = marker.chars().count();
        self.ensure_line_prefix();
        self.current
            .push(Span::styled(marker, self.list_marker_style()));
        self.continuation_prefix = Some(" ".repeat(prefix_len));
    }

    fn start_table_cell(&mut self) {
        if self.first_table_cell {
            self.first_table_cell = false;
        } else {
            self.append_styled_text(" | ", self.muted_style());
        }
    }

    fn append_text(&mut self, text: &str) {
        if self.in_code_block {
            self.append_code_text(text);
            return;
        }

        self.ensure_line_prefix();
        self.current
            .push(Span::styled(text.to_string(), self.current_style()));
    }

    fn append_inline_code(&mut self, text: &str) {
        self.ensure_line_prefix();
        self.current
            .push(Span::styled(text.to_string(), self.inline_code_style()));
    }

    fn append_code_text(&mut self, text: &str) {
        for row in text.split('\n') {
            if row.is_empty() {
                self.lines.push(Line::raw(""));
            } else {
                self.push_wrapped_line(Line::styled(format!("  {row}"), self.code_block_style()));
            }
        }
    }

    fn append_styled_text(&mut self, text: &str, style: Style) {
        self.ensure_line_prefix();
        self.current.push(Span::styled(text.to_string(), style));
    }

    fn ensure_line_prefix(&mut self) {
        if !self.current.is_empty() {
            return;
        }

        for _ in 0..self.quote_depth {
            self.current.push(Span::styled("│ ", self.quote_style()));
        }
        if let Some(prefix) = &self.continuation_prefix {
            self.current
                .push(Span::styled(prefix.clone(), self.muted_style()));
        }
    }

    fn flush_current(&mut self) {
        if self.current.is_empty() {
            return;
        }

        let spans = std::mem::take(&mut self.current);
        self.push_wrapped_line(Line::from(spans));
    }

    fn push_rule(&mut self) {
        self.flush_current();
        self.push_wrapped_line(Line::styled(
            "─".repeat(self.content_width.max(1)),
            color_style(self.theme.border, self.theme.background),
        ));
        self.push_blank();
    }

    fn push_blank(&mut self) {
        if !self.lines.last().is_some_and(is_blank_line) {
            self.lines.push(Line::raw(""));
        }
    }

    fn push_wrapped_line(&mut self, line: Line<'static>) {
        self.lines.extend(wrap_line(line, self.content_width));
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_current();
        while self.lines.last().is_some_and(is_blank_line) {
            self.lines.pop();
        }
        if self.lines.is_empty() {
            self.lines.push(Line::styled(
                "(empty)",
                color_style(self.theme.muted, self.theme.background),
            ));
        }
        self.lines
    }

    fn next_list_marker(&mut self) -> String {
        let depth = self.lists.len().saturating_sub(1);
        let indent = "  ".repeat(depth);
        let Some(list) = self.lists.last_mut() else {
            return format!("{indent}• ");
        };

        match list.next {
            Some(next) => {
                list.next = Some(next.saturating_add(1));
                format!("{indent}{next}. ")
            }
            None => format!("{indent}• "),
        }
    }

    fn push_style(&mut self, style: Style) {
        self.styles.push(style);
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn current_style(&self) -> Style {
        self.styles
            .last()
            .copied()
            .unwrap_or_else(|| color_style(self.theme.text, self.theme.background))
    }

    fn heading_marker_style(&self) -> Style {
        color_style(self.theme.accent, self.theme.background).add_modifier(Modifier::BOLD)
    }

    fn heading_text_style(&self) -> Style {
        color_style(self.theme.text, self.theme.background).add_modifier(Modifier::BOLD)
    }

    fn list_marker_style(&self) -> Style {
        color_style(self.theme.syntax.list_marker, self.theme.background)
            .add_modifier(Modifier::BOLD)
    }

    fn muted_style(&self) -> Style {
        color_style(self.theme.muted, self.theme.background)
    }

    fn quote_style(&self) -> Style {
        color_style(self.theme.muted, self.theme.background)
    }

    fn link_style(&self) -> Style {
        color_style(self.theme.syntax.link, self.theme.background)
            .add_modifier(Modifier::UNDERLINED)
    }

    fn inline_code_style(&self) -> Style {
        color_style(self.theme.syntax.string, self.theme.background_alt)
    }

    fn code_block_style(&self) -> Style {
        color_style(self.theme.syntax.string, self.theme.background_alt)
    }

    fn code_fence_style(&self) -> Style {
        color_style(self.theme.muted, self.theme.background)
    }
}

fn heading_depth(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn is_blank_line(line: &Line<'_>) -> bool {
    line.spans
        .iter()
        .all(|span| span.content.as_ref().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_renders_common_answer_blocks() {
        let lines = markdown_lines(
            "# Summary\n\n- **Safe** change\n- Use `chunk`\n\n```rust\nfn main() {}\n```\n\n> note\n",
            80,
            Theme::github_dark(),
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("# Summary"));
        assert!(text.contains("• Safe change"));
        assert!(text.contains("• Use chunk"));
        assert!(text.contains("```rust"));
        assert!(text.contains("  fn main() {}"));
        assert!(text.contains("│ note"));
        assert!(lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.style.add_modifier.contains(Modifier::BOLD))
        }));
    }

    #[test]
    fn markdown_wraps_long_styled_lines() {
        let lines = markdown_lines("**abcdef** ghijkl", 8, Theme::github_dark());

        assert_eq!(
            lines.iter().map(line_text).collect::<Vec<_>>(),
            ["abcdef ", "ghijkl"]
        );
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }
}
