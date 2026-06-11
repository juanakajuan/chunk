//! Git integration and source loading boundary.
//!
//! All shelling out to Git lives here. Other modules work with parsed model
//! values and should not need to know which Git commands produced them.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use color_eyre::eyre::{Result, eyre};

use crate::model::{
    Changeset, DiffFile, DiffHunk, DiffLineKind, FileStage, FileStatus, SourceSnapshot,
};
use crate::patch::parse_unified_diff;

const MAX_SOURCE_CONTEXT_BYTES: usize = 512 * 1024;
const DEFAULT_BASE_REFS: [&str; 3] = ["origin/HEAD", "main", "master"];
const GIT_DIFF_PATCH_ARGS: [&str; 4] = [
    "--no-color",
    "--patch",
    "--find-renames",
    "--default-prefix",
];

pub(crate) struct LoadedPrDiff {
    pub(crate) changeset: Changeset,
    pub(crate) old_ref: String,
    pub(crate) new_ref: String,
}

pub(crate) fn load_worktree_diff() -> Result<Changeset> {
    let output = Command::new("git")
        .arg("diff")
        .args(GIT_DIFF_PATCH_ARGS)
        .arg("HEAD")
        .output()?;

    ensure_success(&output, "git diff failed")?;

    let untracked_paths = untracked_paths()?;
    let mut patch = String::from_utf8_lossy(&output.stdout).to_string();
    patch.push_str(&load_untracked_patches(&untracked_paths)?);
    let mut changeset = parse_unified_diff(&patch);
    annotate_stage_states(&mut changeset, &untracked_paths)?;
    annotate_hunk_stage_states(&mut changeset, &untracked_paths)?;
    changeset.title = worktree_title();
    changeset.source_label = "git diff HEAD + untracked".to_string();
    Ok(changeset)
}

pub(crate) fn load_pr_diff(base: Option<&str>) -> Result<LoadedPrDiff> {
    let base_ref = resolve_base_ref(base)?;
    let merge_base = merge_base(&base_ref)?;
    let mut changeset = load_ref_diff(&merge_base, "HEAD")?;
    let head = current_branch_label().unwrap_or_else(|| "HEAD".to_string());
    changeset.title = format!("PR review {head} into {base_ref}");
    changeset.source_label = format!("git diff {base_ref}...HEAD");

    Ok(LoadedPrDiff {
        changeset,
        old_ref: merge_base,
        new_ref: "HEAD".to_string(),
    })
}

pub(crate) fn load_ref_diff(old_ref: &str, new_ref: &str) -> Result<Changeset> {
    let output = Command::new("git")
        .arg("diff")
        .args(GIT_DIFF_PATCH_ARGS)
        .arg(old_ref)
        .arg(new_ref)
        .output()?;

    ensure_success(&output, "git diff failed")?;

    Ok(parse_unified_diff(&String::from_utf8_lossy(&output.stdout)))
}

pub(crate) fn load_worktree_source_snapshots(file: &mut DiffFile) {
    let root = worktree_root().ok();
    load_worktree_source_snapshots_with_root(file, root.as_deref());
}

pub(crate) fn load_ref_source_snapshots(file: &mut DiffFile, old_ref: &str, new_ref: &str) {
    let Some((old_context_line, new_context_line)) = source_context_lines(file) else {
        return;
    };

    load_git_snapshot(
        &mut file.old_source,
        old_ref,
        &file.old_path,
        old_context_line,
    );
    load_git_snapshot(&mut file.new_source, new_ref, &file.path, new_context_line);
}

pub(crate) fn toggle_staging_for_file(path: &str) -> Result<()> {
    if is_file_staged(path)? {
        unstage_file(path)
    } else {
        stage_file(path)
    }
}

pub(crate) fn toggle_staging_for_hunk(file: &DiffFile, hunk_index: usize) -> Result<()> {
    let hunk = file.hunks.get(hunk_index).ok_or_else(|| {
        eyre!(
            "hunk {} does not exist in {}",
            hunk_index + 1,
            file.display_path()
        )
    })?;

    match hunk.stage {
        FileStage::Unstaged => apply_matching_hunks_to_index(file, hunk, HunkPatchSource::Unstaged),
        FileStage::Staged | FileStage::Mixed => {
            apply_matching_hunks_to_index(file, hunk, HunkPatchSource::Staged)
        }
    }
}

