use std::process::Command;

use color_eyre::eyre::{Result, eyre};

use crate::model::Changeset;
use crate::patch::parse_unified_diff;

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

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("git diff failed: {}", stderr.trim()));
    }

    let mut patch = String::from_utf8_lossy(&output.stdout).to_string();
    patch.push_str(&load_untracked_patches()?);
    let mut changeset = parse_unified_diff(&patch);
    changeset.title = worktree_title();
    changeset.source_label = "git diff HEAD + untracked".to_string();
    Ok(changeset)
}

pub fn toggle_staging_for_file(path: &str) -> Result<()> {
    if is_file_staged(path)? {
        unstage_file(path)?;
    } else {
        stage_file(path)?;
    }

    Ok(())
}

fn load_untracked_patches() -> Result<String> {
    let mut patches = String::new();

    for path in untracked_paths()? {
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
            .arg(&path)
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

fn untracked_paths() -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("git ls-files failed: {}", stderr.trim()));
    }

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

fn stage_file(path: &str) -> Result<()> {
    let output = Command::new("git").args(["add", "--", path]).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("git add failed for {}: {}", path, stderr.trim()));
    }

    Ok(())
}

fn unstage_file(path: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["restore", "--staged", "--", path])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!(
            "git restore --staged failed for {}: {}",
            path,
            stderr.trim()
        ));
    }

    Ok(())
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
