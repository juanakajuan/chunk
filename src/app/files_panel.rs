//! Stateful changed-files tree.
//!
//! The module owns file selection, tree topology, folder membership, collapse
//! state, cursor navigation, scroll, review marks, reload survival, and the
//! rendered row map used for pointer input. `App` supplies files and translates
//! selected folders into existing worktree mutations.

use std::collections::{BTreeMap, HashMap, HashSet};

use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::model::{DiffFile, FileStage};
use crate::rows;
use crate::scroll_text::VerticalDirection;
use crate::theme::Theme;

#[derive(Debug)]
pub(super) struct FilesPanel {
    tree: FileTree,
    entries: Vec<TreeEntry>,
    collapsed_folders: HashSet<String>,
    selected_file_index: usize,
    cursor: Option<TreeTarget>,
    scroll: usize,
    reviewed_files: HashSet<String>,
    viewport: FilesPanelViewport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FilesPanelTarget(TreeTarget);

#[derive(Debug, Clone, PartialEq, Eq)]
enum TreeTarget {
    File(usize),
    Folder(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TreeEntry {
    Folder {
        path: String,
        name: String,
        depth: usize,
        expanded: bool,
        file_indices: Vec<usize>,
    },
    File {
        index: usize,
        depth: usize,
    },
}

#[derive(Debug, Default)]
struct FileTree {
    root: TreeNode,
    folder_members: HashMap<String, Vec<usize>>,
    has_root_folder: bool,
}

#[derive(Debug, Default)]
struct TreeNode {
    dirs: BTreeMap<String, TreeNode>,
    files: Vec<TreeFile>,
}

#[derive(Debug)]
struct TreeFile {
    name: String,
    index: usize,
}

#[derive(Debug)]
struct FilesPanelViewport {
    area: Option<Rect>,
    row_targets: Vec<FilesPanelTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FocusedFolder {
    pub(super) path: String,
    pub(super) file_indices: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FilesReload {
    pub(super) kept_file_selection: bool,
}

pub(super) struct FilesPanelRender<'a> {
    pub(super) area: Rect,
    pub(super) files: &'a [DiffFile],
    pub(super) empty_message: &'static str,
    pub(super) content_width: usize,
    pub(super) visible_height: usize,
    pub(super) theme: Theme,
    pub(super) focused: bool,
}

struct RenderedTreeEntry {
    target: FilesPanelTarget,
    lines: Vec<Line<'static>>,
}

impl FilesPanel {
    pub(super) fn new(files: &[DiffFile]) -> Self {
        let tree = FileTree::new(files);
        let collapsed_folders = HashSet::new();
        let entries = tree.entries(&collapsed_folders);
        let cursor = entries.first().map(TreeEntry::target);
        Self {
            tree,
            entries,
            collapsed_folders,
            selected_file_index: 0,
            cursor,
            scroll: 0,
            reviewed_files: HashSet::new(),
            viewport: FilesPanelViewport::new(),
        }
    }

    pub(super) fn invalidate_frame(&mut self) {
        self.viewport.invalidate_frame();
    }

    pub(super) fn selected_file_index(&self) -> usize {
        self.selected_file_index
    }

    pub(super) fn render(&mut self, request: FilesPanelRender<'_>) -> Vec<Line<'static>> {
        self.viewport.begin(request.area);
        if request.files.is_empty() {
            self.scroll = 0;
            return vec![rows::render_sidebar_empty(
                request.empty_message,
                request.theme,
            )];
        }

        let active_target = self.active_target(request.files.len());
        let rendered = self
            .entries
            .iter()
            .map(|entry| self.render_entry(entry, &request, active_target.as_ref()))
            .collect::<Vec<_>>();
        let row_counts = rendered
            .iter()
            .map(|entry| entry.lines.len())
            .collect::<Vec<_>>();
        self.scroll = visible_scroll(
            &self.entries,
            &row_counts,
            self.scroll,
            active_target.as_ref(),
            request.visible_height,
        );

        let mut lines = Vec::new();
        for entry in rendered.into_iter().skip(self.scroll) {
            let remaining = request.visible_height.saturating_sub(lines.len());
            if remaining == 0 {
                break;
            }
            let visible_rows = entry.lines.len().min(remaining);
            self.viewport.record_rows(entry.target, visible_rows);
            lines.extend(entry.lines.into_iter().take(visible_rows));
        }
        lines
    }

    pub(super) fn move_cursor(
        &mut self,
        direction: VerticalDirection,
        amount: usize,
        file_count: usize,
    ) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let previous_file = self.selected_file_index;
        let current_target = self.active_target(file_count);
        let target_position = current_target
            .as_ref()
            .and_then(|target| target_position(&self.entries, target))
            .map(|position| direction.shift_clamped(position, amount, self.entries.len() - 1))
            .unwrap_or_else(|| {
                fallback_target_position(&self.entries, self.selected_file_index, direction)
            });
        self.apply_target(self.entries[target_position].target(), file_count);
        self.selected_file_index != previous_file
    }

    pub(super) fn target_at(&self, column: u16, row: u16) -> Option<FilesPanelTarget> {
        self.viewport.target_at(column, row)
    }

    pub(super) fn contains(&self, column: u16, row: u16) -> bool {
        self.viewport.contains(column, row)
    }

    pub(super) fn activate(&mut self, target: FilesPanelTarget, file_count: usize) -> bool {
        let previous_file = self.selected_file_index;
        match target.0 {
            TreeTarget::File(index) => self.apply_target(TreeTarget::File(index), file_count),
            TreeTarget::Folder(path) => {
                self.cursor = Some(TreeTarget::Folder(path.clone()));
                toggle_string_membership(&mut self.collapsed_folders, path);
                self.rebuild_entries();
            }
        }
        self.selected_file_index != previous_file
    }

    pub(super) fn clear_cursor(&mut self) {
        self.cursor = None;
    }

    pub(super) fn focused_folder(&self) -> Option<FocusedFolder> {
        let TreeTarget::Folder(path) = self.cursor.as_ref()? else {
            return None;
        };
        let file_indices = self.tree.folder_members(path)?.to_vec();
        Some(FocusedFolder {
            path: path.clone(),
            file_indices,
        })
    }

    pub(super) fn folder_file_indices(&self, path: &str) -> Option<&[usize]> {
        self.tree.folder_members(path)
    }

    pub(super) fn toggle_selected_reviewed(&mut self, files: &[DiffFile]) {
        let Some(file) = files.get(self.selected_file_index) else {
            return;
        };
        toggle_string_membership(&mut self.reviewed_files, file.display_path().to_string());
    }

    pub(super) fn review_reloaded(
        &mut self,
        previous_files: &[DiffFile],
        files: &[DiffFile],
    ) -> FilesReload {
        let selected_identity = previous_files
            .get(self.selected_file_index)
            .map(DiffFile::display_path);
        let reselected_file_index = selected_identity.and_then(|identity| {
            files
                .iter()
                .position(|file| file.display_path() == identity)
        });
        let kept_file_selection = reselected_file_index.is_some();
        self.selected_file_index = reselected_file_index
            .unwrap_or_else(|| self.selected_file_index.min(files.len().saturating_sub(1)));

        let previous_folder = match self.cursor.take() {
            Some(TreeTarget::Folder(path)) => Some(path),
            Some(TreeTarget::File(_)) | None => None,
        };
        self.tree = FileTree::new(files);
        self.rebuild_entries();
        self.cursor = previous_folder
            .filter(|path| self.tree.folder_members(path).is_some())
            .map(TreeTarget::Folder);
        if files.is_empty() {
            self.scroll = 0;
        }
        self.viewport.invalidate_frame();

        FilesReload {
            kept_file_selection,
        }
    }

    fn active_target(&self, file_count: usize) -> Option<TreeTarget> {
        self.cursor.clone().or_else(|| {
            (file_count > 0).then_some(TreeTarget::File(
                self.selected_file_index.min(file_count - 1),
            ))
        })
    }

    fn apply_target(&mut self, target: TreeTarget, file_count: usize) {
        match target {
            TreeTarget::File(index) if file_count > 0 => {
                self.selected_file_index = index.min(file_count - 1);
                self.cursor = Some(TreeTarget::File(self.selected_file_index));
            }
            TreeTarget::Folder(path) => self.cursor = Some(TreeTarget::Folder(path)),
            TreeTarget::File(_) => {}
        }
    }

    fn rebuild_entries(&mut self) {
        self.entries = self.tree.entries(&self.collapsed_folders);
    }

    fn render_entry(
        &self,
        entry: &TreeEntry,
        request: &FilesPanelRender<'_>,
        active_target: Option<&TreeTarget>,
    ) -> RenderedTreeEntry {
        let target = FilesPanelTarget(entry.target());
        let is_active = active_target.is_some_and(|active| entry.matches_target(active));
        let selected = request.focused && is_active;
        let lines = match entry {
            TreeEntry::File { index, depth } => {
                request.files.get(*index).map_or_else(Vec::new, |file| {
                    rows::render_sidebar_file(
                        file,
                        selected,
                        request.content_width,
                        self.reviewed_files.contains(file.display_path()),
                        *depth,
                        request.theme,
                    )
                })
            }
            TreeEntry::Folder {
                name,
                depth,
                expanded,
                file_indices,
                ..
            } => {
                let hidden_selected_file = !expanded
                    && active_target.is_some_and(|active| {
                        matches!(active, TreeTarget::File(index) if file_indices.contains(index))
                    });
                rows::render_sidebar_folder(
                    name,
                    *depth,
                    *expanded,
                    request.focused && (is_active || hidden_selected_file),
                    folder_stage(request.files, file_indices),
                    request.content_width,
                    request.theme,
                )
            }
        };
        RenderedTreeEntry { target, lines }
    }

    #[cfg(test)]
    pub(super) fn is_selected_file_reviewed(&self, files: &[DiffFile]) -> bool {
        files
            .get(self.selected_file_index)
            .is_some_and(|file| self.reviewed_files.contains(file.display_path()))
    }

    #[cfg(test)]
    pub(super) fn set_selected_file(&mut self, index: usize, file_count: usize) {
        if file_count > 0 {
            self.selected_file_index = index.min(file_count - 1);
        }
    }

    #[cfg(test)]
    pub(super) fn set_cursor_file(&mut self, index: usize) {
        self.cursor = Some(TreeTarget::File(index));
    }

    #[cfg(test)]
    pub(super) fn set_cursor_folder(&mut self, path: impl Into<String>) {
        self.cursor = Some(TreeTarget::Folder(path.into()));
    }

    #[cfg(test)]
    pub(super) fn current_folder_path(&self) -> Option<&str> {
        match self.cursor.as_ref() {
            Some(TreeTarget::Folder(path)) => Some(path),
            Some(TreeTarget::File(_)) | None => None,
        }
    }

    #[cfg(test)]
    pub(super) fn current_file_target(&self) -> Option<usize> {
        match self.cursor {
            Some(TreeTarget::File(index)) => Some(index),
            Some(TreeTarget::Folder(_)) | None => None,
        }
    }

    #[cfg(test)]
    pub(super) fn collapse(&mut self, path: impl Into<String>) {
        self.collapsed_folders.insert(path.into());
        self.rebuild_entries();
    }

    #[cfg(test)]
    pub(super) fn is_collapsed(&self, path: &str) -> bool {
        self.collapsed_folders.contains(path)
    }

    #[cfg(test)]
    pub(super) fn set_scroll(&mut self, scroll: usize) {
        self.scroll = scroll;
    }

    #[cfg(test)]
    pub(super) fn scroll(&self) -> usize {
        self.scroll
    }
}

impl FileTree {
    fn new(files: &[DiffFile]) -> Self {
        let mut root = TreeNode::default();
        let mut folder_members = HashMap::<String, Vec<usize>>::new();
        let mut has_root_file = false;
        for (index, file) in files.iter().enumerate() {
            let path = file.display_path();
            let parts = path
                .split('/')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            root.insert_file(path, &parts, index);
            if parts.len() <= 1 {
                has_root_file = true;
            }
            let mut prefix = String::new();
            for directory in parts.iter().take(parts.len().saturating_sub(1)) {
                prefix = child_path(&prefix, directory);
                folder_members
                    .entry(prefix.clone())
                    .or_default()
                    .push(index);
            }
        }
        if has_root_file {
            folder_members.insert(String::new(), (0..files.len()).collect());
        }
        Self {
            root,
            folder_members,
            has_root_folder: has_root_file,
        }
    }

    fn entries(&self, collapsed: &HashSet<String>) -> Vec<TreeEntry> {
        let mut entries = Vec::new();
        if self.has_root_folder {
            let expanded = !collapsed.contains("");
            entries.push(TreeEntry::Folder {
                path: String::new(),
                name: "/".to_string(),
                depth: 0,
                expanded,
                file_indices: self.folder_members("").unwrap_or_default().to_vec(),
            });
            if expanded {
                self.root.flatten_files(1, &mut entries);
                self.root
                    .flatten_directories("", 1, collapsed, &self.folder_members, &mut entries);
            }
        } else {
            self.root
                .flatten("", 0, collapsed, &self.folder_members, &mut entries);
        }
        entries
    }

    fn folder_members(&self, path: &str) -> Option<&[usize]> {
        self.folder_members.get(path).map(Vec::as_slice)
    }
}

impl TreeNode {
    fn insert_file(&mut self, path: &str, parts: &[&str], index: usize) {
        let Some((file_name, dirs)) = parts.split_last() else {
            self.files.push(TreeFile {
                name: path.to_string(),
                index,
            });
            return;
        };
        let mut node = self;
        for directory in dirs {
            node = node.dirs.entry((*directory).to_string()).or_default();
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
        collapsed: &HashSet<String>,
        folder_members: &HashMap<String, Vec<usize>>,
        entries: &mut Vec<TreeEntry>,
    ) {
        self.flatten_directories(prefix, depth, collapsed, folder_members, entries);
        self.flatten_files(depth, entries);
    }

    fn flatten_directories(
        &self,
        prefix: &str,
        depth: usize,
        collapsed: &HashSet<String>,
        folder_members: &HashMap<String, Vec<usize>>,
        entries: &mut Vec<TreeEntry>,
    ) {
        for (name, node) in &self.dirs {
            let path = child_path(prefix, name);
            let expanded = !collapsed.contains(&path);
            entries.push(TreeEntry::Folder {
                path: path.clone(),
                name: name.clone(),
                depth,
                expanded,
                file_indices: folder_members.get(&path).cloned().unwrap_or_default(),
            });
            if expanded {
                node.flatten(&path, depth + 1, collapsed, folder_members, entries);
            }
        }
    }

    fn flatten_files(&self, depth: usize, entries: &mut Vec<TreeEntry>) {
        let mut files = self.files.iter().collect::<Vec<_>>();
        files.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.index.cmp(&right.index))
        });
        entries.extend(files.into_iter().map(|file| TreeEntry::File {
            index: file.index,
            depth,
        }));
    }
}

impl TreeEntry {
    fn target(&self) -> TreeTarget {
        match self {
            Self::Folder { path, .. } => TreeTarget::Folder(path.clone()),
            Self::File { index, .. } => TreeTarget::File(*index),
        }
    }