pub(crate) fn worktree_root() -> Result<PathBuf> {
    git_stdout(["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .ok_or_else(|| eyre!("could not determine Git worktree root"))
}

fn load_untracked_patches(untracked_paths: &[String]) -> Result<String> {
    let mut patches = String::new();

    for path in untracked_paths {
        let output = Command::new("git")
            .args([
                "diff",
                "--no-color",
                "--patch",
                "--no-index",
                "--default-prefix",
                "--",
            ])
            .arg("/dev/null")
            .arg(path)
            .output()?;

        if !output.status.success() && output.status.code() != Some(1) {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!(
                "git diff --no-index failed for {}: {}",
                path,
                stderr.trim()
            ));
        }

        if !patches.is_empty() && !patches.ends_with('\n') {
            patches.push('\n');
        }
        patches.push_str(&String::from_utf8_lossy(&output.stdout));
    }

    Ok(patches)
}

fn load_staged_diff() -> Result<Changeset> {
    let output = Command::new("git")
        .args(["diff", "--cached"])
        .args(GIT_DIFF_PATCH_ARGS)
        .arg("HEAD")
        .output()?;

    ensure_success(&output, "git diff --cached failed")?;
    Ok(parse_unified_diff(&String::from_utf8_lossy(&output.stdout)))
}

fn load_unstaged_diff(untracked_paths: &[String]) -> Result<Changeset> {
    let output = Command::new("git")
        .arg("diff")
        .args(GIT_DIFF_PATCH_ARGS)
        .output()?;

    ensure_success(&output, "git diff failed")?;

    let mut patch = String::from_utf8_lossy(&output.stdout).to_string();
    patch.push_str(&load_untracked_patches(untracked_paths)?);
    Ok(parse_unified_diff(&patch))
}

fn annotate_stage_states(changeset: &mut Changeset, untracked_paths: &[String]) -> Result<()> {
    for file in &mut changeset.files {
        let path = file.display_path();
        let staged = is_file_staged(path)?;
        let unstaged = is_file_unstaged(path)? || is_untracked_path(untracked_paths, path);

        file.stage = FileStage::from_staged_unstaged(staged, unstaged);
    }

    Ok(())
}

fn annotate_hunk_stage_states(changeset: &mut Changeset, untracked_paths: &[String]) -> Result<()> {
    let staged = load_staged_diff()?;
    let unstaged = load_unstaged_diff(untracked_paths)?;
    annotate_hunk_stage_states_from_diffs(changeset, &staged, &unstaged, untracked_paths);
    Ok(())
}

fn annotate_hunk_stage_states_from_diffs(
    changeset: &mut Changeset,
    staged: &Changeset,
    unstaged: &Changeset,
    untracked_paths: &[String],
) {
    for file in &mut changeset.files {
        let staged_file = matching_file(&staged.files, file);
        let unstaged_file = matching_file(&unstaged.files, file);
        let untracked = is_untracked_path(untracked_paths, file.display_path());

        for hunk in &mut file.hunks {
            let staged = staged_file.is_some_and(|candidate| hunk_overlaps_file(hunk, candidate));
            let unstaged = untracked
                || unstaged_file.is_some_and(|candidate| hunk_overlaps_file(hunk, candidate));

            hunk.stage = FileStage::from_staged_unstaged(staged, unstaged);
        }
    }
}

fn is_untracked_path(untracked_paths: &[String], path: &str) -> bool {
    untracked_paths.iter().any(|candidate| candidate == path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HunkPatchSource {
    Staged,
    Unstaged,
}

impl HunkPatchSource {
    fn label(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Unstaged => "unstaged",
        }
    }

    fn reverse(self) -> bool {
        matches!(self, Self::Staged)
    }
}

fn apply_matching_hunks_to_index(
    file: &DiffFile,
    selected_hunk: &DiffHunk,
    source: HunkPatchSource,
) -> Result<()> {
    let source_changeset = match source {
        HunkPatchSource::Staged => load_staged_diff()?,
        HunkPatchSource::Unstaged => {
            let untracked_paths = untracked_paths()?;
            load_unstaged_diff(&untracked_paths)?
        }
    };
    let source_file = matching_file(&source_changeset.files, file).ok_or_else(|| {
        eyre!(
            "no {} hunk found for {}",
            source.label(),
            file.display_path()
        )
    })?;
    let hunk_indices = overlapping_hunk_indices(source_file, selected_hunk);
    if hunk_indices.is_empty() {
        return Err(eyre!(
            "no {} hunk overlaps selected hunk in {}",
            source.label(),
            file.display_path()
        ));
    }

    let patch = build_hunk_patch(source_file, &hunk_indices);
    apply_patch_to_index(&patch, source.reverse())
}

fn matching_file<'a>(files: &'a [DiffFile], target: &DiffFile) -> Option<&'a DiffFile> {
    files.iter().find(|file| same_file_identity(file, target))
}

