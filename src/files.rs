//! skim-files — Rust-native fuzzy file/directory picker for zsh.
//!
//! Runs `fd` to discover files and directories, presents them via skim
//! with bat/eza preview, and prints selected paths to stdout.
//! Supports multi-select (tab to toggle).

use std::env;
use std::io;

use anyhow::Result;
use skim::options::MatchScheme;
use skim::prelude::SkimItemReader;
use skim::tui::options::PreviewLayout;
use skim_tab::{
    FdTarget, ICON_FILES, ICON_MARKER, base_options, build_options, fd_discover,
    indexed_dirs_under, merge_sources, parse_query, run_skim_on_tty, shell_quote,
};

/// Frecent directories pulled into the feed's head.
///
/// **50, not a larger number, because the spec caps it and asking for more is a
/// bound we do not have.** `FrecencyRankingSpec::skimtab_parity()`'s phase list
/// ends with `TopK { n: 50 }`, so `wadachi::top_n(needle, 200)` returns 50 —
/// measured, after this constant was first written as 200. `skim-cd` passes 200
/// for the same call and also receives 50; the difference is that this says so.
const FRECENT_HEAD: usize = 50;

/// Bound on the background-indexed tail. `fd` itself allows 100k, and the
/// measured `discovered` set under the operator's org root is ~25k, so this is
/// headroom rather than a real cut — but an unbounded tail is a latent cost.
const INDEXED_TAIL: usize = 50_000;

/// Preview command: directories get eza tree, files get bat.
fn preview_command() -> String {
    "if [ -d {} ]; then \
        eza --tree --level=2 --icons --color=always {} 2>/dev/null; \
    else \
        bat --color=always --style=numbers --line-range=:500 {} 2>/dev/null; \
    fi"
    .to_string()
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let query = parse_query(&args);

    // ── Three sources, each earning its slot ────────────────────────────
    //
    // 1. FRECENCY — the operator's most-worn directories, fleet-wide, from the
    //    shared wadachi store. This is the "how often I use it drives where it
    //    shows up" half. Same reader pattern as `skim-cd`: empty needle, because
    //    wadachi's `MatchNeedle` phase is path-shaped and the needle belongs to
    //    skim, not the store. Never records — the shell already did.
    // 2. `fd` — live cwd truth, and the only source of FILES.
    // 3. INDEXED TAIL — background-indexed dirs under the cwd, for the trees
    //    `fd` structurally cannot see. Measured: the pleme-io org root yields 6
    //    entries from `fd` (its .gitignore is `*`) against 25,393 indexed dirs.
    //
    // Deliberately ADDITIVE: `fd`'s own output keeps its relative order and
    // `no_sort` stays OFF, so typing still fuzzy-ranks exactly as before. Adding
    // ranking to the *head* while leaving skim's sort intact is what makes this
    // a coverage win with no ordering regression — flipping `no_sort` here
    // without a fully ranked feed would trade score-ordering for fd-order and
    // is a separate, measured change.
    let frecent: Vec<String> = pleme_io_wadachi::top_n("", FRECENT_HEAD)
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.path.to_string_lossy().into_owned())
        .collect();

    let fd_out = fd_discover(FdTarget::FilesAndDirs)?;
    let fd_lines: Vec<String> = fd_out.lines().map(str::to_owned).collect();

    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let indexed = indexed_dirs_under(&cwd, INDEXED_TAIL);

    let merged = merge_sources(&[&frecent, &fd_lines, &indexed]);

    // `skim-files --feed-report [N]` — the per-source contribution to the feed
    // the picker WOULD show. A picker is only inspectable through a TTY, so
    // without this there is no way to verify a feed change against the real
    // store rather than against fixtures. Sibling of `skim-history
    // --rank-report`, same reasoning.
    if let Some(pos) = args.iter().position(|a| a == "--feed-report") {
        let top: usize = args.get(pos + 1).and_then(|s| s.parse().ok()).unwrap_or(10);
        println!("cwd            {}", cwd.display());
        println!("frecent        {}", frecent.len());
        println!("fd             {}", fd_lines.len());
        println!("indexed dirs   {}", indexed.len());
        println!("merged (dedup) {}", merged.len());
        for (i, e) in merged.iter().take(top).enumerate() {
            println!("{:>4}  {e}", i + 1);
        }
        return Ok(());
    }

    if merged.is_empty() {
        return Ok(());
    }
    let entries = merged.join("\n");

    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(entries));

    let options = build_options(
        base_options(query)
            .scheme(MatchScheme::Path)
            .multi(true)
            .prompt(ICON_FILES.to_string())
            .multi_select_icon(ICON_MARKER.to_string())
            .preview(preview_command())
            .preview_window(PreviewLayout::from("right:50%:wrap"))
            .header("TAB: Multi-select | CTRL-/: Toggle Preview | ESC: Cancel".to_string()),
    )?;

    match run_skim_on_tty(options, Some(items)) {
        Some(out) if !out.is_abort => {
            let paths: Vec<String> = out
                .selected_items
                .iter()
                .map(|item| shell_quote(&item.output()))
                .collect();
            if !paths.is_empty() {
                print!("{}", paths.join(" "));
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
    fn preview_command_is_valid_shell() {
        let cmd = preview_command();
        assert!(cmd.contains("bat"));
        assert!(cmd.contains("eza"));
    }
}
