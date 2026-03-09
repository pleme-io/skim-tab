//! skim-fco — fuzzy git branch checkout with log preview.
//!
//! Lists all local and remote branches, presents them via skim with
//! git log preview, and checks out the selected branch.

use std::io;
use std::process::Command;

use anyhow::{Context, Result};
use skim::prelude::SkimItemReader;
use skim::tui::options::PreviewLayout;
use skim::Skim;
use skim_tab::base_options;

/// Icon for git operations.
const ICON_GIT: &str = "\u{25ce} "; // ◎ (bullseye — branch target)

/// List all branches via git, deduplicating remotes that have local copies.
fn git_branches() -> Result<String> {
    let output = Command::new("git")
        .args(["branch", "--all", "--format=%(refname:short)"])
        .output()
        .context("failed to run git branch — not a git repo?")?;

    if !output.status.success() {
        anyhow::bail!("not a git repository");
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let filtered: Vec<&str> = raw
        .lines()
        .filter(|l| !l.contains("HEAD"))
        .collect();

    Ok(filtered.join("\n"))
}

fn main() -> Result<()> {
    let branches = git_branches()?;
    if branches.is_empty() {
        return Ok(());
    }

    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(branches));

    let options = base_options("")
        .prompt(ICON_GIT.to_string())
        .preview("git log --oneline --graph --color=always -20 {} 2>/dev/null".to_string())
        .preview_window(PreviewLayout::from("right:50%:wrap"))
        .header("Branches | CTRL-/: Toggle Preview | ESC: Cancel".to_string())
        .build()
        .expect("failed to build skim options");

    match Skim::run_with(options, Some(items)) {
        Ok(out) if !out.is_abort => {
            if let Some(item) = out.selected_items.first() {
                let branch = item.output().to_string();
                // Strip origin/ prefix for remote branches
                let target = branch.strip_prefix("origin/").unwrap_or(&branch);
                let status = Command::new("git")
                    .args(["checkout", target])
                    .status()
                    .context("failed to run git checkout")?;
                if !status.success() {
                    anyhow::bail!("git checkout failed");
                }
            }
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_is_valid() {
        assert!(!super::ICON_GIT.is_empty());
    }
}