fn same_file_identity(left: &DiffFile, right: &DiffFile) -> bool {
    left.display_path() == right.display_path()
        || non_empty_eq(&left.path, &right.path)
        || non_empty_eq(&left.old_path, &right.old_path)
}

fn non_empty_eq(left: &str, right: &str) -> bool {
    !left.is_empty() && left == right
}

fn hunk_overlaps_file(hunk: &DiffHunk, file: &DiffFile) -> bool {
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
    if file.old_path.is_empty() {
        file.display_path()
    } else {
        &file.old_path
    }
}

fn patch_new_path(file: &DiffFile) -> &str {
    if file.path.is_empty() {
        file.display_path()
    } else {
        &file.path
    }
}

fn old_patch_header_path(file: &DiffFile, path: &str) -> String {
    if file.status == FileStatus::Added {
        "/dev/null".to_string()
    } else {
        prefixed_patch_path("a", path)
    }
}

fn new_patch_header_path(file: &DiffFile, path: &str) -> String {
    if file.status == FileStatus::Deleted {
        "/dev/null".to_string()
    } else {
        prefixed_patch_path("b", path)
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

fn apply_patch_to_index(patch: &str, reverse: bool) -> Result<()> {
    let mut command = Command::new("git");
    command.args(["apply", "--cached", "--whitespace=nowarn"]);
    if reverse {
        command.arg("--reverse");
    }
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command.spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| eyre!("failed to open git apply stdin"))?;
    stdin.write_all(patch.as_bytes())?;
    drop(stdin);

    let output = child.wait_with_output()?;
    let context = if reverse {
        "git apply --cached --reverse failed"
    } else {
        "git apply --cached failed"
    };
    ensure_success(&output, context)
}

fn resolve_base_ref(base: Option<&str>) -> Result<String> {
    if let Some(requested_base) = base.filter(|base| !base.trim().is_empty()) {
        return Ok(requested_base.to_string());
    }

    DEFAULT_BASE_REFS
        .into_iter()
        .find(|candidate| git_commit_exists(candidate))
        .map(str::to_string)
        .ok_or_else(|| eyre!("could not determine base branch; pass one explicitly"))
}

fn git_commit_exists(rev: &str) -> bool {
    Command::new("git")
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{rev}^{{commit}}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn merge_base(base_ref: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["merge-base", base_ref, "HEAD"])
        .output()?;
    ensure_success(&output, &format!("git merge-base failed for {base_ref}"))?;

    let merge_base = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if merge_base.is_empty() {
        return Err(eyre!("git merge-base returned no commit for {base_ref}"));
    }

    Ok(merge_base)
}

fn load_worktree_source_snapshots_with_root(file: &mut DiffFile, worktree_root: Option<&Path>) {
    let Some((old_context_line, new_context_line)) = source_context_lines(file) else {
        return;
    };

    load_git_snapshot(
        &mut file.old_source,
        "HEAD",
        &file.old_path,
        old_context_line,
    );
    load_worktree_snapshot(
        &mut file.new_source,
        worktree_root,
        &file.path,
        new_context_line,
    );
}

fn load_git_snapshot(snapshot: &mut SourceSnapshot, rev: &str, path: &str, max_context_line: u32) {
    if snapshot.is_unloaded() {
        *snapshot = load_git_source_prefix(rev, path, max_context_line);
    }
}

fn load_worktree_snapshot(
    snapshot: &mut SourceSnapshot,
    worktree_root: Option<&Path>,
    path: &str,
    max_context_line: u32,
) {
    if snapshot.is_unloaded() {
        *snapshot = load_worktree_source_prefix(worktree_root, path, max_context_line);
    }
}

fn source_context_lines(file: &DiffFile) -> Option<(u32, u32)> {
    if file.binary || file.hunks.is_empty() {
        return None;
    }

    Some(file.hunks.iter().fold((0, 0), |(old, new), hunk| {
        (
            old.max(hunk.old_start.saturating_sub(1)),
            new.max(hunk.new_start.saturating_sub(1)),
        )
    }))
}

fn load_git_source_prefix(rev: &str, path: &str, max_context_line: u32) -> SourceSnapshot {
    if let Some(snapshot) = guarded_source_prefix(path, max_context_line) {
        return snapshot;
    }

    let Ok(mut child) = Command::new("git")
        .args(["show", "--textconv", &format!("{rev}:{path}")])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return SourceSnapshot::Unavailable;
    };

    load_source_prefix_from_child(&mut child, max_context_line)
}

