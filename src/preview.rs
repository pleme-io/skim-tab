//! Preview generation for completion candidates.
//!
//! Self-contained module: takes strings, returns strings. No dependency
//! on skim types or the completion protocol.
//!
//! ## Preview dispatch
//!
//! The preview binary (`skim-tab --preview`) receives a manifest path and the
//! selected item text.  It reads the manifest to get command context, resolves
//! the candidate, and dispatches to the appropriate preview handler via
//! `detect_preview_type`.

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

// ── Preview type dispatch ───────────────────────────────────────────

/// Categorized preview type for dispatch.
enum PreviewType {
    Directory(String),
    File(String),
    GitBranch(String),
    GitFile(String),
    K8sResource { _tool: String, _ctx: String, _word: String },
    Process(String),
    Generic(String),
}

/// Detect the appropriate preview type from command context and candidate.
fn detect_preview_type(command: &str, buffer: &str, word: &str, realdir: &str) -> PreviewType {
    let ctx = BufferContext::parse(buffer, command);

    // K8s tools get their own dispatch (handled separately in preview())
    if matches!(ctx.base_cmd, "kubectl" | "kubecolor" | "k" | "flux" | "helm") {
        return PreviewType::K8sResource {
            _tool: ctx.base_cmd.to_string(),
            _ctx: buffer.to_string(),
            _word: word.to_string(),
        };
    }

    // Process previews
    if matches!(command, "kill" | "ps") {
        return PreviewType::Process(word.to_string());
    }

    // Git command previews — handle both curcontext format (git-checkout)
    // and base_cmd format (git with sub(0) = checkout)
    let git_sub = if ctx.base_cmd == "git" {
        ctx.sub(0)
    } else if command.starts_with("git-") {
        &command[4..]
    } else {
        ""
    };

    match git_sub {
        "checkout" | "switch" | "merge" | "rebase" | "log" => {
            if !word.contains('/') || !Path::new(word).exists() {
                return PreviewType::GitBranch(word.to_string());
            }
        }
        "add" | "diff" | "restore" | "reset" | "stash" => {
            return PreviewType::GitFile(word.to_string());
        }
        _ => {}
    }

    // Resolve filesystem path
    let path = if realdir.is_empty() {
        word.to_string()
    } else {
        format!("{realdir}{word}")
    };

    let p = Path::new(&path);
    if p.is_dir() {
        PreviewType::Directory(path)
    } else if p.is_file() {
        PreviewType::File(path)
    } else {
        PreviewType::Generic(word.to_string())
    }
}

// ── Public interface ──────────────────────────────────────────────────

/// Default max lines for preview output.
const DEFAULT_MAX_LINES: usize = 30;

