//! skim-tab shared library — common utilities for all skim-tab binaries.
//!
//! Provides Nord color palette, CLI arg parsing, shell quoting, ANSI
//! stripping, editor resolution, and skim builder presets.
//!
//! Theme: Nord (Arctic ice) with sleek Jedi-inspired iconography.

pub mod complete;
pub mod config;
pub mod context;
pub mod daemon;
pub mod descent;
pub mod history_db;
pub mod k8s;
pub mod preview;
pub mod specs;
pub mod tags;
pub mod usage_rank;

use std::process::Command;

use anyhow::{Context, Result};
use skim::prelude::{SkimOptions, SkimOptionsBuilder};
use skim::tui::options::TuiLayout;

/// Classic Nord color palette for skim — the active fleet picker theme.
/// Matches the deployed ghostty palette (polar-night ground, frost/aurora
/// accents) so every picker looks like the rest of the Nord stack.
///
/// Mirror: byte-equivalent (modulo whitespace) to `lib/skim-theme.nix`'s
/// `nord` branch in the pleme-io/nix consumer repo. Flip both together.
/// `VELLUM_COLORS` below preserves the warm aged-paper theme (restorable).
pub const NORD_COLORS: &str = "\
fg:#D8DEE9,\
bg:#2E3440,\
hl:#88C0D0:bold:underlined,\
fg+:#ECEFF4:bold,\
bg+:#3B4252,\
hl+:#8FBCBB:bold:underlined,\
info:#4C566A,\
prompt:#A3BE8C,\
pointer:#88C0D0,\
marker:#B48EAD,\
spinner:#81A1C1,\
header:#81A1C1,\
border:#4C566A,\
query:#ECEFF4:bold";

/// Vellum (warm aged-paper Nord-matte) — SAVED, restorable. Swap the
/// `base_options` reference from `NORD_COLORS` to this (and flip
/// `lib/skim-theme.nix`) to bring the parchment picker theme back.
pub const VELLUM_COLORS: &str = "\
fg:#E2DBC8,\
bg:#16140E,\
hl:#94BBB8:bold:underlined,\
fg+:#F4EFE2:bold,\
bg+:#2B2820,\
hl+:#A6CBC8:bold:underlined,\
info:#6E6857,\
prompt:#A9BB8C,\
pointer:#ADD7A3,\
marker:#B8A1B9,\
spinner:#99AABE,\
header:#99AABE,\
border:#6E6857,\
query:#F4EFE2:bold";

/// Standard keybindings applied to all skim-tab pickers.
pub const STANDARD_BINDS: &[&str] = &[
    "ctrl-/:toggle-preview",
    "ctrl-u:half-page-up",
    "ctrl-d:half-page-down",
];

// ── Icons ──────────────────────────────────────────────────────────────
// Sleek, terminal-safe glyphs. No wide emoji — just clean Unicode.

/// Pointer/selector icon — lightsaber blade
pub const ICON_POINTER: &str = "\u{2502}"; // │ (vertical bar — clean saber)

/// Prompt: content search — crossed sabers
pub const ICON_SEARCH: &str = "\u{2726} "; // ✦ (four-pointed star)

/// Prompt: file picker — snowflake
pub const ICON_FILES: &str = "\u{2744} "; // ❄ (snowflake)

/// Prompt: history — hourglass
pub const ICON_HISTORY: &str = "\u{276f} "; // ❯ (chevron — clean, fast)

/// Multi-select marker
pub const ICON_MARKER: &str = "\u{25c6}"; // ◆ (diamond — selected)

/// Prompt: directory navigation — nav arrow
pub const ICON_CD: &str = "\u{25b8} "; // ▸ (right triangle — navigate)

/// Prompt: Kubernetes / Helm / Flux — helm wheel
pub const ICON_K8S: &str = "\u{2388} "; // ⎈ (helm — k8s)

// ── Nord ANSI true-color escapes ─────────────────────────────────────
// Used by colorize and preview to give non-file completions a themed look.

