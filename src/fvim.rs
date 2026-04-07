//! skim-fvim — fuzzy file picker that opens selection in $EDITOR.
//!
//! Runs `fd` to discover files, presents them via skim with bat preview,
//! and opens the selected file in the user's editor.

use std::io;
use std::process::Command;

use anyhow::{Context, Result};
use skim::options::MatchScheme;
use skim::prelude::SkimItemReader;
use skim::tui::options::PreviewLayout;
use skim::Skim;
use skim_tab::{base_options, build_options, editor, ICON_FILES};

/// Run fd to discover files only.
fn discover_files() -> Result<String> {
    let output = Command::new("fd")
        .args([
            "--type",
            "f",
            "--hidden",
            "--follow",
            "--exclude",
            ".git",
            "--exclude",
            "node_modules",
            "--exclude",
            "target",
            "--exclude",
            "__pycache__",
            "--exclude",
            ".direnv",
            "--strip-cwd-prefix",
        ])
        .output()
        .context("failed to run fd — is it installed?")?;

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn main() -> Result<()> {
    let entries = discover_files()?;
    if entries.is_empty() {
        return Ok(());
    }

    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(entries));

    let options = build_options(
        base_options("")
            .scheme(MatchScheme::Path)
            .prompt(ICON_FILES.to_string())
            .preview(
                "bat --color=always --style=numbers --line-range=:200 {} 2>/dev/null".to_string(),
            )
            .preview_window(PreviewLayout::from("right:60%:wrap"))
            .header("Files → Editor | CTRL-/: Toggle Preview | ESC: Cancel".to_string()),
    )?;

    match Skim::run_with(options, Some(items)) {
        Ok(out) if !out.is_abort => {
            if let Some(item) = out.selected_items.first() {
                let file = item.output().to_string();
                let ed = editor();
                Command::new(&ed)
                    .arg(&file)
                    .status()
                    .with_context(|| format!("failed to launch {ed}"))?;
            }
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_files_runs() {
        // Just verify it doesn't panic — fd may or may not be in PATH in CI
        let _ = discover_files();
    }
}
