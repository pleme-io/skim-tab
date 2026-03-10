//! skim-tab --complete — native zsh completion via skim.
//!
//! Two input modes:
//!   1. JSON on stdin (for testing / other consumers)
//!   2. `--compcap` mode: reads NUL/STX compcap format on stdin,
//!      with `--command`, `--query`, `--buffer` as CLI args (for the zsh widget)

use crate::{base_options, ANSI_DIM, ANSI_FROST, ANSI_RESET, ANSI_YELLOW, ICON_CD, ICON_K8S, ICON_POINTER};
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

/// Apply Nord-themed ANSI coloring to a completion candidate.
///
/// - File candidates: lscolors (directories blue, executables green, etc.)
/// - Non-file with ` -- ` description: word in accent, description in dim
/// - Flags (`--foo`): yellow
/// - Everything else: frost accent
///
/// The text structure is preserved so that `strip_ansi(colored) == display`.
fn colorize(display: &str, candidate: &Candidate, ls_colors: &LsColors, command: &str) -> String {
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

    // Enrich candidates that have no description with built-in ones
    let enriched;
    let text = if display.contains(" -- ") {
        display
    } else if let Some(desc) = lookup_description(display, command) {
        enriched = format!("{display} -- {desc}");
        &enriched
    } else {
        display
    };

    // Parse "word -- description" and apply colors
    if let Some((word, desc)) = text.split_once(" -- ") {
        let wc = if word.starts_with('-') { ANSI_YELLOW } else { ANSI_FROST };
        format!("{wc}{word}{ANSI_RESET} {ANSI_DIM}-- {desc}{ANSI_RESET}")
    } else if text.starts_with('-') {
        format!("{ANSI_YELLOW}{text}{ANSI_RESET}")
    } else {
        format!("{ANSI_FROST}{text}{ANSI_RESET}")
    }
}

// ── Description registry ─────────────────────────────────────────────
//
// Table-driven description enrichment. To add a new tool:
//   1. Add a ToolDescriptions entry to TOOL_REGISTRY
//   2. That's it — lookup, coloring, icon, and preview all pick it up

struct ToolDescriptions {
    /// Command names that trigger this table (e.g., &["kubectl", "k"])
    commands: &'static [&'static str],
    /// Icon to use for the skim prompt (None = default pointer)
    icon: Option<&'static str>,
    /// (word, description) pairs for enrichment
    entries: &'static [(&'static str, &'static str)],
}

impl ToolDescriptions {
    fn matches(&self, command: &str) -> bool {
        self.commands.contains(&command)
    }

    fn lookup(&self, word: &str) -> Option<&'static str> {
        self.entries
            .iter()
            .find(|(w, _)| *w == word)
            .map(|(_, d)| *d)
    }
}

