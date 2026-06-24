use crate::model::{Changeset, DiffFile, DiffHunk};
use crate::rows::SidebarRowTarget;

use super::{App, overlay::Overlay};

pub(super) fn apply_changeset(app: &mut App, changeset: Changeset, preserve_scroll: bool) {
    let previous_identity = app.selected_file().map(file_identity);
    let previous_hunk_identity = app.selected_hunk().map(hunk_identity);
    let previous_hunk_index = app.diff_pane.selected_hunk_index();
    let previous_sidebar_target = app.sidebar_cursor_target.clone();
    let previous_index = app.selected_file_index;
    let previous_scroll = app.diff_pane.scroll();
    let reselected_file_index = previous_identity
        .as_deref()
        .and_then(|identity| find_file_index(&changeset, identity));
    let fallback_index = previous_index.min(changeset.files.len().saturating_sub(1));
    let kept_selection = reselected_file_index.is_some();
    let selected_file_index = reselected_file_index.unwrap_or(fallback_index);

    app.changeset = changeset;
    app.live_error = None;
    if matches!(app.overlay, Some(Overlay::Discard(_))) {
        app.overlay = None;
    }
    app.text_selection.clear();
    app.selected_file_index = selected_file_index;
    app.sidebar_cursor_target = reloaded_sidebar_target(previous_sidebar_target, &app.changeset);
    app.diff_pane.set_selected_hunk_index(reloaded_hunk_index(
        app.changeset.files.get(selected_file_index),
        kept_selection,
        previous_hunk_identity,
        previous_hunk_index,
    ));
    app.diff_pane
        .set_scroll(if preserve_scroll && kept_selection {
            previous_scroll
        } else {
            0
        });
    app.clear_render_caches();
    app.invalidate_search_matches();
    app.ensure_scroll_bounds();
}

fn file_identity(file: &DiffFile) -> String {
    file.display_path().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HunkIdentity {
    old_start: u32,
    old_lines: u32,
    new_start: u32,
    new_lines: u32,
}

fn hunk_identity(hunk: &DiffHunk) -> HunkIdentity {
    HunkIdentity {
        old_start: hunk.old_start,
        old_lines: hunk.old_lines,
        new_start: hunk.new_start,
        new_lines: hunk.new_lines,
    }
}

fn reloaded_hunk_index(
    file: Option<&DiffFile>,
    kept_file_selection: bool,
    previous_identity: Option<HunkIdentity>,
    previous_index: Option<usize>,
) -> Option<usize> {
    let file = file?;
    if file.hunks.is_empty() {
        return None;
    }

    if !kept_file_selection {
        return Some(0);
    }

    if let Some(index) = previous_identity.and_then(|identity| find_hunk_index(file, identity)) {
        return Some(index);
    }

    if let Some(index) = previous_index {
        return Some(index.min(file.hunks.len() - 1));
    }

    Some(0)
}

fn find_hunk_index(file: &DiffFile, identity: HunkIdentity) -> Option<usize> {
    file.hunks
        .iter()
        .position(|hunk| hunk_identity(hunk) == identity)
}

fn find_file_index(changeset: &Changeset, identity: &str) -> Option<usize> {
    changeset
        .files
        .iter()
        .position(|file| file.display_path() == identity)
}

fn reloaded_sidebar_target(
    previous_target: Option<SidebarRowTarget>,
    changeset: &Changeset,
) -> Option<SidebarRowTarget> {
    match previous_target {
        Some(SidebarRowTarget::Folder(path)) if folder_exists(changeset, &path) => {
            Some(SidebarRowTarget::Folder(path))
        }
        _ => None,
    }
}

fn folder_exists(changeset: &Changeset, folder_path: &str) -> bool {
    changeset
        .files
        .iter()
        .any(|file| path_is_inside_folder(file.display_path(), folder_path))
}

fn path_is_inside_folder(path: &str, folder_path: &str) -> bool {
    path.strip_prefix(folder_path)
        .is_some_and(|suffix| suffix.starts_with('/'))
}
