//! Preview generation for completion candidates.
//!
//! Self-contained module: takes strings, returns strings. No dependency
//! on skim types or the completion protocol.

use std::path::Path;
use std::process::Command;

// ── Buffer context ────────────────────────────────────────────────────

/// Flags that consume the next word as a value (shared by kubectl/flux/helm).
const FLAGS_WITH_VALUE: &[&str] = &[
    "-n", "--namespace", "-o", "--output", "-l", "--selector",
    "-f", "--filename", "--context", "--kubeconfig", "-c", "--container",
    "--type", "--sort-by", "--field-selector", "--server", "--token",
    "--certificate-authority", "--cluster", "--user",
];

/// Parsed command line buffer — extracts base command, positional
/// subcommands, and flag values for context-aware preview dispatch.
struct BufferContext<'a> {
    base_cmd: &'a str,
    subcmds: Vec<&'a str>,
    namespace: Option<&'a str>,
}

impl<'a> BufferContext<'a> {
    fn parse(buffer: &'a str, fallback_cmd: &'a str) -> Self {
        let words: Vec<&str> = buffer.split_whitespace().collect();
        let base_cmd = words.first().copied().unwrap_or(fallback_cmd);

        let mut subcmds = Vec::new();
        let mut skip_next = false;
        for &w in words.iter().skip(1) {
            if skip_next {
                skip_next = false;
                continue;
            }
            if w.starts_with('-') {
                if !w.contains('=') && FLAGS_WITH_VALUE.contains(&w) {
                    skip_next = true;
                }
                continue;
            }
            subcmds.push(w);
        }

        let namespace = find_flag(&words, &["-n", "--namespace"]);
        Self { base_cmd, subcmds, namespace }
    }

    fn sub(&self, idx: usize) -> &str {
        self.subcmds.get(idx).copied().unwrap_or("")
    }

    fn ns_args(&self) -> Vec<&str> {
        self.namespace.map_or_else(Vec::new, |ns| vec!["-n", ns])
    }
}

/// Extract a flag's value from words (e.g., `-n myns` or `--namespace=myns`).
/// Zero-allocation: byte-level prefix check instead of `format!`.
fn find_flag<'a>(words: &[&'a str], flags: &[&str]) -> Option<&'a str> {
    for (i, &w) in words.iter().enumerate() {
        if flags.contains(&w) {
            return words.get(i + 1).copied();
        }
        for flag in flags {
            if w.starts_with(flag) && w.as_bytes().get(flag.len()) == Some(&b'=') {
                return Some(&w[flag.len() + 1..]);
            }
        }
    }
    None
}

// ── Public interface ──────────────────────────────────────────────────

/// Generate a preview string for a completion candidate.
pub fn preview(word: &str, command: &str, buffer: &str, realdir: &str) -> String {
    let path = if realdir.is_empty() {
        word.to_string()
    } else {
        format!("{realdir}{word}")
    };

    let ctx = BufferContext::parse(buffer, command);

    match ctx.base_cmd {
        "kubectl" | "kubecolor" | "k" => return preview_kubectl(&ctx, word),
        "flux" => return preview_flux(&ctx, word),
        "helm" => return preview_helm(&ctx, word),
        _ => {}
    }

    match command {
        "cd" | "pushd" | "z" => preview_dir(&path),
        "kill" | "ps" => preview_proc(word),
        cmd if cmd.starts_with("git-add")
            || cmd.starts_with("git-diff")
            || cmd.starts_with("git-restore") => preview_git("diff", word),
        cmd if cmd.starts_with("git-log") => preview_git("log", word),
        cmd if cmd.starts_with("git-checkout") => preview_git("checkout", word),
        "" => try_path_then_command(&path, word),
        _ => preview_default(&path, word, command),
    }
}

// ── kubectl ──────────────────────────────────────────────────────────

