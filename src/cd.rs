//! skim-cd — Rust-native fuzzy directory navigator for zsh.
//!
//! Runs `fd` to discover directories, presents them via skim with
//! eza tree preview, and prints the selected directory to stdout.

use std::env;
use std::io;

use anyhow::Result;
use skim::options::MatchScheme;
use skim::prelude::SkimItemReader;
use skim::tui::options::PreviewLayout;
use skim::Skim;
use skim_tab::{base_options, build_options, fd_discover, parse_query, shell_quote, FdTarget, ICON_CD};

/// Preview command: eza tree.
fn preview_command() -> String {
    "eza --tree --level=2 --icons --color=always {} 2>/dev/null".to_string()
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let query = parse_query(&args);

    let entries = fd_discover(FdTarget::Directories)?;
    if entries.is_empty() {
        return Ok(());
    }

    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(entries));

    let options = build_options(
        base_options(query)
            .scheme(MatchScheme::Path)
            .prompt(ICON_CD.to_string())
            .preview(preview_command())
            .preview_window(PreviewLayout::from("right:50%:wrap"))
            .header("Directories | CTRL-/: Toggle Preview | ESC: Cancel".to_string()),
    )?;

    match Skim::run_with(options, Some(items)) {
        Ok(out) if !out.is_abort => {
            if let Some(item) = out.selected_items.first() {
                print!("{}", shell_quote(&item.output()));
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
    fn preview_command_uses_eza() {
        let cmd = preview_command();
        assert!(cmd.contains("eza"));
        assert!(cmd.contains("--tree"));
    }
}
