//! skim-tab — Rust-native fuzzy completion bridge for zsh.
//!
//! Drop-in replacement for `fzf` in fzf-tab, using skim as the fuzzy finder.
//! Fixes the `--expect` + `--print-query` output protocol mismatch between
//! skim and fzf that prevents fzf-tab from inserting selected completions.

use std::io::{self, Read as _};
use std::process;

use skim::prelude::*;
use skim::tui::options::TuiLayout;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = parse_args(&args);

    // Read all input from stdin
    let input: String = {
        let mut buf = String::new();
        io::stdin().lock().read_to_string(&mut buf).unwrap_or(0);
        buf
    };

    if input.is_empty() {
        process::exit(1);
    }

    // Build skim options
    let mut bind_strs: Vec<String> = opts.binds.clone();

    // Convert --expect keys to accept bindings (skim 3.x deprecates --expect)
    for key in &opts.expect_keys {
        if !key.is_empty() {
            bind_strs.push(format!("{key}:accept"));
        }
    }

    let height_str = opts.height.clone().unwrap_or_else(|| "40%".to_string());

    let layout = match opts.layout.as_str() {
        "reverse" => TuiLayout::Reverse,
        "reverse-list" => TuiLayout::ReverseList,
        _ => TuiLayout::Default,
    };

    let mut builder = SkimOptionsBuilder::default();
    builder
        .multi(opts.multi)
        .no_sort(opts.no_sort)
        .cycle(opts.cycle)
        .height(height_str)
        .bind(bind_strs)
        .layout(layout);

    if opts.exact {
        builder.exact(true);
    }

    if !opts.query.is_empty() {
        builder.query(opts.query.clone());
    }

    if let Some(ref algo) = opts.algo {
        let algorithm = match algo.as_str() {
            "skim-v1" | "v1" => skim::FuzzyAlgorithm::SkimV1,
            "skim-v2" | "v2" => skim::FuzzyAlgorithm::SkimV2,
            "clangd" => skim::FuzzyAlgorithm::Clangd,
            "fzy" => skim::FuzzyAlgorithm::Fzy,
            "frizbee" => skim::FuzzyAlgorithm::Frizbee,
            "arinae" | "ari" => skim::FuzzyAlgorithm::Arinae,
            _ => skim::FuzzyAlgorithm::SkimV2,
        };
        builder.algorithm(algorithm);
    }

    if let Some(ref scheme) = opts.scheme {
        let match_scheme = match scheme.as_str() {
            "path" => skim::options::MatchScheme::Path,
            "history" => skim::options::MatchScheme::History,
            _ => skim::options::MatchScheme::Default,
        };
        builder.scheme(match_scheme);
    }

    if opts.no_hscroll {
        builder.no_hscroll(true);
    }

    if opts.tabstop > 0 {
        builder.tabstop(opts.tabstop);
    }

    if let Some(ref delim) = opts.delimiter {
        if let Ok(re) = regex::Regex::new(delim) {
            builder.delimiter(re);
        }
    }

    if !opts.nth.is_empty() {
        let nth_parts: Vec<String> = opts.nth.split(',').map(|s| s.to_string()).collect();
        builder.nth(nth_parts);
    }

    if !opts.with_nth.is_empty() {
        let with_nth_parts: Vec<String> = opts.with_nth.split(',').map(|s| s.to_string()).collect();
        builder.with_nth(with_nth_parts);
    }

    if let Some(ref preview) = opts.preview {
        builder.preview(preview.clone());
    }

    if let Some(ref pw) = opts.preview_window {
        builder.preview_window(skim::tui::options::PreviewLayout::from(pw.as_str()));
    }

    if opts.header_lines > 0 {
        builder.header_lines(opts.header_lines);
    }

    if let Some(ref header) = opts.header {
        builder.header(header.clone());
    }

    if !opts.tiebreak.is_empty() {
        let criteria: Vec<skim::item::RankCriteria> = opts
            .tiebreak
            .split(',')
            .filter_map(|s| match s.trim() {
                "begin" => Some(skim::item::RankCriteria::Begin),
                "end" => Some(skim::item::RankCriteria::End),
                "score" => Some(skim::item::RankCriteria::Score),
                "index" => Some(skim::item::RankCriteria::Index),
                "length" => Some(skim::item::RankCriteria::Length),
                _ => None,
            })
            .collect();
        if !criteria.is_empty() {
            builder.tiebreak(criteria);
        }
    }

    if opts.ansi {
        builder.no_strip_ansi(true);
    }

    let skim_opts = match builder.build() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skim-tab: failed to build options: {e}");
            process::exit(2);
        }
    };

    // Use run_items API (skim 3.7+) — simpler than SkimItemReader
    let lines: Vec<String> = input.lines().map(|s| s.to_string()).collect();
    let output = match Skim::run_items(skim_opts, lines) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skim-tab: skim error: {e}");
            process::exit(2);
        }
    };

    // Handle abort (Esc / Ctrl-C)
    if output.is_abort {
        process::exit(130);
    }

    // Format output in fzf-compatible protocol:
    // Line 1: query string        (if --print-query)
    // Line 2: matched expect key  (if --expect; empty string for Enter)
    // Line 3+: selected items

    // Line 1: query
    if opts.print_query {
        println!("{}", output.query);
    }

    // Line 2: expect key — THE CRITICAL FIX
    // fzf-tab expects this line to exist when --expect is used.
    // For regular Enter, it must be an empty line.
    // For expect keys (like /), it must be the key name.
    if !opts.expect_keys.is_empty() {
        let key_name = format_key_event(&output.final_key);
        let matched = opts
            .expect_keys
            .iter()
            .find(|k| k.eq_ignore_ascii_case(&key_name))
            .cloned()
            .unwrap_or_default();
        println!("{matched}");
    }

    // Lines 3+: selected items
    if output.selected_items.is_empty() {
        if let Some(current) = &output.current {
            println!("{}", current.output());
        }
    } else {
        for item in &output.selected_items {
            println!("{}", item.item.output());
        }
    }
}

