//! skim-tab --complete — native zsh completion via skim.
//!
//! Two input modes:
//!   1. JSON on stdin (for testing / other consumers)
//!   2. `--compcap` mode: reads NUL/STX compcap format on stdin,
//!      with `--command`, `--query`, `--buffer` as CLI args (for the zsh widget)

use crate::{
    base_options, config, history_db::HistoryDb, k8s,
    specs::{DescriptionProvider, SpecRegistry},
    ANSI_DIM, ANSI_FROST, ANSI_GREEN, ANSI_PURPLE, ANSI_RESET, ANSI_YELLOW, ICON_CD,
    ICON_POINTER,
};
use config::CompletionMode;
use crossterm::event::{KeyCode, KeyModifiers};
use lscolors::LsColors;
use serde::{Deserialize, Serialize};
use skim::prelude::*;
use std::collections::HashMap;
use std::io::{self, Read as _};

// ── Types ─────────────────────────────────────────────────────────────

/// A completion request received from the zsh widget or JSON stdin.
#[derive(Deserialize)]
pub struct CompletionRequest {
    /// Completion candidates to present in the picker.
    pub candidates: Vec<Candidate>,
    /// Initial query string for fuzzy filtering.
    #[serde(default)]
    pub query: String,
    /// The command being completed (e.g. `"kubectl"`).
    #[serde(default)]
    pub command: String,
    /// Full zsh `LBUFFER` text at the cursor.
    #[serde(default)]
    pub buffer: String,
    /// Completion group names (used for group switching).
    #[serde(default)]
    pub groups: Vec<String>,
    /// Key that triggers directory descent inside the picker.
    #[serde(default)]
    pub continuous_trigger: String,
}

/// A single completion candidate from zsh's `compadd` hook.
#[derive(Deserialize, Clone, Default)]
pub struct Candidate {
    /// The completion word to insert.
    pub word: String,
    /// Display text shown in the picker (falls back to `word`).
    #[serde(default)]
    pub display: String,
    /// Completion group name (e.g. `"directory"`, `"file"`).
    #[serde(default)]
    pub group: String,
    /// Numeric index of the completion group.
    #[serde(default)]
    pub group_index: usize,
    /// Real directory prefix for file candidates (from `compadd -R`).
    #[serde(default)]
    pub realdir: String,
    /// Whether this candidate represents a filesystem path.
    #[serde(default)]
    pub is_file: bool,
    /// Matched prefix text before cursor.
    #[serde(default)]
    pub prefix: String,
    /// Text after cursor that will be replaced (midword completion).
    #[serde(default)]
    pub suffix: String,
    /// Invisible prefix prepended to the word (`compadd -I`).
    #[serde(default)]
    pub iprefix: String,
    /// Invisible suffix appended to the word (`compadd -I`).
    #[serde(default)]
    pub isuffix: String,
    /// Original `zparseopts` args, joined with `\x01`.
    #[serde(default)]
    pub args: String,
}

impl Candidate {
    fn display_text(&self) -> &str {
        if self.display.is_empty() { &self.word } else { &self.display }
    }

    /// Build a selection respecting config flags.
    fn to_selection_with_config(&self, cfg: &config::CompletionConfig) -> Selection {
        let is_dir = self.is_file && {
            let path = if self.realdir.is_empty() {
                self.word.clone()
            } else {
                format!("{}{}", self.realdir, self.word)
            };
            let expanded = if path.starts_with('~') {
                std::env::var("HOME")
                    .map(|h| path.replacen('~', &h, 1))
                    .unwrap_or(path)
            } else {
                path
            };
            std::path::Path::new(&expanded).is_dir()
        };
        let word = if is_dir && cfg.dir_handling.append_slash && !self.word.ends_with('/') {
            format!("{}/", self.word)
        } else {
            self.word.clone()
        };
        Selection {
            word,
            prefix: self.prefix.clone(),
            suffix: self.suffix.clone(),
            iprefix: self.iprefix.clone(),
            isuffix: self.isuffix.clone(),
            args: self.args.clone(),
            is_dir,
        }
    }
}

