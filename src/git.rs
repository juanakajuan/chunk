//! Git integration and source loading boundary.
//!
//! All shelling out to Git lives here. Other modules work with parsed model
//! values and should not need to know which Git commands produced them.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use color_eyre::eyre::{Result, eyre};

use crate::model::{Changeset, DiffFile, DiffHunk, FileStage, SourceSnapshot};
use crate::patch::{hunk_overlaps_file, overlapping_hunk_patch, parse_unified_diff};

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

pub(crate) struct LoadedUnpublishedDiff {
    pub(crate) repo_root: PathBuf,
    pub(crate) text: String,
}

pub(crate) fn load_worktree_diff() -> Result<Changeset> {
    let root = worktree_root()?;
    let untracked_paths = untracked_paths(&root)?;
    let patch = load_worktree_patch(&root, &untracked_paths)?;
    let mut changeset = parse_unified_diff(&patch);
    annotate_stage_states(&root, &mut changeset, &untracked_paths)?;
    annotate_hunk_stage_states(&root, &mut changeset, &untracked_paths)?;
    changeset.title = worktree_title();
    changeset.source_label = "git diff HEAD + untracked".to_string();
    Ok(changeset)
}

pub(crate) fn load_unpublished_diff_text() -> Result<LoadedUnpublishedDiff> {
    let repo_root = worktree_root()?;
    let untracked_paths = untracked_paths(&repo_root)?;
    let text = load_unpublished_patch(&repo_root, &untracked_paths)?;

    Ok(LoadedUnpublishedDiff { repo_root, text })
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
    load_git_diff(&[], &[old_ref, new_ref], "git diff failed")
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

pub(crate) fn stage_files(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }

    let root = worktree_root()?;
    let mut command = git_command(&root);
    command.args(["add", "--"]);
    command.args(paths);
    checked_output(&mut command, "git add failed").map(|_| ())
}

pub(crate) fn unstage_files(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }

    let root = worktree_root()?;
    let mut command = git_command(&root);
    command.args(["restore", "--staged", "--"]);
    command.args(paths);
    checked_output(&mut command, "git restore --staged failed").map(|_| ())
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

pub(crate) fn discard_worktree_file(path: &str) -> Result<()> {
    let root = worktree_root()?;
    let untracked_paths = untracked_paths(&root)?;
    discard_worktree_file_with_untracked(&root, path, &untracked_paths)
}

pub(crate) fn discard_worktree_files(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Err(eyre!("no files to discard"));
    }

    let root = worktree_root()?;
    let untracked_paths = untracked_paths(&root)?;
    for path in paths {
        if !is_untracked_path(&untracked_paths, path) && !is_file_unstaged(path)? {
            return Err(eyre!("no unstaged changes to discard in {path}"));
        }
    }

    for path in paths {
        discard_worktree_file_with_untracked(&root, path, &untracked_paths)?;
    }

    Ok(())
}

fn discard_worktree_file_with_untracked(
    root: &Path,
    path: &str,
    untracked_paths: &[String],
) -> Result<()> {
    if is_untracked_path(untracked_paths, path) {
        return remove_untracked_file(root, path);
    }

    if !is_file_unstaged(path)? {
        return Err(eyre!("no unstaged changes to discard in {path}"));
    }

    restore_worktree_file(root, path)
}

pub(crate) fn discard_worktree_hunk(file: &DiffFile, hunk_index: usize) -> Result<()> {
    let hunk = file.hunks.get(hunk_index).ok_or_else(|| {
        eyre!(
            "hunk {} does not exist in {}",
            hunk_index + 1,
            file.display_path()
        )
    })?;
    let root = worktree_root()?;
    let untracked_paths = untracked_paths(&root)?;

    if is_untracked_path(&untracked_paths, file.display_path()) {
        return Err(eyre!(
            "cannot discard a single hunk from untracked file {}; discard the file from the sidebar",
            file.display_path()
        ));
    }

    discard_matching_hunks_from_worktree(&root, file, hunk, &untracked_paths)
}