/// Format a crossterm KeyEvent into the fzf key name format.
/// We use Debug formatting to avoid crossterm version conflicts,
/// then parse the debug output to extract key info.
fn format_key_event(key: &impl std::fmt::Debug) -> String {
    let debug = format!("{key:?}");

    // Parse crossterm's KeyEvent debug format:
    // KeyEvent { code: Char('/'), modifiers: NONE, ... }
    // KeyEvent { code: Enter, modifiers: NONE, ... }
    // KeyEvent { code: Char('a'), modifiers: CONTROL, ... }
    // KeyEvent { code: Enter, modifiers: ALT, ... }

    let has_ctrl = debug.contains("CONTROL");
    let has_alt = debug.contains("ALT");

    // Extract key code
    if let Some(char_start) = debug.find("Char('") {
        let rest = &debug[char_start + 6..];
        if let Some(end) = rest.find("')") {
            let ch = &rest[..end];
            return if has_ctrl {
                format!("ctrl-{ch}")
            } else if has_alt {
                format!("alt-{ch}")
            } else {
                ch.to_string()
            };
        }
    }

    // Named keys
    for (pattern, name) in [
        ("Enter", "enter"),
        ("Esc", "esc"),
        ("Tab", "tab"),
        ("Backspace", "bspace"),
        ("Delete", "del"),
        ("Up", "up"),
        ("Down", "down"),
        ("Left", "left"),
        ("Right", "right"),
        ("Home", "home"),
        ("End", "end"),
        ("PageUp", "page-up"),
        ("PageDown", "page-down"),
    ] {
        if debug.contains(&format!("code: {pattern}")) {
            return if has_ctrl {
                format!("ctrl-{name}")
            } else if has_alt {
                format!("alt-{name}")
            } else {
                name.to_string()
            };
        }
    }

    // Function keys: F(1), F(2), etc.
    if let Some(f_start) = debug.find("F(") {
        let rest = &debug[f_start + 2..];
        if let Some(end) = rest.find(')') {
            let num = &rest[..end];
            return format!("f{num}");
        }
    }

    String::new()
}

/// Parsed CLI options (fzf-compatible subset needed by fzf-tab).
struct Opts {
    multi: bool,
    cycle: bool,
    no_sort: bool,
    exact: bool,
    print_query: bool,
    ansi: bool,
    no_hscroll: bool,
    query: String,
    height: Option<String>,
    layout: String,
    algo: Option<String>,
    scheme: Option<String>,
    delimiter: Option<String>,
    nth: String,
    with_nth: String,
    tiebreak: String,
    header_lines: usize,
    header: Option<String>,
    tabstop: usize,
    preview: Option<String>,
    preview_window: Option<String>,
    expect_keys: Vec<String>,
    binds: Vec<String>,
}

