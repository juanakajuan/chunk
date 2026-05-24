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

    let patch = String::from_utf8_lossy(&output.stdout);
    let mut changeset = parse_unified_diff(&patch);
    changeset.title = worktree_title();
    changeset.source_label = "git diff HEAD".to_string();
    Ok(changeset)
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