/// Response emitted after the skim picker closes.
#[derive(Serialize)]
pub struct CompletionResponse {
    /// `"select"` when item(s) were chosen, `"abort"` on dismiss.
    pub action: &'static str,
    /// The chosen completion words (empty on abort).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub selections: Vec<Selection>,
    /// Final query text, if relevant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

/// A selected completion item ready for insertion into zsh.
#[derive(Serialize, Clone)]
pub struct Selection {
    /// The word to insert.
    pub word: String,
    /// Matched prefix text (for midword replacement).
    pub prefix: String,
    /// Suffix text after cursor to replace.
    pub suffix: String,
    /// Invisible prefix.
    pub iprefix: String,
    /// Invisible suffix.
    pub isuffix: String,
    /// Original `compadd` args, `\x01`-separated.
    pub args: String,
    /// Whether the selected word is a directory.
    pub is_dir: bool,
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

// ── K8s enrichment ───────────────────────────────────────────────────

/// Live cluster data collected once per completion invocation.
#[derive(Default)]
struct K8sEnrichment {
    /// Resource type → count (Phase 2)
    resource_counts: HashMap<String, usize>,
    /// Namespace → pod count (Phase 3)
    ns_pod_counts: HashMap<String, usize>,
    /// Currently active namespace (Phase 3)
    active_ns: String,
}

/// Check if candidates look like resource types (any has trailing `/`).
fn has_resource_type_candidates(candidates: &[Candidate]) -> bool {
    candidates.iter().any(|c| c.display_text().ends_with('/'))
}

/// Check if the buffer indicates namespace completion (`-n <TAB>` or `--namespace <TAB>`).
fn is_namespace_completion(buffer: &str) -> bool {
    let last = buffer.split_whitespace().last();
    matches!(last, Some("-n" | "--namespace"))
}

// ── Colorize ─────────────────────────────────────────────────────────

/// Apply Nord-themed ANSI coloring to a completion candidate.
///
/// - File candidates: lscolors (directories blue, executables green, etc.)
/// - Non-file with ` -- ` description: word in accent, description in dim
/// - Flags (`--foo`): yellow
/// - Everything else: frost accent
///
/// The text structure is preserved so that `strip_ansi(colored) == display`.
fn colorize(
    display: &str,
    candidate: &Candidate,
    ls_colors: &LsColors,
    command: &str,
    k8s: &K8sEnrichment,
    registry: &dyn DescriptionProvider,
) -> String {
    if candidate.is_file {
        let path = if candidate.realdir.is_empty() {
            display.to_string()
        } else {
            format!("{}{display}", candidate.realdir)
        };
        return ls_colors
            .style_for_path(&path)
            .map(|s| s.to_nu_ansi_term_style().paint(display).to_string())
            .unwrap_or_else(|| display.to_string());
    }

    // Strip trailing `/` for lookup (zsh adds it for resource type completions).
    let lookup_word = display.trim_end_matches('/');

    // Build enriched description from static + live data.
    let enriched = if display.contains(" -- ") {
        None
    } else {
        build_description(lookup_word, command, k8s, registry)
            .map(|d| format!("{display} -- {d}"))
    };
    let text = enriched.as_deref().unwrap_or(display);

    // Parse "word -- description" and apply colors
    if let Some((word, desc)) = text.split_once(" -- ") {
        let styled = color_description(desc);
        // Phase 3: namespace active marker — green highlight
        if !k8s.active_ns.is_empty() && lookup_word == k8s.active_ns {
            return format!("{ANSI_GREEN}{word}{ANSI_RESET} {ANSI_DIM}-- {styled}{ANSI_RESET}");
        }
        let wc = if word.starts_with('-') { ANSI_YELLOW } else { ANSI_FROST };
        format!("{wc}{word}{ANSI_RESET} {ANSI_DIM}-- {styled}{ANSI_RESET}")
    } else if text.starts_with('-') {
        format!("{ANSI_YELLOW}{text}{ANSI_RESET}")
    } else {
        format!("{ANSI_FROST}{text}{ANSI_RESET}")
    }
}

/// Compose a description from static registry + live cluster data.
fn build_description(
    word: &str,
    command: &str,
    k8s: &K8sEnrichment,
    registry: &dyn DescriptionProvider,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    // Static description from YAML specs
    if let Some(desc) = lookup_description(word, command, registry) {
        parts.push(desc);
    }

    // Phase 2: live resource count
    if let Some(&count) = k8s.resource_counts.get(word) {
        parts.push(count.to_string());
    }

    // Phase 3: namespace enrichment
    if !k8s.active_ns.is_empty() {
        let is_active = word == k8s.active_ns;
        match (is_active, k8s.ns_pod_counts.get(word)) {
            (true, Some(&count)) => parts.push(format!("active, {count} pods")),
            (true, None) => parts.push("active".to_string()),
            (false, Some(&count)) => parts.push(format!("{count} pods")),
            (false, None) => {}
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

/// Color description glyph prefix in purple, rest stays dim.
/// Non-ASCII leading character (our category glyph) gets ANSI_PURPLE;
/// ASCII-only descriptions pass through unchanged.
#[must_use]
fn color_description(desc: &str) -> String {
    let mut chars = desc.chars();
    match chars.next() {
        Some(c) if !c.is_ascii() => {
            format!("{ANSI_PURPLE}{c}{ANSI_DIM}{}", chars.as_str())
        }
        _ => desc.to_string(),
    }
}

// ── Description lookup ────────────────────────────────────────────────

/// Look up a description for a candidate word.
///
/// Checks the YAML spec registry (built-in + user + project specs).
/// Results include a glyph prefix (e.g., "◇ Build an image") when available.
#[must_use]
fn lookup_description(word: &str, command: &str, registry: &dyn DescriptionProvider) -> Option<String> {
    let base = command.split(':').next().unwrap_or(command);

    if let Some((glyph, desc)) = registry.lookup(base, word) {
        let formatted = if glyph.is_empty() {
            desc.to_owned()
        } else {
            format!("{glyph} {desc}")
        };
        return Some(formatted);
    }

    None
}

/// Get the prompt icon for a command, or None for the default.
///
/// Normalizes trailing space: icons without a trailing space get one appended.
#[must_use]
fn tool_icon<'a>(command: &str, registry: &'a dyn DescriptionProvider) -> Option<String> {
    registry.icon(command).map(|icon| {
        if icon.ends_with(' ') {
            icon.to_string()
        } else {
            format!("{icon} ")
        }
    })
}

// ── Output ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    Json,
    Eval,
}

fn print_response(action: &str, selections: &[Selection], mode: OutputMode, execute: bool) {
    match mode {
        OutputMode::Json => {
            let resp = CompletionResponse {
                action: if action == "select" { "select" } else { "abort" },
                selections: selections.to_vec(),
                query: None,
            };
            println!("{}", serde_json::to_string(&resp).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}")));
        }
        OutputMode::Eval => {
            println!("{action}");
            for s in selections {
                let dir_flag = if s.is_dir { "d" } else { "" };
                let exec_flag = if execute { "x" } else { "" };
                println!(
                    "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
                    s.word, s.prefix, s.suffix, s.iprefix, s.isuffix, s.args, dir_flag,
                    exec_flag
                );
            }
        }
    }
}

// ── Context helpers ──────────────────────────────────────────────────

/// Extract the base command for color/description lookups.
/// Prefers the buffer's first word (the actual command typed) over
/// the zsh curcontext command name.
#[must_use]
fn completion_base_cmd(command: &str, buffer: &str) -> String {
    buffer
        .split_whitespace()
        .next()
        .unwrap_or(command)
        .to_string()
}

// ── Key helpers (R2a / R6c) ──────────────────────────────────────────

/// A parsed key: (skim bind name, crossterm KeyCode + modifiers).
type ParsedKey = (String, (KeyCode, KeyModifiers));

/// Parse a single-character trigger (e.g., "/") into a skim bind name
/// and the corresponding crossterm KeyCode for `final_key` matching.
/// Returns `None` if the trigger string is empty or unparseable.
fn parse_trigger_keycode(trigger: &str) -> Option<ParsedKey> {
    if trigger.is_empty() {
        return None;
    }
    // Single character trigger — bind as the character itself
    if trigger.len() == 1 || trigger.chars().count() == 1 {
        let ch = trigger.chars().next()?;
        Some((ch.to_string(), (KeyCode::Char(ch), KeyModifiers::NONE)))
    } else {
        // Multi-char triggers like "ctrl-/" are handled by the general parser
        parse_accept_execute_key(trigger)
    }
}

/// Parse a key specification (e.g., "ctrl-x", "ctrl-/", "alt-a") into a
/// skim bind name and the corresponding crossterm KeyCode + modifiers.
/// Returns `None` if the key string is empty.
fn parse_accept_execute_key(key_spec: &str) -> Option<ParsedKey> {
    if key_spec.is_empty() {
        return None;
    }
    let lower = key_spec.to_lowercase();

    // ctrl-<char> patterns
    if let Some(ch_str) = lower.strip_prefix("ctrl-") {
        if ch_str.len() == 1 || ch_str.chars().count() == 1 {
            let ch = ch_str.chars().next()?;
            return Some((
                format!("ctrl-{ch}"),
                (KeyCode::Char(ch), KeyModifiers::CONTROL),
            ));
        }
    }

    // alt-<char> patterns
    if let Some(ch_str) = lower.strip_prefix("alt-") {
        if ch_str.len() == 1 || ch_str.chars().count() == 1 {
            let ch = ch_str.chars().next()?;
            return Some((
                format!("alt-{ch}"),
                (KeyCode::Char(ch), KeyModifiers::ALT),
            ));
        }
    }

    // Bare single character
    if lower.len() == 1 || lower.chars().count() == 1 {
        let ch = lower.chars().next()?;
        return Some((ch.to_string(), (KeyCode::Char(ch), KeyModifiers::NONE)));
    }

    None
}

/// Check if a crossterm `KeyEvent` matches a parsed (KeyCode, KeyModifiers) pair.
fn matches_key(
    event: &crossterm::event::KeyEvent,
    expected: &(KeyCode, KeyModifiers),
) -> bool {
    // Compare code and check that the expected modifiers are present
    // (crossterm may add extra modifiers like SHIFT for uppercase).
    event.code == expected.0 && event.modifiers.contains(expected.1)
}

// ── Completion runner ────────────────────────────────────────────────

fn run_completion(mut req: CompletionRequest, output_mode: OutputMode) {
    if req.candidates.is_empty() {
        print_response("abort", &[], output_mode, false);
        return;
    }

    let cfg = config::load();

    // Initialize the YAML spec registry with user config (lazy singleton —
    // first call wins, so we seed it here with the real config before any
    // lookup_description / tool_icon calls).
    let registry = SpecRegistry::global(&cfg.completion.specs);

    // Single candidate: auto-select or show picker based on config
    if req.candidates.len() == 1 && cfg.completion.single_auto_select {
        let c = &req.candidates[0];
        let sel = c.to_selection_with_config(&cfg.completion);

        // Optional in-picker descent
        if cfg.completion.in_picker_descent && sel.is_dir {
            let final_sel = crate::descent::run_descent(c, &sel, &req.command, matches!(output_mode, OutputMode::Eval));
            print_response("select", &[final_sel], output_mode, false);
            return;
        }

        print_response("select", &[sel], output_mode, false);
        return;
    }

    // Smart menu threshold: auto-insert all when candidate count is below
    // min_candidates but above 1 (skip skim picker for small sets).
    let min = cfg.completion.picker.min_candidates;
    if req.candidates.len() > 1 && req.candidates.len() < min {
        let selections: Vec<Selection> = req
            .candidates
            .iter()
            .map(|c| c.to_selection_with_config(&cfg.completion))
            .collect();
        print_response("select", &selections, output_mode, false);
        return;
    }

    let mode = cfg.completion.mode;
    let ls_colors = if cfg.completion.enrichment.lscolors {
        LsColors::from_env().unwrap_or_default()
    } else {
        LsColors::default()
    };
    let base_cmd = completion_base_cmd(&req.command, &req.buffer);
    let is_k8s = registry.is_k8s_command(&base_cmd);

    // ── Enrichment: service mode (future gRPC client) ───────────
    // When service or hybrid mode is active, we'll query the indexing
    // service here. For now this is a placeholder — the gRPC client
    // will be wired in when the service is built.
    let service_enrichment: Option<K8sEnrichment> = if is_k8s && mode.use_service() {
        // TODO: gRPC client call with cfg.completion.service.endpoint
        //       and cfg.completion.service.timeout_ms
        None // None = service unavailable (triggers direct fallback in hybrid)
    } else {
        None
    };

    // ── Enrichment: direct mode (local subprocess calls) ────────
    // Runs when mode is Direct, or when Hybrid and service was unavailable.
    let use_direct = mode == CompletionMode::Direct
        || (mode == CompletionMode::Hybrid && service_enrichment.is_none());

    // Phase 1: K8s context for header/prompt (pure file read, ~0ms)
    let kube_ctx = if is_k8s { k8s::KubeContext::current() } else { None };

    // Phase 2: Resource counts for resource type candidates
    let resource_counts = if use_direct
        && is_k8s
        && cfg.completion.direct.k8s_enrichment
        && has_resource_type_candidates(&req.candidates)
    {
        let types: Vec<&str> = req
            .candidates
            .iter()
            .map(|c| c.display_text().trim_end_matches('/'))
            .filter(|d| lookup_description(d, &base_cmd, registry).is_some())
            .collect();
        let ns = kube_ctx.as_ref().map(|c| c.namespace.as_str());
        k8s::resource_counts(&types, ns)
    } else {
        HashMap::new()
    };

    // Phase 3: Namespace enrichment
    let (ns_pod_counts, active_ns) = if use_direct
        && is_k8s
        && cfg.completion.direct.k8s_enrichment
        && is_namespace_completion(&req.buffer)
    {
        let active = kube_ctx
            .as_ref()
            .map_or("default", |c| c.namespace.as_str())
            .to_string();
        (k8s::namespace_pod_counts(), active)
    } else {
        (HashMap::new(), String::new())
    };

    let enrichment = K8sEnrichment {
        resource_counts,
        ns_pod_counts,
        active_ns,
    };

    // ── Frecency: open history DB and reorder candidates ────────
    let history_db = if cfg.completion.enrichment.history_boost || cfg.completion.enrichment.frecency
    {
        HistoryDb::open().ok()
    } else {
        None
    };

    if cfg.completion.enrichment.frecency {
        if let Some(ref db) = history_db {
            let cwd = std::env::current_dir()
                .map(|d| d.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Ok(scores) = db.frecency_scores(&base_cmd, &cwd) {
                if !scores.is_empty() {
                    // Stable sort: candidates with higher frecency come first,
                    // candidates without history preserve their original order.
                    req.candidates.sort_by(|a, b| {
                        let sa = scores.get(&a.word).copied().unwrap_or(0.0);
                        let sb = scores.get(&b.word).copied().unwrap_or(0.0);
                        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
            }
        }
    }

    let display_lines: Vec<String> = req
        .candidates
        .iter()
        .map(|c| colorize(c.display_text(), c, &ls_colors, &base_cmd, &enrichment, registry))
        .collect();

    // Prompt: context-aware for k8s, icon for others
    let prompt = match req.command.as_str() {
        "cd" | "pushd" | "z" => ICON_CD.to_string(),
        _ => kube_ctx
            .as_ref()
            .map(|ctx| ctx.prompt())
            .unwrap_or_else(|| tool_icon(&base_cmd, registry).unwrap_or_else(|| ICON_POINTER.to_string())),
    };

    let mut builder = base_options(&req.query);
    builder
        .multi(cfg.completion.picker.multi_select)
        .prompt(prompt)
        .height(cfg.completion.picker.height.clone())
        .cycle(cfg.completion.picker.cycle)
        .no_sort(cfg.completion.picker.no_sort);

    // ── R2a: bind continuous trigger key to accept ────────────────
    // When the user types the trigger character (default "/") in the
    // picker, skim accepts the current selection. We detect this via
    // `output.final_key` after skim returns.
    let trigger_key = &cfg.completion.picker.continuous_trigger;
    let trigger_keycode = parse_trigger_keycode(trigger_key);

    // ── R6c: bind accept-execute key to accept ───────────────────
    let exec_key = &cfg.completion.picker.accept_execute_key;
    let exec_keycode = parse_accept_execute_key(exec_key);

    // Build extra binds for trigger and execute keys, merged with
    // the standard binds (skim's bind setter replaces, doesn't append).
    let mut all_binds: Vec<String> = crate::STANDARD_BINDS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    if let Some((bind_name, _)) = &trigger_keycode {
        all_binds.push(format!("{bind_name}:accept"));
    }
    if let Some((bind_name, _)) = &exec_keycode {
        all_binds.push(format!("{bind_name}:accept"));
    }
    builder.bind(all_binds);

    // Group switching header (R2b): show group count info when candidates have groups
    let mut header_parts: Vec<String> = Vec::new();

    if cfg.completion.picker.show_group_header {
        let mut group_names: Vec<&str> = Vec::new();
        for c in &req.candidates {
            if !c.group.is_empty() && !group_names.contains(&c.group.as_str()) {
                group_names.push(&c.group);
            }
        }
        if group_names.len() > 1 {
            let names = group_names.join(", ");
            header_parts.push(format!(
                "{} groups: {} | F1/F2: switch",
                group_names.len(),
                names
            ));
        }
    }

    // Phase 1: context header (K8s)
    if let Some(ref ctx) = kube_ctx {
        header_parts.push(ctx.header());
    }

    if !header_parts.is_empty() {
        builder.header(header_parts.join(" | "));
    }

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
    let _ = std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}")));

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
        print_response("abort", &[], output_mode, false);
        return;
    }

    // ── Detect which key triggered accept ────────────────────────
    let was_trigger = trigger_keycode
        .as_ref()
        .is_some_and(|(_, kc)| matches_key(&output.final_key, kc));
    let was_execute = exec_keycode
        .as_ref()
        .is_some_and(|(_, kc)| matches_key(&output.final_key, kc));

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
            // Match against original display, or the word part before " -- "
            // (enriched descriptions add " -- desc" that isn't in display_text)
            let match_text = plain.split(" -- ").next().unwrap_or(&plain);
            req.candidates
                .iter()
                .find(|c| c.display_text() == plain || c.display_text() == match_text)
                .map(|c| c.to_selection_with_config(&cfg.completion))
        })
        .collect();

    // ── R2a: continuous trigger — descend into directory ──────────
    // If the trigger key (e.g., "/") was pressed and the selection is
    // a single directory, enter the descent loop immediately.
    if was_trigger && selections.len() == 1 && selections[0].is_dir {
        if let Some(sc) = req.candidates.iter().find(|c| {
            let sel_word = &selections[0].word;
            c.word == *sel_word || format!("{}/", c.word) == *sel_word
        }) {
            let final_sel = crate::descent::run_descent(
                sc,
                &selections[0],
                &req.command,
                matches!(output_mode, OutputMode::Eval),
            );
            print_response("select", &[final_sel], output_mode, false);
            return;
        }
    }

    // Optional in-picker descent for single directory selection from multi-candidate
    // (legacy behavior: descend on any dir select when in_picker_descent is enabled)
    if cfg.completion.in_picker_descent && selections.len() == 1 && selections[0].is_dir {
        if let Some(sc) = req.candidates.iter().find(|c| {
            let sel_word = &selections[0].word;
            // Match with or without trailing /
            c.word == *sel_word || format!("{}/", c.word) == *sel_word
        }) {
            let final_sel = crate::descent::run_descent(sc, &selections[0], &req.command, matches!(output_mode, OutputMode::Eval));
            print_response("select", &[final_sel], output_mode, false);
            return;
        }
    }

    // ── History: record selections ────────────────────────────────
    if cfg.completion.enrichment.history_boost && !selections.is_empty() {
        if let Some(ref db) = history_db {
            let cwd = std::env::current_dir()
                .map(|d| d.to_string_lossy().to_string())
                .unwrap_or_default();
            for sel in &selections {
                let _ = db.record(&base_cmd, &cwd, &sel.word);
            }
        }
    }

    let action = if selections.is_empty() { "abort" } else { "select" };
    print_response(action, &selections, output_mode, was_execute);
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
            print_response("abort", &[], OutputMode::Json, false);
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
    // Match against original display, or the word part before " -- "
    // (enriched descriptions add " -- desc" that isn't in the manifest)
    let match_text = plain.split(" -- ").next().unwrap_or(&plain);
    let candidate = match manifest
        .candidates
        .iter()
        .find(|c| c.display == plain || c.display == match_text)
    {
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

    /// Create a test-only SpecRegistry with all built-in specs.
    fn test_registry() -> SpecRegistry {
        let cfg = config::SpecsConfig {
            enable: true,
            dirs: vec![],
            project_specs: false,
        };
        SpecRegistry::new(&cfg)
    }

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
                is_dir: false,
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
        let cfg = config::CompletionConfig::default();
        let sel = c.to_selection_with_config(&cfg);
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

    #[test]
    fn colorize_file_candidate() {
        let ls = LsColors::from_env().unwrap_or_default();
        let reg = test_registry();
        let c = Candidate {
            word: "src".into(),
            is_file: true,
            realdir: "/tmp/".into(),
            ..Default::default()
        };
        // Should not panic and should return something non-empty
        let result = colorize("src", &c, &ls, "ls", &K8sEnrichment::default(), &reg);
        assert!(!result.is_empty());
    }

    #[test]
    fn colorize_non_file_with_description() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("get -- Display resources", &c, &ls, "kubectl", &K8sEnrichment::default(), &reg);
        // Should contain ANSI codes
        assert!(result.contains('\x1b'));
        // Stripped should match original
        assert_eq!(crate::strip_ansi(&result), "get -- Display resources");
    }

    #[test]
    fn colorize_flag() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("--namespace", &c, &ls, "kubectl", &K8sEnrichment::default(), &reg);
        assert!(result.contains('\x1b'));
        assert_eq!(crate::strip_ansi(&result), "--namespace");
    }

    #[test]
    fn colorize_flag_with_description() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("--output -- Output format", &c, &ls, "kubectl", &K8sEnrichment::default(), &reg);
        assert!(result.contains(ANSI_YELLOW));
        assert_eq!(crate::strip_ansi(&result), "--output -- Output format");
    }