/// Parse fzf-compatible CLI arguments.
fn parse_args(args: &[String]) -> Opts {
    let mut opts = Opts {
        multi: false,
        cycle: false,
        no_sort: false,
        exact: false,
        print_query: false,
        ansi: false,
        no_hscroll: false,
        query: String::new(),
        height: None,
        layout: "default".to_string(),
        algo: None,
        scheme: None,
        delimiter: None,
        nth: String::new(),
        with_nth: String::new(),
        tiebreak: String::new(),
        header_lines: 0,
        header: None,
        tabstop: 0,
        preview: None,
        preview_window: None,
        expect_keys: Vec::new(),
        binds: Vec::new(),
    };

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];

        if arg == "--multi" || arg == "-m" {
            opts.multi = true;
        } else if arg == "+m" || arg == "--no-multi" {
            opts.multi = false;
        } else if arg == "--cycle" {
            opts.cycle = true;
        } else if arg == "--no-sort" || arg == "+s" {
            opts.no_sort = true;
        } else if arg == "--exact" || arg == "-e" {
            opts.exact = true;
        } else if arg == "--print-query" {
            opts.print_query = true;
        } else if arg == "--ansi" {
            opts.ansi = true;
        } else if arg == "--no-hscroll" {
            opts.no_hscroll = true;
        } else if arg == "--reverse" {
            opts.layout = "reverse".to_string();
        } else if let Some(val) = strip_eq_or_next(arg, "--query", args, &mut i) {
            opts.query = val;
        } else if let Some(val) = strip_eq_or_next(arg, "-q", args, &mut i) {
            opts.query = val;
        } else if let Some(val) = strip_eq_or_next(arg, "--height", args, &mut i) {
            opts.height = Some(val);
        } else if let Some(val) = strip_eq_or_next(arg, "--layout", args, &mut i) {
            opts.layout = val;
        } else if let Some(val) = strip_eq_or_next(arg, "--algo", args, &mut i) {
            opts.algo = Some(val);
        } else if let Some(val) = strip_eq_or_next(arg, "--scheme", args, &mut i) {
            opts.scheme = Some(val);
        } else if let Some(val) = strip_eq_or_next(arg, "--delimiter", args, &mut i) {
            opts.delimiter = Some(val);
        } else if let Some(val) = strip_eq_or_next(arg, "-d", args, &mut i) {
            opts.delimiter = Some(val);
        } else if let Some(val) = strip_eq_or_next(arg, "--nth", args, &mut i) {
            opts.nth = val;
        } else if let Some(val) = strip_eq_or_next(arg, "-n", args, &mut i) {
            opts.nth = val;
        } else if let Some(val) = strip_eq_or_next(arg, "--with-nth", args, &mut i) {
            opts.with_nth = val;
        } else if let Some(val) = strip_eq_or_next(arg, "--tiebreak", args, &mut i) {
            opts.tiebreak = val;
        } else if let Some(val) = strip_eq_or_next(arg, "--header-lines", args, &mut i) {
            opts.header_lines = val.parse().unwrap_or(0);
        } else if let Some(val) = strip_eq_or_next(arg, "--header", args, &mut i) {
            opts.header = Some(val);
        } else if let Some(val) = strip_eq_or_next(arg, "--tabstop", args, &mut i) {
            opts.tabstop = val.parse().unwrap_or(0);
        } else if let Some(val) = strip_eq_or_next(arg, "--preview", args, &mut i) {
            opts.preview = Some(val);
        } else if let Some(val) = strip_eq_or_next(arg, "--preview-window", args, &mut i) {
            opts.preview_window = Some(val);
        } else if let Some(val) = strip_eq_or_next(arg, "--expect", args, &mut i) {
            for key in val.split(',') {
                let k = key.trim();
                if !k.is_empty() {
                    opts.expect_keys.push(k.to_string());
                }
            }
        } else if let Some(val) = strip_eq_or_next(arg, "--bind", args, &mut i) {
            opts.binds.push(val);
        } else if let Some(val) = strip_eq_or_next(arg, "--color", args, &mut i) {
            // Pass color spec as bind-compatible env: skim reads FZF_DEFAULT_OPTS/SKIM_DEFAULT_OPTIONS
            // for colors, but if fzf-tab passes --color directly, we forward to environment
            std::env::set_var("SKIM_COLORS", val);
        } else if let Some(val) = strip_eq_or_next(arg, "--info", args, &mut i) {
            // info style (inline, hidden, default) — skim reads this from SKIM_DEFAULT_OPTIONS
            // inject into env for skim to pick up
            let current = std::env::var("SKIM_DEFAULT_OPTIONS").unwrap_or_default();
            std::env::set_var("SKIM_DEFAULT_OPTIONS", format!("{current} --info={val}"));
        }
        // Silently ignore unknown flags

        i += 1;
    }

    opts
}

