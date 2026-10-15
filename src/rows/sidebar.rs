use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::model::{DiffFile, FileStage, FileStatus};
use crate::theme::Theme;

use super::file_summary::{
    file_icon, file_icon_color, format_file_stats, push_stat_spans, reviewed_glyph,
    sidebar_file_label, stats_width,
};
use super::text::{color_style, display_width, muted_line};

const TREE_INDENT: &str = "  ";

struct FileLabelLayout {
    label: String,
    padding: String,
}

pub(crate) fn render_sidebar_empty(message: &'static str, theme: Theme) -> Line<'static> {
    muted_line(message, theme)
}

pub(crate) fn render_sidebar_file(
    file: &DiffFile,
    selected: bool,
    content_width: usize,
    reviewed: bool,
    depth: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let row_background = selection_background(selected, theme);
    let label_style = color_style(
        if reviewed { theme.muted } else { theme.text },
        row_background,
    );
    let file_name_style = if reviewed {
        label_style
    } else {
        sidebar_file_name_style(file, row_background, theme)
    };
    let icon = if reviewed {
        reviewed_glyph()
    } else {
        file_icon(file.display_path())
    };
    let icon_style = if reviewed {
        color_style(theme.added, row_background)
    } else {
        color_style(file_icon_color(file.display_path(), theme), row_background)
    };
    let tree_prefix = tree_file_prefix(depth);
    let label = sidebar_file_label(file);
    let stats = format_file_stats(file);
    let layout = file_label_layout(
        &tree_prefix,
        icon,
        &label,
        stats_width(&stats),
        content_width,
    );

    let mut content_spans = Vec::new();
    if !tree_prefix.is_empty() {
        content_spans.push(Span::styled(tree_prefix, label_style));
    }
    content_spans.extend([
        Span::styled(icon, icon_style),
        Span::styled(" ", label_style),
        Span::styled(layout.label, file_name_style),
        Span::styled(layout.padding, label_style),
    ]);
    push_stat_spans(&mut content_spans, file, row_background, theme);

    vec![fill_selected_row(
        Line::from(content_spans),
        selected,
        content_width,
        theme,
    )]
}

fn file_label_layout(
    tree_prefix: &str,
    icon: &str,
    label: &str,
    stats_width: usize,
    content_width: usize,
) -> FileLabelLayout {
    let reserved_stats_width = stats_width
        .saturating_add(usize::from(stats_width > 0))
        .min(content_width);
    let leading_width = combined_width(&[tree_prefix, icon, " "]);
    let label_width = content_width
        .saturating_sub(leading_width)
        .saturating_sub(reserved_stats_width);
    let label = truncate_to_width(label, label_width);
    let used_width = leading_width
        .saturating_add(display_width(&label))
        .saturating_add(stats_width);
    let padding = if stats_width == 0 {
        String::new()
    } else {
        " ".repeat(content_width.saturating_sub(used_width))
    };

    FileLabelLayout { label, padding }
}

fn sidebar_file_name_style(file: &DiffFile, row_background: Color, theme: Theme) -> Style {
    let (color, is_bold) = match file.stage {
        FileStage::Staged => (theme.added, true),
        FileStage::Mixed => (theme.accent, true),
        FileStage::Unstaged | FileStage::Committed => match file.status {
            FileStatus::Added => (theme.added, false),
            FileStatus::Deleted => (theme.removed, false),
            FileStatus::Renamed | FileStatus::Copied => (theme.file_renamed, false),
            FileStatus::Modified => (theme.text, false),
        },
    };

    let style = color_style(color, row_background);
    if is_bold {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_sidebar_folder(
    name: &str,
    depth: usize,
    expanded: bool,
    selected: bool,
    stage: FileStage,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let row_background = selection_background(selected, theme);
    let label_style = color_style(theme.text, row_background);
    let folder_name_style = match stage {
        FileStage::Staged => color_style(theme.added, row_background).add_modifier(Modifier::BOLD),
        FileStage::Mixed => color_style(theme.accent, row_background).add_modifier(Modifier::BOLD),
        FileStage::Unstaged | FileStage::Committed => label_style,
    };
    let muted_style = color_style(theme.muted, row_background);
    let folder_icon = if expanded { "\u{e5fe}" } else { "\u{e5ff}" };
    let chevron = if expanded { "▾" } else { "▸" };
    let tree_prefix = tree_folder_prefix(depth);
    let decoration_width = combined_width(&[&tree_prefix, chevron, " ", folder_icon, " "]);
    let folder_label = if name == "/" {
        name.to_string()
    } else {
        format!("{name}/")
    };
    let label = truncate_to_width(
        &folder_label,
        content_width.saturating_sub(decoration_width),
    );
    let content_spans = vec![
        Span::styled(tree_prefix, muted_style),
        Span::styled(chevron, color_style(theme.accent, row_background)),
        Span::styled(" ", muted_style),
        Span::styled(folder_icon, color_style(theme.file_renamed, row_background)),
        Span::styled(" ", muted_style),
        Span::styled(label, folder_name_style),
    ];

    vec![fill_selected_row(
        Line::from(content_spans),
        selected,
        content_width,
        theme,
    )]
}

fn selection_background(selected: bool, theme: Theme) -> Color {
    if selected {
        theme.selected
    } else {
        theme.background
    }
}

fn combined_width(parts: &[&str]) -> usize {
    parts
        .iter()
        .fold(0, |width, part| width.saturating_add(display_width(part)))
}

fn fill_selected_row(
    mut line: Line<'static>,
    selected: bool,
    content_width: usize,
    theme: Theme,
) -> Line<'static> {
    if selected {
        let padding = content_width.saturating_sub(line.width());
        line.spans.push(Span::styled(
            " ".repeat(padding),
            color_style(theme.text, theme.selected),
        ));
    }
    line
}

fn tree_file_prefix(depth: usize) -> String {
    format!("{}  ", TREE_INDENT.repeat(depth))
}

fn tree_folder_prefix(depth: usize) -> String {
    TREE_INDENT.repeat(depth)
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    if display_width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }

    let ellipsis = "…";
    let ellipsis_width = display_width(ellipsis);
    if max_width <= ellipsis_width {
        return ellipsis.to_string();
    }

    let available = max_width - ellipsis_width;
    let mut truncated = String::new();
    let mut width = 0;
    for character in text.chars() {
        let character = character.to_string();
        let character_width = display_width(&character);
        if width + character_width > available {
            break;
        }

        truncated.push_str(&character);
        width += character_width;
    }
    truncated.push_str(ellipsis);
    truncated
}

#[cfg(test)]
mod tests {
    use crate::model::SourceSnapshot;

    use super::*;

    #[test]
    fn file_row_keeps_stats_within_the_requested_width() {
        let mut file = diff_file("extremely_long_component_name.rs");
        file.additions = 42;
        file.deletions = 7;

        let rows = render_sidebar_file(&file, true, 18, false, 1, Theme::github_dark());

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].width(), 18);
        assert!(row_text(&rows[0]).contains("+42"));
        assert!(row_text(&rows[0]).contains("-7"));
    }

    #[test]
    fn reviewed_file_uses_review_glyph_and_muted_label() {
        let theme = Theme::github_dark();
        let rows = render_sidebar_file(&diff_file("example.rs"), false, 30, true, 0, theme);

        assert!(row_text(&rows[0]).contains(reviewed_glyph()));
        let label = rows[0]
            .spans
            .iter()
            .find(|span| span.content.contains("example.rs"))
            .expect("file label should be rendered");
        assert_eq!(label.style.fg, Some(theme.muted));
    }

    fn diff_file(path: &str) -> DiffFile {
        DiffFile {
            id: path.to_string(),
            old_path: path.to_string(),
            path: path.to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions: 0,
            deletions: 0,
            hunks: Vec::new(),
            binary: false,
        }
    }

    fn row_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }
}