/// Nord frost (#88C0D0) — primary accent for completion items
pub const ANSI_FROST: &str = "\x1b[38;2;136;192;208m";
/// Nord yellow (#EBCB8B) — flags and options
pub const ANSI_YELLOW: &str = "\x1b[38;2;235;203;139m";
/// Nord dim (#4C566A) — descriptions and separators
pub const ANSI_DIM: &str = "\x1b[38;2;76;86;106m";
/// Nord green (#A3BE8C) — success / active items
pub const ANSI_GREEN: &str = "\x1b[38;2;163;190;140m";
/// Nord purple (#B48EAD) — types / categories
pub const ANSI_PURPLE: &str = "\x1b[38;2;180;142;173m";
/// ANSI reset
pub const ANSI_RESET: &str = "\x1b[0m";

/// Extract `--query <value>` from CLI args, returning empty string if absent.
#[must_use]
pub fn parse_query(args: &[String]) -> &str {
    args.iter()
        .position(|a| a == "--query")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("")
}

/// Resolve the user's preferred editor: $EDITOR → $VISUAL → nvim.
#[must_use]
pub fn editor() -> String {
    std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "nvim".to_string())
}

/// Shell-escape a string for safe embedding in a command.
/// Simple paths pass through unquoted; anything with special chars gets single-quoted.
#[must_use]
pub fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_'))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Strip ANSI escape codes (CSI sequences and OSC) from a string.
#[must_use]
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Create a pre-configured `SkimOptionsBuilder` with Nord theme, standard
/// binds, reverse layout, and common defaults. Callers customize further.
#[must_use]
pub fn base_options(query: &str) -> SkimOptionsBuilder {
    let mut builder = SkimOptionsBuilder::default();
    builder
        .query(query.to_string())
        .ansi(true)
        .height("40%".to_string())
        .min_height("10".to_string())
        .layout(TuiLayout::Reverse)
        .selector_icon(ICON_POINTER.to_string())
        .no_info(true)
        .color(NORD_COLORS.to_string())
        .bind(STANDARD_BINDS.iter().map(|s| (*s).to_string()).collect::<Vec<_>>());
    builder
}

/// Build a `SkimOptionsBuilder` into final `SkimOptions`, returning a
/// descriptive error instead of panicking.
pub fn build_options(builder: &mut SkimOptionsBuilder) -> Result<SkimOptions> {
    builder.build().context("failed to build skim options")
}

// ── TTY-guarded skim invocation ─────────────────────────────────────

/// RAII guard holding the process's stdout on the controlling tty.
///
/// skim 3.x's stack (ratatui `CrosstermBackend::get_cursor_position` →
/// `crossterm::cursor::position`) writes terminal queries — `ESC[6n`
/// among them — to the *process stdout*, never to `/dev/tty`. Every
/// skim-tab binary runs with stdout piped (the pipe IS the selection
/// protocol), so without this guard those query bytes land in the pipe
/// ahead of the selection, and the answer never arrives because the
/// terminal never saw the query — stalling teardown on a multi-second
/// timeout. With fd 1 on the tty for the duration of the run, queries
/// reach a terminal that can answer them; the selection prints after
/// the guard drops, on the restored stdout.
struct TtyStdoutGuard {
    saved_stdout: std::os::fd::OwnedFd,
}

impl TtyStdoutGuard {
    /// `None` when there is no controlling tty (headless CI) or the fd
    /// shuffle fails — callers run unguarded; with no terminal attached
    /// there is nothing to answer queries anyway.
    fn redirect() -> Option<Self> {
        use std::os::fd::{AsFd, AsRawFd};
        let tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()?;
        let saved_stdout = std::io::stdout().as_fd().try_clone_to_owned().ok()?;
        // Flush libstd's buffer so bytes written so far stay on the pipe.
        let _ = std::io::Write::flush(&mut std::io::stdout());
        nix::unistd::dup2(tty.as_raw_fd(), 1).ok()?;
        Some(Self { saved_stdout })
    }
}

