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
    changeset.title = "Working tree changes".to_string();
    changeset.source_label = "git diff HEAD".to_string();
    Ok(changeset)
}