/// Generate a preview string for a completion candidate.
pub fn preview(word: &str, command: &str, buffer: &str, realdir: &str) -> String {
    let ctx = BufferContext::parse(buffer, command);

    // K8s tools: resource type candidates (trailing `/`) handled uniformly,
    // then tool-specific dispatch.
    if let cmd @ ("kubectl" | "kubecolor" | "k" | "flux" | "helm") = ctx.base_cmd {
        if word.ends_with('/') {
            return preview_resource_type(cmd, word.trim_end_matches('/'), &ctx.ns_args());
        }
        return match cmd {
            "flux" => preview_flux(&ctx, word),
            "helm" => preview_helm(&ctx, word),
            _ => preview_kubectl(&ctx, word),
        };
    }

    let path = if realdir.is_empty() {
        word.to_string()
    } else {
        format!("{realdir}{word}")
    };

    match detect_preview_type(command, buffer, word, realdir) {
        PreviewType::Directory(p) => preview_dir(&p, DEFAULT_MAX_LINES),
        PreviewType::File(p) => preview_file(&p, DEFAULT_MAX_LINES),
        PreviewType::GitBranch(name) => preview_git_branch(&name),
        PreviewType::GitFile(file) => preview_git_diff(&file, DEFAULT_MAX_LINES),
        PreviewType::Process(pid) => preview_proc(&pid),
        PreviewType::K8sResource { .. } => String::new(), // handled above
        PreviewType::Generic(text) => {
            // Context-aware generic preview: try subcommand help for known tools,
            // then path-based preview, then command help.
            match ctx.base_cmd {
                "cd" | "pushd" | "z" => preview_dir(&path, DEFAULT_MAX_LINES),
                // Tools with subcommands: show `tool subcmd --help`
                "git" if !text.starts_with('-') && ctx.subcmds.is_empty() => {
                    preview_subcommand("git", &text)
                }
                "cargo" if ctx.subcmds.is_empty() => preview_subcommand("cargo", &text),
                "docker" | "podman" if ctx.subcmds.is_empty() => {
                    preview_subcommand(ctx.base_cmd, &text)
                }
                "nix" if ctx.subcmds.is_empty() => preview_subcommand("nix", &text),
                "npm" | "pnpm" | "yarn" if ctx.subcmds.is_empty() => {
                    preview_subcommand(ctx.base_cmd, &text)
                }
                "terraform" | "tofu" if ctx.subcmds.is_empty() => {
                    preview_subcommand(ctx.base_cmd, &text)
                }
                "rustup" if ctx.subcmds.is_empty() => preview_subcommand("rustup", &text),
                "brew" if ctx.subcmds.is_empty() => preview_subcommand("brew", &text),
                "systemctl" if ctx.subcmds.is_empty() => preview_subcommand("systemctl", &text),
                "make" | "just" => try_path_then_command(&path, &text, DEFAULT_MAX_LINES),
                "" => try_path_then_command(&path, &text, DEFAULT_MAX_LINES),
                _ => preview_default(&path, &text, ctx.base_cmd, DEFAULT_MAX_LINES),
            }
        }
    }
}

// ── K8s resource listing ────────────────────────────────────────────

/// List resources via `kubectl get`, formatted with count and sample.
fn kubectl_resource_listing(resource_type: &str, ns: &[&str]) -> String {
    let out = run("kubectl", &with_ns(&["get", resource_type, "--no-headers"], ns));
    let count = out.lines().count();
    let sample: String = out.lines().take(25).collect::<Vec<_>>().join("\n");
    format!("  {} resources: {count}\n\n{sample}", resource_type.to_uppercase())
}

/// Preview a resource type candidate (trailing `/` already stripped).
/// For flux, tries `flux get` first; falls back to kubectl.
fn preview_resource_type(tool: &str, resource_type: &str, ns: &[&str]) -> String {
    if tool == "flux" {
        let out = run("flux", &with_ns(&["get", resource_type], ns));
        if !out.is_empty() {
            return format!("  {}\n\n{}", resource_type.to_uppercase(), out);
        }
    }
    kubectl_resource_listing(resource_type, ns)
}

// ── kubectl ──────────────────────────────────────────────────────────

fn preview_kubectl(ctx: &BufferContext, candidate: &str) -> String {
    let ns = ctx.ns_args();
    let (sub0, sub1) = (ctx.sub(0), ctx.sub(1));

    match sub0 {
        "get" | "describe" | "edit" | "delete" if !sub1.is_empty() => {
            truncated("kubectl", &with_ns(&["describe", sub1, candidate], &ns), 60)
        }
        "get" | "describe" | "edit" | "delete" => kubectl_resource_listing(candidate, &ns),
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
        "apply" | "create" => try_path_then_command(candidate, candidate, DEFAULT_MAX_LINES),
        "" if candidate.starts_with('-') => preview_command("kubectl"),
        "" => preview_subcommand("kubectl", candidate),
        _ => preview_command("kubectl"),
    }
}

// ── flux ─────────────────────────────────────────────────────────────

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
        "" if candidate.starts_with('-') => preview_command("flux"),
        "" => preview_subcommand("flux", candidate),
        _ => preview_command("flux"),
    }
}

// ── helm ─────────────────────────────────────────────────────────────

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
        "" if candidate.starts_with('-') => preview_command("helm"),
        "" => preview_subcommand("helm", candidate),
        _ => preview_command("helm"),
    }
}

