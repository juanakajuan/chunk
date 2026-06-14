//! Unified diff parser.
//!
//! The parser intentionally keeps a small surface: it accepts Git-style unified
//! patch text and returns model values. It does not run Git and does not perform
//! terminal formatting.

use crate::model::{
    Changeset, DiffFile, DiffHunk, DiffLine, DiffLineKind, FileStage, FileStatus, SourceSnapshot,
};

#[derive(Debug)]
struct FileBuilder {
    old_path: String,
    path: String,
    status: FileStatus,
    additions: usize,
    deletions: usize,
    hunks: Vec<DiffHunk>,
    current_hunk: Option<HunkBuilder>,
    binary: bool,
}

impl FileBuilder {
    fn new(old_path: String, path: String) -> Self {
        Self {
            old_path,
            path,
            status: FileStatus::Modified,
            additions: 0,
            deletions: 0,
            hunks: Vec::new(),
            current_hunk: None,
            binary: false,
        }
    }

    fn finish_hunk(&mut self) {
        if let Some(hunk) = self.current_hunk.take() {
            self.hunks.push(hunk.finish());
        }
    }

    fn push_patch_line(&mut self, line: &str) {
        if apply_file_metadata(self, line) {
            return;
        }

        if line.starts_with("@@ ") {
            self.start_hunk(line);
            return;
        }

        self.push_hunk_line(line);
    }

    fn start_hunk(&mut self, header: &str) {
        self.finish_hunk();
        self.current_hunk = Some(HunkBuilder::new(header));
    }

    fn push_hunk_line(&mut self, line: &str) {
        let Some(hunk) = self.current_hunk.as_mut() else {
            return;
        };

        match parse_hunk_line(line) {
            HunkLine::Added(content) => {
                hunk.push_added(content);
                self.additions += 1;
            }
            HunkLine::Removed(content) => {
                hunk.push_removed(content);
                self.deletions += 1;
            }
            HunkLine::Context(content) => hunk.push_context(content),
            HunkLine::Meta(content) => hunk.push_meta(content),
        }
    }

    fn finish(mut self, id: usize) -> DiffFile {
        self.finish_hunk();

        DiffFile {
            id: id.to_string(),
            old_path: self.old_path,
            path: self.path,
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: self.status,
            stage: FileStage::Unstaged,
            additions: self.additions,
            deletions: self.deletions,
            hunks: self.hunks,
            binary: self.binary,
        }
    }
}

#[derive(Debug)]
struct HunkBuilder {
    header: String,
    old_start: u32,
    old_lines: u32,
    new_start: u32,
    new_lines: u32,
    next_old_line: u32,
    next_new_line: u32,
    lines: Vec<DiffLine>,
}

impl HunkBuilder {
    fn new(header: &str) -> Self {
        let range = parse_hunk_range(header).unwrap_or_default();

        Self {
            header: header.to_string(),
            old_start: range.old_start,
            old_lines: range.old_lines,
            new_start: range.new_start,
            new_lines: range.new_lines,
            next_old_line: range.old_start,
            next_new_line: range.new_start,
            lines: Vec::new(),
        }
    }

    fn push_added(&mut self, content: &str) {
        self.push_line(DiffLineKind::Added, None, Some(self.next_new_line), content);
        self.next_new_line += 1;
    }

    fn push_removed(&mut self, content: &str) {
        self.push_line(
            DiffLineKind::Removed,
            Some(self.next_old_line),
            None,
            content,
        );
        self.next_old_line += 1;
    }

    fn push_context(&mut self, content: &str) {
        self.push_line(
            DiffLineKind::Context,
            Some(self.next_old_line),
            Some(self.next_new_line),
            content,
        );
        self.next_old_line += 1;
        self.next_new_line += 1;
    }

    fn push_meta(&mut self, content: &str) {
        self.push_line(DiffLineKind::Meta, None, None, content);
    }