fn load_source_prefix_from_child(child: &mut Child, max_context_line: u32) -> SourceSnapshot {
    let Some(stdout) = child.stdout.take() else {
        stop_child(child);
        return SourceSnapshot::Unavailable;
    };

    let mut reader = BufReader::new(stdout);
    let prefix = match read_source_prefix(&mut reader, max_context_line) {
        Ok(prefix) => prefix,
        Err(_) => {
            stop_child(child);
            return SourceSnapshot::Unavailable;
        }
    };

    if prefix.line_limit_reached {
        stop_child(child);
        return SourceSnapshot::loaded(prefix.content);
    }

    if child.wait().is_ok_and(|status| status.success()) {
        SourceSnapshot::loaded(prefix.content)
    } else {
        SourceSnapshot::Unavailable
    }
}

fn load_worktree_source_prefix(
    worktree_root: Option<&Path>,
    path: &str,
    max_context_line: u32,
) -> SourceSnapshot {
    if let Some(snapshot) = guarded_source_prefix(path, max_context_line) {
        return snapshot;
    }

    let path = match worktree_root {
        Some(root) => root.join(path),
        None => PathBuf::from(path),
    };

    let Ok(file) = File::open(path) else {
        return SourceSnapshot::Unavailable;
    };
    let mut reader = BufReader::new(file);

    load_source_prefix_from_reader(&mut reader, max_context_line)
}

fn load_source_prefix_from_reader(
    reader: &mut impl BufRead,
    max_context_line: u32,
) -> SourceSnapshot {
    match read_source_prefix(reader, max_context_line) {
        Ok(prefix) => SourceSnapshot::loaded(prefix.content),
        Err(_) => SourceSnapshot::Unavailable,
    }
}

fn guarded_source_prefix(path: &str, max_context_line: u32) -> Option<SourceSnapshot> {
    if max_context_line == 0 {
        return Some(SourceSnapshot::loaded(String::new()));
    }

    path.is_empty().then_some(SourceSnapshot::Unavailable)
}

struct SourcePrefix {
    content: String,
    line_limit_reached: bool,
}

fn read_source_prefix(
    reader: &mut impl BufRead,
    max_context_line: u32,
) -> io::Result<SourcePrefix> {
    let mut content = String::new();
    for _ in 0..max_context_line {
        let mut line = String::new();
        let byte_count = reader.read_line(&mut line)?;
        if byte_count == 0 {
            return Ok(SourcePrefix {
                content,
                line_limit_reached: false,
            });
        }

        if content.len().saturating_add(line.len()) > MAX_SOURCE_CONTEXT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "source context exceeds byte cap",
            ));
        }

        content.push_str(&line);
    }

    Ok(SourcePrefix {
        content,
        line_limit_reached: true,
    })
}

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn untracked_paths() -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output()?;

    ensure_success(&output, "git ls-files failed")?;

    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn worktree_title() -> String {
    current_branch_label().map_or_else(
        || "Working tree changes".to_string(),
        |branch| format!("Working tree changes ({branch})"),
    )
}

fn current_branch_label() -> Option<String> {
    git_stdout(["branch", "--show-current"]).or_else(|| {
        git_stdout(["rev-parse", "--short", "HEAD"]).map(|sha| format!("detached {sha}"))
    })
}

fn git_stdout<const N: usize>(args: [&str; N]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn ensure_success(output: &Output, context: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(eyre!("{}: {}", context, stderr.trim()))
}

fn stage_file(path: &str) -> Result<()> {
    let output = Command::new("git").args(["add", "--", path]).output()?;
    ensure_success(&output, &format!("git add failed for {path}"))
}

fn unstage_file(path: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["restore", "--staged", "--", path])
        .output()?;
    ensure_success(&output, &format!("git restore --staged failed for {path}"))
}

fn is_file_staged(path: &str) -> Result<bool> {
    has_file_diff(path, true)
}

fn is_file_unstaged(path: &str) -> Result<bool> {
    has_file_diff(path, false)
}

