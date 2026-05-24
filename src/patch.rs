use crate::model::{Changeset, DiffFile, DiffHunk, DiffLine, DiffLineKind, FileStatus};

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

    fn finish(mut self, id: usize) -> DiffFile {
        self.finish_hunk();

        DiffFile {
            id: id.to_string(),
            old_path: self.old_path,
            path: self.path,
            status: self.status,
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

pub fn parse_unified_diff(input: &str) -> Changeset {
    let mut files = Vec::new();
    let mut current_file: Option<FileBuilder> = None;

    for line in input.lines() {
        if let Some((old_path, new_path)) = parse_diff_git_line(line) {
            finish_current_file(&mut files, &mut current_file);
            current_file = Some(FileBuilder::new(old_path, new_path));
            continue;
        }

        let Some(file) = current_file.as_mut() else {
            continue;
        };

        if apply_file_metadata(file, line) {
            continue;
        }

        if line.starts_with("@@ ") {
            file.finish_hunk();
            file.current_hunk = Some(HunkBuilder::new(line));
            continue;
        }

        let Some(hunk) = file.current_hunk.as_mut() else {
            continue;
        };

        if let Some(content) = line.strip_prefix('+') {
            hunk.push_added(content);
            file.additions += 1;
            continue;
        }

        if let Some(content) = line.strip_prefix('-') {
            hunk.push_removed(content);
            file.deletions += 1;
            continue;
        }

        if let Some(content) = line.strip_prefix(' ') {
            hunk.push_context(content);
            continue;
        }

        hunk.push_meta(line);
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
    if line.strip_prefix("new file mode ").is_some() {
        file.status = FileStatus::Added;
        return true;
    }

    if line.strip_prefix("deleted file mode ").is_some() {
        file.status = FileStatus::Deleted;
        return true;
    }

    if let Some(path) = line.strip_prefix("copy from ") {
        file.status = FileStatus::Copied;
        file.old_path = path.to_string();
        return true;
    }

    if let Some(path) = line.strip_prefix("copy to ") {
        file.status = FileStatus::Copied;
        file.path = path.to_string();
        return true;
    }

    if let Some(path) = line.strip_prefix("rename from ") {
        file.status = FileStatus::Renamed;
        file.old_path = path.to_string();
        return true;
    }

    if let Some(path) = line.strip_prefix("rename to ") {
        file.status = FileStatus::Renamed;
        file.path = path.to_string();
        return true;
    }

    if line.strip_prefix("Binary files ").is_some() {
        file.binary = true;
        return true;
    }

    if let Some(path) = line.strip_prefix("--- ") {
        update_path_unless_dev_null(&mut file.old_path, path);
        return true;
    }

    if let Some(path) = line.strip_prefix("+++ ") {
        update_path_unless_dev_null(&mut file.path, path);
        return true;
    }

    false
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
        .unwrap_or(unquoted)
        .to_string()
}

fn parse_hunk_range(header: &str) -> Option<HunkRange> {
    let mut parts = header.split_whitespace();
    let marker = parts.next()?;
    if marker != "@@" {
        return None;
    }

    let old_range = parse_one_range(parts.next()?, '-')?;
    let new_range = parse_one_range(parts.next()?, '+')?;

    Some(HunkRange {
        old_start: old_range.0,
        old_lines: old_range.1,
        new_start: new_range.0,
        new_lines: new_range.1,
    })
}

fn parse_one_range(input: &str, sign: char) -> Option<(u32, u32)> {
    let without_sign = input.strip_prefix(sign)?;
    let mut parts = without_sign.split(',');
    let start = parts.next()?.parse().ok()?;
    let lines = parts.next().map_or(Some(1), |value| value.parse().ok())?;
    Some((start, lines))
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
}