    fn push_line(
        &mut self,
        kind: DiffLineKind,
        old_line: Option<u32>,
        new_line: Option<u32>,
        content: &str,
    ) {
        self.lines.push(DiffLine {
            kind,
            old_line,
            new_line,
            content: content.to_string(),
        });
    }

    fn finish(self) -> DiffHunk {
        DiffHunk {
            header: self.header,
            old_start: self.old_start,
            old_lines: self.old_lines,
            new_start: self.new_start,
            new_lines: self.new_lines,
            stage: FileStage::Unstaged,
            lines: self.lines,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct HunkRange {
    old_start: u32,
    old_lines: u32,
    new_start: u32,
    new_lines: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineRange {
    start: u32,
    lines: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HunkLine<'a> {
    Added(&'a str),
    Removed(&'a str),
    Context(&'a str),
    Meta(&'a str),
}

pub fn parse_unified_diff(input: &str) -> Changeset {
    let mut files = Vec::new();
    let mut current_file: Option<FileBuilder> = None;

    for line in input.lines() {
        match parse_diff_git_line(line) {
            Some((old_path, path)) => {
                finish_current_file(&mut files, &mut current_file);
                current_file = Some(FileBuilder::new(old_path, path));
            }
            None => {
                if let Some(file) = current_file.as_mut() {
                    file.push_patch_line(line);
                }
            }
        }
    }

    finish_current_file(&mut files, &mut current_file);

    Changeset {
        title: String::new(),
        source_label: String::new(),
        files,
    }
}

fn finish_current_file(files: &mut Vec<DiffFile>, current_file: &mut Option<FileBuilder>) {
    if let Some(file) = current_file.take() {
        files.push(file.finish(files.len()));
    }
}

fn apply_file_metadata(file: &mut FileBuilder, line: &str) -> bool {
    for (prefix, status) in [
        ("new file mode ", FileStatus::Added),
        ("deleted file mode ", FileStatus::Deleted),
    ] {
        if line.starts_with(prefix) {
            file.status = status;
            return true;
        }
    }

    for (from_prefix, to_prefix, status) in [
        ("copy from ", "copy to ", FileStatus::Copied),
        ("rename from ", "rename to ", FileStatus::Renamed),
    ] {
        if apply_path_change_metadata(file, line, from_prefix, to_prefix, status) {
            return true;
        }
    }

    if line.starts_with("Binary files ") {
        file.binary = true;
        return true;
    }

    update_prefixed_path(&mut file.old_path, line, "--- ")
        || update_prefixed_path(&mut file.path, line, "+++ ")
}

fn apply_path_change_metadata(
    file: &mut FileBuilder,
    line: &str,
    from_prefix: &str,
    to_prefix: &str,
    status: FileStatus,
) -> bool {
    if let Some(path) = line.strip_prefix(from_prefix) {
        file.status = status;
        file.old_path = path.to_string();
        return true;
    }

    if let Some(path) = line.strip_prefix(to_prefix) {
        file.status = status;
        file.path = path.to_string();
        return true;
    }

    false
}

fn update_prefixed_path(target: &mut String, line: &str, prefix: &str) -> bool {
    let Some(path) = line.strip_prefix(prefix) else {
        return false;
    };

    update_path_unless_dev_null(target, path);
    true
}

fn update_path_unless_dev_null(target: &mut String, path: &str) {
    let path = clean_git_path(path);
    if path != "/dev/null" {
        *target = path;
    }
}

fn parse_diff_git_line(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("diff --git ")?;
    let mut parts = rest.split_whitespace();
    let old_path = clean_git_path(parts.next()?);
    let new_path = clean_git_path(parts.next()?);
    Some((old_path, new_path))
}

fn clean_git_path(path: &str) -> String {
    let trimmed = path.trim();
    let unquoted = trimmed.trim_matches('"');
    unquoted
        .strip_prefix("a/")
        .or_else(|| unquoted.strip_prefix("b/"))
        .or_else(|| unquoted.strip_prefix("1/"))
        .or_else(|| unquoted.strip_prefix("2/"))
        .unwrap_or(unquoted)
        .to_string()
}

fn parse_hunk_range(header: &str) -> Option<HunkRange> {
    let mut parts = header.split_whitespace();
    let marker = parts.next()?;
    if marker != "@@" {
        return None;
    }

    let old_range = parse_line_range(parts.next()?, '-')?;
    let new_range = parse_line_range(parts.next()?, '+')?;

    Some(HunkRange {
        old_start: old_range.start,
        old_lines: old_range.lines,
        new_start: new_range.start,
        new_lines: new_range.lines,
    })
}

fn parse_line_range(input: &str, sign: char) -> Option<LineRange> {
    let without_sign = input.strip_prefix(sign)?;
    let mut parts = without_sign.split(',');
    let start = parts.next()?.parse().ok()?;
    let lines = match parts.next() {
        Some(value) => value.parse().ok()?,
        None => 1,
    };

    Some(LineRange { start, lines })
}

fn parse_hunk_line(line: &str) -> HunkLine<'_> {
    let mut chars = line.chars();
    match chars.next() {
        Some('+') => HunkLine::Added(chars.as_str()),
        Some('-') => HunkLine::Removed(chars.as_str()),
        Some(' ') => HunkLine::Context(chars.as_str()),
        _ => HunkLine::Meta(line),
    }
}

// --- Patch construction -----------------------------------------------------
//
// The inverse of `parse_unified_diff`: given a freshly-loaded source `DiffFile`
// and the hunk a caller acted on, emit Git unified diff text for the overlapping
// hunks. Kept beside the parser so the patch format lives in one place and the
// build path round-trips against the parse path in tests, with no Git index.

/// Build a patch containing every hunk in `file` that overlaps `selected_hunk`.
///
/// `file` is the source side (staged or unstaged) just loaded from Git;
/// `selected_hunk` is the hunk the user acted on. Returns `None` when no source
/// hunk overlaps, letting callers raise a precise "nothing to apply" error. The
/// text is a Git unified diff and round-trips through [`parse_unified_diff`].
pub(crate) fn overlapping_hunk_patch(file: &DiffFile, selected_hunk: &DiffHunk) -> Option<String> {
    let hunk_indices = overlapping_hunk_indices(file, selected_hunk);
    if hunk_indices.is_empty() {
        return None;
    }
    Some(build_hunk_patch(file, &hunk_indices))
}

/// Whether any hunk in `file` overlaps `hunk` by old- or new-side line span.
pub(crate) fn hunk_overlaps_file(hunk: &DiffHunk, file: &DiffFile) -> bool {
    file.hunks
        .iter()
        .any(|candidate| hunks_overlap(hunk, candidate))
}

fn overlapping_hunk_indices(file: &DiffFile, selected_hunk: &DiffHunk) -> Vec<usize> {
    file.hunks
        .iter()
        .enumerate()
        .filter_map(|(index, hunk)| hunks_overlap(selected_hunk, hunk).then_some(index))
        .collect()
}

fn hunks_overlap(left: &DiffHunk, right: &DiffHunk) -> bool {
    ranges_overlap(
        line_span(left.old_start, left.old_lines),
        line_span(right.old_start, right.old_lines),
    ) || ranges_overlap(
        line_span(left.new_start, left.new_lines),
        line_span(right.new_start, right.new_lines),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineSpan {
    start: u32,
    end: u32,
}

fn line_span(start: u32, lines: u32) -> LineSpan {
    LineSpan {
        start,
        end: start.saturating_add(lines),
    }
}

fn ranges_overlap(left: LineSpan, right: LineSpan) -> bool {
    left.start < left.end
        && right.start < right.end
        && left.start < right.end
        && right.start < left.end
}

fn build_hunk_patch(file: &DiffFile, hunk_indices: &[usize]) -> String {
    let mut patch = String::new();
    let old_path = patch_old_path(file);
    let new_path = patch_new_path(file);

    patch.push_str(&format!(
        "diff --git {} {}\n",
        prefixed_patch_path("a", old_path),
        prefixed_patch_path("b", new_path)
    ));
    match file.status {
        FileStatus::Added => patch.push_str("new file mode 100644\n"),
        FileStatus::Deleted => patch.push_str("deleted file mode 100644\n"),
        _ => {}
    }
    patch.push_str(&format!("--- {}\n", old_patch_header_path(file, old_path)));
    patch.push_str(&format!("+++ {}\n", new_patch_header_path(file, new_path)));

    for index in hunk_indices {
        if let Some(hunk) = file.hunks.get(*index) {
            push_hunk_patch(&mut patch, hunk);
        }
    }

    patch
}

fn patch_old_path(file: &DiffFile) -> &str {
    patch_side_path(file, &file.old_path)
}

fn patch_new_path(file: &DiffFile) -> &str {
    patch_side_path(file, &file.path)
}

fn patch_side_path<'a>(file: &'a DiffFile, path: &'a str) -> &'a str {
    if path.is_empty() {
        file.display_path()
    } else {
        path
    }
}

fn old_patch_header_path(file: &DiffFile, path: &str) -> String {
    patch_header_path(file.status, FileStatus::Added, "a", path)
}

fn new_patch_header_path(file: &DiffFile, path: &str) -> String {
    patch_header_path(file.status, FileStatus::Deleted, "b", path)
}

fn patch_header_path(
    status: FileStatus,
    null_status: FileStatus,
    prefix: &str,
    path: &str,
) -> String {
    if status == null_status {
        "/dev/null".to_string()
    } else {
        prefixed_patch_path(prefix, path)
    }
}

fn prefixed_patch_path(prefix: &str, path: &str) -> String {
    format!("{prefix}/{path}")
}

fn push_hunk_patch(patch: &mut String, hunk: &DiffHunk) {
    patch.push_str(&hunk.header);
    patch.push('\n');

    for line in &hunk.lines {
        match line.kind {
            DiffLineKind::Context => patch.push(' '),
            DiffLineKind::Added => patch.push('+'),
            DiffLineKind::Removed => patch.push('-'),
            DiffLineKind::Meta => {}
        }
        patch.push_str(&line.content);
        patch.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modified_file() {
        let changeset = parse_unified_diff(
            "diff --git a/src/main.rs b/src/main.rs\n\
             index 1111111..2222222 100644\n\
             --- a/src/main.rs\n\
             +++ b/src/main.rs\n\
             @@ -1,2 +1,2 @@\n\
              fn main() {\n\
             -    println!(\"old\");\n\
             +    println!(\"new\");\n\
              }\n",
        );

        assert_eq!(changeset.files.len(), 1);
        let file = &changeset.files[0];
        assert_eq!(file.path, "src/main.rs");
        assert_eq!(file.status, FileStatus::Modified);
        assert_eq!(file.old_source, SourceSnapshot::Unloaded);
        assert_eq!(file.new_source, SourceSnapshot::Unloaded);
        assert_eq!(file.additions, 1);
        assert_eq!(file.deletions, 1);
        assert_eq!(file.hunks[0].lines.len(), 4);
    }

    #[test]
    fn parses_renamed_file() {
        let changeset = parse_unified_diff(
            "diff --git a/old.rs b/new.rs\n\
             similarity index 88%\n\
             rename from old.rs\n\
             rename to new.rs\n\
             --- a/old.rs\n\
             +++ b/new.rs\n\
             @@ -1 +1 @@\n\
             -old\n\
             +new\n",
        );

        let file = &changeset.files[0];
        assert_eq!(file.old_path, "old.rs");
        assert_eq!(file.path, "new.rs");
        assert_eq!(file.status, FileStatus::Renamed);
    }

    fn line(kind: DiffLineKind, content: &str) -> DiffLine {
        DiffLine {
            kind,
            old_line: None,
            new_line: None,
            content: content.to_string(),
        }
    }

    fn hunk(
        header: &str,
        old_start: u32,
        old_lines: u32,
        new_start: u32,
        new_lines: u32,
        lines: Vec<DiffLine>,
    ) -> DiffHunk {
        DiffHunk {
            header: header.to_string(),
            old_start,
            old_lines,
            new_start,
            new_lines,
            stage: FileStage::Unstaged,
            lines,
        }
    }

    fn file(path: &str, status: FileStatus, hunks: Vec<DiffHunk>) -> DiffFile {
        DiffFile {
            id: String::new(),
            old_path: path.to_string(),
            path: path.to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status,
            stage: FileStage::Unstaged,
            additions: 0,
            deletions: 0,
            hunks,
            binary: false,
        }
    }

    fn line_shapes(hunk: &DiffHunk) -> Vec<(DiffLineKind, &str)> {
        hunk.lines
            .iter()
            .map(|line| (line.kind, line.content.as_str()))
            .collect()
    }

    #[test]
    fn overlapping_hunk_patch_selects_only_overlapping_hunks() {
        let source = file(
            "src/lib.rs",
            FileStatus::Modified,
            vec![
                hunk(
                    "@@ -1,3 +1,3 @@",
                    1,
                    3,
                    1,
                    3,
                    vec![
                        line(DiffLineKind::Context, "fn a() {"),
                        line(DiffLineKind::Removed, "    let x = 1;"),
                        line(DiffLineKind::Added, "    let x = 2;"),
                    ],
                ),
                hunk(
                    "@@ -20,3 +20,3 @@",
                    20,
                    3,
                    20,
                    3,
                    vec![
                        line(DiffLineKind::Context, "fn b() {"),
                        line(DiffLineKind::Removed, "    let y = 1;"),
                        line(DiffLineKind::Added, "    let y = 2;"),
                    ],
                ),
            ],
        );
        let selected = hunk("@@ -1,3 +1,3 @@", 1, 3, 1, 3, Vec::new());

        let patch = overlapping_hunk_patch(&source, &selected).expect("overlap");
        let parsed = parse_unified_diff(&patch);

        assert_eq!(parsed.files.len(), 1);
        let parsed_file = &parsed.files[0];
        assert_eq!(parsed_file.path, "src/lib.rs");
        assert_eq!(parsed_file.hunks.len(), 1);
        assert_eq!(
            line_shapes(&parsed_file.hunks[0]),
            vec![
                (DiffLineKind::Context, "fn a() {"),
                (DiffLineKind::Removed, "    let x = 1;"),
                (DiffLineKind::Added, "    let x = 2;"),
            ]
        );
    }

    #[test]
    fn overlapping_hunk_patch_returns_none_when_disjoint() {
        let source = file(
            "src/lib.rs",
            FileStatus::Modified,
            vec![hunk(
                "@@ -1,3 +1,3 @@",
                1,
                3,
                1,
                3,
                vec![line(DiffLineKind::Context, "fn a() {")],
            )],
        );
        let selected = hunk("@@ -50,2 +50,2 @@", 50, 2, 50, 2, Vec::new());

        assert!(overlapping_hunk_patch(&source, &selected).is_none());
    }

    #[test]
    fn added_file_patch_round_trips_with_dev_null_old_side() {
        let mut source = file(
            "new.rs",
            FileStatus::Added,
            vec![hunk(
                "@@ -0,0 +1,2 @@",
                0,
                0,
                1,
                2,
                vec![
                    line(DiffLineKind::Added, "fn new() {}"),
                    line(DiffLineKind::Added, "// added"),
                ],
            )],
        );
        source.old_path = String::new();
        let selected = hunk("@@ -0,0 +1,2 @@", 0, 0, 1, 2, Vec::new());

        let patch = overlapping_hunk_patch(&source, &selected).expect("overlap");
        assert!(patch.contains("new file mode 100644"));
        assert!(patch.contains("--- /dev/null"));

        let parsed = parse_unified_diff(&patch);
        assert_eq!(parsed.files[0].status, FileStatus::Added);
        assert_eq!(parsed.files[0].path, "new.rs");
    }
}