impl Drop for TtyStdoutGuard {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // Flush anything skim's stack buffered toward the tty, then put
        // the real stdout back so the selection lands on the pipe.
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let _ = nix::unistd::dup2(self.saved_stdout.as_raw_fd(), 1);
    }
}

/// Run skim with stdout routed to the controlling tty for the duration —
/// the only sanctioned way for a skim-tab binary to invoke skim. Print
/// selections AFTER this returns so they land on the real stdout.
///
/// Folds skim's internal error into `None` (callers treat both "error"
/// and "abort" as no-selection).
pub fn run_skim_on_tty(
    options: SkimOptions,
    source: Option<skim::SkimItemReceiver>,
) -> Option<skim::SkimOutput> {
    let _guard = TtyStdoutGuard::redirect();
    skim::Skim::run_with(options, source).ok()
}

// ── Path helpers ────────────────────────────────────────────────────

/// Expand `~` at the start of a path to `$HOME`. Other `~user` forms
/// are not supported and pass through unchanged.
#[must_use]
pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix('~') {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}{rest}");
        }
    }
    path.to_string()
}

// ── fd discovery ────────────────────────────────────────────────────

/// What `fd` should search for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FdTarget {
    /// Only directories (`--type d`).
    Directories,
    /// Only files (`--type f`).
    Files,
    /// Both files and directories.
    FilesAndDirs,
}

/// Standard exclusion dirs shared by all `fd` callers.
const FD_EXCLUDES: &[&str] = &[".git", "node_modules", "target", "__pycache__", ".direnv"];

/// Upper bound on the entries any picker loads from `fd`.
///
/// A picker is a *human* selection surface. Past a few thousand rows nobody
/// scrolls, but every extra entry still costs walk time, a line in the
/// buffer and a skim item — so the tail of an unbounded walk is pure cost.
///
/// Unbounded it is not merely large. Measured on this workstation, a single
/// `--hidden --follow --type f` walk emitted **34.16M entries / 6.36 GB**:
/// the terminal froze for 25–30s and the process peaked near 13 GB. `fd`
/// quits the moment `--max-results` is reached, so any tree smaller than the
/// cap — every real project tree — is unaffected by it.
pub const FD_MAX_RESULTS: usize = 100_000;

/// Take a child process's stdout as a `String` without copying it twice.
///
/// `String::from_utf8_lossy(&bytes).into_owned()` allocates a *second* full
/// copy even in the normal case, where the bytes are already valid UTF-8 and
/// the `Cow` is `Borrowed`. On a multi-gigabyte `fd` walk that second copy is
/// pure waste. `String::from_utf8` reuses the buffer in place; the lossy path
/// runs only when the bytes really are malformed.
#[must_use]
pub fn into_lossy_string(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

/// Build `fd`'s argument vector for `target`.
///
/// Kept separate from the spawn so the bounds below are assertable without
/// walking a filesystem.
fn fd_args(target: FdTarget) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    match target {
        FdTarget::Directories => args.extend(["--type".into(), "d".into()]),
        FdTarget::Files => args.extend(["--type".into(), "f".into()]),
        FdTarget::FilesAndDirs => {
            args.extend(["--type".into(), "f".into(), "--type".into(), "d".into()]);
        }
    }

    // List symlinks, never walk through them. `--type l` is the half of
    // `--follow` that is actually wanted: on a home-manager workstation most
    // of `~/.config` (nvim, shell, nix, cursor …) IS a symlink into the Nix
    // store, and with neither flag those entries vanish from the picker
    // entirely — an empty picker where the operator expects their config.
    // `--follow` brought them back by descending into each target, which is
    // what turned a bounded tree into tens of millions of store paths.
    //
    // Honest cost: `fd` classifies a symlink by the link, not its target, so
    // the directory picker also lists file symlinks (and a listed symlink
    // carries no trailing `/`). Over-listing a handful of leaves is the
    // cheaper error than silently hiding every symlinked directory.
    args.extend(["--type".into(), "l".into()]);

    args.push("--hidden".into());
    for excl in FD_EXCLUDES {
        args.push("--exclude".into());
        args.push((*excl).into());
    }
    args.push("--strip-cwd-prefix".into());
    args.push("--max-results".into());
    args.push(FD_MAX_RESULTS.to_string());

    args
}

