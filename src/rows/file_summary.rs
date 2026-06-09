use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::model::{DiffFile, FileStage, FileStatus};
use crate::theme::Theme;

use super::text::{color_style, display_width};

pub(super) struct StageDisplay {
    pub(super) checkbox: &'static str,
    pub(super) suffix: &'static str,
    pub(super) style: Style,
}

pub(super) fn push_stat_spans(
    spans: &mut Vec<Span<'static>>,
    file: &DiffFile,
    background: Color,
    theme: Theme,
) {
    let has_additions = file.additions > 0;
    let has_deletions = file.deletions > 0;

    if has_additions {
        spans.push(Span::styled(
            format!("+{}", file.additions),
            color_style(theme.added, background),
        ));
    }

    if has_additions && has_deletions {
        spans.push(Span::styled(" ", Style::default().bg(background)));
    }

    if has_deletions {
        spans.push(Span::styled(
            format!("-{}", file.deletions),
            color_style(theme.removed, background),
        ));
    }
}

pub(super) fn file_header_label(file: &DiffFile) -> String {
    if has_path_change_label(file) {
        format!("{} -> {}", file.old_path, file.path)
    } else {
        file.display_path().to_string()
    }
}

pub(super) fn sidebar_file_label(file: &DiffFile) -> String {
    if has_path_change_label(file) {
        format!("{} -> {}", basename(&file.old_path), basename(&file.path))
    } else {
        basename(file.display_path()).to_string()
    }
}

fn has_path_change_label(file: &DiffFile) -> bool {
    matches!(file.status, FileStatus::Renamed | FileStatus::Copied) && file.old_path != file.path
}

pub(super) fn file_status_suffix(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Added => " (new)",
        FileStatus::Deleted => " (deleted)",
        FileStatus::Copied => " (copied)",
        FileStatus::Renamed | FileStatus::Modified => "",
    }
}

pub(super) fn stage_display(stage: FileStage, background: Color, theme: Theme) -> StageDisplay {
    match stage {
        FileStage::Unstaged => StageDisplay {
            checkbox: "[ ]",
            suffix: " [unstaged]",
            style: color_style(theme.muted, background),
        },
        FileStage::Staged => StageDisplay {
            checkbox: "[x]",
            suffix: " [staged]",
            style: color_style(theme.added, background).add_modifier(Modifier::BOLD),
        },
        FileStage::Mixed => StageDisplay {
            checkbox: "[-]",
            suffix: " [mixed]",
            style: color_style(theme.accent, background).add_modifier(Modifier::BOLD),
        },
    }
}

pub(super) fn status_color(status: FileStatus, theme: Theme) -> Color {
    match status {
        FileStatus::Added => theme.file_new,
        FileStatus::Deleted => theme.file_deleted,
        FileStatus::Modified => theme.file_modified,
        FileStatus::Renamed | FileStatus::Copied => theme.file_renamed,
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub(super) fn format_file_stats(file: &DiffFile) -> String {
    match (file.additions, file.deletions) {
        (0, 0) => String::new(),
        (additions, 0) => format!("+{additions}"),
        (0, deletions) => format!("-{deletions}"),
        (additions, deletions) => format!("+{additions} -{deletions}"),
    }
}

pub(super) fn padding_before_stats(
    content_width: usize,
    used_width: usize,
    stats_width: usize,
) -> String {
    if stats_width == 0 {
        String::new()
    } else {
        " ".repeat(content_width.saturating_sub(used_width).max(1))
    }
}

pub(super) fn stats_width(stats: &str) -> usize {
    display_width(stats)
}
