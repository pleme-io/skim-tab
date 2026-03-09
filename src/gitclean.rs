//! skim-gitclean — delete merged git branches.
//!
//! Lists branches merged into the current branch, filters out protected
//! branches (main, master, develop), and deletes the rest.

use std::process::Command;

use anyhow::{Context, Result};

/// Branches that should never be deleted.
const PROTECTED: &[&str] = &["main", "master", "develop"];

fn main() -> Result<()> {
    let output = Command::new("git")
        .args(["branch", "--merged"])
        .output()
        .context("failed to run git branch --merged")?;

    if !output.status.success() {
        anyhow::bail!("not a git repository");
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let to_delete: Vec<&str> = raw
        .lines()
        .map(|l| l.trim().trim_start_matches("* "))
        .filter(|b| !b.is_empty() && !PROTECTED.contains(b))
        .collect();

    if to_delete.is_empty() {
        eprintln!("No merged branches to clean");
        return Ok(());
    }

    for branch in &to_delete {
        let status = Command::new("git")
            .args(["branch", "-d", branch])
            .status()
            .with_context(|| format!("failed to delete branch {branch}"))?;

        if status.success() {
            eprintln!("Deleted branch {branch}");
        }
    }

    Ok(())
}