    #[test]
    fn colorize_enriches_kubectl_subcommand() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate { word: "get".into(), display: "get".into(), ..Default::default() };
        let result = colorize("get", &c, &ls, "kubectl", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("get"));
        assert!(stripped.contains(" -- "));
        assert!(stripped.contains("Display resources"));
    }

    #[test]
    fn colorize_enriches_helm_subcommand() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("install", &c, &ls, "helm", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("install"));
        assert!(stripped.contains(" -- "));
        assert!(stripped.contains("Install a chart"));
    }

    #[test]
    fn colorize_enriches_flux_resource_type() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("kustomizations", &c, &ls, "flux", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("Kustomize reconciler"));
    }

    #[test]
    fn colorize_enriches_trailing_slash_resource() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("jobs/", &c, &ls, "kubectl", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("jobs/"));
        assert!(stripped.contains(" -- "));
        assert!(stripped.contains("Run-to-completion tasks"));
    }

    #[test]
    fn colorize_no_enrichment_for_unknown() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("my-random-pod", &c, &ls, "kubectl", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert_eq!(stripped, "my-random-pod");
    }

    #[test]
    fn lookup_description_kubectl() {
        let reg = test_registry();
        assert_eq!(lookup_description("pods", "kubectl", &reg).as_deref(), Some("\u{25C9} Pod workloads"));
        assert_eq!(lookup_description("deploy", "k", &reg).as_deref(), Some("\u{25CE} Managed replicas"));
        assert_eq!(lookup_description("unknown-thing", "kubectl", &reg), None);
    }

    #[test]
    fn lookup_description_helm() {
        let reg = test_registry();
        assert_eq!(lookup_description("upgrade", "helm", &reg).as_deref(), Some("\u{25C7} Upgrade a release"));
        assert_eq!(lookup_description("nope", "helm", &reg), None);
    }

    #[test]
    fn lookup_description_flux() {
        let reg = test_registry();
        assert_eq!(lookup_description("reconcile", "flux", &reg).as_deref(), Some("\u{21BB} Trigger reconciliation"));
        assert_eq!(lookup_description("hr", "flux", &reg).as_deref(), Some("\u{2295} Helm release reconciler"));
        assert_eq!(lookup_description("nope", "flux", &reg), None);
    }

    #[test]
    fn tool_icon_registry() {
        let reg = test_registry();
        let k8s_icon = "\u{2388} ";
        assert_eq!(tool_icon("kubectl", &reg).as_deref(), Some(k8s_icon));
        assert_eq!(tool_icon("k", &reg).as_deref(), Some(k8s_icon));
        assert_eq!(tool_icon("helm", &reg).as_deref(), Some(k8s_icon));
        assert_eq!(tool_icon("flux", &reg).as_deref(), Some(k8s_icon));
        assert_eq!(tool_icon("cd", &reg), None);
        assert_eq!(tool_icon("ls", &reg), None);
    }

    #[test]
    fn is_k8s_command_check() {
        let reg = test_registry();
        assert!(reg.is_k8s_command("kubectl"));
        assert!(reg.is_k8s_command("kubecolor"));
        assert!(reg.is_k8s_command("k"));
        assert!(reg.is_k8s_command("helm"));
        assert!(reg.is_k8s_command("flux"));
        assert!(!reg.is_k8s_command("aws"));
        assert!(!reg.is_k8s_command("gcloud"));
        assert!(!reg.is_k8s_command("az"));
        assert!(!reg.is_k8s_command("docker"));
        assert!(!reg.is_k8s_command("cd"));
    }

    #[test]
    fn completion_base_cmd_prefers_buffer() {
        assert_eq!(completion_base_cmd("", "kubectl get pods"), "kubectl");
        assert_eq!(completion_base_cmd("helm", ""), "helm");
        assert_eq!(completion_base_cmd("cd", "cd /tmp"), "cd");
    }

    #[test]
    fn has_resource_type_candidates_detects() {
        let with_slash = vec![
            Candidate { display: "pods/".into(), ..Default::default() },
            Candidate { display: "services/".into(), ..Default::default() },
        ];
        assert!(has_resource_type_candidates(&with_slash));

        let without = vec![
            Candidate { display: "get".into(), ..Default::default() },
            Candidate { display: "describe".into(), ..Default::default() },
        ];
        assert!(!has_resource_type_candidates(&without));
    }

    #[test]
    fn is_namespace_completion_detects() {
        assert!(is_namespace_completion("kubectl -n"));
        assert!(is_namespace_completion("kubectl get pods --namespace"));
        assert!(!is_namespace_completion("kubectl get pods"));
        assert!(!is_namespace_completion("kubectl -n default get"));
    }

    #[test]
    fn colorize_with_resource_count() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let k8s = K8sEnrichment {
            resource_counts: HashMap::from([("pods".to_string(), 42)]),
            ..Default::default()
        };
        let result = colorize("pods/", &c, &ls, "kubectl", &k8s, &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("Pod workloads"));
        assert!(stripped.contains("42"));
    }

    #[test]
    fn colorize_with_namespace_enrichment() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let k8s = K8sEnrichment {
            ns_pod_counts: HashMap::from([
                ("default".to_string(), 12),
                ("kube-system".to_string(), 8),
            ]),
            active_ns: "default".to_string(),
            ..Default::default()
        };
        // Active namespace
        let result = colorize("default", &c, &ls, "kubectl", &k8s, &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("active"));
        assert!(stripped.contains("12 pods"));
        // Active namespace gets green color
        assert!(result.contains(ANSI_GREEN));

        // Non-active namespace
        let result2 = colorize("kube-system", &c, &ls, "kubectl", &k8s, &reg);
        let stripped2 = crate::strip_ansi(&result2);
        assert!(!stripped2.contains("active"));
        assert!(stripped2.contains("8 pods"));
    }

    #[test]
    fn color_description_glyph_gets_purple() {
        let result = color_description("◉ Pod workloads");
        assert!(result.contains(ANSI_PURPLE));
        assert!(result.contains(ANSI_DIM));
        assert_eq!(crate::strip_ansi(&result), "◉ Pod workloads");
    }

    #[test]
    fn color_description_ascii_passthrough() {
        let result = color_description("active, 12 pods");
        assert!(!result.contains(ANSI_PURPLE));
        assert_eq!(result, "active, 12 pods");
    }

    #[test]
    fn build_description_combines_parts() {
        let reg = test_registry();
        let k8s = K8sEnrichment {
            resource_counts: HashMap::from([("pods".to_string(), 72)]),
            ..Default::default()
        };
        let desc = build_description("pods", "kubectl", &k8s, &reg);
        assert!(desc.is_some());
        let d = desc.unwrap();
        assert!(d.contains("Pod workloads"));
        assert!(d.contains("72"));
    }

    #[test]
    fn build_description_static_only() {
        let reg = test_registry();
        let k8s = K8sEnrichment::default();
        let desc = build_description("pods", "kubectl", &k8s, &reg);
        assert!(desc.is_some());
        assert!(desc.unwrap().contains("Pod workloads"));
    }

    #[test]
    fn build_description_count_only() {
        let reg = test_registry();
        let k8s = K8sEnrichment {
            resource_counts: HashMap::from([("unknown-type".to_string(), 5)]),
            ..Default::default()
        };
        let desc = build_description("unknown-type", "kubectl", &k8s, &reg);
        assert_eq!(desc, Some("5".to_string()));
    }

    #[test]
    fn build_description_namespace() {
        let reg = test_registry();
        let k8s = K8sEnrichment {
            ns_pod_counts: HashMap::from([("default".to_string(), 12)]),
            active_ns: "default".to_string(),
            ..Default::default()
        };
        let desc = build_description("default", "kubectl", &k8s, &reg);
        assert_eq!(desc, Some("active, 12 pods".to_string()));
    }

    // ── R2a: trigger key parsing ──────────────────────────────────

    #[test]
    fn parse_trigger_keycode_slash() {
        let result = parse_trigger_keycode("/");
        assert!(result.is_some());
        let (bind_name, (code, mods)) = result.unwrap();
        assert_eq!(bind_name, "/");
        assert_eq!(code, KeyCode::Char('/'));
        assert_eq!(mods, KeyModifiers::NONE);
    }

    #[test]
    fn parse_trigger_keycode_empty() {
        assert!(parse_trigger_keycode("").is_none());
    }

    #[test]
    fn parse_trigger_keycode_ctrl_slash() {
        let result = parse_trigger_keycode("ctrl-/");
        assert!(result.is_some());
        let (bind_name, (code, mods)) = result.unwrap();
        assert_eq!(bind_name, "ctrl-/");
        assert_eq!(code, KeyCode::Char('/'));
        assert_eq!(mods, KeyModifiers::CONTROL);
    }

    // ── R6c: accept-execute key parsing ───────────────────────────

    #[test]
    fn parse_accept_execute_key_ctrl_x() {
        let result = parse_accept_execute_key("ctrl-x");
        assert!(result.is_some());
        let (bind_name, (code, mods)) = result.unwrap();
        assert_eq!(bind_name, "ctrl-x");
        assert_eq!(code, KeyCode::Char('x'));
        assert_eq!(mods, KeyModifiers::CONTROL);
    }

    #[test]
    fn parse_accept_execute_key_alt_enter() {
        let result = parse_accept_execute_key("alt-a");
        assert!(result.is_some());
        let (bind_name, (code, mods)) = result.unwrap();
        assert_eq!(bind_name, "alt-a");
        assert_eq!(code, KeyCode::Char('a'));
        assert_eq!(mods, KeyModifiers::ALT);
    }

    #[test]
    fn parse_accept_execute_key_empty() {
        assert!(parse_accept_execute_key("").is_none());
    }

    #[test]
    fn parse_accept_execute_key_bare_char() {
        let result = parse_accept_execute_key("x");
        assert!(result.is_some());
        let (bind_name, (code, mods)) = result.unwrap();
        assert_eq!(bind_name, "x");
        assert_eq!(code, KeyCode::Char('x'));
        assert_eq!(mods, KeyModifiers::NONE);
    }

    // ── matches_key ───────────────────────────────────────────────

    #[test]
    fn matches_key_exact() {
        let event = crossterm::event::KeyEvent::new(
            KeyCode::Char('/'),
            KeyModifiers::NONE,
        );
        assert!(matches_key(&event, &(KeyCode::Char('/'), KeyModifiers::NONE)));
    }

    #[test]
    fn matches_key_ctrl() {
        let event = crossterm::event::KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::CONTROL,
        );
        assert!(matches_key(&event, &(KeyCode::Char('x'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn matches_key_wrong_char() {
        let event = crossterm::event::KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
        );
        assert!(!matches_key(&event, &(KeyCode::Char('/'), KeyModifiers::NONE)));
    }

    #[test]
    fn matches_key_wrong_modifier() {
        let event = crossterm::event::KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
        );
        assert!(!matches_key(&event, &(KeyCode::Char('x'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn matches_key_superset_modifiers_ok() {
        // crossterm may report CONTROL|SHIFT when the user hits ctrl-X
        // (uppercase). Our matcher checks that CONTROL is present.
        let event = crossterm::event::KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert!(matches_key(&event, &(KeyCode::Char('x'), KeyModifiers::CONTROL)));
    }

    // ── Midword completion tests ────────────────────────────────────

    #[test]
    fn parse_compcap_midword_prefix_and_suffix() {
        // Simulates cursor in middle of "commit": "com|mit"
        // PREFIX=com, SUFFIX=mit
        let entry = b"commit\x02<\x00>\x00PREFIX\x00com\x00SUFFIX\x00mit\x00word\x00commit";
        let req = parse_compcap(entry, "git", "^com", "git com");
        assert_eq!(req.candidates.len(), 1);
        let c = &req.candidates[0];
        assert_eq!(c.word, "commit");
        assert_eq!(c.prefix, "com");
        assert_eq!(c.suffix, "mit");
    }

    #[test]
    fn parse_compcap_midword_all_fields() {
        // All 4 positional fields set (PREFIX, SUFFIX, IPREFIX, ISUFFIX)
        let entry = b"word\x02<\x00>\x00PREFIX\x00pre\x00SUFFIX\x00suf\x00IPREFIX\x00ipre\x00ISUFFIX\x00isuf\x00word\x00word";
        let req = parse_compcap(entry, "cmd", "", "");
        let c = &req.candidates[0];
        assert_eq!(c.prefix, "pre");
        assert_eq!(c.suffix, "suf");
        assert_eq!(c.iprefix, "ipre");
        assert_eq!(c.isuffix, "isuf");
    }

    #[test]
    fn parse_compcap_midword_empty_suffix() {
        // Cursor at end of word — SUFFIX should be empty
        let entry = b"commit\x02<\x00>\x00PREFIX\x00commit\x00word\x00commit";
        let req = parse_compcap(entry, "git", "", "");
        let c = &req.candidates[0];
        assert_eq!(c.prefix, "commit");
        assert_eq!(c.suffix, ""); // no SUFFIX key → empty
    }

    #[test]
    fn candidate_to_selection_preserves_suffix() {
        let c = Candidate {
            word: "commit".into(),
            prefix: "com".into(),
            suffix: "mit".into(),
            iprefix: "insert-".into(),
            isuffix: "-end".into(),
            args: "-Q\x01-f".into(),
            ..Default::default()
        };
        let cfg = config::CompletionConfig::default();
        let sel = c.to_selection_with_config(&cfg);
        assert_eq!(sel.word, "commit");
        assert_eq!(sel.prefix, "com");
        assert_eq!(sel.suffix, "mit");
        assert_eq!(sel.iprefix, "insert-");
        assert_eq!(sel.isuffix, "-end");
        assert_eq!(sel.args, "-Q\x01-f");
        assert!(!sel.is_dir);
    }

    #[test]
    fn eval_response_format_has_8_fields() {
        let sel = Selection {
            word: "commit".into(),
            prefix: "com".into(),
            suffix: "mit".into(),
            iprefix: "".into(),
            isuffix: "".into(),
            args: "-Q".into(),
            is_dir: false,
        };
        let dir_flag = if sel.is_dir { "d" } else { "" };
        let exec_flag = "";
        let line = format!(
            "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
            sel.word, sel.prefix, sel.suffix,
            sel.iprefix, sel.isuffix, sel.args,
            dir_flag, exec_flag
        );
        let fields: Vec<&str> = line.split('\x1f').collect();
        assert_eq!(fields.len(), 8, "eval response must have exactly 8 fields");
        assert_eq!(fields[0], "commit");
        assert_eq!(fields[1], "com");   // PREFIX
        assert_eq!(fields[2], "mit");   // SUFFIX — critical for midword
        assert_eq!(fields[3], "");      // IPREFIX
        assert_eq!(fields[4], "");      // ISUFFIX
        assert_eq!(fields[5], "-Q");    // args
        assert_eq!(fields[6], "");      // dir_flag
        assert_eq!(fields[7], "");      // exec_flag
    }

    #[test]
    fn eval_response_dir_flag_set() {
        let sel = Selection {
            word: "scripts/".into(),
            prefix: "scr".into(),
            suffix: "ipts".into(),
            iprefix: "".into(),
            isuffix: "".into(),
            args: "".into(),
            is_dir: true,
        };
        let dir_flag = if sel.is_dir { "d" } else { "" };
        let line = format!(
            "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f",
            sel.word, sel.prefix, sel.suffix,
            sel.iprefix, sel.isuffix, sel.args, dir_flag
        );
        let fields: Vec<&str> = line.split('\x1f').collect();
        assert_eq!(fields[6], "d");
        assert_eq!(fields[2], "ipts"); // SUFFIX preserved even for dirs
    }

    #[test]
    fn parse_compcap_roundtrip_midword() {
        // Full roundtrip: parse compcap → candidate → selection → eval format
        let entry = b"screenshot\x02<\x00>\x00PREFIX\x00scr\x00SUFFIX\x00ipt.sh\x00word\x00screenshot";
        let req = parse_compcap(entry, "vim", "^scr", "vim scr");
        let c = &req.candidates[0];
        assert_eq!(c.prefix, "scr");
        assert_eq!(c.suffix, "ipt.sh");

        let cfg = config::CompletionConfig::default();
        let sel = c.to_selection_with_config(&cfg);
        assert_eq!(sel.suffix, "ipt.sh");

        // Verify eval output preserves suffix
        let dir_flag = if sel.is_dir { "d" } else { "" };
        let line = format!(
            "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f",
            sel.word, sel.prefix, sel.suffix,
            sel.iprefix, sel.isuffix, sel.args, dir_flag
        );
        let fields: Vec<&str> = line.split('\x1f').collect();
        assert_eq!(fields[0], "screenshot"); // word
        assert_eq!(fields[1], "scr");        // prefix
        assert_eq!(fields[2], "ipt.sh");     // suffix preserved through pipeline
    }

    #[test]
    fn parse_compcap_midword_with_args_and_suffix() {
        // Complex case: midword + zparseopts args
        let entry = b"target\x02<\x00>\x00PREFIX\x00tar\x00SUFFIX\x00get\x00args\x00-P\x01./\x01-f\x00word\x00target";
        let req = parse_compcap(entry, "ls", "", "");
        let c = &req.candidates[0];
        assert_eq!(c.prefix, "tar");
        assert_eq!(c.suffix, "get");
        assert_eq!(c.args, "-P\x01./\x01-f");
    }

    #[test]
    fn multiple_selections_preserve_suffix() {
        // When multiple candidates are selected, each should preserve its own suffix
        let c1 = Candidate {
            word: "commit".into(),
            prefix: "com".into(),
            suffix: "mit".into(),
            ..Default::default()
        };
        let c2 = Candidate {
            word: "compare".into(),
            prefix: "com".into(),
            suffix: "mit".into(), // same suffix context
            ..Default::default()
        };
        let cfg = config::CompletionConfig::default();
        let s1 = c1.to_selection_with_config(&cfg);
        let s2 = c2.to_selection_with_config(&cfg);
        assert_eq!(s1.suffix, "mit");
        assert_eq!(s2.suffix, "mit");
        // Both selections carry the suffix through
        assert_ne!(s1.word, s2.word); // but different words
    }

    // ── Cursor position edge cases ──────────────────────────────────

    #[test]
    fn parse_compcap_cursor_at_word_start() {
        // Cursor at start of word: "|commit" → PREFIX="", SUFFIX="commit"
        let entry = b"commit\x02<\x00>\x00SUFFIX\x00commit\x00word\x00commit";
        let req = parse_compcap(entry, "git", "", "git ");
        let c = &req.candidates[0];
        assert_eq!(c.word, "commit");
        assert_eq!(c.prefix, "");      // empty — cursor at start
        assert_eq!(c.suffix, "commit"); // entire word is suffix
    }

    #[test]
    fn parse_compcap_hyphenated_midword() {
        // "kubectl --name|space" → PREFIX="--name", SUFFIX="space"
        let entry = b"--namespace\x02<\x00>\x00PREFIX\x00--name\x00SUFFIX\x00space\x00word\x00--namespace";
        let req = parse_compcap(entry, "kubectl", "^--name", "kubectl --name");
        let c = &req.candidates[0];
        assert_eq!(c.word, "--namespace");
        assert_eq!(c.prefix, "--name");
        assert_eq!(c.suffix, "space");
    }

    #[test]
    fn parse_compcap_path_midword() {
        // "cd ~/co|de/github" → PREFIX="co", SUFFIX="de", IPREFIX="~/"
        let entry = b"code\x02<\x00>\x00PREFIX\x00co\x00SUFFIX\x00de\x00IPREFIX\x00~/\x00word\x00code";
        let req = parse_compcap(entry, "cd", "^co", "cd ~/co");
        let c = &req.candidates[0];
        assert_eq!(c.prefix, "co");
        assert_eq!(c.suffix, "de");
        assert_eq!(c.iprefix, "~/");
    }

    #[test]
    fn candidate_to_selection_cursor_at_start() {
        // Empty prefix, full word as suffix
        let c = Candidate {
            word: "commit".into(),
            prefix: "".into(),
            suffix: "commit".into(),
            ..Default::default()
        };
        let cfg = config::CompletionConfig::default();
        let sel = c.to_selection_with_config(&cfg);
        assert_eq!(sel.prefix, "");
        assert_eq!(sel.suffix, "commit");
    }

    #[test]
    fn eval_response_empty_prefix_nonempty_suffix() {
        // Cursor at word start: fields[1] (prefix) empty, fields[2] (suffix) full
        let sel = Selection {
            word: "commit".into(),
            prefix: "".into(),
            suffix: "commit".into(),
            iprefix: "".into(),
            isuffix: "".into(),
            args: "".into(),
            is_dir: false,
        };
        let line = format!(
            "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
            sel.word, sel.prefix, sel.suffix,
            sel.iprefix, sel.isuffix, sel.args, "", ""
        );
        let fields: Vec<&str> = line.split('\x1f').collect();
        assert_eq!(fields[0], "commit");
        assert_eq!(fields[1], "");       // PREFIX empty
        assert_eq!(fields[2], "commit"); // SUFFIX is the full word
    }

    #[test]
    fn parse_compcap_equals_sign_in_suffix() {
        // "kubectl --namespace=def|ault" → SUFFIX="ault"
        let entry = b"default\x02<\x00>\x00PREFIX\x00def\x00SUFFIX\x00ault\x00word\x00default";
        let req = parse_compcap(entry, "kubectl", "^def", "kubectl --namespace=def");
        let c = &req.candidates[0];
        assert_eq!(c.prefix, "def");
        assert_eq!(c.suffix, "ault");
    }

    #[test]
    fn roundtrip_cursor_at_start() {
        let entry = b"screenshot\x02<\x00>\x00SUFFIX\x00screenshot\x00word\x00screenshot";
        let req = parse_compcap(entry, "vim", "", "vim ");
        let c = &req.candidates[0];
        assert_eq!(c.prefix, "");
        assert_eq!(c.suffix, "screenshot");

        let cfg = config::CompletionConfig::default();
        let sel = c.to_selection_with_config(&cfg);
        assert_eq!(sel.suffix, "screenshot");

        let line = format!(
            "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f",
            sel.word, sel.prefix, sel.suffix,
            sel.iprefix, sel.isuffix, sel.args,
            if sel.is_dir { "d" } else { "" }
        );
        let fields: Vec<&str> = line.split('\x1f').collect();
        assert_eq!(fields[0], "screenshot");
        assert_eq!(fields[1], "");
        assert_eq!(fields[2], "screenshot");
    }

    // ── parse_compcap boundary conditions ────────────────────────────

    #[test]
    fn parse_compcap_no_stx_marker() {
        // Entry without STX should be skipped entirely
        let data = b"nodisplay_and_no_stx_marker";
        let req = parse_compcap(data, "cd", "", "cd ");
        assert!(req.candidates.is_empty());
    }

    #[test]
    fn parse_compcap_only_separator_bytes() {
        let data = b"\x03\x03\x03";
        let req = parse_compcap(data, "cd", "", "cd ");
        assert!(req.candidates.is_empty());
    }

    #[test]
    fn parse_compcap_missing_word_key() {
        // Entry with STX but no "word" key → word defaults to empty
        let entry = b"display\x02<\x00>\x00PREFIX\x00pre";
        let req = parse_compcap(entry, "cd", "", "cd ");
        assert_eq!(req.candidates.len(), 1);
        assert_eq!(req.candidates[0].word, "");
        assert_eq!(req.candidates[0].display, "display");
    }

    #[test]
    fn parse_compcap_continuous_trigger_default() {
        let req = parse_compcap(b"", "cd", "", "cd ");
        assert_eq!(req.continuous_trigger, "/");
    }

    #[test]
    fn parse_compcap_unicode_display() {
        let entry = "éàü\x02<\x00>\x00word\x00éàü".as_bytes();
        let req = parse_compcap(entry, "cd", "", "cd ");
        assert_eq!(req.candidates.len(), 1);
        assert_eq!(req.candidates[0].word, "éàü");
        assert_eq!(req.candidates[0].display, "éàü");
    }

    // ── candidate display_text ───────────────────────────────────────

    #[test]
    fn candidate_display_text_empty_display_uses_word() {
        let c = Candidate {
            word: "the-word".into(),
            display: "".into(),
            ..Default::default()
        };
        assert_eq!(c.display_text(), "the-word");
    }

    // ── to_selection_with_config dir detection ───────────────────────

    #[test]
    fn candidate_to_selection_non_file_never_dir() {
        let c = Candidate {
            word: "/tmp".into(),
            is_file: false,
            ..Default::default()
        };
        let cfg = config::CompletionConfig::default();
        let sel = c.to_selection_with_config(&cfg);
        assert!(!sel.is_dir, "non-file candidate should never be detected as dir");
    }

    #[test]
    fn candidate_to_selection_file_real_dir() {
        let c = Candidate {
            word: "/tmp".into(),
            is_file: true,
            ..Default::default()
        };
        let cfg = config::CompletionConfig::default();
        let sel = c.to_selection_with_config(&cfg);
        assert!(sel.is_dir);
        assert!(sel.word.ends_with('/'));
    }

    #[test]
    fn candidate_to_selection_dir_already_has_slash() {
        let c = Candidate {
            word: "/tmp/".into(),
            is_file: true,
            ..Default::default()
        };
        let cfg = config::CompletionConfig::default();
        let sel = c.to_selection_with_config(&cfg);
        assert!(sel.is_dir);
        assert_eq!(sel.word, "/tmp/");
    }

    #[test]
    fn candidate_to_selection_append_slash_disabled() {
        let c = Candidate {
            word: "/tmp".into(),
            is_file: true,
            ..Default::default()
        };
        let mut cfg = config::CompletionConfig::default();
        cfg.dir_handling.append_slash = false;
        let sel = c.to_selection_with_config(&cfg);
        assert!(sel.is_dir);
        assert_eq!(sel.word, "/tmp");
    }

    // ── colorize edge cases ──────────────────────────────────────────

    #[test]
    fn colorize_plain_word_gets_frost() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("some-random-thing", &c, &ls, "unknown-cmd", &K8sEnrichment::default(), &reg);
        assert!(result.contains(ANSI_FROST));
        assert_eq!(crate::strip_ansi(&result), "some-random-thing");
    }

    #[test]
    fn colorize_file_with_empty_realdir() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate {
            word: "file.txt".into(),
            is_file: true,
            realdir: "".into(),
            ..Default::default()
        };
        let result = colorize("file.txt", &c, &ls, "ls", &K8sEnrichment::default(), &reg);
        assert!(!result.is_empty());
    }

    #[test]
    fn colorize_namespace_active_no_pods() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let k8s = K8sEnrichment {
            ns_pod_counts: HashMap::new(),
            active_ns: "myns".to_string(),
            ..Default::default()
        };
        let result = colorize("myns", &c, &ls, "kubectl", &k8s, &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("active"));
        assert!(!stripped.contains("pods"));
    }

    #[test]
    fn colorize_namespace_inactive_no_pods() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let k8s = K8sEnrichment {
            ns_pod_counts: HashMap::new(),
            active_ns: "other-ns".to_string(),
            ..Default::default()
        };
        let result = colorize("myns", &c, &ls, "kubectl", &k8s, &reg);
        let stripped = crate::strip_ansi(&result);
        // inactive namespace with no pod count should have no enrichment
        assert_eq!(stripped, "myns");
    }

    // ── color_description edge cases ─────────────────────────────────

    #[test]
    fn color_description_empty_string() {
        let result = color_description("");
        assert_eq!(result, "");
    }

    #[test]
    fn color_description_single_ascii_char() {
        let result = color_description("x");
        assert_eq!(result, "x");
    }

    #[test]
    fn color_description_single_unicode_char() {
        let result = color_description("◉");
        assert!(result.contains(ANSI_PURPLE));
    }

    // ── build_description edge cases ─────────────────────────────────

    #[test]
    fn build_description_no_match_returns_none() {
        let reg = test_registry();
        let k8s = K8sEnrichment::default();
        assert!(build_description("completely-unknown-xyz", "unknown-tool", &k8s, &reg).is_none());
    }

    #[test]
    fn build_description_namespace_active_with_pods_and_static() {
        let reg = test_registry();
        let k8s = K8sEnrichment {
            resource_counts: HashMap::from([("default".to_string(), 3)]),
            ns_pod_counts: HashMap::from([("default".to_string(), 10)]),
            active_ns: "default".to_string(),
        };
        let desc = build_description("default", "kubectl", &k8s, &reg).unwrap();
        assert!(desc.contains("3"));
        assert!(desc.contains("active"));
        assert!(desc.contains("10 pods"));
    }

    // ── tool_icon edge cases ─────────────────────────────────────────

    #[test]
    fn tool_icon_appends_space_when_missing() {
        use crate::specs::MockDescriptionProvider;
        let mut mock = MockDescriptionProvider::new();
        mock.icons.insert("test".to_string(), "X".to_string());
        let result = tool_icon("test", &mock);
        assert_eq!(result.as_deref(), Some("X "));
    }

    #[test]
    fn tool_icon_preserves_trailing_space() {
        use crate::specs::MockDescriptionProvider;
        let mut mock = MockDescriptionProvider::new();
        mock.icons.insert("test".to_string(), "X ".to_string());
        let result = tool_icon("test", &mock);
        assert_eq!(result.as_deref(), Some("X "));
    }

    #[test]
    fn tool_icon_returns_none_for_unknown() {
        use crate::specs::MockDescriptionProvider;
        let mock = MockDescriptionProvider::new();
        assert!(tool_icon("unknown", &mock).is_none());
    }

    // ── completion_base_cmd edge cases ────────────────────────────────

    #[test]
    fn completion_base_cmd_empty_both() {
        assert_eq!(completion_base_cmd("", ""), "");
    }

    #[test]
    fn completion_base_cmd_whitespace_only_buffer() {
        assert_eq!(completion_base_cmd("fallback", "   "), "fallback");
    }

    // ── has_resource_type_candidates ──────────────────────────────────

    #[test]
    fn has_resource_type_candidates_empty_list() {
        assert!(!has_resource_type_candidates(&[]));
    }

    #[test]
    fn has_resource_type_candidates_uses_display_over_word() {
        let c = Candidate {
            word: "pods".into(),
            display: "pods/".into(),
            ..Default::default()
        };
        assert!(has_resource_type_candidates(&[c]));
    }

    // ── is_namespace_completion ───────────────────────────────────────

    #[test]
    fn is_namespace_completion_empty_buffer() {
        assert!(!is_namespace_completion(""));
    }

    #[test]
    fn is_namespace_completion_only_flag() {
        assert!(is_namespace_completion("-n"));
        assert!(is_namespace_completion("--namespace"));
    }

    // ── parse_trigger_keycode edge cases ─────────────────────────────

    #[test]
    fn parse_trigger_keycode_unicode_char() {
        let result = parse_trigger_keycode("é");
        assert!(result.is_some());
        let (bind, (code, mods)) = result.unwrap();
        assert_eq!(bind, "é");
        assert_eq!(code, KeyCode::Char('é'));
        assert_eq!(mods, KeyModifiers::NONE);
    }

    #[test]
    fn parse_trigger_keycode_multi_char_non_modifier() {
        // e.g., "ab" — multi-char that isn't ctrl-/alt- → delegated
        assert!(parse_trigger_keycode("ab").is_none());
    }

    // ── parse_accept_execute_key edge cases ──────────────────────────

    #[test]
    fn parse_accept_execute_key_uppercase() {
        let result = parse_accept_execute_key("CTRL-A");
        assert!(result.is_some());
        let (bind, (code, mods)) = result.unwrap();
        assert_eq!(bind, "ctrl-a");
        assert_eq!(code, KeyCode::Char('a'));
        assert_eq!(mods, KeyModifiers::CONTROL);
    }

    #[test]
    fn parse_accept_execute_key_alt_multi_char() {
        // "alt-ab" is too long for a single char
        assert!(parse_accept_execute_key("alt-ab").is_none());
    }

    #[test]
    fn parse_accept_execute_key_ctrl_multi_char() {
        assert!(parse_accept_execute_key("ctrl-ab").is_none());
    }

    // ── matches_key with ALT modifier ────────────────────────────────

    #[test]
    fn matches_key_alt() {
        let event = crossterm::event::KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::ALT,
        );
        assert!(matches_key(&event, &(KeyCode::Char('a'), KeyModifiers::ALT)));
        assert!(!matches_key(&event, &(KeyCode::Char('a'), KeyModifiers::CONTROL)));
    }

    // ── lookup_description edge cases ────────────────────────────────

    #[test]
    fn lookup_description_command_with_colon() {
        // curcontext format like "git:checkout:"
        let reg = test_registry();
        let result = lookup_description("get", "kubectl:get:", &reg);
        assert!(result.is_some());
        assert!(result.unwrap().contains("Display resources"));
    }

    #[test]
    fn lookup_description_returns_none_for_unknown() {
        let reg = test_registry();
        assert!(lookup_description("xyz-no-such", "kubectl", &reg).is_none());
    }

    // ── CompletionResponse serialization ─────────────────────────────

    #[test]
    fn serialize_response_abort_no_selections() {
        let resp = CompletionResponse {
            action: "abort",
            selections: vec![],
            query: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"action\":\"abort\""));
        assert!(!json.contains("selections"));
    }

    #[test]
    fn serialize_response_with_query() {
        let resp = CompletionResponse {
            action: "select",
            selections: vec![],
            query: Some("test".to_string()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"query\":\"test\""));
    }

    // ── parse_kv_arg edge cases ──────────────────────────────────────

    #[test]
    fn parse_kv_arg_key_at_end() {
        let args: Vec<String> = vec!["--command".to_string()];
        assert_eq!(parse_kv_arg(&args, "--command"), "");
    }

    #[test]
    fn parse_kv_arg_empty_args() {
        let args: Vec<String> = vec![];
        assert_eq!(parse_kv_arg(&args, "--command"), "");
    }

    #[test]
    fn parse_kv_arg_duplicate_keys() {
        // First occurrence wins
        let args: Vec<String> = vec!["--cmd", "first", "--cmd", "second"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(parse_kv_arg(&args, "--cmd"), "first");
    }

    // ── CompletionRequest deserialization edge cases ──────────────────

    #[test]
    fn deserialize_request_minimal() {
        let json = r#"{"candidates":[]}"#;
        let req: CompletionRequest = serde_json::from_str(json).unwrap();
        assert!(req.candidates.is_empty());
        assert!(req.command.is_empty());
        assert!(req.query.is_empty());
        assert!(req.buffer.is_empty());
        assert!(req.groups.is_empty());
        assert!(req.continuous_trigger.is_empty());
    }

    #[test]
    fn deserialize_request_with_groups() {
        let json = r#"{"candidates":[],"groups":["files","flags","subcommands"]}"#;
        let req: CompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.groups.len(), 3);
        assert_eq!(req.groups[0], "files");
    }

    #[test]
    fn deserialize_candidate_defaults() {
        let json = r#"{"word":"test"}"#;
        let c: Candidate = serde_json::from_str(json).unwrap();
        assert_eq!(c.word, "test");
        assert!(c.display.is_empty());
        assert!(c.group.is_empty());
        assert_eq!(c.group_index, 0);
        assert!(c.realdir.is_empty());
        assert!(!c.is_file);
        assert!(c.prefix.is_empty());
        assert!(c.suffix.is_empty());
        assert!(c.iprefix.is_empty());
        assert!(c.isuffix.is_empty());
        assert!(c.args.is_empty());
    }

    // ── Selection serialization ───────────────────────────────────────

    #[test]
    fn serialize_selection_roundtrip() {
        let sel = Selection {
            word: "pods".into(),
            prefix: "po".into(),
            suffix: "ds".into(),
            iprefix: "".into(),
            isuffix: "".into(),
            args: "-Q".into(),
            is_dir: false,
        };
        let json = serde_json::to_string(&sel).unwrap();
        assert!(json.contains("\"word\":\"pods\""));
        assert!(json.contains("\"prefix\":\"po\""));
        assert!(json.contains("\"suffix\":\"ds\""));
        assert!(json.contains("\"is_dir\":false"));
    }

    #[test]
    fn serialize_response_multiple_selections() {
        let resp = CompletionResponse {
            action: "select",
            selections: vec![
                Selection {
                    word: "a".into(),
                    prefix: "".into(),
                    suffix: "".into(),
                    iprefix: "".into(),
                    isuffix: "".into(),
                    args: "".into(),
                    is_dir: false,
                },
                Selection {
                    word: "b".into(),
                    prefix: "".into(),
                    suffix: "".into(),
                    iprefix: "".into(),
                    isuffix: "".into(),
                    args: "".into(),
                    is_dir: true,
                },
            ],
            query: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"action\":\"select\""));
        assert!(json.contains("\"word\":\"a\""));
        assert!(json.contains("\"word\":\"b\""));
    }

    // ── colorize dispatch: all 12 spec commands ───────────────────────

    #[test]
    fn colorize_enriches_docker_subcommand() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("run", &c, &ls, "docker", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("Run a container"));
    }

    #[test]
    fn colorize_enriches_git_subcommand() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("commit", &c, &ls, "git", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("Record changes"));
    }

    #[test]
    fn colorize_enriches_cargo_subcommand() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("build", &c, &ls, "cargo", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("Compile the current package"));
    }

    #[test]
    fn colorize_enriches_npm_subcommand() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("install", &c, &ls, "npm", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains(" -- "));
    }

    #[test]
    fn colorize_enriches_terraform_subcommand() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("plan", &c, &ls, "terraform", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("Show execution plan"));
    }

    #[test]
    fn colorize_enriches_aws_subcommand() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("s3", &c, &ls, "aws", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("Object storage"));
    }

    #[test]
    fn colorize_enriches_nix_subcommand() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("build", &c, &ls, "nix", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("Build a derivation"));
    }

    // ── colorize: existing description is preserved ───────────────────

    #[test]
    fn colorize_does_not_double_enrich() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("get -- Display resources", &c, &ls, "kubectl", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert_eq!(stripped, "get -- Display resources");
        assert_eq!(stripped.matches(" -- ").count(), 1, "should not double-add description");
    }

    // ── colorize with podman alias ────────────────────────────────────

    #[test]
    fn colorize_enriches_podman_alias() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let result = colorize("build", &c, &ls, "podman", &K8sEnrichment::default(), &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("Build an image"));
    }

    // ── K8sEnrichment: combined resource counts + namespace ──────────

    #[test]
    fn colorize_combines_resource_count_and_description() {
        let ls = LsColors::default();
        let reg = test_registry();
        let c = Candidate::default();
        let k8s = K8sEnrichment {
            resource_counts: HashMap::from([
                ("deploy".to_string(), 5),
                ("services".to_string(), 3),
            ]),
            ..Default::default()
        };
        let result = colorize("deploy", &c, &ls, "kubectl", &k8s, &reg);
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("Managed replicas"));
        assert!(stripped.contains("5"));
    }

    // ── parse_compcap: group index extraction ─────────────────────────

    #[test]
    fn parse_compcap_extracts_group_index() {
        let entry = b"item\x02<\x00>\x00group\x001\x00word\x00item";
        let req = parse_compcap(entry, "cmd", "", "cmd ");
        assert_eq!(req.candidates[0].group_index, 1);
    }

    #[test]
    fn parse_compcap_group_index_non_numeric_defaults_to_zero() {
        let entry = b"item\x02<\x00>\x00group\x00files\x00word\x00item";
        let req = parse_compcap(entry, "cmd", "", "cmd ");
        assert_eq!(req.candidates[0].group_index, 0);
    }

    // ── K8sEnrichment: namespace inactive with pods ───────────────────

    #[test]
    fn build_description_inactive_ns_with_pods() {
        let reg = test_registry();
        let k8s = K8sEnrichment {
            ns_pod_counts: HashMap::from([("kube-system".to_string(), 15)]),
            active_ns: "default".to_string(),
            ..Default::default()
        };
        let desc = build_description("kube-system", "kubectl", &k8s, &reg);
        assert!(desc.is_some());
        assert!(desc.as_ref().unwrap().contains("15 pods"));
        assert!(!desc.unwrap().contains("active"));
    }

    #[test]
    fn build_description_active_ns_without_pods() {
        let reg = test_registry();
        let k8s = K8sEnrichment {
            ns_pod_counts: HashMap::new(),
            active_ns: "myns".to_string(),
            ..Default::default()
        };
        let desc = build_description("myns", "kubectl", &k8s, &reg);
        assert!(desc.is_some());
        assert_eq!(desc.unwrap(), "active");
    }

    // ── completion_base_cmd: multi-word buffer ────────────────────────

    #[test]
    fn completion_base_cmd_strips_args() {
        assert_eq!(completion_base_cmd("", "git commit -m 'msg'"), "git");
    }

    #[test]
    fn completion_base_cmd_pipe_buffer() {
        assert_eq!(completion_base_cmd("", "kubectl get pods | grep"), "kubectl");
    }

    // ── Eval output format validation ─────────────────────────────────

    #[test]
    fn eval_response_execute_flag() {
        let sel = Selection {
            word: "commit".into(),
            prefix: "".into(),
            suffix: "".into(),
            iprefix: "".into(),
            isuffix: "".into(),
            args: "".into(),
            is_dir: false,
        };
        let dir_flag = if sel.is_dir { "d" } else { "" };
        let exec_flag = "x";
        let line = format!(
            "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
            sel.word, sel.prefix, sel.suffix,
            sel.iprefix, sel.isuffix, sel.args,
            dir_flag, exec_flag
        );
        let fields: Vec<&str> = line.split('\x1f').collect();
        assert_eq!(fields.len(), 8);
        assert_eq!(fields[7], "x");
    }

    #[test]
    fn eval_response_dir_and_execute_flags_combined() {
        let sel = Selection {
            word: "scripts/".into(),
            prefix: "".into(),
            suffix: "".into(),
            iprefix: "".into(),
            isuffix: "".into(),
            args: "".into(),
            is_dir: true,
        };
        let dir_flag = if sel.is_dir { "d" } else { "" };
        let exec_flag = "x";
        let line = format!(
            "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
            sel.word, sel.prefix, sel.suffix,
            sel.iprefix, sel.isuffix, sel.args,
            dir_flag, exec_flag
        );
        let fields: Vec<&str> = line.split('\x1f').collect();
        assert_eq!(fields[6], "d");
        assert_eq!(fields[7], "x");
    }
}