static TOOL_REGISTRY: &[ToolDescriptions] = &[
    // ── kubectl ──────────────────────────────────────────────────
    ToolDescriptions {
        commands: &["kubectl", "kubecolor", "k"],
        icon: Some(ICON_K8S),
        entries: &[
            // Subcommands
            ("get", "Display resources"),
            ("describe", "Show resource details"),
            ("apply", "Apply configuration"),
            ("delete", "Delete resources"),
            ("edit", "Edit a resource"),
            ("create", "Create from file or stdin"),
            ("expose", "Expose as a service"),
            ("run", "Run a pod"),
            ("set", "Set resource fields"),
            ("explain", "Documentation of resources"),
            ("rollout", "Manage rollouts"),
            ("scale", "Scale a resource"),
            ("autoscale", "Auto-scale a resource"),
            ("exec", "Execute in a container"),
            ("logs", "Print container logs"),
            ("attach", "Attach to a container"),
            ("port-forward", "Forward ports to a pod"),
            ("cp", "Copy files to/from containers"),
            ("top", "Resource usage (CPU/memory)"),
            ("debug", "Debug workloads"),
            ("cordon", "Mark node unschedulable"),
            ("uncordon", "Mark node schedulable"),
            ("drain", "Drain a node"),
            ("taint", "Set node taints"),
            ("label", "Update labels"),
            ("annotate", "Update annotations"),
            ("patch", "Patch a resource"),
            ("replace", "Replace a resource"),
            ("wait", "Wait for a condition"),
            ("config", "Modify kubeconfig"),
            ("cluster-info", "Cluster endpoint info"),
            ("api-resources", "List API resource types"),
            ("api-versions", "List API versions"),
            ("version", "Client and server version"),
            ("diff", "Diff live vs applied"),
            ("kustomize", "Build kustomization target"),
            ("auth", "Inspect authorization"),
            ("certificate", "Certificate operations"),
            ("proxy", "API server proxy"),
            ("plugin", "Plugin utilities"),
            ("completion", "Shell completion"),
            // Resource types (including abbreviations)
            ("pods", "Pod workloads"),
            ("pod", "Pod workloads"),
            ("po", "Pod workloads"),
            ("deployments", "Managed replicas"),
            ("deployment", "Managed replicas"),
            ("deploy", "Managed replicas"),
            ("services", "Network endpoints"),
            ("service", "Network endpoints"),
            ("svc", "Network endpoints"),
            ("nodes", "Cluster machines"),
            ("node", "Cluster machines"),
            ("no", "Cluster machines"),
            ("namespaces", "Resource scopes"),
            ("namespace", "Resource scopes"),
            ("ns", "Resource scopes"),
            ("configmaps", "Configuration data"),
            ("configmap", "Configuration data"),
            ("cm", "Configuration data"),
            ("secrets", "Sensitive data"),
            ("secret", "Sensitive data"),
            ("ingresses", "External access rules"),
            ("ingress", "External access rules"),
            ("ing", "External access rules"),
            ("persistentvolumeclaims", "Storage claims"),
            ("pvc", "Storage claims"),
            ("persistentvolumes", "Storage volumes"),
            ("pv", "Storage volumes"),
            ("statefulsets", "Stateful workloads"),
            ("statefulset", "Stateful workloads"),
            ("sts", "Stateful workloads"),
            ("daemonsets", "Per-node workloads"),
            ("daemonset", "Per-node workloads"),
            ("ds", "Per-node workloads"),
            ("jobs", "Run-to-completion tasks"),
            ("job", "Run-to-completion tasks"),
            ("cronjobs", "Scheduled jobs"),
            ("cronjob", "Scheduled jobs"),
            ("cj", "Scheduled jobs"),
            ("replicasets", "Pod replica sets"),
            ("replicaset", "Pod replica sets"),
            ("rs", "Pod replica sets"),
            ("serviceaccounts", "Identities for pods"),
            ("serviceaccount", "Identities for pods"),
            ("sa", "Identities for pods"),
            ("roles", "Namespaced permissions"),
            ("role", "Namespaced permissions"),
            ("clusterroles", "Cluster-wide permissions"),
            ("clusterrole", "Cluster-wide permissions"),
            ("rolebindings", "Bind role to subject"),
            ("rolebinding", "Bind role to subject"),
            ("clusterrolebindings", "Cluster role binding"),
            ("clusterrolebinding", "Cluster role binding"),
            ("networkpolicies", "Network access rules"),
            ("networkpolicy", "Network access rules"),
            ("netpol", "Network access rules"),
            ("storageclasses", "Storage provisioners"),
            ("storageclass", "Storage provisioners"),
            ("sc", "Storage provisioners"),
            ("events", "Cluster events"),
            ("event", "Cluster events"),
            ("ev", "Cluster events"),
            ("endpoints", "Service endpoints"),
            ("ep", "Service endpoints"),
            ("horizontalpodautoscalers", "Auto-scaling rules"),
            ("hpa", "Auto-scaling rules"),
            ("poddisruptionbudgets", "Disruption limits"),
            ("pdb", "Disruption limits"),
            ("limitranges", "Resource constraints"),
            ("limitrange", "Resource constraints"),
            ("limits", "Resource constraints"),
            ("resourcequotas", "Namespace quotas"),
            ("resourcequota", "Namespace quotas"),
            ("quota", "Namespace quotas"),
            ("customresourcedefinitions", "Custom API types"),
            ("crd", "Custom API types"),
            ("crds", "Custom API types"),
        ],
    },
    // ── helm ─────────────────────────────────────────────────────
    ToolDescriptions {
        commands: &["helm"],
        icon: Some(ICON_K8S),
        entries: &[
            ("install", "Install a chart"),
            ("upgrade", "Upgrade a release"),
            ("uninstall", "Uninstall a release"),
            ("list", "List releases"),
            ("ls", "List releases"),
            ("status", "Release status"),
            ("history", "Release history"),
            ("rollback", "Rollback to a revision"),
            ("template", "Render templates locally"),
            ("show", "Show chart information"),
            ("get", "Get release details"),
            ("repo", "Manage chart repos"),
            ("search", "Search for charts"),
            ("pull", "Download a chart"),
            ("push", "Push to a registry"),
            ("package", "Package a chart"),
            ("create", "Create a new chart"),
            ("lint", "Lint a chart"),
            ("test", "Test a release"),
            ("dependency", "Manage dependencies"),
            ("dep", "Manage dependencies"),
            ("env", "Helm environment info"),
            ("plugin", "Manage plugins"),
            ("registry", "Registry operations"),
            ("verify", "Verify a signed chart"),
            ("version", "Client version"),
            ("completion", "Shell completion"),
            // show subcommands
            ("chart", "Chart metadata"),
            ("values", "Chart default values"),
            ("readme", "Chart README"),
            ("crds", "Chart CRDs"),
            ("all", "All chart info"),
        ],
    },
    // ── flux ─────────────────────────────────────────────────────
    ToolDescriptions {
        commands: &["flux"],
        icon: Some(ICON_K8S),
        entries: &[
            ("get", "Display Flux resources"),
            ("reconcile", "Trigger reconciliation"),
            ("suspend", "Suspend reconciliation"),
            ("resume", "Resume reconciliation"),
            ("create", "Create Flux resources"),
            ("delete", "Delete Flux resources"),
            ("export", "Export resources as YAML"),
            ("install", "Install Flux components"),
            ("uninstall", "Uninstall Flux"),
            ("bootstrap", "Bootstrap Flux on a cluster"),
            ("check", "Pre-flight checks"),
            ("logs", "Flux controller logs"),
            ("events", "Flux events"),
            ("tree", "Resource dependency tree"),
            ("trace", "Trace a Flux resource"),
            ("stats", "Reconciliation statistics"),
            ("diff", "Diff live vs desired"),
            ("build", "Build kustomization locally"),
            ("push", "Push artifact to OCI"),
            ("pull", "Pull artifact from OCI"),
            ("tag", "Tag an OCI artifact"),
            ("version", "Flux CLI version"),
            ("completion", "Shell completion"),
            // Resource types
            ("kustomizations", "Kustomize reconciler"),
            ("kustomization", "Kustomize reconciler"),
            ("ks", "Kustomize reconciler"),
            ("helmreleases", "Helm release reconciler"),
            ("helmrelease", "Helm release reconciler"),
            ("hr", "Helm release reconciler"),
            ("gitrepositories", "Git source"),
            ("gitrepository", "Git source"),
            ("helmrepositories", "Helm chart source"),
            ("helmrepository", "Helm chart source"),
            ("helmcharts", "Helm chart artifact"),
            ("helmchart", "Helm chart artifact"),
            ("ocirepositories", "OCI artifact source"),
            ("ocirepository", "OCI artifact source"),
            ("buckets", "S3-compatible source"),
            ("bucket", "S3-compatible source"),
            ("receivers", "Webhook receiver"),
            ("receiver", "Webhook receiver"),
            ("alerts", "Alert rule"),
            ("alert", "Alert rule"),
            ("providers", "Notification provider"),
            ("provider", "Notification provider"),
            ("imagepolicies", "Image update policy"),
            ("imagepolicy", "Image update policy"),
            ("imagerepositories", "Image scan config"),
            ("imagerepository", "Image scan config"),
            ("imageupdateautomations", "Image auto-update"),
            ("imageupdateautomation", "Image auto-update"),
        ],
    },
    // ── Add new tools here ──────────────────────────────────────
    // ToolDescriptions {
    //     commands: &["docker", "podman"],
    //     icon: None,  // uses default pointer
    //     entries: &[
    //         ("run", "Run a container"),
    //         ("build", "Build an image"),
    //         ...
    //     ],
    // },
];

