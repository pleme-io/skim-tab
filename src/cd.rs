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
use skim_tab::{
    FdTarget, ICON_CD, base_options, build_options, fd_discover, parse_query, run_skim_on_tty,
    shell_quote,
};

/// Preview command: eza tree.
fn preview_command() -> String {
    "eza --tree --level=2 --icons --color=always {} 2>/dev/null".to_string()
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let query = parse_query(&args);

    // Frecency-ranked directories first (fleet-wide, includes indexer-
    // discovered) from the shared wadachi store — skim-cd is a READER, it never
    // records (the shell already did at cd time). Graceful if the store is
    // empty/absent. Then cwd-local `fd` discoveries, de-duplicated.
    let mut entries_vec: Vec<String> = pleme_io_wadachi::top_n("", 200)
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.path.to_string_lossy().into_owned())
        .collect();
    let mut seen: std::collections::HashSet<String> = entries_vec.iter().cloned().collect();
    for dir in fd_discover(FdTarget::Directories)?.lines() {
        if seen.insert(dir.to_string()) {
            entries_vec.push(dir.to_string());
        }
    }
    if entries_vec.is_empty() {
        return Ok(());
    }
    let entries = entries_vec.join("\n");

    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(entries));

    let options = build_options(
        base_options(query)
            // Preserve frecency order until the user types (skim would otherwise
            // re-sort) — the same recipe as the completion picker.
            .no_sort(true)
            .scheme(MatchScheme::Path)
            .prompt(ICON_CD.to_string())
            .preview(preview_command())
            .preview_window(PreviewLayout::from("right:50%:wrap"))
            .header("Directories (frecency) | CTRL-/: Toggle Preview | ESC: Cancel".to_string()),
    )?;

    match run_skim_on_tty(options, Some(items)) {
        Some(out) if !out.is_abort => {
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
