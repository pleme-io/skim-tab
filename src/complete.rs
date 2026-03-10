//! skim-tab --complete — native zsh completion via skim.
//!
//! Two input modes:
//!   1. JSON on stdin (for testing / other consumers)
//!   2. `--compcap` mode: reads NUL/STX compcap format on stdin,
//!      with `--command`, `--query`, `--buffer` as CLI args (for the zsh widget)

use crate::{base_options, ICON_CD, ICON_POINTER};
use lscolors::LsColors;
use serde::{Deserialize, Serialize};
use skim::prelude::*;
use std::collections::HashMap;
use std::io::{self, Read as _};

// ── Types ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CompletionRequest {
    pub candidates: Vec<Candidate>,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub buffer: String,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub continuous_trigger: String,
}

#[derive(Deserialize, Clone, Default)]
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

impl Candidate {
    fn display_text(&self) -> &str {
        if self.display.is_empty() { &self.word } else { &self.display }
    }

    fn to_selection(&self) -> Selection {
        Selection {
            word: self.word.clone(),
            prefix: self.prefix.clone(),
            suffix: self.suffix.clone(),
            iprefix: self.iprefix.clone(),
            isuffix: self.isuffix.clone(),
            args: self.args.clone(),
        }
    }
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
    pub prefix: String,
    pub suffix: String,
    pub iprefix: String,
    pub isuffix: String,
    pub args: String,
}

// ── Compcap parser ──────────────────────────────────────────────────

/// Parse compcap format from raw bytes.
///
/// Input: entries separated by ETX (\x03).
/// Each entry: `display\x02<\x00>\x00key\x00value\x00...\x00word\x00theword`
fn parse_compcap(data: &[u8], command: &str, query: &str, buffer: &str) -> CompletionRequest {
    let mut candidates = Vec::new();

    for entry in data.split(|&b| b == 0x03) {
        if entry.is_empty() {
            continue;
        }

        let stx_pos = match entry.iter().position(|&b| b == 0x02) {
            Some(pos) => pos,
            None => continue,
        };

        let display = String::from_utf8_lossy(&entry[..stx_pos]).to_string();
        let parts: Vec<&[u8]> = entry[stx_pos + 1..].split(|&b| b == 0x00).collect();
        let mut map: HashMap<String, String> = HashMap::new();

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
        // Parse group_index BEFORE removing group from map
        let group_index = map.get("group").and_then(|s| s.parse().ok()).unwrap_or(0);

        candidates.push(Candidate {
            word: map.remove("word").unwrap_or_default(),
            display,
            group: map.remove("group").unwrap_or_default(),
            group_index,
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
        buffer: buffer.to_string(),
        groups: vec![],
        continuous_trigger: "/".to_string(),
    }
}

// ── Colorize ─────────────────────────────────────────────────────────

fn colorize(display: &str, candidate: &Candidate, ls_colors: &LsColors) -> String {
    if !candidate.is_file {
        return display.to_string();
    }
    let path = if candidate.realdir.is_empty() {
        display.to_string()
    } else {
        format!("{}{display}", candidate.realdir)
    };
    ls_colors
        .style_for_path(&path)
        .map(|s| s.to_nu_ansi_term_style().paint(display).to_string())
        .unwrap_or_else(|| display.to_string())
}

// ── Output ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    Json,
    Eval,
}

fn print_response(action: &str, selections: &[Selection], mode: OutputMode) {
    match mode {
        OutputMode::Json => {
            let resp = CompletionResponse {
                action: if action == "select" { "select" } else { "abort" },
                selections: selections.to_vec(),
                query: None,
            };
            println!("{}", serde_json::to_string(&resp).unwrap());
        }
        OutputMode::Eval => {
            println!("{action}");
            for s in selections {
                println!(
                    "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
                    s.word, s.prefix, s.suffix, s.iprefix, s.isuffix, s.args
                );
            }
        }
    }
}

// ── Completion runner ────────────────────────────────────────────────

fn run_completion(req: CompletionRequest, output_mode: OutputMode) {
    if req.candidates.is_empty() {
        print_response("abort", &[], output_mode);
        return;
    }

    if req.candidates.len() == 1 {
        print_response("select", &[req.candidates[0].to_selection()], output_mode);
        return;
    }

    let ls_colors = LsColors::from_env().unwrap_or_default();
    let display_lines: Vec<String> = req
        .candidates
        .iter()
        .map(|c| colorize(c.display_text(), c, &ls_colors))
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

    // Write preview manifest for the --preview callback
    let manifest_path = std::env::temp_dir().join(format!(
        "skim-tab-manifest-{}.json",
        std::process::id()
    ));
    let manifest = serde_json::json!({
        "command": &req.command,
        "buffer": &req.buffer,
        "candidates": req.candidates.iter().map(|c| serde_json::json!({
            "word": c.word,
            "display": c.display_text(),
            "realdir": c.realdir,
        })).collect::<Vec<_>>(),
    });
    let _ = std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap());

    builder.preview(format!(
        "skim-tab --preview {} '{{}}'",
        manifest_path.display()
    ));
    builder.preview_window(skim::tui::options::PreviewLayout::from("right:50%:wrap"));

    let skim_opts = match builder.build() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skim-tab: failed to build options: {e}");
            std::process::exit(2);
        }
    };

    let items_text = display_lines.join("\n");
    let item_reader = SkimItemReader::new(SkimItemReaderOption::default().ansi(true));
    let items = item_reader.of_bufread(io::Cursor::new(items_text));

    let output = match Skim::run_with(skim_opts, Some(items)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skim-tab: skim error: {e}");
            let _ = std::fs::remove_file(&manifest_path);
            std::process::exit(2);
        }
    };

    let _ = std::fs::remove_file(&manifest_path);

    if output.is_abort {
        print_response("abort", &[], output_mode);
        return;
    }

    let selected_texts: Vec<String> = if output.selected_items.is_empty() {
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

    let selections: Vec<Selection> = selected_texts
        .iter()
        .filter_map(|text| {
            let plain = crate::strip_ansi(text);
            req.candidates
                .iter()
                .find(|c| c.display_text() == plain)
                .map(Candidate::to_selection)
        })
        .collect();

    let action = if selections.is_empty() { "abort" } else { "select" };
    print_response(action, &selections, output_mode);
}

