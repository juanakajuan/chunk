use std::process::Command;

use color_eyre::eyre::{Result, eyre};

use crate::model::Changeset;
use crate::patch::parse_unified_diff;

pub fn load_worktree_diff() -> Result<Changeset> {
    let output = Command::new("git")
        .args(["diff", "--no-color", "--patch", "--find-renames", "HEAD"])
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

fn load_untracked_patches() -> Result<String> {
    let mut patches = String::new();

    for path in untracked_paths()? {
        let output = Command::new("git")
            .args(["diff", "--no-color", "--patch", "--no-index", "--"])
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