/// Look up a built-in description for a candidate word.
fn lookup_description(word: &str, command: &str) -> Option<&'static str> {
    let base = command.split(':').next().unwrap_or(command);
    TOOL_REGISTRY
        .iter()
        .find(|t| t.matches(base))
        .and_then(|t| t.lookup(word))
}

/// Get the prompt icon for a command, or None for the default.
fn tool_icon(command: &str) -> Option<&'static str> {
    TOOL_REGISTRY
        .iter()
        .find(|t| t.matches(command))
        .and_then(|t| t.icon)
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

// ── Context helpers ──────────────────────────────────────────────────

/// Extract the base command for color/description lookups.
/// Prefers the buffer's first word (the actual command typed) over
/// the zsh curcontext command name.
fn completion_base_cmd(command: &str, buffer: &str) -> String {
    buffer
        .split_whitespace()
        .next()
        .unwrap_or(command)
        .to_string()
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
    let cmd_for_color = completion_base_cmd(&req.command, &req.buffer);
    let display_lines: Vec<String> = req
        .candidates
        .iter()
        .map(|c| colorize(c.display_text(), c, &ls_colors, &cmd_for_color))
        .collect();

    let base_cmd = completion_base_cmd(&req.command, &req.buffer);
    let prompt = match req.command.as_str() {
        "cd" | "pushd" | "z" => ICON_CD,
        _ => tool_icon(&base_cmd).unwrap_or(ICON_POINTER),
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
            // Match against original display, or the word part before " -- "
            // (enriched descriptions add " -- desc" that isn't in display_text)
            let match_text = plain.split(" -- ").next().unwrap_or(&plain);
            req.candidates
                .iter()
                .find(|c| c.display_text() == plain || c.display_text() == match_text)
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

    #[test]
    fn colorize_file_candidate() {
        let ls = LsColors::from_env().unwrap_or_default();
        let c = Candidate {
            word: "src".into(),
            is_file: true,
            realdir: "/tmp/".into(),
            ..Default::default()
        };
        // Should not panic and should return something non-empty
        let result = colorize("src", &c, &ls, "ls");
        assert!(!result.is_empty());
    }

    #[test]
    fn colorize_non_file_with_description() {
        let ls = LsColors::default();
        let c = Candidate::default();
        let result = colorize("get -- Display resources", &c, &ls, "kubectl");
        // Should contain ANSI codes
        assert!(result.contains('\x1b'));
        // Stripped should match original
        assert_eq!(crate::strip_ansi(&result), "get -- Display resources");
    }

    #[test]
    fn colorize_flag() {
        let ls = LsColors::default();
        let c = Candidate::default();
        let result = colorize("--namespace", &c, &ls, "kubectl");
        assert!(result.contains('\x1b'));
        assert_eq!(crate::strip_ansi(&result), "--namespace");
    }

    #[test]
    fn colorize_flag_with_description() {
        let ls = LsColors::default();
        let c = Candidate::default();
        let result = colorize("--output -- Output format", &c, &ls, "kubectl");
        assert!(result.contains(ANSI_YELLOW));
        assert_eq!(crate::strip_ansi(&result), "--output -- Output format");
    }

    #[test]
    fn colorize_enriches_kubectl_subcommand() {
        let ls = LsColors::default();
        let c = Candidate { word: "get".into(), display: "get".into(), ..Default::default() };
        let result = colorize("get", &c, &ls, "kubectl");
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("get"));
        assert!(stripped.contains(" -- "));
        assert!(stripped.contains("Display resources"));
    }

    #[test]
    fn colorize_enriches_helm_subcommand() {
        let ls = LsColors::default();
        let c = Candidate::default();
        let result = colorize("install", &c, &ls, "helm");
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("install"));
        assert!(stripped.contains(" -- "));
        assert!(stripped.contains("Install a chart"));
    }

    #[test]
    fn colorize_enriches_flux_resource_type() {
        let ls = LsColors::default();
        let c = Candidate::default();
        let result = colorize("kustomizations", &c, &ls, "flux");
        let stripped = crate::strip_ansi(&result);
        assert!(stripped.contains("Kustomize reconciler"));
    }

    #[test]
    fn colorize_no_enrichment_for_unknown() {
        let ls = LsColors::default();
        let c = Candidate::default();
        let result = colorize("my-random-pod", &c, &ls, "kubectl");
        let stripped = crate::strip_ansi(&result);
        assert_eq!(stripped, "my-random-pod");
    }

    #[test]
    fn lookup_description_kubectl() {
        assert_eq!(lookup_description("pods", "kubectl"), Some("Pod workloads"));
        assert_eq!(lookup_description("deploy", "k"), Some("Managed replicas"));
        assert_eq!(lookup_description("unknown-thing", "kubectl"), None);
    }

    #[test]
    fn lookup_description_helm() {
        assert_eq!(lookup_description("upgrade", "helm"), Some("Upgrade a release"));
        assert_eq!(lookup_description("nope", "helm"), None);
    }

    #[test]
    fn lookup_description_flux() {
        assert_eq!(lookup_description("reconcile", "flux"), Some("Trigger reconciliation"));
        assert_eq!(lookup_description("hr", "flux"), Some("Helm release reconciler"));
        assert_eq!(lookup_description("nope", "flux"), None);
    }

    #[test]
    fn tool_icon_registry() {
        assert_eq!(tool_icon("kubectl"), Some(ICON_K8S));
        assert_eq!(tool_icon("k"), Some(ICON_K8S));
        assert_eq!(tool_icon("helm"), Some(ICON_K8S));
        assert_eq!(tool_icon("flux"), Some(ICON_K8S));
        assert_eq!(tool_icon("cd"), None);
        assert_eq!(tool_icon("ls"), None);
    }

    #[test]
    fn completion_base_cmd_prefers_buffer() {
        assert_eq!(completion_base_cmd("", "kubectl get pods"), "kubectl");
        assert_eq!(completion_base_cmd("helm", ""), "helm");
        assert_eq!(completion_base_cmd("cd", "cd /tmp"), "cd");
    }
}