// ── Subcommand previewer ─────────────────────────────────────────────

/// Preview a tool's subcommand: try `tool subcmd --help`, then tldr, then generic.
fn preview_subcommand(tool: &str, subcmd: &str) -> String {
    // Try the specific subcommand help (e.g., `kubectl get --help`)
    let help = run(tool, &[subcmd, "--help"]);
    if !help.is_empty() {
        return help.lines().take(50).collect::<Vec<_>>().join("\n");
    }
    // Try tldr (e.g., `tldr kubectl-get`)
    let tldr = preview_command(&format!("{tool}-{subcmd}"));
    if !tldr.is_empty() {
        return tldr;
    }
    // Fallback to generic tool help
    preview_command(tool)
}

// ── Generic previewers ───────────────────────────────────────────────

fn try_path_then_command(path: &str, word: &str, max_lines: usize) -> String {
    if Path::new(path).is_dir() {
        preview_dir(path, max_lines)
    } else if Path::new(path).is_file() {
        preview_file(path, max_lines)
    } else {
        preview_command(word)
    }
}

fn preview_default(path: &str, word: &str, command: &str, max_lines: usize) -> String {
    if Path::new(path).is_dir() {
        return preview_dir(path, max_lines);
    }
    if Path::new(path).is_file() {
        return preview_file(path, max_lines);
    }
    if word.starts_with('-') {
        return preview_command(command);
    }
    let result = preview_command(word);
    if result.is_empty() { preview_command(command) } else { result }
}

// ── R3a: Enhanced directory preview ─────────────────────────────────

/// Preview a directory with entry listing and count header.
///
/// Prefers `eza -la --icons --color=always --no-user --no-time` for rich
/// output; falls back to `ls -la` if eza is not available.
fn preview_dir(path: &str, max_lines: usize) -> String {
    let entry_count = std::fs::read_dir(path)
        .map(|rd| rd.count())
        .unwrap_or(0);

    let header = format!("  \x1b[1;36m{}\x1b[0m  ({} entries)\n\n", path, entry_count);

    let body = if has_tool("eza") {
        run("eza", &[
            "-la", "--icons", "--color=always", "--no-user", "--no-time", path,
        ])
    } else {
        run("ls", &["-la", path])
    };

    let limited: String = body
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n");

    format!("{header}{limited}")
}

// ── R3b: Enhanced file preview ──────────────────────────────────────

/// Known binary file extensions that should not be previewed as text.
const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "tiff", "tif", "svg",
    "mp3", "mp4", "mkv", "avi", "mov", "flac", "wav", "ogg", "opus", "aac",
    "zip", "tar", "gz", "bz2", "xz", "zst", "7z", "rar",
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
    "exe", "dll", "so", "dylib", "a", "o", "obj",
    "wasm", "class", "pyc", "pyo",
    "sqlite", "db", "sqlite3",
];

/// Image extensions for dimension info.
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "tiff", "tif", "svg",
];

/// Detect whether a file is binary by extension, falling back to the `file` command.
fn is_binary_file(path: &str) -> bool {
    if let Some(ext) = Path::new(path).extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_ascii_lowercase();
        if BINARY_EXTENSIONS.contains(&ext_lower.as_str()) {
            return true;
        }
    }
    // Fall back to `file` command for detection
    let file_output = run("file", &["--brief", path]);
    file_output.contains("data")
        || file_output.contains("executable")
        || file_output.contains("binary")
        || file_output.contains("archive")
        || file_output.contains("compressed")
}