// ── CLI arg helper ───────────────────────────────────────────────────

fn parse_kv_arg<'a>(args: &'a [String], key: &str) -> &'a str {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("")
}

// ── Entry points ────────────────────────────────────────────────────

/// JSON mode: reads CompletionRequest JSON from stdin, outputs JSON.
pub fn run() {
    let mut input = String::new();
    io::stdin().lock().read_to_string(&mut input).unwrap_or(0);

    let req: CompletionRequest = match serde_json::from_str(&input) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skim-tab: invalid JSON: {e}");
            print_response("abort", &[], OutputMode::Json);
            std::process::exit(1);
        }
    };

    run_completion(req, OutputMode::Json);
}

/// Compcap mode: reads compcap format from stdin, outputs eval-friendly lines.
pub fn run_compcap(args: &[String]) {
    let command = parse_kv_arg(args, "--command");
    let query = parse_kv_arg(args, "--query");
    let buffer = parse_kv_arg(args, "--buffer");

    let mut data = Vec::new();
    io::stdin().lock().read_to_end(&mut data).unwrap_or(0);

    let req = parse_compcap(&data, command, query, buffer);
    run_completion(req, OutputMode::Eval);
}

/// Preview subcommand: skim-tab --preview <manifest.json> <display_text>
pub fn run_preview(args: &[String]) {
    if args.len() < 2 {
        return;
    }

    let manifest_json = match std::fs::read_to_string(&args[0]) {
        Ok(s) => s,
        Err(_) => return,
    };

    #[derive(Deserialize)]
    struct Manifest {
        command: String,
        #[serde(default)]
        buffer: String,
        candidates: Vec<ManifestCandidate>,
    }

    #[derive(Deserialize)]
    struct ManifestCandidate {
        word: String,
        display: String,
        #[serde(default)]
        realdir: String,
    }

    let manifest: Manifest = match serde_json::from_str(&manifest_json) {
        Ok(m) => m,
        Err(_) => return,
    };

    let plain = crate::strip_ansi(&args[1]);
    let candidate = match manifest.candidates.iter().find(|c| c.display == plain) {
        Some(c) => c,
        None => return,
    };

    let output = crate::preview::preview(
        &candidate.word,
        &manifest.command,
        &manifest.buffer,
        &candidate.realdir,
    );
    print!("{output}");
}

