//! In-picker directory descent — optional feature for skim-tab.
//!
//! When enabled, selecting a directory in the skim picker triggers an
//! in-process readdir + new skim session, letting the user navigate
//! a directory tree without returning to zsh between levels.
//!
//! Disabled by default. Enable via shikumi config:
//!   completion.in_picker_descent = true
//!
//! When disabled (default), directories are returned to zsh with a
//! trailing `/` and the user tabs again for the next level.

use crate::{base_options, ICON_CD};
use lscolors::LsColors;
use skim::prelude::*;
use std::io;
use std::path::Path;

use super::complete::{Candidate, Selection};

// ── Path helpers ─────────────────────────────────────────────────────

/// Expand `~` to `$HOME`.
pub fn expand_home(path: &str) -> String {
    if path.starts_with('~') {
        std::env::var("HOME")
            .map(|h| path.replacen('~', &h, 1))
            .unwrap_or_else(|_| path.to_string())
    } else {
        path.to_string()
    }
}

/// Resolve the filesystem path for a candidate.
/// Descent candidates have `realdir=""` and `word` is the full relative path.
pub fn candidate_fs_path(c: &Candidate) -> String {
    let raw = if c.realdir.is_empty() {
        c.word.clone()
    } else {
        format!("{}{}", c.realdir, c.word)
    };
    expand_home(&raw)
}

/// Check if a candidate is a directory on the filesystem.
pub fn is_dir_candidate(c: &Candidate) -> bool {
    c.is_file && Path::new(&candidate_fs_path(c)).is_dir()
}

// ── Directory reading ────────────────────────────────────────────────

/// Read a directory and build file-completion candidates.
///
/// `base_dir` is the filesystem path to read.
/// `prefix_path` is the accumulated user-visible path prefix (e.g., `.git/hooks/`).
/// `dirs_only` filters to directories (for cd/pushd/z/rmdir).
pub fn readdir_candidates(base_dir: &str, prefix_path: &str, dirs_only: bool) -> Vec<Candidate> {
    let Ok(entries) = std::fs::read_dir(base_dir) else {
        return vec![];
    };
    let mut candidates: Vec<Candidate> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            if dirs_only {
                e.path().is_dir() // follows symlinks
            } else {
                true
            }
        })
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            Candidate {
                word: format!("{prefix_path}{name}"),
                display: name,
                is_file: true,
                realdir: String::new(),
                ..Candidate::default()
            }
        })
        .collect();
    candidates.sort_by(|a, b| a.display.cmp(&b.display));
    candidates
}

// ── Skim picker for descent ──────────────────────────────────────────

/// Run a skim session showing directory contents.
/// Returns the selected candidate's display text, or None on abort/ESC.
pub fn run_descent_picker(
    candidates: &[Candidate],
    path_so_far: &str,
    ls_colors: &LsColors,
) -> Option<String> {
    if candidates.is_empty() {
        return None;
    }

    let display_lines: Vec<String> = candidates
        .iter()
        .map(|c| {
            let full_path = candidate_fs_path(c);
            ls_colors
                .style_for_path(&full_path)
                .map(|s| s.to_nu_ansi_term_style().paint(&c.display).to_string())
                .unwrap_or_else(|| c.display.clone())
        })
        .collect();

    let header = if path_so_far.is_empty() {
        "Enter: descend | ESC: cancel".to_string()
    } else {
        format!("{path_so_far} | Enter: descend | ESC: accept parent")
    };

    let mut builder = base_options("");
    builder
        .multi(false)
        .prompt(ICON_CD.to_string())
        .header(header)
        .height("40%".to_string())
        .cycle(true)
        .no_sort(true);

    let skim_opts = builder.build().ok()?;
    let items_text = display_lines.join("\n");
    let reader = SkimItemReader::new(SkimItemReaderOption::default().ansi(true));
    let items = reader.of_bufread(io::Cursor::new(items_text));

    let output = Skim::run_with(skim_opts, Some(items)).ok()?;
    if output.is_abort {
        return None;
    }

    let selected = if output.selected_items.is_empty() {
        output.current.as_ref().map(|c| c.output().to_string())
    } else {
        output
            .selected_items
            .first()
            .map(|s| s.item.output().to_string())
    };

    selected.map(|s| crate::strip_ansi(&s))
}

// ── Descent loop ─────────────────────────────────────────────────────

/// Run the in-picker descent loop starting from a directory candidate.
///
/// Returns a `Selection` with the accumulated path when the user:
/// - Selects a non-directory (file) → full path to that file
/// - Presses ESC → the current directory level
/// - Hits an empty directory → the current directory level
///
/// `base_sel` provides the original zsh completion metadata (prefix, iprefix, args)
/// that must be preserved in the returned selection.
pub fn run_descent(
    initial_candidate: &Candidate,
    base_sel: &Selection,
    command: &str,
    _output_mode_is_eval: bool,
) -> Selection {
    let ls_colors = LsColors::from_env().unwrap_or_default();
    let dirs_only = matches!(command, "cd" | "pushd" | "z" | "rmdir");
    let mut current_word = initial_candidate.word.clone();
    let mut current_fs = candidate_fs_path(initial_candidate);

    loop {
        let path_display = format!("{current_word}/");
        let sub_candidates = readdir_candidates(&current_fs, &path_display, dirs_only);

        if sub_candidates.is_empty() {
            break;
        }

        match run_descent_picker(&sub_candidates, &path_display, &ls_colors) {
            Some(selected_display) => {
                if let Some(sub) = sub_candidates
                    .iter()
                    .find(|c| c.display == selected_display)
                {
                    let sub_fs = candidate_fs_path(sub);
                    if Path::new(&sub_fs).is_dir() {
                        current_word = sub.word.clone();
                        current_fs = sub_fs;
                        continue;
                    }
                    // Non-directory selected — return full path
                    return Selection {
                        word: sub.word.clone(),
                        is_dir: false,
                        ..base_sel.clone()
                    };
                }
                break;
            }
            None => break, // ESC — accept current level
        }
    }

    // Broke out: empty dir or ESC — return current directory
    Selection {
        word: current_word,
        is_dir: true,
        ..base_sel.clone()
    }
}
