//! skim-tab --complete — native zsh completion via skim.
//!
//! Two input modes:
//!   1. JSON on stdin (for testing / other consumers)
//!   2. `--compcap` mode: reads fzf-tab's NUL/STX compcap format on stdin,
//!      with `--command` and `--query` as CLI args (for the zsh widget)
//!
//! Runs skim for fuzzy selection with preview, writes JSON response to stdout.
//! Replaces fzf-tab entirely — no fzf compatibility layer, no shell-based
//! preview, no NUL-delimited protocols on output. Pure Rust + JSON boundary.

use crate::{base_options, ICON_CD, ICON_POINTER};
use lscolors::LsColors;
use serde::{Deserialize, Serialize};
use skim::prelude::*;
use std::collections::HashMap;
use std::io::{self, Read as _};
use std::path::Path;
use std::process::Command;

// ── JSON protocol ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CompletionRequest {
    pub candidates: Vec<Candidate>,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub continuous_trigger: String,
}

#[derive(Deserialize, Clone)]
pub struct Candidate {
    pub word: String,
    #[serde(default)]
    pub display: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub group_index: usize,
    #[serde(default)]
    pub realdir: String,
    #[serde(default)]
    pub is_file: bool,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default)]
    pub iprefix: String,
    #[serde(default)]
    pub isuffix: String,
    /// Original zparseopts args, joined with \x01
    #[serde(default)]
    pub args: String,
}

#[derive(Serialize)]
pub struct CompletionResponse {
    pub action: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub selections: Vec<Selection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct Selection {
    pub word: String,
    pub iprefix: String,
    pub prefix: String,
    pub suffix: String,
    pub isuffix: String,
    pub args: String,
}

// ── Display item for skim ───────────────────────────────────────────────

struct CompItem {
    display: String,
}

impl SkimItem for CompItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.display)
    }
}

// ── Compcap parser ──────────────────────────────────────────────────────

/// Parse fzf-tab's compcap format from raw bytes.
///
/// Input: compcap entries separated by ETX (\x03).
/// Each entry: `display\x02<\x00>\x00key\x00value\x00...\x00word\x00theword`
///
/// The NUL-separated key-value pairs after the `<\x00>` marker contain:
///   PREFIX, SUFFIX, IPREFIX, ISUFFIX, apre, hpre, group, realdir, args, word
fn parse_compcap(data: &[u8], command: &str, query: &str) -> CompletionRequest {
    let mut candidates = Vec::new();

    for entry in data.split(|&b| b == 0x03) {
        if entry.is_empty() {
            continue;
        }

        // Split on STX (0x02): display | metadata
        let stx_pos = entry.iter().position(|&b| b == 0x02);
        let (display_bytes, meta_bytes) = match stx_pos {
            Some(pos) => (&entry[..pos], &entry[pos + 1..]),
            None => continue,
        };

        let display = String::from_utf8_lossy(display_bytes).to_string();

        // Parse NUL-separated key-value pairs from metadata.
        // The metadata starts with `<\x00>` marker, then NUL-separated pairs.
        let parts: Vec<&[u8]> = meta_bytes.split(|&b| b == 0x00).collect();
        let mut map: HashMap<String, String> = HashMap::new();

        // Skip the leading marker tokens. The format is:
        //   `<` NUL `>` NUL key NUL value NUL key NUL value ...
        // After splitting on NUL: ["<", ">", key, value, key, value, ...]
        // So real key-value pairs start at index 2.
        let start = if parts.len() >= 2
            && parts[0] == b"<"
            && (parts[1] == b">" || parts[1].is_empty())
        {
            2
        } else {
            0
        };

        let mut i = start;
        while i + 1 < parts.len() {
            let key = String::from_utf8_lossy(parts[i]).to_string();
            let value = String::from_utf8_lossy(parts[i + 1]).to_string();
            if !key.is_empty() {
                map.insert(key, value);
            }
            i += 2;
        }

        let has_realdir = map.contains_key("realdir");
        candidates.push(Candidate {
            word: map.remove("word").unwrap_or_default(),
            display,
            group: map.remove("group").unwrap_or_default(),
            group_index: map
                .get("group")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            realdir: map.remove("realdir").unwrap_or_default(),
            is_file: has_realdir,
            prefix: map.remove("PREFIX").unwrap_or_default(),
            suffix: map.remove("SUFFIX").unwrap_or_default(),
            iprefix: map.remove("IPREFIX").unwrap_or_default(),
            isuffix: map.remove("ISUFFIX").unwrap_or_default(),
            args: map.remove("args").unwrap_or_default(),
        });
    }

    CompletionRequest {
        candidates,
        query: query.to_string(),
        command: command.to_string(),
        groups: vec![],
        continuous_trigger: "/".to_string(),
    }
}