// ── Tests ────────────────────────────────────────────────────────────

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
                prefix: String::new(),
                suffix: String::new(),
                iprefix: String::new(),
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
    fn candidate_display_text() {
        let with_display = Candidate {
            word: "foo".into(),
            display: "bar".into(),
            ..Default::default()
        };
        assert_eq!(with_display.display_text(), "bar");

        let without = Candidate {
            word: "foo".into(),
            ..Default::default()
        };
        assert_eq!(without.display_text(), "foo");
    }

    #[test]
    fn candidate_to_selection() {
        let c = Candidate {
            word: "pod-1".into(),
            prefix: "p".into(),
            iprefix: "i".into(),
            args: "-Q\x01-f".into(),
            ..Default::default()
        };
        let sel = c.to_selection();
        assert_eq!(sel.word, "pod-1");
        assert_eq!(sel.prefix, "p");
        assert_eq!(sel.iprefix, "i");
        assert_eq!(sel.args, "-Q\x01-f");
    }

    #[test]
    fn parse_compcap_basic() {
        let entry = b".claude\x02<\x00>\x00PREFIX\x00.c\x00word\x00.claude";
        let req = parse_compcap(entry, "cd", ".c", "cd ");
        assert_eq!(req.candidates.len(), 1);
        assert_eq!(req.candidates[0].word, ".claude");
        assert_eq!(req.candidates[0].display, ".claude");
        assert_eq!(req.candidates[0].prefix, ".c");
        assert_eq!(req.command, "cd");
        assert_eq!(req.query, ".c");
    }

    #[test]
    fn parse_compcap_with_realdir() {
        let entry = b".git\x02<\x00>\x00realdir\x00/Users/drzzln/\x00word\x00.git";
        let req = parse_compcap(entry, "cd", "", "cd ");
        assert_eq!(req.candidates.len(), 1);
        assert!(req.candidates[0].is_file);
        assert_eq!(req.candidates[0].realdir, "/Users/drzzln/");
    }

    #[test]
    fn parse_compcap_multiple_entries() {
        let mut data = Vec::new();
        data.extend_from_slice(b".claude\x02<\x00>\x00word\x00.claude");
        data.push(0x03);
        data.extend_from_slice(b".git\x02<\x00>\x00word\x00.git");
        data.push(0x03);

        let req = parse_compcap(&data, "cd", ".c", "cd .");
        assert_eq!(req.candidates.len(), 2);
        assert_eq!(req.candidates[0].word, ".claude");
        assert_eq!(req.candidates[1].word, ".git");
    }

    #[test]
    fn parse_compcap_with_args() {
        let entry = b"item\x02<\x00>\x00args\x00-P\x01/usr/\x01-f\x00word\x00item";
        let req = parse_compcap(entry, "ls", "", "ls ");
        assert_eq!(req.candidates.len(), 1);
        assert_eq!(req.candidates[0].args, "-P\x01/usr/\x01-f");
    }

    #[test]
    fn parse_compcap_empty() {
        let req = parse_compcap(b"", "cd", "", "cd ");
        assert!(req.candidates.is_empty());
    }

    #[test]
    fn parse_kv_arg_present() {
        let args: Vec<String> = vec!["--command", "cd", "--query", "foo"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(parse_kv_arg(&args, "--command"), "cd");
        assert_eq!(parse_kv_arg(&args, "--query"), "foo");
    }

    #[test]
    fn parse_kv_arg_missing() {
        let args: Vec<String> = vec!["--command", "cd"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(parse_kv_arg(&args, "--query"), "");
    }
}