fn preview_kubectl(ctx: &BufferContext, candidate: &str) -> String {
    let ns = ctx.ns_args();
    let (sub0, sub1) = (ctx.sub(0), ctx.sub(1));

    match sub0 {
        "get" | "describe" | "edit" | "delete" if !sub1.is_empty() => {
            truncated("kubectl", &with_ns(&["describe", sub1, candidate], &ns), 60)
        }
        "get" | "describe" | "edit" | "delete" => {
            let out = run("kubectl", &with_ns(&["get", candidate, "--no-headers"], &ns));
            let count = out.lines().count();
            let sample: String = out.lines().take(25).collect::<Vec<_>>().join("\n");
            format!("  {} resources: {count}\n\n{sample}", candidate.to_uppercase())
        }
        "logs" => {
            run("kubectl", &with_ns(&["logs", "--tail=30", "--timestamps", candidate], &ns))
        }
        "exec" | "attach" | "port-forward" | "cp" => {
            let wide = run("kubectl", &with_ns(&["get", "pod", candidate, "-o", "wide"], &ns));
            let desc = truncated("kubectl", &with_ns(&["describe", "pod", candidate], &ns), 40);
            format!("{wide}\n{desc}")
        }
        "top" if sub1 == "pod" || sub1.is_empty() => {
            run("kubectl", &with_ns(&["top", "pod", candidate], &ns))
        }
        "rollout" => {
            run("kubectl", &with_ns(&["rollout", "status", sub1, candidate], &ns))
        }
        "scale" if !sub1.is_empty() => {
            run("kubectl", &with_ns(&["get", sub1, candidate, "-o", "wide"], &ns))
        }
        "apply" | "create" => try_path_then_command(candidate, candidate),
        "" if candidate.starts_with('-') => preview_command("kubectl"),
        "" => {
            let tldr = preview_command(&format!("kubectl-{candidate}"));
            if tldr.is_empty() { preview_command("kubectl") } else { tldr }
        }
        _ => preview_command("kubectl"),
    }
}

// ── flux ──────────────────────────────────────────────────────────────

fn preview_flux(ctx: &BufferContext, candidate: &str) -> String {
    let ns = ctx.ns_args();
    let (sub0, sub1) = (ctx.sub(0), ctx.sub(1));

    match sub0 {
        "get" if !sub1.is_empty() => {
            run("flux", &with_ns(&["get", sub1, candidate], &ns))
        }
        "get" | "reconcile" if sub1.is_empty() => {
            run("flux", &with_ns(&["get", candidate], &ns))
        }
        "reconcile" => {
            let status = run("flux", &with_ns(&["get", sub1, candidate], &ns));
            format!("Current status (before reconcile):\n\n{status}")
        }
        "suspend" | "resume" if !sub1.is_empty() => {
            run("flux", &with_ns(&["get", sub1, candidate], &ns))
        }
        "logs" => run("flux", &["logs", "--tail=30"]),
        "events" => run("flux", &["events", "--for", candidate]),
        _ => preview_command("flux"),
    }
}

// ── helm ──────────────────────────────────────────────────────────────

fn preview_helm(ctx: &BufferContext, candidate: &str) -> String {
    let ns = ctx.ns_args();
    let (sub0, sub1) = (ctx.sub(0), ctx.sub(1));

    match sub0 {
        "status" | "uninstall" | "rollback" | "history" => {
            run("helm", &with_ns(&[sub0, candidate], &ns))
        }
        "upgrade" | "list" => {
            run("helm", &with_ns(&["status", candidate], &ns))
        }
        "install" | "template" if !sub1.is_empty() => {
            truncated("helm", &["show", "chart", candidate], 50)
        }
        "show" if !sub1.is_empty() => {
            truncated("helm", &["show", sub1, candidate], 60)
        }
        "repo" => run("helm", &["repo", "list"]),
        _ => preview_command("helm"),
    }
}

// ── Generic previewers ───────────────────────────────────────────────

fn try_path_then_command(path: &str, word: &str) -> String {
    if Path::new(path).is_dir() {
        preview_dir(path)
    } else if Path::new(path).is_file() {
        preview_file(path)
    } else {
        preview_command(word)
    }
}

fn preview_default(path: &str, word: &str, command: &str) -> String {
    if Path::new(path).is_dir() {
        return preview_dir(path);
    }
    if Path::new(path).is_file() {
        return preview_file(path);
    }
    if word.starts_with('-') {
        return preview_command(command);
    }
    let result = preview_command(word);
    if result.is_empty() { preview_command(command) } else { result }
}

fn preview_dir(path: &str) -> String {
    run("eza", &["--tree", "--level=2", "--icons", "--color=always", path])
}

fn preview_file(path: &str) -> String {
    run("bat", &["--color=always", "--style=numbers", "--line-range=:200", path])
}

fn preview_proc(word: &str) -> String {
    run("ps", &["-p", word, "-o", "pid,ppid,%cpu,%mem,start,command"])
}

fn preview_git(subcmd: &str, word: &str) -> String {
    match subcmd {
        "diff" => run("git", &["diff", "--color=always", "--", word]),
        "log" => run("git", &["log", "--oneline", "--graph", "--color=always", "-20", word]),
        "checkout" => run("git", &["log", "--oneline", "--graph", "--color=always", "-10", word]),
        _ => String::new(),
    }
}