// ── Preview ─────────────────────────────────────────────────────────────

fn preview_candidate(candidate: &Candidate, command: &str) -> String {
    let word = &candidate.word;
    let path = if candidate.realdir.is_empty() {
        word.to_string()
    } else {
        format!("{}{}", candidate.realdir, word)
    };

    match command {
        "cd" | "pushd" | "z" => preview_dir(&path),
        "kill" | "ps" => preview_proc(word),
        cmd if cmd.starts_with("git-add")
            || cmd.starts_with("git-diff")
            || cmd.starts_with("git-restore") =>
        {
            preview_git("diff", word)
        }
        cmd if cmd.starts_with("git-log") => preview_git("log", word),
        cmd if cmd.starts_with("git-checkout") => preview_git("checkout", word),
        _ => {
            if Path::new(&path).is_dir() {
                preview_dir(&path)
            } else if Path::new(&path).is_file() {
                preview_file(&path)
            } else {
                String::new()
            }
        }
    }
}

fn preview_dir(path: &str) -> String {
    Command::new("eza")
        .args(["--tree", "--level=2", "--icons", "--color=always", path])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn preview_file(path: &str) -> String {
    Command::new("bat")
        .args([
            "--color=always",
            "--style=numbers",
            "--line-range=:200",
            path,
        ])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn preview_proc(word: &str) -> String {
    Command::new("ps")
        .args(["-p", word, "-o", "pid,ppid,%cpu,%mem,start,command"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn preview_git(subcmd: &str, word: &str) -> String {
    match subcmd {
        "diff" => Command::new("git")
            .args(["diff", "--color=always", "--", word])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default(),
        "log" => Command::new("git")
            .args([
                "log",
                "--oneline",
                "--graph",
                "--color=always",
                "-20",
                word,
            ])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default(),
        "checkout" => Command::new("git")
            .args(["log", "--oneline", "--graph", "--color=always", "-10", word])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

// ── Colorize with LS_COLORS ────────────────────────────────────────────

fn colorize(display: &str, candidate: &Candidate, ls_colors: &LsColors) -> String {
    if !candidate.is_file {
        return display.to_string();
    }
    let path = if candidate.realdir.is_empty() {
        display.to_string()
    } else {
        format!("{}{}", candidate.realdir, display)
    };
    match ls_colors.style_for_path(&path) {
        Some(s) => s.to_nu_ansi_term_style().paint(display).to_string(),
        None => display.to_string(),
    }
}

// ── Output format ───────────────────────────────────────────────────────

/// Output mode determines the response format.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    /// JSON response (for testing / non-zsh consumers)
    Json,
    /// Eval-friendly format for zsh (no parsing needed):
    ///   Line 1: "select" or "abort"
    ///   Lines 2+: one selection per line, fields separated by \x1f (unit separator)
    ///             word \x1f prefix \x1f suffix \x1f iprefix \x1f isuffix \x1f args
    Eval,
}

// ── Shared completion logic ─────────────────────────────────────────────

fn run_completion(req: CompletionRequest, output_mode: OutputMode) {
    if req.candidates.is_empty() {
        print_response("abort", &[], output_mode);
        return;
    }

    // Single candidate — auto-select
    if req.candidates.len() == 1 {
        let c = &req.candidates[0];
        let sel = vec![Selection {
            word: c.word.clone(),
            iprefix: c.iprefix.clone(),
            prefix: c.prefix.clone(),
            suffix: c.suffix.clone(),
            isuffix: c.isuffix.clone(),
            args: c.args.clone(),
        }];
        print_response("select", &sel, output_mode);
        return;
    }

    let ls_colors = LsColors::from_env().unwrap_or_default();

    let items: Vec<Arc<dyn SkimItem>> = req
        .candidates
        .iter()
        .map(|c| {
            let display = if c.display.is_empty() {
                &c.word
            } else {
                &c.display
            };
            let colored = colorize(display, c, &ls_colors);
            Arc::new(CompItem { display: colored }) as Arc<dyn SkimItem>
        })
        .collect();

    let prompt = match req.command.as_str() {
        "cd" | "pushd" | "z" => ICON_CD,
        _ => ICON_POINTER,
    };

    let mut builder = base_options(&req.query);
    builder
        .multi(false)
        .prompt(prompt.to_string())
        .height("40%".to_string())
        .cycle(true)
        .no_sort(true);

    // Write preview manifest
    let manifest_path = std::env::temp_dir().join(format!(
        "skim-tab-manifest-{}.json",
        std::process::id()
    ));
    let manifest = serde_json::json!({
        "command": &req.command,
        "candidates": req.candidates.iter().map(|c| {
            serde_json::json!({
                "word": c.word,
                "display": if c.display.is_empty() { &c.word } else { &c.display },
                "realdir": c.realdir,
                "is_file": c.is_file,
                "group": c.group,
            })
        }).collect::<Vec<_>>(),
    });
    let _ = std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap());

    let preview_cmd = format!(
        "skim-tab --preview {} {{}}",
        manifest_path.display()
    );
    builder.preview(preview_cmd);
    builder.preview_window(
        skim::tui::options::PreviewLayout::from("right:50%:wrap"),
    );

    let skim_opts = match builder.build() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skim-tab --complete: failed to build options: {e}");
            std::process::exit(2);
        }
    };

    let display_strings: Vec<String> = items
        .iter()
        .map(|item| item.text().into_owned())
        .collect();

    let output = match Skim::run_items(skim_opts, display_strings) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skim-tab --complete: skim error: {e}");
            let _ = std::fs::remove_file(&manifest_path);
            std::process::exit(2);
        }
    };

    let _ = std::fs::remove_file(&manifest_path);

    if output.is_abort {
        print_response("abort", &[], output_mode);
        return;
    }

    let selected_items = if output.selected_items.is_empty() {
        output
            .current
            .as_ref()
            .map(|c| vec![c.output().to_string()])
            .unwrap_or_default()
    } else {
        output
            .selected_items
            .iter()
            .map(|s| s.item.output().to_string())
            .collect()
    };

    let mut selections = Vec::new();
    for selected_text in &selected_items {
        let plain = crate::strip_ansi(selected_text);
        if let Some(c) = req.candidates.iter().find(|c| {
            let display = if c.display.is_empty() {
                &c.word
            } else {
                &c.display
            };
            *display == plain
        }) {
            selections.push(Selection {
                word: c.word.clone(),
                iprefix: c.iprefix.clone(),
                prefix: c.prefix.clone(),
                suffix: c.suffix.clone(),
                isuffix: c.isuffix.clone(),
                args: c.args.clone(),
            });
        }
    }

    let action = if selections.is_empty() {
        "abort"
    } else {
        "select"
    };
    print_response(action, &selections, output_mode);
}

fn print_response(action: &str, selections: &[Selection], mode: OutputMode) {
    match mode {
        OutputMode::Json => {
            let resp = CompletionResponse {
                action: if action == "select" {
                    "select"
                } else {
                    "abort"
                },
                selections: selections.to_vec(),
                query: None,
            };
            println!("{}", serde_json::to_string(&resp).unwrap());
        }
        OutputMode::Eval => {
            // Line 1: action
            println!("{action}");
            // Lines 2+: one selection per line, fields separated by \x1f
            let us = '\x1f'; // unit separator
            for s in selections {
                println!(
                    "{}{us}{}{us}{}{us}{}{us}{}{us}{}",
                    s.word, s.prefix, s.suffix, s.iprefix, s.isuffix, s.args
                );
            }
        }
    }
}

// ── Entry points ────────────────────────────────────────────────────────

/// JSON mode: reads CompletionRequest JSON from stdin, outputs JSON.
pub fn run() {
    let mut input = String::new();
    io::stdin().lock().read_to_string(&mut input).unwrap_or(0);

    let req: CompletionRequest = match serde_json::from_str(&input) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skim-tab --complete: invalid JSON: {e}");
            print_response("abort", &[], OutputMode::Json);
            std::process::exit(1);
        }
    };

    run_completion(req, OutputMode::Json);
}