/// Extract value from `--flag=value` or `--flag value` format.
fn strip_eq_or_next(arg: &str, flag: &str, args: &[String], i: &mut usize) -> Option<String> {
    if let Some(val) = arg.strip_prefix(&format!("{flag}=")) {
        Some(val.to_string())
    } else if arg == flag {
        *i += 1;
        args.get(*i).cloned()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args_basic() {
        let args: Vec<String> = vec![
            "--multi", "--cycle", "--no-sort", "--print-query", "--ansi",
            "--query=hello", "--height=30%", "--layout=reverse",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let opts = parse_args(&args);
        assert!(opts.multi);
        assert!(opts.cycle);
        assert!(opts.no_sort);
        assert!(opts.print_query);
        assert!(opts.ansi);
        assert_eq!(opts.query, "hello");
        assert_eq!(opts.height, Some("30%".to_string()));
        assert_eq!(opts.layout, "reverse");
    }

    #[test]
    fn test_parse_args_expect() {
        let args: Vec<String> = vec!["--expect=/,alt-enter,ctrl-x"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.expect_keys, vec!["/", "alt-enter", "ctrl-x"]);
    }

    #[test]
    fn test_parse_args_bind() {
        let args: Vec<String> = vec![
            "--bind=tab:down,btab:up",
            "--bind=ctrl-space:toggle",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.binds.len(), 2);
    }

    #[test]
    fn test_parse_args_delimiter() {
        let args: Vec<String> = vec!["--delimiter=\\x00", "--nth=2,3"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.delimiter, Some("\\x00".to_string()));
        assert_eq!(opts.nth, "2,3");
    }

    #[test]
    fn test_parse_args_separate_value() {
        let args: Vec<String> = vec!["--query", "test", "--height", "50%"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.query, "test");
        assert_eq!(opts.height, Some("50%".to_string()));
    }

    #[test]
    fn test_parse_args_header_lines() {
        let args: Vec<String> = vec!["--header-lines=3"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.header_lines, 3);
    }

    #[test]
    fn test_parse_args_preview() {
        let args: Vec<String> = vec![
            "--preview=bat --color=always {}",
            "--preview-window=right:50%:wrap",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.preview, Some("bat --color=always {}".to_string()));
        assert_eq!(opts.preview_window, Some("right:50%:wrap".to_string()));
    }

    #[test]
    fn test_parse_args_short_flags() {
        let args: Vec<String> = vec!["-m", "-d", "\\x00", "-n", "2,3"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert!(opts.multi);
        assert_eq!(opts.delimiter, Some("\\x00".to_string()));
        assert_eq!(opts.nth, "2,3");
    }

    #[test]
    fn test_parse_args_reverse_shorthand() {
        let args: Vec<String> = vec!["--reverse"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.layout, "reverse");
    }

    #[test]
    fn test_parse_args_tiebreak() {
        let args: Vec<String> = vec!["--tiebreak=begin"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.tiebreak, "begin");
    }

    #[test]
    fn test_parse_args_unknown_flags_ignored() {
        let args: Vec<String> = vec![
            "--multi", "--unknown-flag", "--another=value", "--print-query",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let opts = parse_args(&args);
        assert!(opts.multi);
        assert!(opts.print_query);
    }

    #[test]
    fn test_parse_args_algo() {
        let args: Vec<String> = vec!["--algo=arinae"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.algo, Some("arinae".to_string()));
    }

    #[test]
    fn test_parse_args_scheme() {
        let args: Vec<String> = vec!["--scheme=path"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.scheme, Some("path".to_string()));
    }

    #[test]
    fn test_parse_args_exact() {
        let args: Vec<String> = vec!["--exact"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert!(opts.exact);
    }

    #[test]
    fn test_parse_args_with_nth() {
        let args: Vec<String> = vec!["--with-nth=2.."]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.with_nth, "2..");
    }

    #[test]
    fn test_parse_args_header() {
        let args: Vec<String> = vec!["--header=Select an item"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.header, Some("Select an item".to_string()));
    }

    #[test]
    fn test_parse_args_no_multi() {
        let args: Vec<String> = vec!["--multi", "+m"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert!(!opts.multi);
    }

    #[test]
    fn test_parse_args_tabstop() {
        let args: Vec<String> = vec!["--tabstop=4"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.tabstop, 4);
    }

    #[test]
    fn test_parse_args_short_query() {
        let args: Vec<String> = vec!["-q", "search term"]
            .into_iter()
            .map(String::from)
            .collect();

        let opts = parse_args(&args);
        assert_eq!(opts.query, "search term");
    }
}