/// Check if the file has an image extension.
fn is_image_file(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Format a byte count into a human-readable size string.
fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Preview a file with type-aware rendering.
///
/// - **Text files:** Uses `bat` for syntax-highlighted preview with line
///   numbers; falls back to `head` if bat is not available.
/// - **Image files:** Shows dimensions and file info.
/// - **Binary files:** Shows `file` type output and size.
fn preview_file(path: &str, max_lines: usize) -> String {
    if is_image_file(path) {
        return preview_image_file(path);
    }

    if is_binary_file(path) {
        return preview_binary_file(path);
    }

    // Text file: bat with fallback to head
    preview_text_file(path, max_lines)
}

/// Preview a text file with syntax highlighting.
fn preview_text_file(path: &str, max_lines: usize) -> String {
    let range = format!(":{max_lines}");

    if has_tool("bat") {
        let output = run("bat", &[
            "--color=always",
            "--style=numbers,changes",
            &format!("--line-range={range}"),
            path,
        ]);
        if !output.is_empty() {
            return output;
        }
    }

    // Fallback: head
    let n = max_lines.to_string();
    run("head", &[&format!("-{n}"), path])
}

/// Preview a binary file with type and size info.
fn preview_binary_file(path: &str) -> String {
    let file_type = run("file", &["--brief", path]).trim().to_string();
    let size = std::fs::metadata(path)
        .map(|m| human_size(m.len()))
        .unwrap_or_else(|_| "unknown".to_string());

    format!(
        "  \x1b[1;36m{}\x1b[0m\n\n\
         \x1b[33mType:\x1b[0m  {}\n\
         \x1b[33mSize:\x1b[0m  {}",
        path, file_type, size
    )
}

/// Preview an image file with dimensions and info.
fn preview_image_file(path: &str) -> String {
    let file_info = run("file", &["--brief", path]).trim().to_string();
    let size = std::fs::metadata(path)
        .map(|m| human_size(m.len()))
        .unwrap_or_else(|_| "unknown".to_string());

    format!(
        "  \x1b[1;36m{}\x1b[0m\n\n\
         \x1b[33mType:\x1b[0m  {}\n\
         \x1b[33mSize:\x1b[0m  {}",
        path, file_info, size
    )
}

// ── R3d: Git preview ────────────────────────────────────────────────

/// Preview a git branch: show recent commit log.
fn preview_git_branch(name: &str) -> String {
    let output = run("git", &[
        "log", "--oneline", "--color=always", "--graph", "-10", name,
    ]);
    if output.is_empty() {
        return format!("  Branch: {name}\n  (no commits or not found)");
    }
    format!("  \x1b[1;36mBranch:\x1b[0m {name}\n\n{output}")
}

/// Preview a git file diff with line limit.
fn preview_git_diff(file: &str, max_lines: usize) -> String {
    let output = run("git", &["diff", "--color=always", "--", file]);
    if output.is_empty() {
        // No staged/unstaged diff — show the file content instead
        let status = run("git", &["status", "--short", "--", file]);
        if status.is_empty() {
            return format!("  {file}\n  (no changes)");
        }
        return format!("  \x1b[1;36m{file}\x1b[0m\n\n{status}");
    }
    let limited: String = output
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n");
    format!("  \x1b[1;36m{file}\x1b[0m\n\n{limited}")
}

fn preview_proc(word: &str) -> String {
    run("ps", &["-p", word, "-o", "pid,ppid,%cpu,%mem,start,command"])
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

// ── Tool availability ───────────────────────────────────────────────

/// Check if an external tool is available on PATH.
fn has_tool(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
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

    // ── R3 dispatch tests ───────────────────────────────────────────

    #[test]
    fn detect_git_checkout_as_branch() {
        match detect_preview_type("git-checkout", "git checkout", "main", "") {
            PreviewType::GitBranch(name) => assert_eq!(name, "main"),
            _ => panic!("expected GitBranch"),
        }
    }

    #[test]
    fn detect_git_switch_as_branch() {
        match detect_preview_type("git-switch", "git switch", "feature/foo", "") {
            PreviewType::GitBranch(name) => assert_eq!(name, "feature/foo"),
            _ => panic!("expected GitBranch"),
        }
    }

    #[test]
    fn detect_git_merge_as_branch() {
        match detect_preview_type("git-merge", "git merge", "develop", "") {
            PreviewType::GitBranch(name) => assert_eq!(name, "develop"),
            _ => panic!("expected GitBranch"),
        }
    }

    #[test]
    fn detect_git_rebase_as_branch() {
        match detect_preview_type("git-rebase", "git rebase", "main", "") {
            PreviewType::GitBranch(name) => assert_eq!(name, "main"),
            _ => panic!("expected GitBranch"),
        }
    }

    #[test]
    fn detect_git_add_as_file() {
        match detect_preview_type("git-add", "git add", "src/main.rs", "") {
            PreviewType::GitFile(path) => assert_eq!(path, "src/main.rs"),
            _ => panic!("expected GitFile"),
        }
    }

    #[test]
    fn detect_git_diff_as_file() {
        match detect_preview_type("git-diff", "git diff", "Cargo.toml", "") {
            PreviewType::GitFile(path) => assert_eq!(path, "Cargo.toml"),
            _ => panic!("expected GitFile"),
        }
    }

    #[test]
    fn detect_git_restore_as_file() {
        match detect_preview_type("git-restore", "git restore", "README.md", "") {
            PreviewType::GitFile(path) => assert_eq!(path, "README.md"),
            _ => panic!("expected GitFile"),
        }
    }

    #[test]
    fn detect_git_log_as_branch() {
        match detect_preview_type("git-log", "git log", "main", "") {
            PreviewType::GitBranch(name) => assert_eq!(name, "main"),
            _ => panic!("expected GitBranch"),
        }
    }

    #[test]
    fn detect_kill_as_process() {
        match detect_preview_type("kill", "", "12345", "") {
            PreviewType::Process(pid) => assert_eq!(pid, "12345"),
            _ => panic!("expected Process"),
        }
    }

    #[test]
    fn detect_kubectl_as_k8s() {
        match detect_preview_type("kubectl", "kubectl get pods", "my-pod", "") {
            PreviewType::K8sResource { _tool, .. } => assert_eq!(_tool, "kubectl"),
            _ => panic!("expected K8sResource"),
        }
    }

    #[test]
    fn detect_nonexistent_path_as_generic() {
        match detect_preview_type("", "", "nonexistent-thing-xyz", "") {
            PreviewType::Generic(text) => assert_eq!(text, "nonexistent-thing-xyz"),
            _ => panic!("expected Generic"),
        }
    }

    #[test]
    fn human_size_bytes() {
        assert_eq!(human_size(500), "500 B");
    }

    #[test]
    fn human_size_kilobytes() {
        assert_eq!(human_size(2048), "2.0 KB");
    }

    #[test]
    fn human_size_megabytes() {
        assert_eq!(human_size(1_500_000), "1.4 MB");
    }

    #[test]
    fn human_size_gigabytes() {
        assert_eq!(human_size(2_000_000_000), "1.9 GB");
    }

    #[test]
    fn binary_extension_detected() {
        assert!(BINARY_EXTENSIONS.contains(&"png"));
        assert!(BINARY_EXTENSIONS.contains(&"zip"));
        assert!(BINARY_EXTENSIONS.contains(&"pdf"));
        assert!(!BINARY_EXTENSIONS.contains(&"rs"));
        assert!(!BINARY_EXTENSIONS.contains(&"txt"));
    }

    #[test]
    fn image_extension_detected() {
        assert!(is_image_file("photo.png"));
        assert!(is_image_file("image.JPG"));
        assert!(is_image_file("art.webp"));
        assert!(!is_image_file("code.rs"));
        assert!(!is_image_file("data.zip"));
    }

    #[test]
    fn preview_dir_for_existing_dir() {
        // /tmp always exists on macOS/Linux
        let output = preview_dir("/tmp", 10);
        assert!(output.contains("/tmp"));
        assert!(output.contains("entries"));
    }

    #[test]
    fn preview_text_file_for_cargo_toml() {
        let output = preview_text_file("Cargo.toml", 5);
        assert!(!output.is_empty());
    }

    #[test]
    fn detect_directory_preview() {
        match detect_preview_type("", "", "/tmp", "") {
            PreviewType::Directory(p) => assert_eq!(p, "/tmp"),
            _ => panic!("expected Directory for /tmp"),
        }
    }
}