/// Compcap mode: reads fzf-tab's compcap format from stdin.
/// CLI args provide command and query.
/// Outputs eval-friendly format (not JSON) for direct zsh consumption.
pub fn run_compcap(args: &[String]) {
    let mut command = String::new();
    let mut query = String::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--command" => {
                i += 1;
                if i < args.len() {
                    command = args[i].clone();
                }
            }
            "--query" => {
                i += 1;
                if i < args.len() {
                    query = args[i].clone();
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Read binary stdin (may contain NUL bytes)
    let mut data = Vec::new();
    io::stdin().lock().read_to_end(&mut data).unwrap_or(0);

    let req = parse_compcap(&data, &command, &query);
    run_completion(req, OutputMode::Eval);
}

/// Preview subcommand: skim-tab --preview <manifest.json> <display_text>
pub fn run_preview(args: &[String]) {
    if args.len() < 2 {
        return;
    }
    let manifest_path = &args[0];
    let display_text = &args[1];

    let manifest_json = match std::fs::read_to_string(manifest_path) {
        Ok(s) => s,
        Err(_) => return,
    };

    #[derive(Deserialize)]
    struct PreviewManifest {
        command: String,
        candidates: Vec<PreviewCandidate>,
    }
    #[derive(Deserialize)]
    struct PreviewCandidate {
        word: String,
        display: String,
        #[serde(default)]
        realdir: String,
        #[serde(default)]
        is_file: bool,
        #[serde(default)]
        group: String,
    }

    let manifest: PreviewManifest = match serde_json::from_str(&manifest_json) {
        Ok(m) => m,
        Err(_) => return,
    };

    let plain = crate::strip_ansi(display_text);

    let candidate = match manifest.candidates.iter().find(|c| c.display == plain) {
        Some(c) => c,
        None => return,
    };

    let c = Candidate {
        word: candidate.word.clone(),
        display: candidate.display.clone(),
        realdir: candidate.realdir.clone(),
        is_file: candidate.is_file,
        group: candidate.group.clone(),
        group_index: 0,
        prefix: String::new(),
        suffix: String::new(),
        iprefix: String::new(),
        isuffix: String::new(),
        args: String::new(),
    };

    let output = preview_candidate(&c, &manifest.command);
    print!("{output}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_request() {
        let json = r#"{
            "candidates": [
                {"word": ".claude", "display": ".claude", "is_file": true, "realdir": "/Users/drzzln/"},
                {"word": ".git", "display": ".git", "is_file": true}
            ],
            "query": ".c",
            "command": "cd",
            "groups": ["directory"],
            "continuous_trigger": "/"
        }"#;
        let req: CompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.candidates.len(), 2);
        assert_eq!(req.command, "cd");
        assert_eq!(req.query, ".c");
        assert!(req.candidates[0].is_file);
        assert_eq!(req.candidates[0].realdir, "/Users/drzzln/");
    }

    #[test]
    fn serialize_response() {
        let resp = CompletionResponse {
            action: "select",
            selections: vec![Selection {
                word: ".claude".into(),
                iprefix: String::new(),
                prefix: String::new(),
                suffix: String::new(),
                isuffix: String::new(),
                args: String::new(),
            }],
            query: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"action\":\"select\""));
        assert!(json.contains(".claude"));
    }

    #[test]
    fn single_candidate_auto_selects() {
        let json = r#"{"candidates":[{"word":"only-one"}],"command":"cd"}"#;
        let req: CompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.candidates.len(), 1);
    }

    #[test]
    fn parse_compcap_basic() {
        // Simulate compcap format: display\x02<\x00>\x00PREFIX\x00.c\x00word\x00.claude
        let entry = b".claude\x02<\x00>\x00PREFIX\x00.c\x00word\x00.claude";
        let req = parse_compcap(entry, "cd", ".c");
        assert_eq!(req.candidates.len(), 1);
        assert_eq!(req.candidates[0].word, ".claude");
        assert_eq!(req.candidates[0].display, ".claude");
        assert_eq!(req.candidates[0].prefix, ".c");
        assert_eq!(req.command, "cd");
        assert_eq!(req.query, ".c");
    }

    #[test]
    fn parse_compcap_with_realdir() {
        // Entry with realdir → is_file should be true
        let entry =
            b".git\x02<\x00>\x00realdir\x00/Users/drzzln/\x00word\x00.git";
        let req = parse_compcap(entry, "cd", "");
        assert_eq!(req.candidates.len(), 1);
        assert!(req.candidates[0].is_file);
        assert_eq!(req.candidates[0].realdir, "/Users/drzzln/");
    }

    #[test]
    fn parse_compcap_multiple_entries() {
        // Two entries separated by ETX
        let mut data = Vec::new();
        data.extend_from_slice(b".claude\x02<\x00>\x00word\x00.claude");
        data.push(0x03);
        data.extend_from_slice(b".git\x02<\x00>\x00word\x00.git");
        data.push(0x03);

        let req = parse_compcap(&data, "cd", ".c");
        assert_eq!(req.candidates.len(), 2);
        assert_eq!(req.candidates[0].word, ".claude");
        assert_eq!(req.candidates[1].word, ".git");
    }

    #[test]
    fn parse_compcap_with_args() {
        // Entry with args containing SOH-separated flags
        let entry =
            b"item\x02<\x00>\x00args\x00-P\x01/usr/\x01-f\x00word\x00item";
        let req = parse_compcap(entry, "ls", "");
        assert_eq!(req.candidates.len(), 1);
        assert_eq!(req.candidates[0].args, "-P\x01/usr/\x01-f");
    }

    #[test]
    fn parse_compcap_empty() {
        let req = parse_compcap(b"", "cd", "");
        assert!(req.candidates.is_empty());
    }
}
