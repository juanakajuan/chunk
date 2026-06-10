//! Rendered terminal rows for sidebar, diff content, and status bars.
//!
//! This module is the row-rendering interface. Submodules own focused pieces of
//! the implementation: sidebar rows, diff rows, intraline emphasis, and wrapping.
//! `ui` owns pane layout and Ratatui widget drawing.

use ratatui::style::Style;
use ratatui::text::Line;

use crate::model::Changeset;
use crate::theme::Theme;

mod diff;
mod file_summary;
mod intraline;
mod sidebar;
mod text;

pub(crate) use diff::{diff_lines_until, hunk_offsets};
pub(crate) use sidebar::SidebarRowsInput;
pub(crate) use sidebar::{sidebar_row_counts, sidebar_rows};

use text::{color_style, muted_line, wrap_line};

pub(crate) const DIFF_PREFETCH_ROWS: usize = 120;
const RAIL_MARKER: &str = "▌";

pub(crate) fn no_diff_lines(
    message: &'static str,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    wrap_line(muted_line(message, theme), content_width)
}

pub(crate) fn live_status_lines(
    error: Option<&str>,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let Some(error) = error else {
        return Vec::new();
    };

    wrap_line(
        Line::styled(
            format!("! {error}"),
            color_style(theme.removed, theme.background),
        ),
        content_width,
    )
}

pub(crate) fn keybind_bar_line(
    files_panel_visible: bool,
    stage_hint: Option<&'static str>,
    theme: Theme,
) -> Line<'static> {
    let mut hints = vec![if files_panel_visible {
        "[f] hide files"
    } else {
        "[f] show files"
    }];

    if files_panel_visible {
        hints.push("[Tab] switch focus");
    }
    if let Some(stage_hint) = stage_hint {
        hints.push(stage_hint);
    }
    hints.push("[j/k] move");
    hints.push("[Ctrl-d/u] scroll");
    hints.push("[n/N] hunk");
    hints.push("[e] edit");
    hints.push("[q] quit");

    Line::styled(hints.join("  |  "), Style::default().fg(theme.muted))
}

pub(crate) fn changeset_title(changeset: &Changeset) -> String {
    let additions: usize = changeset.files.iter().map(|file| file.additions).sum();
    let deletions: usize = changeset.files.iter().map(|file| file.deletions).sum();
    let title = if changeset.title.is_empty() {
        "changeset"
    } else {
        &changeset.title
    };

    format!("{}  +{}  -{}", title, additions, deletions)
}