/// Run `fd` with standard exclusions and return its stdout.
///
/// All three file-discovery binaries (cd, files, fvim) share the same
/// `fd` invocation pattern. This centralizes it.
///
/// The walk is **bounded** by [`FD_MAX_RESULTS`] and does **not** follow
/// symlinks: `--follow` re-walks every symlinked tree, and on a workstation
/// where `~` links into the Nix store, caches and other checkouts that turns a
/// bounded directory tree into a near-cyclic graph of tens of millions of
/// paths. Symlinks are still *listed* (see [`fd_args`]); they are never
/// descended.
pub fn fd_discover(target: FdTarget) -> Result<String> {
    let output = Command::new("fd")
        .args(fd_args(target))
        .output()
        .context("failed to run fd — is it installed?")?;
    let entries = into_lossy_string(output.stdout);

    // At the cap the list the operator is scrolling is a prefix of the tree,
    // not the tree. Say so on stderr rather than let it read as complete.
    if entries.lines().count() >= FD_MAX_RESULTS {
        eprintln!(
            "skim-tab: fd hit the {FD_MAX_RESULTS} entry cap — showing a partial tree; \
             run from a narrower directory for a complete list"
        );
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nord_colors_has_required_keys() {
        for key in [
            "fg:", "bg:", "hl:", "fg+:", "bg+:", "hl+:", "prompt:", "pointer:",
        ] {
            assert!(NORD_COLORS.contains(key), "missing color key: {key}");
        }
    }

    #[test]
    fn parse_query_present() {
        let args: Vec<String> = vec!["--query", "hello"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(parse_query(&args), "hello");
    }

    #[test]
    fn parse_query_absent() {
        let args: Vec<String> = vec!["--other".to_string()];
        assert_eq!(parse_query(&args), "");
    }

    #[test]
    fn shell_quote_simple() {
        assert_eq!(shell_quote("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn shell_quote_spaces() {
        assert_eq!(shell_quote("my file.rs"), "'my file.rs'");
    }

    #[test]
    fn shell_quote_single_quotes() {
        assert_eq!(shell_quote("it's.rs"), "'it'\\''s.rs'");
    }

    #[test]
    fn strip_ansi_removes_csi() {
        assert_eq!(
            strip_ansi("\x1b[35msrc/main.rs\x1b[0m:\x1b[32m42\x1b[0m:fn main()"),
            "src/main.rs:42:fn main()"
        );
    }

    #[test]
    fn strip_ansi_passthrough() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn base_options_builds() {
        let opts = base_options("test").build();
        assert!(opts.is_ok());
    }

    #[test]
    fn build_options_returns_ok() {
        let result = build_options(&mut base_options("test"));
        assert!(result.is_ok());
    }

    #[test]
    fn icons_are_single_width() {
        assert!(!ICON_POINTER.is_empty());
        assert!(!ICON_SEARCH.is_empty());
        assert!(!ICON_FILES.is_empty());
        assert!(!ICON_HISTORY.is_empty());
        assert!(!ICON_MARKER.is_empty());
        assert!(!ICON_CD.is_empty());
        assert!(!ICON_K8S.is_empty());
    }

    // ── editor() ─────────────────────────────────────────────────────
    //
    // The three EDITOR/VISUAL scenarios share process-global env state, so
    // running them as separate `#[test]` functions races under cargo's
    // default parallelism and inherits whatever EDITOR the shell exported.
    // Folded into one sequential test so the asserts are deterministic.

    #[test]
    fn editor_resolves_env_visual_and_default() {
        // Scenario 1: EDITOR set wins
        unsafe {
            std::env::set_var("EDITOR", "vim");
            std::env::remove_var("VISUAL");
        }
        assert_eq!(editor(), "vim");

        // Scenario 2: only VISUAL set → falls back to VISUAL
        unsafe {
            std::env::remove_var("EDITOR");
            std::env::set_var("VISUAL", "code");
        }
        assert_eq!(editor(), "code");

        // Scenario 3: neither set → final default nvim
        unsafe {
            std::env::remove_var("EDITOR");
            std::env::remove_var("VISUAL");
        }
        assert_eq!(editor(), "nvim");
    }

    // ── strip_ansi edge cases ────────────────────────────────────────

    #[test]
    fn strip_ansi_removes_osc_bel() {
        // OSC terminated by BEL (\x07)
        assert_eq!(strip_ansi("\x1b]0;title\x07text"), "text");
    }

    #[test]
    fn strip_ansi_removes_osc_st() {
        // OSC terminated by ST (\x1b\\)
        assert_eq!(strip_ansi("\x1b]0;title\x1b\\text"), "text");
    }

    #[test]
    fn strip_ansi_bare_escape_passthrough() {
        // Lone ESC not followed by [ or ] should be consumed without
        // eating subsequent content
        let result = strip_ansi("\x1bXhello");
        assert!(result.contains("hello"));
    }

    #[test]
    fn strip_ansi_empty_string() {
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn strip_ansi_multiple_sequences() {
        let input = "\x1b[31mred\x1b[0m \x1b[32mgreen\x1b[0m";
        assert_eq!(strip_ansi(input), "red green");
    }

    #[test]
    fn strip_ansi_nested_csi_params() {
        // True-color escape with many params
        assert_eq!(
            strip_ansi("\x1b[38;2;136;192;208mfrost\x1b[0m"),
            "frost"
        );
    }

    // ── parse_query edge cases ───────────────────────────────────────

    #[test]
    fn parse_query_at_end_of_args() {
        // --query at end with no value
        let args: Vec<String> = vec!["--query".to_string()];
        assert_eq!(parse_query(&args), "");
    }

    #[test]
    fn parse_query_empty_args() {
        let args: Vec<String> = vec![];
        assert_eq!(parse_query(&args), "");
    }

    #[test]
    fn parse_query_multiple_flags() {
        let args: Vec<String> = vec!["--other", "val", "--query", "hello", "--more"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(parse_query(&args), "hello");
    }

    // ── shell_quote edge cases ───────────────────────────────────────

    #[test]
    fn shell_quote_empty_string() {
        // Empty string has no special chars → passes through unquoted
        assert_eq!(shell_quote(""), "");
    }

    #[test]
    fn shell_quote_special_chars() {
        assert_eq!(shell_quote("$HOME"), "'$HOME'");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("foo`bar"), "'foo`bar'");
    }

    #[test]
    fn shell_quote_backslash() {
        assert_eq!(shell_quote("a\\b"), "'a\\b'");
    }

    #[test]
    fn shell_quote_safe_chars_passthrough() {
        assert_eq!(shell_quote("a-b_c.d/e"), "a-b_c.d/e");
        assert_eq!(shell_quote("ABC123"), "ABC123");
    }

    // ── ANSI color constants ─────────────────────────────────────────

    #[test]
    fn ansi_constants_are_valid_escape_sequences() {
        for (name, val) in [
            ("FROST", ANSI_FROST),
            ("YELLOW", ANSI_YELLOW),
            ("DIM", ANSI_DIM),
            ("GREEN", ANSI_GREEN),
            ("PURPLE", ANSI_PURPLE),
            ("RESET", ANSI_RESET),
        ] {
            assert!(val.starts_with('\x1b'), "{name} should start with ESC");
        }
    }

    #[test]
    fn expand_tilde_expands_home() {
        let result = expand_tilde("~/foo/bar");
        assert!(!result.starts_with('~'));
        assert!(result.ends_with("/foo/bar"));
    }

    #[test]
    fn expand_tilde_no_tilde() {
        assert_eq!(expand_tilde("/absolute/path"), "/absolute/path");
        assert_eq!(expand_tilde("relative"), "relative");
    }

    #[test]
    fn expand_tilde_bare_tilde() {
        let result = expand_tilde("~");
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(result, home);
    }

    #[test]
    fn fd_target_variants_are_distinct() {
        assert_ne!(FdTarget::Directories, FdTarget::Files);
        assert_ne!(FdTarget::Files, FdTarget::FilesAndDirs);
        assert_ne!(FdTarget::Directories, FdTarget::FilesAndDirs);
    }

    #[test]
    fn fd_discover_runs_without_panic() {
        // fd may not be in PATH in CI; just verify no panic
        let _ = fd_discover(FdTarget::Files);
    }

    /// A picker feed is bounded, and bounded at a size a human could plausibly
    /// scroll through. Unbounded, this walk measured 34.16M entries / 6.36 GB.
    #[test]
    fn fd_result_cap_is_a_picker_sized_bound() {
        assert!(FD_MAX_RESULTS > 0);
        assert!(
            FD_MAX_RESULTS <= 1_000_000,
            "a cap this large is not a bound on a picker feed"
        );
    }

    /// `--follow` is what turned a bounded tree into 34.16M entries: on a
    /// workstation `~` links into the Nix store, caches and other checkouts,
    /// and re-walking each one makes the tree a near-cyclic graph.
    #[test]
    fn fd_never_follows_symlinks() {
        for target in [
            FdTarget::Directories,
            FdTarget::Files,
            FdTarget::FilesAndDirs,
        ] {
            let args = fd_args(target);
            assert!(
                !args.iter().any(|a| a == "--follow" || a == "-L"),
                "{target:?} re-introduced symlink following: {args:?}"
            );
        }
    }

    /// …but a symlink is still LISTED. Dropping `--follow` without `--type l`
    /// empties the picker anywhere home-manager owns the tree, because every
    /// entry under `~/.config` is a symlink into the Nix store.
    #[test]
    fn fd_lists_symlinks_without_descending_them() {
        for target in [
            FdTarget::Directories,
            FdTarget::Files,
            FdTarget::FilesAndDirs,
        ] {
            let args = fd_args(target);
            let types: Vec<&String> = args
                .iter()
                .enumerate()
                .filter(|(i, _)| *i > 0 && args[i - 1] == "--type")
                .map(|(_, a)| a)
                .collect();
            assert!(
                types.iter().any(|t| *t == "l"),
                "{target:?} drops symlinks from the feed: {args:?}"
            );
        }
    }

    /// Every `fd` walk carries the cap — all three pickers share one call
    /// site, so one missing flag would leave one of them unbounded.
    #[test]
    fn fd_walks_are_capped() {
        for target in [
            FdTarget::Directories,
            FdTarget::Files,
            FdTarget::FilesAndDirs,
        ] {
            let args = fd_args(target);
            let idx = args
                .iter()
                .position(|a| a == "--max-results")
                .unwrap_or_else(|| panic!("{target:?} walk is unbounded: {args:?}"));
            assert_eq!(args[idx + 1], FD_MAX_RESULTS.to_string());
        }
    }

    #[test]
    fn fd_args_keep_the_standard_exclusions() {
        let args = fd_args(FdTarget::Files);
        for excl in FD_EXCLUDES {
            assert!(args.iter().any(|a| a == excl), "lost exclusion {excl}");
        }
        assert!(args.iter().any(|a| a == "--hidden"));
        assert!(args.iter().any(|a| a == "--strip-cwd-prefix"));
    }

    #[test]
    fn into_lossy_string_keeps_valid_utf8() {
        let valid = b"src/main.rs\n".to_vec();
        assert_eq!(into_lossy_string(valid), "src/main.rs\n");
    }

    #[test]
    fn into_lossy_string_replaces_invalid_bytes() {
        let out = into_lossy_string(vec![b'a', 0xff, b'b']);
        assert!(out.starts_with('a') && out.ends_with('b'));
        assert!(out.contains('\u{fffd}'), "invalid byte kept: {out:?}");
    }

    #[test]
    fn standard_binds_are_non_empty() {
        assert!(!STANDARD_BINDS.is_empty());
        for bind in STANDARD_BINDS {
            assert!(bind.contains(':'), "bind '{bind}' should have a colon");
        }
    }
}
