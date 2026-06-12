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

/// Nerd Font octicon glyph conveying the Git status of a file.
pub(super) fn status_glyph(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Added => "\u{f457}",    // diff-added
        FileStatus::Deleted => "\u{f458}",  // diff-removed
        FileStatus::Modified => "\u{f459}", // diff-modified
        FileStatus::Renamed => "\u{f45a}",  // diff-renamed
        FileStatus::Copied => "\u{f0c5}",   // copy
    }
}

/// Nerd Font glyph hinting at a file's language or type from its path.
pub(super) fn file_icon(path: &str) -> &'static str {
    let name = basename(path);
    let lowercased = name.to_ascii_lowercase();
    if lowercased.ends_with(".lock") || lowercased == "cargo.lock" {
        return "\u{f023}"; // lock
    }

    let extension = lowercased.rsplit_once('.').map(|(_, ext)| ext);
    match extension {
        Some("rs") => "\u{e7a8}",
        Some("ts") => "\u{e628}",
        Some("tsx" | "jsx") => "\u{e7ba}",
        Some("js" | "mjs" | "cjs") => "\u{e74e}",
        Some("json") => "\u{e60b}",
        Some("py") => "\u{e606}",
        Some("go") => "\u{e627}",
        Some("rb") => "\u{e21e}",
        Some("java") => "\u{e256}",
        Some("c") => "\u{e61e}",
        Some("h" | "hpp" | "hh") => "\u{f0fd}",
        Some("cpp" | "cc" | "cxx") => "\u{e61d}",
        Some("cs") => "\u{e648}",
        Some("md" | "markdown") => "\u{e609}",
        Some("html" | "htm") => "\u{e736}",
        Some("css") => "\u{e749}",
        Some("scss" | "sass") => "\u{e603}",
        Some("vue") => "\u{fd42}",
        Some("sh" | "bash" | "zsh") => "\u{e795}",
        Some("toml" | "ini" | "cfg" | "conf") => "\u{f013}",
        Some("yaml" | "yml") => "\u{e615}",
        Some("txt") => "\u{f15c}",
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp") => "\u{f1c5}",
        _ => "\u{f15b}", // generic file
    }
}

pub(super) fn stage_display(stage: FileStage, background: Color, theme: Theme) -> StageDisplay {
    match stage {
        FileStage::Unstaged => StageDisplay {
            checkbox: "\u{f10c}",
            suffix: "  \u{f10c} unstaged",
            style: color_style(theme.muted, background),
        },
        FileStage::Staged => StageDisplay {
            checkbox: "\u{f058}",
            suffix: "  \u{f058} staged",
            style: color_style(theme.added, background).add_modifier(Modifier::BOLD),
        },
        FileStage::Mixed => StageDisplay {
            checkbox: "\u{f056}",
            suffix: "  \u{f056} mixed",
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
