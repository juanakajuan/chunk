use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use color_eyre::eyre::{Result, eyre};

use crate::model::{Changeset, DiffFile, FileStage, SourceSnapshot};
use crate::patch::parse_unified_diff;

const MAX_SOURCE_CONTEXT_BYTES: usize = 512 * 1024;

pub fn load_worktree_diff() -> Result<Changeset> {
    let output = Command::new("git")
        .args([
            "diff",
            "--no-color",
            "--patch",
            "--find-renames",
            "--default-prefix",
            "HEAD",
        ])
        .output()?;

    ensure_success(&output, "git diff failed")?;

    let untracked_paths = untracked_paths()?;
    let mut patch = String::from_utf8_lossy(&output.stdout).to_string();
    patch.push_str(&load_untracked_patches(&untracked_paths)?);
    let mut changeset = parse_unified_diff(&patch);
    annotate_stage_states(&mut changeset, &untracked_paths)?;
    changeset.title = worktree_title();
    changeset.source_label = "git diff HEAD + untracked".to_string();
    Ok(changeset)
}

pub fn load_source_snapshots(file: &mut DiffFile) {
    if file.binary || file.hunks.is_empty() {
        return;
    }

    let root = worktree_root();
    load_source_snapshots_with_root(file, root.as_deref());
}

pub fn toggle_staging_for_file(path: &str) -> Result<()> {
    if is_file_staged(path)? {
        unstage_file(path)?;
    } else {
        stage_file(path)?;
    }

    Ok(())
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

fn annotate_stage_states(changeset: &mut Changeset, untracked_paths: &[String]) -> Result<()> {
    for file in &mut changeset.files {
        let path = file.display_path();
        let staged = is_file_staged(path)?;
        let unstaged =
            is_file_unstaged(path)? || untracked_paths.iter().any(|candidate| candidate == path);

        file.stage = match (staged, unstaged) {
            (true, true) => FileStage::Mixed,
            (true, false) => FileStage::Staged,
            (false, _) => FileStage::Unstaged,
        };
    }

    Ok(())
}

fn load_source_snapshots_with_root(file: &mut DiffFile, worktree_root: Option<&Path>) {
    if file.old_source.is_unloaded() {
        file.old_source = load_head_source_prefix(&file.old_path, max_old_context_line(file));
    }

    if file.new_source.is_unloaded() {
        file.new_source =
            load_worktree_source_prefix(worktree_root, &file.path, max_new_context_line(file));
    }
}

fn max_old_context_line(file: &DiffFile) -> u32 {
    file.hunks
        .iter()
        .map(|hunk| hunk.old_start.saturating_sub(1))
        .max()
        .unwrap_or(0)
}

fn max_new_context_line(file: &DiffFile) -> u32 {
    file.hunks
        .iter()
        .map(|hunk| hunk.new_start.saturating_sub(1))
        .max()
        .unwrap_or(0)
}

fn load_head_source_prefix(path: &str, max_context_line: u32) -> SourceSnapshot {
    if max_context_line == 0 {
        return SourceSnapshot::loaded(String::new());
    }

    if path.is_empty() {
        return SourceSnapshot::Unavailable;
    }

    let mut child = match Command::new("git")
        .args(["show", "--textconv", &format!("HEAD:{path}")])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return SourceSnapshot::Unavailable,
    };

    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return SourceSnapshot::Unavailable;
    };

    let mut reader = BufReader::new(stdout);
    let prefix = match read_source_prefix(&mut reader, max_context_line) {
        Ok(prefix) => prefix,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return SourceSnapshot::Unavailable;
        }
    };

    if prefix.reached_line_limit {
        stop_child(child);
        return SourceSnapshot::loaded(prefix.content);
    }

    match child.wait() {
        Ok(status) if status.success() => SourceSnapshot::loaded(prefix.content),
        _ => SourceSnapshot::Unavailable,
    }
}

fn load_worktree_source_prefix(
    worktree_root: Option<&Path>,
    path: &str,
    max_context_line: u32,
) -> SourceSnapshot {
    if max_context_line == 0 {
        return SourceSnapshot::loaded(String::new());
    }

    if path.is_empty() {
        return SourceSnapshot::Unavailable;
    }

    let path = match worktree_root {
        Some(root) => root.join(path),
        None => PathBuf::from(path),
    };

    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return SourceSnapshot::Unavailable,
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

struct SourcePrefix {
    content: String,
    reached_line_limit: bool,
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
                reached_line_limit: false,
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
        reached_line_limit: true,
    })
}

fn stop_child(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn worktree_root() -> Option<PathBuf> {
    git_stdout(["rev-parse", "--show-toplevel"]).map(PathBuf::from)
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
    git_stdout(["branch", "--show-current"])
        .filter(|branch| !branch.is_empty())
        .or_else(|| {
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
    let status = Command::new("git")
        .args(["diff", "--cached", "--quiet", "--"])
        .arg(path)
        .status()?;

    match status.code() {
        Some(0) => Ok(false), // no staged diff for path
        Some(1) => Ok(true),  // staged diff exists
        _ => Err(eyre!("git diff --cached failed for {path}")),
    }
}

fn is_file_unstaged(path: &str) -> Result<bool> {
    let status = Command::new("git")
        .args(["diff", "--quiet", "--"])
        .arg(path)
        .status()?;

    match status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(eyre!("git diff failed for {path}")),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::model::{DiffHunk, FileStatus};

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
                lines: Vec::new(),
            }],
            binary: false,
        };

        load_source_snapshots_with_root(&mut file, Some(&root));

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
