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

// ── Description enrichment ──────────────────────────────────────────

/// Look up a built-in description for a candidate word.
///
/// Returns `None` if no enrichment is available — the candidate displays as-is.
/// This is the central registry: add new tool descriptions here.
fn lookup_description(word: &str, command: &str) -> Option<&'static str> {
    let base = command.split(':').next().unwrap_or(command);
    match base {
        "kubectl" | "kubecolor" | "k" => kubectl_desc(word),
        "helm" => helm_desc(word),
        "flux" => flux_desc(word),
        _ => None,
    }
}

fn kubectl_desc(word: &str) -> Option<&'static str> {
    // Subcommands
    match word {
        "get" => Some("Display resources"),
        "describe" => Some("Show resource details"),
        "apply" => Some("Apply configuration"),
        "delete" => Some("Delete resources"),
        "edit" => Some("Edit a resource"),
        "create" => Some("Create from file or stdin"),
        "expose" => Some("Expose as a service"),
        "run" => Some("Run a pod"),
        "set" => Some("Set resource fields"),
        "explain" => Some("Documentation of resources"),
        "rollout" => Some("Manage rollouts"),
        "scale" => Some("Scale a resource"),
        "autoscale" => Some("Auto-scale a resource"),
        "exec" => Some("Execute in a container"),
        "logs" => Some("Print container logs"),
        "attach" => Some("Attach to a container"),
        "port-forward" => Some("Forward ports to a pod"),
        "cp" => Some("Copy files to/from containers"),
        "top" => Some("Resource usage (CPU/memory)"),
        "debug" => Some("Debug workloads"),
        "cordon" => Some("Mark node unschedulable"),
        "uncordon" => Some("Mark node schedulable"),
        "drain" => Some("Drain a node"),
        "taint" => Some("Set node taints"),
        "label" => Some("Update labels"),
        "annotate" => Some("Update annotations"),
        "patch" => Some("Patch a resource"),
        "replace" => Some("Replace a resource"),
        "wait" => Some("Wait for a condition"),
        "config" => Some("Modify kubeconfig"),
        "cluster-info" => Some("Cluster endpoint info"),
        "api-resources" => Some("List API resource types"),
        "api-versions" => Some("List API versions"),
        "version" => Some("Client and server version"),
        "diff" => Some("Diff live vs applied"),
        "kustomize" => Some("Build kustomization target"),
        "auth" => Some("Inspect authorization"),
        "certificate" => Some("Certificate operations"),
        "proxy" => Some("API server proxy"),
        "plugin" => Some("Plugin utilities"),
        "completion" => Some("Shell completion"),
        // Resource types
        "pods" | "pod" | "po" => Some("Pod workloads"),
        "deployments" | "deployment" | "deploy" => Some("Managed replicas"),
        "services" | "service" | "svc" => Some("Network endpoints"),
        "nodes" | "node" | "no" => Some("Cluster machines"),
        "namespaces" | "namespace" | "ns" => Some("Resource scopes"),
        "configmaps" | "configmap" | "cm" => Some("Configuration data"),
        "secrets" | "secret" => Some("Sensitive data"),
        "ingresses" | "ingress" | "ing" => Some("External access rules"),
        "persistentvolumeclaims" | "pvc" => Some("Storage claims"),
        "persistentvolumes" | "pv" => Some("Storage volumes"),
        "statefulsets" | "statefulset" | "sts" => Some("Stateful workloads"),
        "daemonsets" | "daemonset" | "ds" => Some("Per-node workloads"),
        "jobs" | "job" => Some("Run-to-completion tasks"),
        "cronjobs" | "cronjob" | "cj" => Some("Scheduled jobs"),
        "replicasets" | "replicaset" | "rs" => Some("Pod replica sets"),
        "serviceaccounts" | "serviceaccount" | "sa" => Some("Identities for pods"),
        "roles" | "role" => Some("Namespaced permissions"),
        "clusterroles" | "clusterrole" => Some("Cluster-wide permissions"),
        "rolebindings" | "rolebinding" => Some("Bind role to subject"),
        "clusterrolebindings" | "clusterrolebinding" => Some("Cluster role binding"),
        "networkpolicies" | "networkpolicy" | "netpol" => Some("Network access rules"),
        "storageclasses" | "storageclass" | "sc" => Some("Storage provisioners"),
        "events" | "event" | "ev" => Some("Cluster events"),
        "endpoints" | "ep" => Some("Service endpoints"),
        "horizontalpodautoscalers" | "hpa" => Some("Auto-scaling rules"),
        "poddisruptionbudgets" | "pdb" => Some("Disruption limits"),
        "limitranges" | "limitrange" | "limits" => Some("Resource constraints"),
        "resourcequotas" | "resourcequota" | "quota" => Some("Namespace quotas"),
        "customresourcedefinitions" | "crd" | "crds" => Some("Custom API types"),
        _ => None,
    }
}

