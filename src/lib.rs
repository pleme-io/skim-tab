//! skim-tab shared library — common utilities for all skim-tab binaries.
//!
//! Provides Nord color palette, CLI arg parsing, shell quoting, ANSI
//! stripping, editor resolution, and skim builder presets.
//!
//! Theme: Nord (Arctic ice) with sleek Jedi-inspired iconography.

pub mod complete;
pub mod config;
pub mod context;
pub mod descent;
pub mod history_db;
pub mod k8s;
pub mod preview;
pub mod specs;

use skim::prelude::SkimOptionsBuilder;
use skim::tui::options::TuiLayout;

/// Nord color palette for skim, used by all binaries.
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
header:#5E81AC,\
border:#4C566A,\
query:#ECEFF4:bold";

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
pub fn parse_query(args: &[String]) -> &str {
    args.iter()
        .position(|a| a == "--query")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("")
}

/// Resolve the user's preferred editor: $EDITOR → $VISUAL → nvim.
pub fn editor() -> String {
    std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "nvim".to_string())
}

/// Shell-escape a string for safe embedding in a command.
/// Simple paths pass through unquoted; anything with special chars gets single-quoted.
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
    fn icons_are_single_width() {
        // All icons should be terminal-safe single-width characters
        assert!(!ICON_POINTER.is_empty());
        assert!(!ICON_SEARCH.is_empty());
        assert!(!ICON_FILES.is_empty());
        assert!(!ICON_HISTORY.is_empty());
        assert!(!ICON_MARKER.is_empty());
        assert!(!ICON_CD.is_empty());
    }
}
