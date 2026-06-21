use std::collections::{BTreeMap, HashSet};

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::model::{DiffFile, FileStage};
use crate::theme::Theme;

use super::file_summary::{
    file_icon, format_file_stats, push_stat_spans, reviewed_glyph, sidebar_file_label, stats_width,
    status_color,
};
use super::text::{color_style, display_width, muted_line};

const TREE_INDENT: &str = "  ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidebarRowTarget {
    File(usize),
    Folder(String),
}

pub(crate) struct SidebarRowsInput<'a> {
    pub(crate) files: &'a [DiffFile],
    pub(crate) empty_message: &'static str,
    pub(crate) collapsed_dirs: &'a HashSet<String>,
    pub(crate) selected_file_index: usize,
    pub(crate) sidebar_scroll: usize,
    pub(crate) row_counts: &'a [usize],
    pub(crate) content_width: usize,
    pub(crate) visible_height: usize,
    pub(crate) theme: Theme,
    pub(crate) reviewed_files: &'a HashSet<String>,
    pub(crate) active_target: Option<&'a SidebarRowTarget>,
}

pub(crate) struct SidebarRowCountsInput<'a> {
    pub(crate) files: &'a [DiffFile],
    pub(crate) collapsed_dirs: &'a HashSet<String>,
    pub(crate) content_width: usize,
    pub(crate) theme: Theme,
}

pub(crate) struct RenderedSidebarRows {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) row_records: Vec<SidebarRowRecord>,
    pub(crate) sidebar_scroll: usize,
}

pub(crate) struct SidebarRowRecord {
    pub(crate) target: SidebarRowTarget,
    pub(crate) row_count: usize,
}

struct SidebarRenderContext<'a> {
    files: &'a [DiffFile],
    content_width: usize,
    reviewed_files: &'a HashSet<String>,
    active_target: SidebarRowTarget,
    theme: Theme,
}

struct FileEntryRenderInput<'a> {
    file: &'a DiffFile,
    is_selected: bool,
    content_width: usize,
    is_reviewed: bool,
    depth: usize,
    theme: Theme,
}