fn has_file_diff(path: &str, cached: bool) -> Result<bool> {
    let mut command = Command::new("git");
    command.arg("diff");
    if cached {
        command.arg("--cached");
    }

    let status = command.args(["--quiet", "--"]).arg(path).status()?;
    let failure_context = if cached {
        "git diff --cached failed"
    } else {
        "git diff failed"
    };

    match status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(eyre!("{failure_context} for {path}")),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::model::{DiffHunk, FileStatus};

    static GIT_CWD_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn source_snapshots_load_only_pre_hunk_prefix() {
        let root = temp_root();
        let path = "src/App.vue";
        let full_path = root.join(path);
        fs::create_dir_all(full_path.parent().expect("fixture parent")).unwrap();
        fs::write(
            &full_path,
            "<template>\n</template>\n<script setup lang=\"ts\">\nconst changed = true;\n",
        )
        .unwrap();

        let mut file = DiffFile {
            id: "0".to_string(),
            old_path: String::new(),
            path: path.to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Added,
            stage: FileStage::Unstaged,
            additions: 1,
            deletions: 0,
            hunks: vec![DiffHunk {
                header: "@@ -0,0 +4 @@".to_string(),
                old_start: 0,
                old_lines: 0,
                new_start: 4,
                new_lines: 1,
                stage: FileStage::Unstaged,
                lines: Vec::new(),
            }],
            binary: false,
        };

        load_worktree_source_snapshots_with_root(&mut file, Some(&root));

        assert_eq!(file.old_source.as_str(), Some(""));
        assert_eq!(
            file.new_source.as_str(),
            Some("<template>\n</template>\n<script setup lang=\"ts\">\n")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_prefix_rejects_context_over_byte_cap() {
        let source = "x".repeat(MAX_SOURCE_CONTEXT_BYTES + 1);
        let mut reader = Cursor::new(source);

        assert!(read_source_prefix(&mut reader, 1).is_err());
    }

    #[test]
    fn toggle_hunk_staging_moves_only_selected_hunk() {
        let _lock = GIT_CWD_LOCK.lock().expect("git cwd lock");
        let root = temp_root();
        let cwd = CurrentDirGuard::enter(&root);

        run_git(["init"]);
        run_git(["config", "user.email", "chunk@example.test"]);
        run_git(["config", "user.name", "Chunk Test"]);
        fs::write("sample.txt", numbered_lines()).unwrap();
        run_git(["add", "sample.txt"]);
        run_git(["commit", "-m", "initial"]);

        fs::write("sample.txt", changed_numbered_lines()).unwrap();

        let changeset = load_worktree_diff().unwrap();
        let file = &changeset.files[0];
        assert_eq!(file.hunks.len(), 2);
        assert_eq!(file.hunks[0].stage, FileStage::Unstaged);
        assert_eq!(file.hunks[1].stage, FileStage::Unstaged);

        toggle_staging_for_hunk(file, 0).unwrap();

        let cached = git_output(["diff", "--cached", "--", "sample.txt"]);
        assert!(cached.contains("line two"));
        assert!(!cached.contains("line eighteen"));
        let unstaged = git_output(["diff", "--", "sample.txt"]);
        assert!(!unstaged.contains("line two"));
        assert!(unstaged.contains("line eighteen"));

        let changeset = load_worktree_diff().unwrap();
        let file = &changeset.files[0];
        assert_eq!(file.stage, FileStage::Mixed);
        assert_eq!(file.hunks[0].stage, FileStage::Staged);
        assert_eq!(file.hunks[1].stage, FileStage::Unstaged);

        toggle_staging_for_hunk(file, 0).unwrap();

        let cached = git_output(["diff", "--cached", "--", "sample.txt"]);
        assert!(!cached.contains("line two"));
        assert!(!cached.contains("line eighteen"));
        let unstaged = git_output(["diff", "--", "sample.txt"]);
        assert!(unstaged.contains("line two"));
        assert!(unstaged.contains("line eighteen"));

        drop(cwd);
        fs::remove_dir_all(root).unwrap();
    }

    struct CurrentDirGuard {
        previous: PathBuf,
    }

    impl CurrentDirGuard {
        fn enter(path: &Path) -> Self {
            let previous = std::env::current_dir().unwrap();
            std::env::set_current_dir(path).unwrap();
            Self { previous }
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.previous).unwrap();
        }
    }

    fn run_git<const N: usize>(args: [&str; N]) {
        let output = Command::new("git").args(args).output().unwrap();
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_output<const N: usize>(args: [&str; N]) -> String {
        let output = Command::new("git").args(args).output().unwrap();
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn numbered_lines() -> String {
        (1..=20)
            .map(|line| format!("line {line}\n"))
            .collect::<String>()
    }

    fn changed_numbered_lines() -> String {
        let mut lines = (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>();
        lines[1] = "line two".to_string();
        lines[17] = "line eighteen".to_string();
        format!("{}\n", lines.join("\n"))
    }

    fn temp_root() -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("chunk-source-prefix-{now}"));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