    fn matches_target(&self, target: &TreeTarget) -> bool {
        match (self, target) {
            (Self::Folder { path, .. }, TreeTarget::Folder(target_path)) => path == target_path,
            (Self::File { index, .. }, TreeTarget::File(target_index)) => index == target_index,
            _ => false,
        }
    }
}

impl FilesPanelViewport {
    fn new() -> Self {
        Self {
            area: None,
            row_targets: Vec::new(),
        }
    }

    fn begin(&mut self, area: Rect) {
        self.area = Some(area);
        self.row_targets.clear();
    }

    fn invalidate_frame(&mut self) {
        self.area = None;
        self.row_targets.clear();
    }

    fn record_rows(&mut self, target: FilesPanelTarget, row_count: usize) {
        self.row_targets
            .extend(std::iter::repeat_n(target, row_count));
    }

    fn target_at(&self, column: u16, row: u16) -> Option<FilesPanelTarget> {
        let area = self.area?;
        if !rect_inner_contains(area, column, row) {
            return None;
        }
        let row_offset = usize::from(row.saturating_sub(area.y.saturating_add(1)));
        self.row_targets.get(row_offset).cloned()
    }

    fn contains(&self, column: u16, row: u16) -> bool {
        self.area
            .is_some_and(|area| rect_contains(area, column, row))
    }
}

fn folder_stage(files: &[DiffFile], file_indices: &[usize]) -> FileStage {
    let mut stages = file_indices
        .iter()
        .filter_map(|index| files.get(*index))
        .map(|file| file.stage);
    let Some(first) = stages.next() else {
        return FileStage::Unstaged;
    };
    if stages.all(|stage| stage == first) {
        first
    } else {
        FileStage::Mixed
    }
}

fn visible_scroll(
    entries: &[TreeEntry],
    row_counts: &[usize],
    scroll: usize,
    active_target: Option<&TreeTarget>,
    visible_height: usize,
) -> usize {
    if entries.is_empty() {
        return 0;
    }
    let clamped = scroll.min(entries.len() - 1);
    let Some(active_index) = active_target.and_then(|target| target_position(entries, target))
    else {
        return clamped;
    };
    if active_index < clamped {
        return active_index;
    }
    let visible_height = visible_height.max(1);
    let rows_to_active: usize = row_counts[clamped..=active_index].iter().sum();
    if rows_to_active <= visible_height {
        return clamped;
    }
    let mut new_scroll = active_index;
    let mut used_rows = row_counts.get(active_index).copied().unwrap_or(1);
    while new_scroll > 0 {
        let previous_rows = row_counts.get(new_scroll - 1).copied().unwrap_or(1);
        if used_rows.saturating_add(previous_rows) > visible_height {
            break;
        }
        new_scroll -= 1;
        used_rows += previous_rows;
    }
    new_scroll
}

fn target_position(entries: &[TreeEntry], target: &TreeTarget) -> Option<usize> {
    entries
        .iter()
        .position(|entry| entry.matches_target(target))
}

fn fallback_target_position(
    entries: &[TreeEntry],
    selected_file_index: usize,
    direction: VerticalDirection,
) -> usize {
    match direction {
        VerticalDirection::Down => entries
            .iter()
            .enumerate()
            .filter_map(|(position, entry)| match entry {
                TreeEntry::File { index, .. } if *index > selected_file_index => {
                    Some((position, *index))
                }
                _ => None,
            })
            .min_by_key(|(_, index)| *index)
            .map(|(position, _)| position)
            .unwrap_or(entries.len() - 1),
        VerticalDirection::Up => entries
            .iter()
            .enumerate()
            .filter_map(|(position, entry)| match entry {
                TreeEntry::File { index, .. } if *index < selected_file_index => {
                    Some((position, *index))
                }
                _ => None,
            })
            .max_by_key(|(_, index)| *index)
            .map(|(position, _)| position)
            .unwrap_or(0),
    }
}

fn child_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

fn toggle_string_membership(set: &mut HashSet<String>, value: String) {
    if !set.remove(&value) {
        set.insert(value);
    }
}

fn rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

fn rect_inner_contains(area: Rect, column: u16, row: u16) -> bool {
    column > area.x
        && column < area.x.saturating_add(area.width).saturating_sub(1)
        && row > area.y
        && row < area.y.saturating_add(area.height).saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use crate::model::{FileStatus, SourceSnapshot};

    use super::*;

    #[test]
    fn one_tree_drives_render_hits_collapse_membership_and_navigation() {
        let files = diff_files(&[
            "src/app.rs",
            "src/bin/chunk.rs",
            "src/config.rs",
            "README.md",
        ]);
        let mut panel = FilesPanel::new(&files);
        let request = || FilesPanelRender {
            area: Rect::new(0, 0, 40, 9),
            files: &files,
            empty_message: "No changes",
            content_width: 38,
            visible_height: 7,
            theme: Theme::github_dark(),
            focused: true,
        };

        let expanded = panel.render(request());
        assert!(rendered_text(&expanded).contains("chunk.rs"));

        let bin_folder = panel
            .target_at(1, 4)
            .expect("rendered folder should have a pointer target");
        assert!(!panel.activate(bin_folder, files.len()));
        assert_eq!(
            panel.focused_folder(),
            Some(FocusedFolder {
                path: "src/bin".to_string(),
                file_indices: vec![1],
            })
        );

        let collapsed = panel.render(request());
        assert!(rendered_text(&collapsed).contains("bin/"));
        assert!(!rendered_text(&collapsed).contains("chunk.rs"));

        panel.set_selected_file(1, files.len());
        panel.clear_cursor();
        assert!(panel.move_cursor(VerticalDirection::Down, 1, files.len()));
        assert_eq!(panel.selected_file_index(), 2);
    }

    #[test]
    fn reload_preserves_file_folder_and_review_identity() {
        let previous = diff_files(&["a.txt", "src/bin/chunk.rs", "src/app.rs"]);
        let files = diff_files(&["src/app.rs", "a.txt", "src/bin/chunk.rs"]);
        let mut panel = FilesPanel::new(&previous);
        panel.set_selected_file(1, previous.len());
        panel.toggle_selected_reviewed(&previous);
        panel.set_cursor_folder("src/bin");

        let reload = panel.review_reloaded(&previous, &files);

        assert!(reload.kept_file_selection);
        assert_eq!(panel.selected_file_index(), 2);
        assert!(panel.is_selected_file_reviewed(&files));
        assert_eq!(panel.current_folder_path(), Some("src/bin"));
        assert_eq!(
            panel.focused_folder(),
            Some(FocusedFolder {
                path: "src/bin".to_string(),
                file_indices: vec![2],
            })
        );
    }

    #[test]
    fn rendering_scrolls_to_and_maps_the_active_file() {
        let files = diff_files(&["a.txt", "b.txt"]);
        let mut panel = FilesPanel::new(&files);
        panel.set_selected_file(1, files.len());
        panel.set_cursor_file(1);

        let rows = panel.render(FilesPanelRender {
            area: Rect::new(0, 0, 30, 3),
            files: &files,
            empty_message: "No changes",
            content_width: 28,
            visible_height: 1,
            theme: Theme::github_dark(),
            focused: true,
        });

        assert_eq!(panel.scroll(), 2);
        assert!(rendered_text(&rows).contains("b.txt"));
        let target = panel
            .target_at(1, 1)
            .expect("visible row should have a pointer target");
        assert!(!panel.activate(target, files.len()));
        assert_eq!(panel.current_file_target(), Some(1));
    }

    fn diff_files(paths: &[&str]) -> Vec<DiffFile> {
        paths
            .iter()
            .map(|path| DiffFile {
                id: (*path).to_string(),
                old_path: (*path).to_string(),
                path: (*path).to_string(),
                old_source: SourceSnapshot::Unloaded,
                new_source: SourceSnapshot::Unloaded,
                status: FileStatus::Modified,
                stage: FileStage::Unstaged,
                additions: 0,
                deletions: 0,
                hunks: Vec::new(),
                binary: false,
            })
            .collect()
    }

    fn rendered_text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect()
    }
}