/// Preview a command using tldr with fallback to --help.
fn preview_command(cmd: &str) -> String {
    if let Ok(out) = Command::new("tldr").args(["--color=always", cmd]).output() {
        if out.status.success() && !out.stdout.is_empty() {
            return String::from_utf8_lossy(&out.stdout).into_owned();
        }
    }
    if let Ok(out) = Command::new(cmd).arg("--help").output() {
        let text = if out.stdout.is_empty() {
            String::from_utf8_lossy(&out.stderr)
        } else {
            String::from_utf8_lossy(&out.stdout)
        };
        if !text.is_empty() {
            return text.lines().take(80).collect::<Vec<_>>().join("\n");
        }
    }
    String::new()
}

// ── Shell helpers ────────────────────────────────────────────────────

fn run(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| {
            let out = String::from_utf8_lossy(&o.stdout);
            if out.is_empty() {
                String::from_utf8_lossy(&o.stderr).into_owned()
            } else {
                out.into_owned()
            }
        })
        .unwrap_or_default()
}

fn truncated(cmd: &str, args: &[&str], max_lines: usize) -> String {
    let output = run(cmd, args);
    output.lines().take(max_lines).collect::<Vec<_>>().join("\n")
}

fn with_ns<'a>(args: &[&'a str], ns: &[&'a str]) -> Vec<&'a str> {
    let mut v: Vec<&str> = args.to_vec();
    v.extend_from_slice(ns);
    v
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_context_simple() {
        let ctx = BufferContext::parse("kubectl get pods my-pod", "");
        assert_eq!(ctx.base_cmd, "kubectl");
        assert_eq!(ctx.subcmds, vec!["get", "pods", "my-pod"]);
        assert_eq!(ctx.namespace, None);
    }

    #[test]
    fn buffer_context_with_namespace() {
        let ctx = BufferContext::parse("kubectl -n kube-system get pods", "");
        assert_eq!(ctx.base_cmd, "kubectl");
        assert_eq!(ctx.subcmds, vec!["get", "pods"]);
        assert_eq!(ctx.namespace, Some("kube-system"));
        assert_eq!(ctx.ns_args(), vec!["-n", "kube-system"]);
    }

    #[test]
    fn buffer_context_with_equals_flag() {
        let ctx = BufferContext::parse("kubectl get pods --namespace=default -o=json", "");
        assert_eq!(ctx.subcmds, vec!["get", "pods"]);
        assert_eq!(ctx.namespace, Some("default"));
    }

    #[test]
    fn buffer_context_empty_uses_fallback() {
        let ctx = BufferContext::parse("", "cd");
        assert_eq!(ctx.base_cmd, "cd");
        assert!(ctx.subcmds.is_empty());
    }

    #[test]
    fn buffer_context_sub_out_of_range() {
        let ctx = BufferContext::parse("kubectl get", "");
        assert_eq!(ctx.sub(0), "get");
        assert_eq!(ctx.sub(1), "");
        assert_eq!(ctx.sub(99), "");
    }

    #[test]
    fn buffer_context_ns_args_empty_when_none() {
        let ctx = BufferContext::parse("kubectl get pods", "");
        assert!(ctx.ns_args().is_empty());
    }

    #[test]
    fn find_flag_short_form() {
        let words = vec!["kubectl", "-n", "myns", "get", "pods"];
        assert_eq!(find_flag(&words, &["-n", "--namespace"]), Some("myns"));
    }

    #[test]
    fn find_flag_long_equals() {
        let words = vec!["kubectl", "--namespace=myns", "get"];
        assert_eq!(find_flag(&words, &["-n", "--namespace"]), Some("myns"));
    }

    #[test]
    fn find_flag_missing() {
        let words = vec!["kubectl", "get", "pods"];
        assert_eq!(find_flag(&words, &["-n", "--namespace"]), None);
    }

    #[test]
    fn find_flag_no_false_prefix() {
        let words = vec!["kubectl", "--namespace-override=foo", "get"];
        assert_eq!(find_flag(&words, &["--namespace"]), None);
    }

    #[test]
    fn with_ns_appends() {
        assert_eq!(
            with_ns(&["get", "pods"], &["-n", "myns"]),
            vec!["get", "pods", "-n", "myns"]
        );
    }

    #[test]
    fn with_ns_empty() {
        assert_eq!(with_ns(&["get", "pods"], &[]), vec!["get", "pods"]);
    }

    #[test]
    fn buffer_context_skips_boolean_flags() {
        let ctx = BufferContext::parse("kubectl get pods --watch --all-namespaces", "");
        assert_eq!(ctx.subcmds, vec!["get", "pods"]);
    }
}