pub(crate) fn worktree_root() -> Result<PathBuf> {
    git_stdout(["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .ok_or_else(|| eyre!("could not determine Git worktree root"))
}

fn load_git_diff(pre_args: &[&str], post_args: &[&str], context: &str) -> Result<Changeset> {
    let patch = git_diff_patch(pre_args, post_args, context)?;
    Ok(parse_unified_diff(&patch))
}

fn git_diff_patch(pre_args: &[&str], post_args: &[&str], context: &str) -> Result<String> {
    git_diff_patch_with_dir(None, pre_args, post_args, context)
}

fn git_diff_patch_in(
    current_dir: &Path,
    pre_args: &[&str],
    post_args: &[&str],
    context: &str,
) -> Result<String> {
    git_diff_patch_with_dir(Some(current_dir), pre_args, post_args, context)
}

fn git_diff_patch_with_dir(
    current_dir: Option<&Path>,
    pre_args: &[&str],
    post_args: &[&str],
    context: &str,
) -> Result<String> {
    let mut command = git_command_with_dir(current_dir);
    command
        .arg("diff")
        .args(pre_args)
        .args(GIT_DIFF_PATCH_ARGS)
        .args(post_args);

    let output = checked_output(&mut command, context)?;
    Ok(stdout_text(&output))
}

fn load_untracked_patches(root: &Path, untracked_paths: &[String]) -> Result<String> {
    let mut patches = String::new();

    for path in untracked_paths {
        let mut command = git_command(root);
        command.args([
            "diff",
            "--no-color",
            "--patch",
            "--no-index",
            "--default-prefix",
            "--",
        ]);
        command.arg("/dev/null").arg(path);
        let output = command.output()?;

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
        patches.push_str(&stdout_text(&output));
    }

    Ok(patches)
}

fn load_staged_diff(root: &Path) -> Result<Changeset> {
    let patch = git_diff_patch_in(root, &["--cached"], &["HEAD"], "git diff --cached failed")?;
    Ok(parse_unified_diff(&patch))
}

fn load_unstaged_diff(root: &Path, untracked_paths: &[String]) -> Result<Changeset> {
    Ok(parse_unified_diff(&load_unstaged_patch(
        root,
        untracked_paths,
    )?))
}

fn load_unstaged_patch(root: &Path, untracked_paths: &[String]) -> Result<String> {
    load_patch_with_untracked(root, &[], untracked_paths)
}

fn load_worktree_patch(root: &Path, untracked_paths: &[String]) -> Result<String> {
    load_patch_with_untracked(root, &["HEAD"], untracked_paths)
}

fn load_unpublished_patch(root: &Path, untracked_paths: &[String]) -> Result<String> {
    let Some(base_ref) = unpublished_base_ref(root) else {
        return load_worktree_patch(root, untracked_paths);
    };
    let merge_base = merge_base_in(root, &base_ref)?;
    load_patch_with_untracked(root, &[merge_base.as_str()], untracked_paths)
}

fn load_patch_with_untracked(
    root: &Path,
    post_args: &[&str],
    untracked_paths: &[String],
) -> Result<String> {
    let mut patch = git_diff_patch_in(root, &[], post_args, "git diff failed")?;
    patch.push_str(&load_untracked_patches(root, untracked_paths)?);
    Ok(patch)
}

fn annotate_stage_states(
    root: &Path,
    changeset: &mut Changeset,
    untracked_paths: &[String],
) -> Result<()> {
    for file in &mut changeset.files {
        let path = file.display_path();
        let staged = has_file_diff_in(root, path, true)?;
        let unstaged =
            has_file_diff_in(root, path, false)? || is_untracked_path(untracked_paths, path);

        file.stage = FileStage::from_staged_unstaged(staged, unstaged);
    }

    Ok(())
}

fn annotate_hunk_stage_states(
    root: &Path,
    changeset: &mut Changeset,
    untracked_paths: &[String],
) -> Result<()> {
    let staged = load_staged_diff(root)?;
    let unstaged = load_unstaged_diff(root, untracked_paths)?;
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
    let root = worktree_root()?;
    let source_changeset = match source {
        HunkPatchSource::Staged => load_staged_diff(&root)?,
        HunkPatchSource::Unstaged => {
            let untracked_paths = untracked_paths(&root)?;
            load_unstaged_diff(&root, &untracked_paths)?
        }
    };
    let source_file = matching_file(&source_changeset.files, file).ok_or_else(|| {
        eyre!(
            "no {} hunk found for {}",
            source.label(),
            file.display_path()
        )
    })?;
    let patch = overlapping_hunk_patch(source_file, selected_hunk).ok_or_else(|| {
        eyre!(
            "no {} hunk overlaps selected hunk in {}",
            source.label(),
            file.display_path()
        )
    })?;
    apply_patch_to_index(&root, &patch, source.reverse())
}

fn discard_matching_hunks_from_worktree(
    root: &Path,
    file: &DiffFile,
    selected_hunk: &DiffHunk,
    untracked_paths: &[String],
) -> Result<()> {
    let source_changeset = load_unstaged_diff(root, untracked_paths)?;
    let source_file = matching_file(&source_changeset.files, file)
        .ok_or_else(|| eyre!("no unstaged hunk found for {}", file.display_path()))?;
    let patch = overlapping_hunk_patch(source_file, selected_hunk).ok_or_else(|| {
        eyre!(
            "no unstaged hunk overlaps selected hunk in {}",
            file.display_path()
        )
    })?;
    apply_patch_to_worktree(root, &patch, true)
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

fn apply_patch_to_index(root: &Path, patch: &str, reverse: bool) -> Result<()> {
    apply_patch(root, patch, GitApplyTarget::Index, reverse)
}

fn apply_patch_to_worktree(root: &Path, patch: &str, reverse: bool) -> Result<()> {
    apply_patch(root, patch, GitApplyTarget::Worktree, reverse)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitApplyTarget {
    Index,
    Worktree,
}

fn apply_patch(root: &Path, patch: &str, target: GitApplyTarget, reverse: bool) -> Result<()> {
    let mut command = git_apply_command(root, target, reverse);
    apply_patch_from_stdin(&mut command, patch, git_apply_context(target, reverse))
}

fn git_apply_command(root: &Path, target: GitApplyTarget, reverse: bool) -> Command {
    let mut command = git_command(root);
    command.arg("apply");
    if target == GitApplyTarget::Index {
        command.arg("--cached");
    }
    command.arg("--whitespace=nowarn");
    if reverse {
        command.arg("--reverse");
    }

    command
}

fn git_apply_context(target: GitApplyTarget, reverse: bool) -> &'static str {
    match (target, reverse) {
        (GitApplyTarget::Index, true) => "git apply --cached --reverse failed",
        (GitApplyTarget::Index, false) => "git apply --cached failed",
        (GitApplyTarget::Worktree, true) => "git apply --reverse failed",
        (GitApplyTarget::Worktree, false) => "git apply failed",
    }
}

fn apply_patch_from_stdin(command: &mut Command, patch: &str, context: &str) -> Result<()> {
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

fn unpublished_base_ref(root: &Path) -> Option<String> {
    upstream_ref(root)
        .or_else(|| origin_current_branch_ref(root))
        .or_else(|| {
            DEFAULT_BASE_REFS
                .into_iter()
                .find(|candidate| git_commit_exists_in(root, candidate))
                .map(str::to_string)
        })
}

fn upstream_ref(root: &Path) -> Option<String> {
    git_stdout_in(
        root,
        [
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
}

fn origin_current_branch_ref(root: &Path) -> Option<String> {
    let branch = current_branch_name_in(root)?;
    let candidate = format!("origin/{branch}");
    git_commit_exists_in(root, &candidate).then_some(candidate)
}

fn current_branch_name_in(root: &Path) -> Option<String> {
    git_stdout_in(root, ["branch", "--show-current"])
}

fn git_commit_exists(rev: &str) -> bool {
    git_commit_exists_with_dir(None, rev)
}

fn git_commit_exists_in(root: &Path, rev: &str) -> bool {
    git_commit_exists_with_dir(Some(root), rev)
}

fn git_commit_exists_with_dir(current_dir: Option<&Path>, rev: &str) -> bool {
    git_command_with_dir(current_dir)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{rev}^{{commit}}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn merge_base(base_ref: &str) -> Result<String> {
    merge_base_with_dir(None, base_ref)
}

fn merge_base_in(root: &Path, base_ref: &str) -> Result<String> {
    merge_base_with_dir(Some(root), base_ref)
}

fn merge_base_with_dir(current_dir: Option<&Path>, base_ref: &str) -> Result<String> {
    let mut command = git_command_with_dir(current_dir);
    command.args(["merge-base", base_ref, "HEAD"]);
    let output = checked_output(
        &mut command,
        &format!("git merge-base failed for {base_ref}"),
    )?;

    let merge_base = trimmed_stdout_text(&output);
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

fn untracked_paths(root: &Path) -> Result<Vec<String>> {
    let output = checked_output(
        git_command(root).args([
            "ls-files",
            "--full-name",
            "--others",
            "--exclude-standard",
            "-z",
        ]),
        "git ls-files failed",
    )?;

    Ok(stdout_text(&output)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn git_command_with_dir(current_dir: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    command
}

fn git_command(root: &Path) -> Command {
    git_command_with_dir(Some(root))
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
    git_stdout_with_dir(None, args)
}

fn git_stdout_in<const N: usize>(root: &Path, args: [&str; N]) -> Option<String> {
    git_stdout_with_dir(Some(root), args)
}

fn git_stdout_with_dir<const N: usize>(
    current_dir: Option<&Path>,
    args: [&str; N],
) -> Option<String> {
    let output = git_command_with_dir(current_dir).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = trimmed_stdout_text(&output);
    (!value.is_empty()).then_some(value)
}

fn checked_output(command: &mut Command, context: &str) -> Result<Output> {
    let output = command.output()?;
    ensure_success(&output, context)?;
    Ok(output)
}

fn ensure_success(output: &Output, context: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }

    Err(eyre!("{}: {}", context, command_failure_text(output)))
}

fn command_failure_text(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = stderr.trim();
    let stdout = stdout.trim();

    match (stderr.is_empty(), stdout.is_empty()) {
        (false, false) => format!("{stderr}\n{stdout}"),
        (false, true) => stderr.to_string(),
        (true, false) => stdout.to_string(),
        (true, true) => format!("exit status {}", output.status),
    }
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn trimmed_stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn stage_file(path: &str) -> Result<()> {
    let root = worktree_root()?;
    checked_output(
        git_command(&root).args(["add", "--", path]),
        &format!("git add failed for {path}"),
    )
    .map(|_| ())
}

fn unstage_file(path: &str) -> Result<()> {
    let root = worktree_root()?;
    checked_output(
        git_command(&root).args(["restore", "--staged", "--", path]),
        &format!("git restore --staged failed for {path}"),
    )
    .map(|_| ())
}

fn restore_worktree_file(root: &Path, path: &str) -> Result<()> {
    checked_output(
        git_command(root).args(["restore", "--worktree", "--", path]),
        &format!("git restore --worktree failed for {path}"),
    )
    .map(|_| ())
}

fn remove_untracked_file(root: &Path, path: &str) -> Result<()> {
    let full_path = root.join(path);
    fs::remove_file(&full_path)
        .map_err(|error| eyre!("failed to remove untracked file {path}: {error}"))
}

fn is_file_staged(path: &str) -> Result<bool> {
    has_file_diff(path, true)
}

fn is_file_unstaged(path: &str) -> Result<bool> {
    has_file_diff(path, false)
}

fn has_file_diff(path: &str, cached: bool) -> Result<bool> {
    let root = worktree_root()?;
    has_file_diff_in(&root, path, cached)
}

fn has_file_diff_in(root: &Path, path: &str, cached: bool) -> Result<bool> {
    let mut command = git_command(root);
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

    #[test]
    fn toggle_hunk_staging_handles_git_quoted_paths() {
        let _lock = GIT_CWD_LOCK.lock().expect("git cwd lock");
        let root = temp_root();
        let cwd = CurrentDirGuard::enter(&root);
        let path = "tab\tfile.txt";

        run_git(["init"]);
        run_git(["config", "user.email", "chunk@example.test"]);
        run_git(["config", "user.name", "Chunk Test"]);
        fs::write(path, "old\n").unwrap();
        run_git(["add", path]);
        run_git(["commit", "-m", "initial"]);

        fs::write(path, "new\n").unwrap();
        let changeset = load_worktree_diff().unwrap();
        let file = changeset
            .files
            .iter()
            .find(|file| file.display_path() == path)
            .expect("changed tabbed path should be parsed");

        toggle_staging_for_hunk(file, 0).unwrap();

        let cached = git_output(["diff", "--cached", "--", path]);
        assert!(cached.contains("new"));
        let unstaged = git_output(["diff", "--", path]);
        assert!(unstaged.is_empty());

        drop(cwd);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discard_hunk_reverts_only_selected_unstaged_hunk() {
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

        discard_worktree_hunk(file, 0).unwrap();

        let unstaged = git_output(["diff", "--", "sample.txt"]);
        assert!(!unstaged.contains("line two"));
        assert!(unstaged.contains("line eighteen"));
        let content = fs::read_to_string("sample.txt").unwrap();
        assert!(content.contains("line 2\n"));
        assert!(content.contains("line eighteen\n"));

        drop(cwd);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discard_file_reverts_unstaged_changes_and_preserves_index() {
        let _lock = GIT_CWD_LOCK.lock().expect("git cwd lock");
        let root = temp_root();
        let cwd = CurrentDirGuard::enter(&root);

        run_git(["init"]);
        run_git(["config", "user.email", "chunk@example.test"]);
        run_git(["config", "user.name", "Chunk Test"]);
        fs::write("sample.txt", numbered_lines()).unwrap();
        run_git(["add", "sample.txt"]);
        run_git(["commit", "-m", "initial"]);

        fs::write("sample.txt", line_two_changed()).unwrap();
        run_git(["add", "sample.txt"]);
        fs::write("sample.txt", line_two_and_eighteen_changed()).unwrap();

        discard_worktree_file("sample.txt").unwrap();

        let cached = git_output(["diff", "--cached", "--", "sample.txt"]);
        assert!(cached.contains("line two"));
        assert!(!cached.contains("line eighteen"));
        let unstaged = git_output(["diff", "--", "sample.txt"]);
        assert!(!unstaged.contains("line two"));
        assert!(!unstaged.contains("line eighteen"));
        let content = fs::read_to_string("sample.txt").unwrap();
        assert!(content.contains("line two\n"));
        assert!(content.contains("line 18\n"));

        drop(cwd);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discard_file_removes_untracked_file() {
        let _lock = GIT_CWD_LOCK.lock().expect("git cwd lock");
        let root = temp_root();
        let cwd = CurrentDirGuard::enter(&root);

        run_git(["init"]);
        run_git(["config", "user.email", "chunk@example.test"]);
        run_git(["config", "user.name", "Chunk Test"]);
        fs::write("sample.txt", "base\n").unwrap();
        run_git(["add", "sample.txt"]);
        run_git(["commit", "-m", "initial"]);

        fs::write("new.txt", "untracked\n").unwrap();

        discard_worktree_file("new.txt").unwrap();

        assert!(!Path::new("new.txt").exists());

        drop(cwd);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worktree_diff_uses_root_relative_paths_for_untracked_files_from_subdir() {
        let _lock = GIT_CWD_LOCK.lock().expect("git cwd lock");
        let root = temp_root();
        let cwd = CurrentDirGuard::enter(&root);

        run_git(["init"]);
        run_git(["config", "user.email", "chunk@example.test"]);
        run_git(["config", "user.name", "Chunk Test"]);
        fs::create_dir_all("crates/clickup_graph/src/tui").unwrap();
        fs::write("crates/clickup_graph/src/tui/mod.rs", "base\n").unwrap();
        run_git(["add", "."]);
        run_git(["commit", "-m", "initial"]);

        let subdir_cwd = CurrentDirGuard::enter(&root.join("crates/clickup_graph"));
        fs::write("src/tui/mod.rs", "changed\n").unwrap();
        fs::write("src/tui/app.rs", "new\n").unwrap();

        let changeset = load_worktree_diff().unwrap();
        let paths = changeset
            .files
            .iter()
            .map(|file| file.display_path())
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![
                "crates/clickup_graph/src/tui/mod.rs",
                "crates/clickup_graph/src/tui/app.rs"
            ]
        );

        drop(subdir_cwd);
        drop(cwd);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unpublished_diff_text_includes_local_commits_index_worktree_and_untracked_files() {
        let _lock = GIT_CWD_LOCK.lock().expect("git cwd lock");
        let root = temp_root();
        let remote = temp_root();
        let cwd = CurrentDirGuard::enter(&root);

        run_git(["init", "--bare", remote.to_str().expect("remote path")]);
        run_git(["init"]);
        run_git(["config", "user.email", "chunk@example.test"]);
        run_git(["config", "user.name", "Chunk Test"]);
        run_git(["branch", "-M", "main"]);
        run_git([
            "remote",
            "add",
            "origin",
            remote.to_str().expect("remote path"),
        ]);
        fs::write("committed.txt", "base committed\n").unwrap();
        fs::write("indexed.txt", "base indexed\n").unwrap();
        fs::write("unstaged.txt", "base unstaged\n").unwrap();
        run_git(["add", "."]);
        run_git(["commit", "-m", "initial"]);
        run_git(["push", "-u", "origin", "main"]);

        fs::write("committed.txt", "local committed change\n").unwrap();
        run_git(["add", "committed.txt"]);
        run_git(["commit", "-m", "local change"]);
        fs::write("indexed.txt", "indexed change\n").unwrap();
        run_git(["add", "indexed.txt"]);
        fs::write("unstaged.txt", "unstaged change\n").unwrap();
        fs::write("new.txt", "untracked change\n").unwrap();

        let diff = load_unpublished_diff_text().unwrap();

        assert!(diff.text.contains("committed.txt"));
        assert!(diff.text.contains("local committed change"));
        assert!(diff.text.contains("indexed.txt"));
        assert!(diff.text.contains("indexed change"));
        assert!(diff.text.contains("unstaged.txt"));
        assert!(diff.text.contains("unstaged change"));
        assert!(diff.text.contains("new.txt"));
        assert!(diff.text.contains("untracked change"));

        drop(cwd);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(remote).unwrap();
    }

    #[test]
    fn discard_files_reverts_multiple_worktree_paths() {
        let _lock = GIT_CWD_LOCK.lock().expect("git cwd lock");
        let root = temp_root();
        let cwd = CurrentDirGuard::enter(&root);

        run_git(["init"]);
        run_git(["config", "user.email", "chunk@example.test"]);
        run_git(["config", "user.name", "Chunk Test"]);
        fs::create_dir_all("src").unwrap();
        fs::write("src/a.txt", "a base\n").unwrap();
        fs::write("src/b.txt", "b base\n").unwrap();
        fs::write("outside.txt", "outside base\n").unwrap();
        run_git(["add", "."]);
        run_git(["commit", "-m", "initial"]);

        fs::write("src/a.txt", "a changed\n").unwrap();
        fs::write("src/b.txt", "b changed\n").unwrap();
        fs::write("src/new.txt", "new\n").unwrap();
        fs::write("outside.txt", "outside changed\n").unwrap();

        discard_worktree_files(&[
            "src/a.txt".to_string(),
            "src/b.txt".to_string(),
            "src/new.txt".to_string(),
        ])
        .unwrap();

        assert_eq!(fs::read_to_string("src/a.txt").unwrap(), "a base\n");
        assert_eq!(fs::read_to_string("src/b.txt").unwrap(), "b base\n");
        assert!(!Path::new("src/new.txt").exists());
        assert_eq!(
            fs::read_to_string("outside.txt").unwrap(),
            "outside changed\n"
        );
        assert!(git_output(["diff", "--", "src/a.txt", "src/b.txt"]).is_empty());

        drop(cwd);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stage_files_stages_multiple_worktree_paths() {
        let _lock = GIT_CWD_LOCK.lock().expect("git cwd lock");
        let root = temp_root();
        let cwd = CurrentDirGuard::enter(&root);

        run_git(["init"]);
        run_git(["config", "user.email", "chunk@example.test"]);
        run_git(["config", "user.name", "Chunk Test"]);
        fs::create_dir_all("src").unwrap();
        fs::write("src/a.txt", "a base\n").unwrap();
        fs::write("src/b.txt", "b base\n").unwrap();
        fs::write("outside.txt", "outside base\n").unwrap();
        run_git(["add", "."]);
        run_git(["commit", "-m", "initial"]);

        fs::write("src/a.txt", "a changed\n").unwrap();
        fs::write("src/b.txt", "b changed\n").unwrap();
        fs::write("src/new.txt", "new\n").unwrap();
        fs::write("outside.txt", "outside changed\n").unwrap();

        stage_files(&[
            "src/a.txt".to_string(),
            "src/b.txt".to_string(),
            "src/new.txt".to_string(),
        ])
        .unwrap();

        let staged = git_output(["diff", "--cached", "--name-only"]);
        assert!(staged.contains("src/a.txt"));
        assert!(staged.contains("src/b.txt"));
        assert!(staged.contains("src/new.txt"));
        assert!(!staged.contains("outside.txt"));

        let unstaged = git_output(["diff", "--name-only"]);
        assert!(!unstaged.contains("src/a.txt"));
        assert!(!unstaged.contains("src/b.txt"));
        assert!(unstaged.contains("outside.txt"));

        drop(cwd);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unstage_files_unstages_multiple_worktree_paths() {
        let _lock = GIT_CWD_LOCK.lock().expect("git cwd lock");
        let root = temp_root();
        let cwd = CurrentDirGuard::enter(&root);

        run_git(["init"]);
        run_git(["config", "user.email", "chunk@example.test"]);
        run_git(["config", "user.name", "Chunk Test"]);
        fs::create_dir_all("src").unwrap();
        fs::write("src/a.txt", "a base\n").unwrap();
        fs::write("src/b.txt", "b base\n").unwrap();
        fs::write("outside.txt", "outside base\n").unwrap();
        run_git(["add", "."]);
        run_git(["commit", "-m", "initial"]);

        fs::write("src/a.txt", "a changed\n").unwrap();
        fs::write("src/b.txt", "b changed\n").unwrap();
        fs::write("outside.txt", "outside changed\n").unwrap();
        run_git(["add", "src/a.txt", "src/b.txt", "outside.txt"]);

        unstage_files(&["src/a.txt".to_string(), "src/b.txt".to_string()]).unwrap();

        let staged = git_output(["diff", "--cached", "--name-only"]);
        assert!(!staged.contains("src/a.txt"));
        assert!(!staged.contains("src/b.txt"));
        assert!(staged.contains("outside.txt"));

        let unstaged = git_output(["diff", "--name-only"]);
        assert!(unstaged.contains("src/a.txt"));
        assert!(unstaged.contains("src/b.txt"));

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

    fn line_two_changed() -> String {
        let mut lines = numbered_lines_vec();
        lines[1] = "line two".to_string();
        format!("{}\n", lines.join("\n"))
    }

    fn line_two_and_eighteen_changed() -> String {
        let mut lines = numbered_lines_vec();
        lines[1] = "line two".to_string();
        lines[17] = "line eighteen".to_string();
        format!("{}\n", lines.join("\n"))
    }

    fn numbered_lines_vec() -> Vec<String> {
        (1..=20).map(|line| format!("line {line}")).collect()
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