pub(crate) fn sidebar_rows(input: SidebarRowsInput<'_>) -> RenderedSidebarRows {
    if input.files.is_empty() {
        return RenderedSidebarRows {
            lines: vec![muted_line(input.empty_message, input.theme)],
            row_records: Vec::new(),
            sidebar_scroll: 0,
        };
    }

    let entries = sidebar_entries(input.files, input.collapsed_dirs);
    let fallback_target =
        SidebarRowTarget::File(input.selected_file_index.min(input.files.len() - 1));
    let active_target = input.active_target.cloned().unwrap_or(fallback_target);
    let sidebar_scroll = visible_sidebar_scroll(
        &entries,
        input.row_counts,
        input.sidebar_scroll,
        &active_target,
        input.visible_height,
    );
    let render_context = SidebarRenderContext {
        files: input.files,
        content_width: input.content_width,
        reviewed_files: input.reviewed_files,
        active_target,
        theme: input.theme,
    };

    let mut lines = Vec::new();
    let mut row_records = Vec::new();
    for entry in entries.iter().skip(sidebar_scroll) {
        let remaining_height = input.visible_height.saturating_sub(lines.len());
        if remaining_height == 0 {
            break;
        }

        let entry_lines = render_sidebar_entry(entry, &render_context);
        let visible_rows = entry_lines.len().min(remaining_height);

        row_records.push(SidebarRowRecord {
            target: entry.target(),
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

pub(crate) fn sidebar_row_counts(input: SidebarRowCountsInput<'_>) -> Vec<usize> {
    let entries = sidebar_entries(input.files, input.collapsed_dirs);
    entries
        .iter()
        .map(|entry| match entry {
            SidebarEntry::File { index, depth } => {
                let Some(file) = input.files.get(*index) else {
                    return 1;
                };
                // Review state swaps the file-type icon for a check glyph of
                // the same width, so row counts are unaffected; pass `false`
                // to compute counts against the unreviewed layout.
                render_file_entry(FileEntryRenderInput {
                    file,
                    is_selected: false,
                    content_width: input.content_width,
                    is_reviewed: false,
                    depth: *depth,
                    theme: input.theme,
                })
                .len()
            }
            SidebarEntry::Folder {
                name,
                depth,
                expanded,
                ..
            } => render_folder_entry(
                name,
                *depth,
                *expanded,
                false,
                FileStage::Unstaged,
                input.content_width,
                input.theme,
            )
            .len(),
        })
        .collect()
}

#[cfg(test)]
fn visible_file_indices(files: &[DiffFile], collapsed_dirs: &HashSet<String>) -> Vec<usize> {
    visible_sidebar_targets(files, collapsed_dirs)
        .into_iter()
        .filter_map(|target| match target {
            SidebarRowTarget::File(index) => Some(index),
            SidebarRowTarget::Folder(_) => None,
        })
        .collect()
}

pub(crate) fn visible_sidebar_targets(
    files: &[DiffFile],
    collapsed_dirs: &HashSet<String>,
) -> Vec<SidebarRowTarget> {
    sidebar_entries(files, collapsed_dirs)
        .into_iter()
        .map(|entry| entry.target())
        .collect()
}

fn visible_sidebar_scroll(
    entries: &[SidebarEntry],
    row_counts: &[usize],
    sidebar_scroll: usize,
    active_target: &SidebarRowTarget,
    visible_height: usize,
) -> usize {
    if entries.is_empty() {
        return 0;
    }

    let clamped_scroll = sidebar_scroll.min(entries.len() - 1);
    let Some(selected_entry_index) = selected_sidebar_entry_index(entries, active_target) else {
        return clamped_scroll;
    };

    if selected_entry_index < clamped_scroll {
        return selected_entry_index;
    }

    if sidebar_selection_visible(
        row_counts,
        clamped_scroll,
        selected_entry_index,
        visible_height,
    ) {
        return clamped_scroll;
    }

    sidebar_scroll_for_selected(row_counts, selected_entry_index, visible_height)
}

fn selected_sidebar_entry_index(
    entries: &[SidebarEntry],
    active_target: &SidebarRowTarget,
) -> Option<usize> {
    entries
        .iter()
        .position(|entry| entry.matches_target(active_target))
}

fn sidebar_selection_visible(
    row_counts: &[usize],
    scroll: usize,
    selected_index: usize,
    visible_height: usize,
) -> bool {
    if selected_index < scroll {
        return false;
    }
    if scroll >= row_counts.len() || selected_index >= row_counts.len() {
        return false;
    }

    let Some(selected_row_count) = row_counts.get(selected_index).copied() else {
        return false;
    };

    let visible_height = visible_height.max(1);
    let rows_above_selection: usize = row_counts[scroll..selected_index].iter().sum();
    if rows_above_selection == 0 {
        return true;
    }

    if rows_above_selection >= visible_height {
        return false;
    }

    rows_above_selection + selected_row_count <= visible_height
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum SidebarEntry {
    Folder {
        path: String,
        name: String,
        depth: usize,
        expanded: bool,
    },
    File {
        index: usize,
        depth: usize,
    },
}

impl SidebarEntry {
    fn target(&self) -> SidebarRowTarget {
        match self {
            Self::Folder { path, .. } => SidebarRowTarget::Folder(path.clone()),
            Self::File { index, .. } => SidebarRowTarget::File(*index),
        }
    }

    fn matches_target(&self, target: &SidebarRowTarget) -> bool {
        match (self, target) {
            (Self::Folder { path, .. }, SidebarRowTarget::Folder(target_path)) => {
                path == target_path
            }
            (Self::File { index, .. }, SidebarRowTarget::File(target_index)) => {
                index == target_index
            }
            _ => false,
        }
    }
}

#[derive(Debug, Default)]
struct TreeNode {
    dirs: BTreeMap<String, TreeNode>,
    files: Vec<TreeFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeFile {
    name: String,
    index: usize,
}

impl TreeNode {
    fn insert_file(&mut self, path: &str, index: usize) {
        let parts = path
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let Some((file_name, dirs)) = parts.split_last() else {
            self.files.push(TreeFile {
                name: path.to_string(),
                index,
            });
            return;
        };

        let mut node = self;
        for dir in dirs {
            node = node.dirs.entry((*dir).to_string()).or_default();
        }
        node.files.push(TreeFile {
            name: (*file_name).to_string(),
            index,
        });
    }

    fn flatten(
        &self,
        prefix: &str,
        depth: usize,
        collapsed_dirs: &HashSet<String>,
        entries: &mut Vec<SidebarEntry>,
    ) {
        for (name, node) in &self.dirs {
            let path = child_path(prefix, name);
            let expanded = !collapsed_dirs.contains(&path);
            entries.push(SidebarEntry::Folder {
                path: path.clone(),
                name: name.clone(),
                depth,
                expanded,
            });
            if expanded {
                node.flatten(&path, depth + 1, collapsed_dirs, entries);
            }
        }

        let mut files = self.files.iter().collect::<Vec<_>>();
        files.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.index.cmp(&right.index))
        });
        entries.extend(files.into_iter().map(|file| SidebarEntry::File {
            index: file.index,
            depth,
        }));
    }
}

fn sidebar_entries(files: &[DiffFile], collapsed_dirs: &HashSet<String>) -> Vec<SidebarEntry> {
    tree_sidebar_entries(files, collapsed_dirs)
}

fn tree_sidebar_entries(files: &[DiffFile], collapsed_dirs: &HashSet<String>) -> Vec<SidebarEntry> {
    let mut root = TreeNode::default();
    for (index, file) in files.iter().enumerate() {
        root.insert_file(file.display_path(), index);
    }

    let mut entries = Vec::new();
    root.flatten("", 0, collapsed_dirs, &mut entries);
    entries
}

fn child_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

fn render_sidebar_entry(
    entry: &SidebarEntry,
    context: &SidebarRenderContext<'_>,
) -> Vec<Line<'static>> {
    match entry {
        SidebarEntry::File { index, depth } => {
            let Some(file) = context.files.get(*index) else {
                return Vec::new();
            };
            let is_reviewed = context.reviewed_files.contains(file.display_path());
            let is_selected = matches!(
                &context.active_target,
                SidebarRowTarget::File(active_index) if active_index == index
            );
            render_file_entry(FileEntryRenderInput {
                file,
                is_selected,
                content_width: context.content_width,
                is_reviewed,
                depth: *depth,
                theme: context.theme,
            })
        }
        SidebarEntry::Folder {
            path,
            name,
            depth,
            expanded,
        } => {
            let folder_stage = folder_stage(context.files, path);
            let is_selected = matches!(
                &context.active_target,
                SidebarRowTarget::Folder(active_path) if active_path == path
            );
            render_folder_entry(
                name,
                *depth,
                *expanded,
                is_selected
                    || active_file_hidden_by_folder(
                        context.files,
                        &context.active_target,
                        path,
                        *expanded,
                    ),
                folder_stage,
                context.content_width,
                context.theme,
            )
        }
    }
}

fn active_file_hidden_by_folder(
    files: &[DiffFile],
    active_target: &SidebarRowTarget,
    folder_path: &str,
    expanded: bool,
) -> bool {
    if expanded {
        return false;
    }

    let SidebarRowTarget::File(active_index) = active_target else {
        return false;
    };

    files
        .get(*active_index)
        .map(|file| path_is_inside_folder(file.display_path(), folder_path))
        .unwrap_or(false)
}

fn path_is_inside_folder(path: &str, folder_path: &str) -> bool {
    path.strip_prefix(folder_path)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn render_file_entry(input: FileEntryRenderInput<'_>) -> Vec<Line<'static>> {
    let FileEntryRenderInput {
        file,
        is_selected,
        content_width,
        is_reviewed,
        depth,
        theme,
    } = input;
    let row_background = if is_selected {
        theme.selected
    } else {
        theme.background
    };
    let base_style = color_style(theme.text, row_background);
    let label_style = if is_reviewed {
        color_style(theme.muted, row_background)
    } else {
        base_style
    };
    let file_name_style = if is_reviewed {
        label_style
    } else {
        match file.stage {
            FileStage::Staged => {
                color_style(theme.added, row_background).add_modifier(Modifier::BOLD)
            }
            FileStage::Mixed => {
                color_style(theme.accent, row_background).add_modifier(Modifier::BOLD)
            }
            FileStage::Unstaged => label_style,
        }
    };
    let icon = if is_reviewed {
        reviewed_glyph()
    } else {
        file_icon(file.display_path())
    };
    let icon_style = if is_reviewed {
        color_style(theme.added, row_background)
    } else {
        color_style(status_color(file.status, theme), row_background)
    };
    let tree_prefix = tree_file_prefix(depth);
    let file_name = sidebar_file_label(file);
    let stats = format_file_stats(file);
    let stats_width = stats_width(&stats);
    let content_capacity = content_width;
    let reserved_stats_width = stats_width
        .saturating_add(usize::from(stats_width > 0))
        .min(content_capacity);
    let fixed_content_width = display_width(&tree_prefix)
        .saturating_add(display_width(icon))
        .saturating_add(1)
        .saturating_add(reserved_stats_width);
    let file_name = truncate_to_width(
        &file_name,
        content_capacity.saturating_sub(fixed_content_width),
    );
    let used_without_padding = display_width(&tree_prefix)
        .saturating_add(display_width(icon))
        .saturating_add(1)
        .saturating_add(display_width(&file_name))
        .saturating_add(stats_width);
    let padding = if stats_width == 0 {
        String::new()
    } else {
        " ".repeat(content_capacity.saturating_sub(used_without_padding))
    };

    let mut content_spans = Vec::new();
    if !tree_prefix.is_empty() {
        content_spans.push(Span::styled(tree_prefix, label_style));
    }
    content_spans.extend([
        Span::styled(icon, icon_style),
        Span::styled(" ", label_style),
        Span::styled(file_name, file_name_style),
        Span::styled(padding, label_style),
    ]);
    push_stat_spans(&mut content_spans, file, row_background, theme);

    vec![Line::from(content_spans)]
}

fn render_folder_entry(
    name: &str,
    depth: usize,
    expanded: bool,
    contains_hidden_selection: bool,
    stage: FileStage,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let row_background = if contains_hidden_selection {
        theme.selected
    } else {
        theme.background
    };
    let label_style = color_style(theme.text, row_background);
    let folder_name_style = match stage {
        FileStage::Staged => color_style(theme.added, row_background).add_modifier(Modifier::BOLD),
        FileStage::Mixed => color_style(theme.accent, row_background).add_modifier(Modifier::BOLD),
        FileStage::Unstaged => label_style,
    };
    let muted_style = color_style(theme.muted, row_background);
    let folder_icon = if expanded { "\u{e5fe}" } else { "\u{e5ff}" };
    let chevron = if expanded { "▾" } else { "▸" };
    let content_capacity = content_width;
    let tree_prefix = tree_folder_prefix(depth);
    let fixed_content_width = display_width(&tree_prefix)
        .saturating_add(display_width(chevron))
        .saturating_add(1)
        .saturating_add(display_width(folder_icon))
        .saturating_add(1);
    let label = truncate_to_width(
        &format!("{name}/"),
        content_capacity.saturating_sub(fixed_content_width),
    );
    let content_spans = vec![
        Span::styled(tree_prefix, muted_style),
        Span::styled(chevron, color_style(theme.accent, row_background)),
        Span::styled(" ", muted_style),
        Span::styled(folder_icon, color_style(theme.file_renamed, row_background)),
        Span::styled(" ", muted_style),
        Span::styled(label, folder_name_style),
    ];

    vec![Line::from(content_spans)]
}

fn folder_stage(files: &[DiffFile], folder_path: &str) -> FileStage {
    let mut matching_stages = files
        .iter()
        .filter(|file| path_is_inside_folder(file.display_path(), folder_path))
        .map(|file| file.stage);
    let Some(first_stage) = matching_stages.next() else {
        return FileStage::Unstaged;
    };

    matching_stages.fold(first_stage, |combined, stage| {
        if combined == stage && stage != FileStage::Mixed {
            combined
        } else {
            FileStage::Mixed
        }
    })
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
    use std::collections::HashSet;

    use ratatui::text::Line;

    use crate::model::{
        DiffFile, DiffHunk, DiffLine, DiffLineKind, FileStage, FileStatus, SourceSnapshot,
    };

    use super::*;

    #[test]
    fn long_sidebar_entries_truncate_to_one_row() {
        let files = vec![diff_file_with_path("extremely_long_file_name_component.rs")];
        let content_width = 16;
        let collapsed_dirs = HashSet::new();
        let row_counts = test_row_counts(&files, content_width, &collapsed_dirs);
        let rows = sidebar_rows(SidebarRowsInput {
            files: &files,
            empty_message: "No tracked changes",
            collapsed_dirs: &collapsed_dirs,
            selected_file_index: 0,
            sidebar_scroll: 0,
            row_counts: &row_counts,
            content_width,
            visible_height: 8,
            theme: Theme::github_dark(),
            reviewed_files: &HashSet::new(),
            active_target: None,
        });

        assert_eq!(rows.lines.len(), 1);
        assert!(rows.lines.iter().all(|line| line.width() <= content_width));
        assert_eq!(rows.row_records.len(), 1);
        assert_eq!(rows.row_records[0].target, SidebarRowTarget::File(0));
        assert_eq!(rows.row_records[0].row_count, 1);
    }

    #[test]
    fn sidebar_scroll_keeps_selected_entry_visible() {
        let files = vec![
            diff_file_with_path("first_extremely_long_file_name.rs"),
            diff_file_with_path("second_extremely_long_file_name.rs"),
        ];
        let collapsed_dirs = HashSet::new();
        let row_counts = test_row_counts(&files, 14, &collapsed_dirs);
        let rows = sidebar_rows(SidebarRowsInput {
            files: &files,
            empty_message: "No tracked changes",
            collapsed_dirs: &collapsed_dirs,
            selected_file_index: 1,
            sidebar_scroll: 0,
            row_counts: &row_counts,
            content_width: 14,
            visible_height: 1,
            theme: Theme::github_dark(),
            reviewed_files: &HashSet::new(),
            active_target: None,
        });

        assert_eq!(rows.sidebar_scroll, 1);
        assert_eq!(rows.lines.len(), 1);
        assert_eq!(rows.row_records.len(), 1);
        assert_eq!(rows.row_records[0].target, SidebarRowTarget::File(1));
        assert_eq!(rows.row_records[0].row_count, 1);
    }

    #[test]
    fn file_stats_stay_on_same_row_when_label_must_shrink() {
        let mut file = diff_file_with_path("src/rows/sidebar.rs");
        file.additions = 588;
        file.deletions = 63;
        let rows = render_file_entry(FileEntryRenderInput {
            file: &file,
            is_selected: true,
            content_width: 32,
            is_reviewed: false,
            depth: 2,
            theme: Theme::github_dark(),
        });
        let text = line_text(&rows[0]);

        assert_eq!(rows.len(), 1);
        assert!(rows[0].width() <= 32, "row was: {text}");
        assert!(text.contains("+588"), "row was: {text}");
        assert!(text.contains("-63"), "row was: {text}");
    }

    #[test]
    fn review_mode_omits_sidebar_staging_affordances() {
        let file = diff_file_with_path("src/main.rs");
        let sidebar = render_test_file_entry(&file, 80, false, Theme::github_dark());

        assert!(!line_text(&sidebar[0]).contains("[ ]"));
    }

    #[test]
    fn reviewed_files_render_with_check_glyph_and_muted_label() {
        let file = diff_file_with_path("src/main.rs");
        let theme = Theme::github_dark();
        let sidebar = render_test_file_entry(&file, 80, true, theme);

        let check_glyph = reviewed_glyph();
        let name_span = sidebar[0]
            .spans
            .iter()
            .find(|span| span.content.contains("main.rs"))
            .expect("file name span should render");
        let icon_span = sidebar[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == check_glyph)
            .expect("reviewed check glyph should render");

        assert_eq!(name_span.style.fg, Some(theme.muted));
        assert_eq!(icon_span.style.fg, Some(theme.added));
    }

    #[test]
    fn unreviewed_files_keep_file_icon_and_text_color() {
        let file = diff_file_with_path("src/main.rs");
        let theme = Theme::github_dark();
        let sidebar = render_test_file_entry(&file, 80, false, theme);

        let check_glyph = reviewed_glyph();
        assert!(
            !sidebar[0]
                .spans
                .iter()
                .any(|span| span.content.as_ref() == check_glyph)
        );

        let name_span = sidebar[0]
            .spans
            .iter()
            .find(|span| span.content.contains("main.rs"))
            .expect("file name span should render");
        assert_eq!(name_span.style.fg, Some(theme.text));
    }

    #[test]
    fn staged_files_render_green_name_without_stage_marker() {
        let mut file = diff_file_with_path("src/main.rs");
        file.stage = FileStage::Staged;
        let theme = Theme::github_dark();
        let sidebar = render_test_file_entry(&file, 80, false, theme);
        let text = line_text(&sidebar[0]);

        assert!(!text.contains("\u{f058}"), "row was: {text}");
        assert!(!text.contains("\u{f10c}"), "row was: {text}");

        let name_span = sidebar[0]
            .spans
            .iter()
            .find(|span| span.content.contains("main.rs"))
            .expect("file name span should render");
        assert_eq!(name_span.style.fg, Some(theme.added));
    }

    #[test]
    fn reviewed_row_count_matches_unreviewed_row_count() {
        let file = diff_file_with_path("src/components/extremely_long_file_name_component.rs");
        let content_width = 16;
        let theme = Theme::github_dark();
        let unreviewed = render_test_file_entry(&file, content_width, false, theme).len();
        let reviewed = render_test_file_entry(&file, content_width, true, theme).len();

        assert_eq!(unreviewed, reviewed);
    }

    #[test]
    fn tree_rows_group_paths_and_record_click_targets() {
        let files = files_with_paths([
            "src/app.rs",
            "src/bin/chunk.rs",
            "src/config.rs",
            "README.md",
        ]);
        let collapsed_dirs = HashSet::new();
        let row_counts = test_row_counts(&files, 80, &collapsed_dirs);
        let rows = sidebar_rows(SidebarRowsInput {
            files: &files,
            empty_message: "No tracked changes",
            collapsed_dirs: &collapsed_dirs,
            selected_file_index: 1,
            sidebar_scroll: 0,
            row_counts: &row_counts,
            content_width: 80,
            visible_height: 8,
            theme: Theme::github_dark(),
            reviewed_files: &HashSet::new(),
            active_target: None,
        });

        let texts = rows.lines.iter().map(line_text).collect::<Vec<_>>();
        assert!(texts[0].starts_with("▾ "), "rows were: {texts:?}");
        assert!(texts[0].contains("src/"), "rows were: {texts:?}");
        assert!(texts[1].contains("bin/"), "rows were: {texts:?}");
        assert!(texts[2].contains("chunk.rs"), "rows were: {texts:?}");
        assert!(texts[3].contains("app.rs"), "rows were: {texts:?}");
        assert!(texts[4].contains("config.rs"), "rows were: {texts:?}");
        assert!(texts[5].contains("README.md"), "rows were: {texts:?}");

        assert_eq!(
            rows.row_records
                .iter()
                .map(|record| record.target.clone())
                .collect::<Vec<_>>(),
            vec![
                SidebarRowTarget::Folder("src".to_string()),
                SidebarRowTarget::Folder("src/bin".to_string()),
                SidebarRowTarget::File(1),
                SidebarRowTarget::File(0),
                SidebarRowTarget::File(2),
                SidebarRowTarget::File(3),
            ]
        );
    }

    #[test]
    fn active_folder_row_uses_selected_background() {
        let files = files_with_paths([
            "src/app.rs",
            "src/bin/chunk.rs",
            "src/config.rs",
            "README.md",
        ]);
        let collapsed_dirs = HashSet::new();
        let active_target = SidebarRowTarget::Folder("src".to_string());
        let theme = Theme::github_dark();
        let row_counts = test_row_counts(&files, 80, &collapsed_dirs);
        let rows = sidebar_rows(SidebarRowsInput {
            files: &files,
            empty_message: "No tracked changes",
            collapsed_dirs: &collapsed_dirs,
            selected_file_index: 3,
            sidebar_scroll: 0,
            row_counts: &row_counts,
            content_width: 80,
            visible_height: 8,
            theme,
            reviewed_files: &HashSet::new(),
            active_target: Some(&active_target),
        });

        assert!(
            rows.lines[0]
                .spans
                .iter()
                .all(|span| span.style.bg == Some(theme.selected))
        );
        assert!(
            rows.lines[1]
                .spans
                .iter()
                .all(|span| span.style.bg != Some(theme.selected))
        );
    }

    #[test]
    fn staged_folder_row_uses_staged_name_color() {
        let mut files = files_with_paths(["src/app.rs", "src/config.rs", "README.md"]);
        files[0].stage = FileStage::Staged;
        files[1].stage = FileStage::Staged;
        let collapsed_dirs = HashSet::new();
        let theme = Theme::github_dark();
        let row_counts = test_row_counts(&files, 80, &collapsed_dirs);
        let rows = sidebar_rows(SidebarRowsInput {
            files: &files,
            empty_message: "No tracked changes",
            collapsed_dirs: &collapsed_dirs,
            selected_file_index: 2,
            sidebar_scroll: 0,
            row_counts: &row_counts,
            content_width: 80,
            visible_height: 8,
            theme,
            reviewed_files: &HashSet::new(),
            active_target: None,
        });

        let folder_label = rows.lines[0].spans.last().expect("folder label span");
        assert_eq!(folder_label.content.as_ref(), "src/");
        assert_eq!(folder_label.style.fg, Some(theme.added));
        assert!(folder_label.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn mixed_folder_row_uses_mixed_name_color() {
        let mut files = files_with_paths(["src/app.rs", "src/config.rs", "README.md"]);
        files[0].stage = FileStage::Staged;
        let collapsed_dirs = HashSet::new();
        let theme = Theme::github_dark();
        let row_counts = test_row_counts(&files, 80, &collapsed_dirs);
        let rows = sidebar_rows(SidebarRowsInput {
            files: &files,
            empty_message: "No tracked changes",
            collapsed_dirs: &collapsed_dirs,
            selected_file_index: 2,
            sidebar_scroll: 0,
            row_counts: &row_counts,
            content_width: 80,
            visible_height: 8,
            theme,
            reviewed_files: &HashSet::new(),
            active_target: None,
        });

        let folder_label = rows.lines[0].spans.last().expect("folder label span");
        assert_eq!(folder_label.content.as_ref(), "src/");
        assert_eq!(folder_label.style.fg, Some(theme.accent));
        assert!(folder_label.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn collapsed_tree_rows_hide_descendants() {
        let files = files_with_paths([
            "src/app.rs",
            "src/bin/chunk.rs",
            "src/config.rs",
            "README.md",
        ]);
        let collapsed_dirs = HashSet::from(["src/bin".to_string()]);
        let row_counts = test_row_counts(&files, 80, &collapsed_dirs);
        let rows = sidebar_rows(SidebarRowsInput {
            files: &files,
            empty_message: "No tracked changes",
            collapsed_dirs: &collapsed_dirs,
            selected_file_index: 1,
            sidebar_scroll: 0,
            row_counts: &row_counts,
            content_width: 80,
            visible_height: 8,
            theme: Theme::github_dark(),
            reviewed_files: &HashSet::new(),
            active_target: None,
        });
        let joined = rows
            .lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("bin/"));
        assert!(!joined.contains("chunk.rs"));
        assert_eq!(visible_file_indices(&files, &collapsed_dirs), vec![0, 2, 3]);
        assert_eq!(
            rows.row_records
                .iter()
                .map(|record| record.target.clone())
                .collect::<Vec<_>>(),
            vec![
                SidebarRowTarget::Folder("src".to_string()),
                SidebarRowTarget::Folder("src/bin".to_string()),
                SidebarRowTarget::File(0),
                SidebarRowTarget::File(2),
                SidebarRowTarget::File(3),
            ]
        );
    }

    fn test_row_counts(
        files: &[DiffFile],
        content_width: usize,
        collapsed_dirs: &HashSet<String>,
    ) -> Vec<usize> {
        sidebar_row_counts(SidebarRowCountsInput {
            files,
            collapsed_dirs,
            content_width,
            theme: Theme::github_dark(),
        })
    }

    fn files_with_paths<const N: usize>(paths: [&str; N]) -> Vec<DiffFile> {
        paths.into_iter().map(diff_file_with_path).collect()
    }

    fn render_test_file_entry(
        file: &DiffFile,
        content_width: usize,
        is_reviewed: bool,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        render_file_entry(FileEntryRenderInput {
            file,
            is_selected: true,
            content_width,
            is_reviewed,
            depth: 0,
            theme,
        })
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
