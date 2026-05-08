//! skim-tab — Rust-native fuzzy completion for zsh.
//!
//! Subcommands:
//!   --complete             Read JSON completion request from stdin, run skim, output JSON
//!   --complete --compcap   Read compcap binary format from stdin, run skim, output eval lines
//!   --preview <manifest> <display>   Preview a candidate (called by skim during completion)
//!   daemon [--socket P]    Start the enrichment daemon on a Unix socket
//!   daemon-status [--socket P]  Ping the daemon and print its response

use std::path::PathBuf;

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
        Some("daemon") => run_daemon(&args[1..]),
        Some("daemon-status") => run_daemon_status(&args[1..]),
        _ => {
            eprintln!("Usage: skim-tab --complete [--compcap --command CMD --query Q]");
            eprintln!("       skim-tab --preview <manifest.json> <display_text>");
            eprintln!("       skim-tab daemon [--socket PATH]");
            eprintln!("       skim-tab daemon-status [--socket PATH]");
            std::process::exit(1);
        }
    }
}

/// Parse `--socket PATH` from a flag list, falling back to the platform default.
fn socket_from_args(args: &[String]) -> PathBuf {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--socket" {
            if let Some(p) = iter.next() {
                return PathBuf::from(p);
            }
        }
    }
    skim_tab::daemon::default_socket()
}

fn run_daemon(args: &[String]) {
    let socket = socket_from_args(args);
    let cfg = skim_tab::config::load();
    let server = skim_tab::daemon::Server::new(&cfg.completion.specs);
    if let Err(e) = server.serve(&socket) {
        eprintln!("skim-tab daemon: {e:#}");
        std::process::exit(1);
    }
}

fn run_daemon_status(args: &[String]) {
    let socket = socket_from_args(args);
    let req = skim_tab::daemon::Request {
        kind: "ping".to_string(),
        command: String::new(),
        word: String::new(),
        words: Vec::new(),
    };
    match skim_tab::daemon::query(&socket, &req) {
        Ok(line) => println!("{line}"),
        Err(e) => {
            eprintln!("skim-tab daemon-status: {e:#}");
            std::process::exit(1);
        }
    }
}