fn helm_desc(word: &str) -> Option<&'static str> {
    match word {
        "install" => Some("Install a chart"),
        "upgrade" => Some("Upgrade a release"),
        "uninstall" => Some("Uninstall a release"),
        "list" | "ls" => Some("List releases"),
        "status" => Some("Release status"),
        "history" => Some("Release history"),
        "rollback" => Some("Rollback to a revision"),
        "template" => Some("Render templates locally"),
        "show" => Some("Show chart information"),
        "get" => Some("Get release details"),
        "repo" => Some("Manage chart repos"),
        "search" => Some("Search for charts"),
        "pull" => Some("Download a chart"),
        "push" => Some("Push to a registry"),
        "package" => Some("Package a chart"),
        "create" => Some("Create a new chart"),
        "lint" => Some("Lint a chart"),
        "test" => Some("Test a release"),
        "dependency" | "dep" => Some("Manage dependencies"),
        "env" => Some("Helm environment info"),
        "plugin" => Some("Manage plugins"),
        "registry" => Some("Registry operations"),
        "verify" => Some("Verify a signed chart"),
        "version" => Some("Client version"),
        "completion" => Some("Shell completion"),
        // show subcommands
        "chart" => Some("Chart metadata"),
        "values" => Some("Chart default values"),
        "readme" => Some("Chart README"),
        "crds" => Some("Chart CRDs"),
        "all" => Some("All chart info"),
        _ => None,
    }
}

fn flux_desc(word: &str) -> Option<&'static str> {
    match word {
        "get" => Some("Display Flux resources"),
        "reconcile" => Some("Trigger reconciliation"),
        "suspend" => Some("Suspend reconciliation"),
        "resume" => Some("Resume reconciliation"),
        "create" => Some("Create Flux resources"),
        "delete" => Some("Delete Flux resources"),
        "export" => Some("Export resources as YAML"),
        "install" => Some("Install Flux components"),
        "uninstall" => Some("Uninstall Flux"),
        "bootstrap" => Some("Bootstrap Flux on a cluster"),
        "check" => Some("Pre-flight checks"),
        "logs" => Some("Flux controller logs"),
        "events" => Some("Flux events"),
        "tree" => Some("Resource dependency tree"),
        "trace" => Some("Trace a Flux resource"),
        "stats" => Some("Reconciliation statistics"),
        "diff" => Some("Diff live vs desired"),
        "build" => Some("Build kustomization locally"),
        "push" => Some("Push artifact to OCI"),
        "pull" => Some("Pull artifact from OCI"),
        "tag" => Some("Tag an OCI artifact"),
        "version" => Some("Flux CLI version"),
        "completion" => Some("Shell completion"),
        // Resource types
        "kustomizations" | "kustomization" | "ks" => Some("Kustomize reconciler"),
        "helmreleases" | "helmrelease" | "hr" => Some("Helm release reconciler"),
        "gitrepositories" | "gitrepository" => Some("Git source"),
        "helmrepositories" | "helmrepository" => Some("Helm chart source"),
        "helmcharts" | "helmchart" => Some("Helm chart artifact"),
        "ocirepositories" | "ocirepository" => Some("OCI artifact source"),
        "buckets" | "bucket" => Some("S3-compatible source"),
        "receivers" | "receiver" => Some("Webhook receiver"),
        "alerts" | "alert" => Some("Alert rule"),
        "providers" | "provider" => Some("Notification provider"),
        "imagepolicies" | "imagepolicy" => Some("Image update policy"),
        "imagerepositories" | "imagerepository" => Some("Image scan config"),
        "imageupdateautomations" | "imageupdateautomation" => Some("Image auto-update"),
        _ => None,
    }
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

/// Check if the completion context is kubectl/helm/flux.
fn is_k8s_context(command: &str, buffer: &str) -> bool {
    let base = buffer.split_whitespace().next().unwrap_or(command);
    matches!(base, "kubectl" | "kubecolor" | "k" | "helm" | "flux")
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

    let prompt = match req.command.as_str() {
        "cd" | "pushd" | "z" => ICON_CD,
        _ if is_k8s_context(&req.command, &req.buffer) => ICON_K8S,
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
        assert_eq!(kubectl_desc("pods"), Some("Pod workloads"));
        assert_eq!(kubectl_desc("deploy"), Some("Managed replicas"));
        assert_eq!(kubectl_desc("unknown-thing"), None);
    }

    #[test]
    fn lookup_description_helm() {
        assert_eq!(helm_desc("upgrade"), Some("Upgrade a release"));
        assert_eq!(helm_desc("nope"), None);
    }

    #[test]
    fn lookup_description_flux() {
        assert_eq!(flux_desc("reconcile"), Some("Trigger reconciliation"));
        assert_eq!(flux_desc("hr"), Some("Helm release reconciler"));
        assert_eq!(flux_desc("nope"), None);
    }

    #[test]
    fn is_k8s_context_detects() {
        assert!(is_k8s_context("kubectl", "kubectl get pods"));
        assert!(is_k8s_context("", "helm install foo"));
        assert!(is_k8s_context("", "flux get ks"));
        assert!(is_k8s_context("k", "k get pods"));
        assert!(!is_k8s_context("cd", "cd /tmp"));
        assert!(!is_k8s_context("ls", "ls -la"));
    }

    #[test]
    fn completion_base_cmd_prefers_buffer() {
        assert_eq!(completion_base_cmd("", "kubectl get pods"), "kubectl");
        assert_eq!(completion_base_cmd("helm", ""), "helm");
        assert_eq!(completion_base_cmd("cd", "cd /tmp"), "cd");
    }
}
