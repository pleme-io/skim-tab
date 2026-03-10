//! skim-tab — Rust-native fuzzy completion for zsh.
//!
//! Subcommands:
//!   --complete             Read JSON completion request from stdin, run skim, output JSON
//!   --complete --compcap   Read compcap binary format from stdin, run skim, output eval lines
//!   --preview <manifest> <display>   Preview a candidate (called by skim during completion)

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(|s| s.as_str()) {
        Some("--complete") => {
            if args.iter().any(|a| a == "--compcap") {
                let remaining: Vec<String> = args
                    .iter()
                    .filter(|a| *a != "--complete" && *a != "--compcap")
                    .cloned()
                    .collect();
                skim_tab::complete::run_compcap(&remaining);
            } else {
                skim_tab::complete::run();
            }
        }
        Some("--preview") => {
            skim_tab::complete::run_preview(&args[1..]);
        }
        _ => {
            eprintln!("Usage: skim-tab --complete [--compcap --command CMD --query Q]");
            eprintln!("       skim-tab --preview <manifest.json> <display_text>");
            std::process::exit(1);
        }
    }
}
