//! YAML completion specs — portable, shareable completion definitions.
//!
//! Specs are loaded from:
//!   1. Built-in defaults (compiled into the binary)
//!   2. User specs: ~/.config/skim-tab/specs/*.yaml
//!   3. Project specs: .skim-tab/specs/*.yaml (in CWD)

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use crate::config;

// ── Spec types ────────────────────────────────────────────────────────

/// A single YAML completion spec file.
#[derive(Debug, Clone, Deserialize)]
pub struct CompletionSpec {
    /// Command name(s) that trigger this spec (e.g., ["docker", "podman"]).
    pub commands: Vec<String>,
    /// Icon for the skim prompt (Unicode glyph).
    #[serde(default)]
    pub icon: Option<String>,
    /// Subcommand completions.
    #[serde(default)]
    pub subcommands: HashMap<String, SubcommandSpec>,
    /// Global flags.
    #[serde(default)]
    pub flags: HashMap<String, String>,
}

/// A subcommand entry within a completion spec.
#[derive(Debug, Clone, Deserialize)]
pub struct SubcommandSpec {
    /// Description shown in completion.
    #[serde(default)]
    pub description: String,
    /// Category glyph (e.g., "◈" for view, "◇" for mutate).
    #[serde(default)]
    pub glyph: String,
    /// Nested subcommands.
    #[serde(default)]
    pub subcommands: HashMap<String, SubcommandSpec>,
    /// Flags specific to this subcommand.
    #[serde(default)]
    pub flags: HashMap<String, String>,
}

// ── Trait ─────────────────────────────────────────────────────────────

/// Abstraction over description lookup and icon resolution.
///
/// Implemented by `SpecRegistry` for production use and
/// `MockDescriptionProvider` for testing.
pub trait DescriptionProvider {
    /// Look up (glyph, description) for a word under a command.
    fn lookup(&self, command: &str, word: &str) -> Option<(&str, &str)>;
    /// Get the prompt icon for a command, or None.
    fn icon(&self, command: &str) -> Option<&str>;
    /// Check if a command is a Kubernetes-related tool.
    fn is_k8s_command(&self, cmd: &str) -> bool;
}

// ── Registry ──────────────────────────────────────────────────────────

/// Aggregated registry of all loaded completion specs.
///
/// Merges built-in, user, and project specs. Later sources override
/// earlier ones (project > user > built-in) for the same command.
pub struct SpecRegistry {
    specs: Vec<CompletionSpec>,
}

/// Built-in spec YAML files, compiled into the binary.
const BUILTIN_SPECS: &[(&str, &str)] = &[
    ("docker.yaml", include_str!("../specs/docker.yaml")),
    ("nix.yaml", include_str!("../specs/nix.yaml")),
    ("cargo.yaml", include_str!("../specs/cargo.yaml")),
    ("git.yaml", include_str!("../specs/git.yaml")),
    ("npm.yaml", include_str!("../specs/npm.yaml")),
    ("terraform.yaml", include_str!("../specs/terraform.yaml")),
    ("aws.yaml", include_str!("../specs/aws.yaml")),
    ("gcloud.yaml", include_str!("../specs/gcloud.yaml")),
    ("az.yaml", include_str!("../specs/az.yaml")),
    ("kubectl.yaml", include_str!("../specs/kubectl.yaml")),
    ("helm.yaml", include_str!("../specs/helm.yaml")),
    ("flux.yaml", include_str!("../specs/flux.yaml")),
];

/// Global singleton registry, lazily initialized on first access.
static GLOBAL_REGISTRY: OnceLock<SpecRegistry> = OnceLock::new();

impl SpecRegistry {
    /// Create a new registry, loading specs from all configured sources.
    ///
    /// Load order (later wins for same command):
    ///   1. Built-in defaults (compiled into the binary)
    ///   2. User specs from each directory in `specs_cfg.dirs`
    ///   3. Project specs from `.skim-tab/specs/` in CWD (if enabled)
    pub fn new(specs_cfg: &config::SpecsConfig) -> Self {
        let mut specs = Vec::new();

        if !specs_cfg.enable {
            return Self { specs };
        }

        // 1. Built-in defaults
        for (name, content) in BUILTIN_SPECS {
            match serde_yaml::from_str::<CompletionSpec>(content) {
                Ok(spec) => specs.push(spec),
                Err(e) => eprintln!("skim-tab: failed to parse built-in spec {name}: {e}"),
            }
        }

        // 2. User spec directories
        for dir in &specs_cfg.dirs {
            let expanded = shellexpand_tilde(dir);
            let path = Path::new(&expanded);
            if path.is_dir() {
                specs.extend(load_specs_from_dir(path));
            }
        }

        // 3. Project specs (CWD/.skim-tab/specs/)
        if specs_cfg.project_specs {
            let project_dir = Path::new(".skim-tab/specs");
            if project_dir.is_dir() {
                specs.extend(load_specs_from_dir(project_dir));
            }
        }

        Self { specs }
    }

    /// Get the global singleton registry.
    ///
    /// Initializes on first call using the provided config. Subsequent
    /// calls return the same registry regardless of config changes.
    pub fn global(specs_cfg: &config::SpecsConfig) -> &'static Self {
        GLOBAL_REGISTRY.get_or_init(|| Self::new(specs_cfg))
    }
}

impl DescriptionProvider for SpecRegistry {
    /// Look up (glyph, description) for a word under a command.
    ///
    /// Searches specs in reverse order so that later-loaded specs
    /// (project > user > built-in) take priority.
    fn lookup(&self, command: &str, word: &str) -> Option<(&str, &str)> {
        // Search in reverse so later specs (higher priority) win.
        for spec in self.specs.iter().rev() {
            if spec.commands.iter().any(|c| c == command) {
                if let Some(sub) = spec.subcommands.get(word) {
                    return Some((&sub.glyph, &sub.description));
                }
            }
        }
        None
    }

    /// Get the prompt icon for a command from specs, or None.
    fn icon(&self, command: &str) -> Option<&str> {
        for spec in self.specs.iter().rev() {
            if spec.commands.iter().any(|c| c == command) {
                if let Some(ref icon) = spec.icon {
                    return Some(icon.as_str());
                }
            }
        }
        None
    }

    /// Check if a command is a Kubernetes-related tool.
    ///
    /// Returns `true` if the command matches any spec that uses the
    /// K8s icon ("\u{2388} ").
    fn is_k8s_command(&self, cmd: &str) -> bool {
        self.specs.iter().any(|s| {
            s.commands.iter().any(|c| c == cmd)
                && s.icon.as_deref() == Some("\u{2388} ")
        })
    }
}

// ── Mock provider ─────────────────────────────────────────────────────

/// Test-only description provider with configurable responses.
#[cfg(test)]
pub struct MockDescriptionProvider {
    /// (command, word) -> (glyph, description)
    pub entries: HashMap<(String, String), (String, String)>,
    /// command -> icon
    pub icons: HashMap<String, String>,
    /// Commands considered K8s-related
    pub k8s_commands: Vec<String>,
}

#[cfg(test)]
impl MockDescriptionProvider {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            icons: HashMap::new(),
            k8s_commands: Vec::new(),
        }
    }
}

#[cfg(test)]
impl DescriptionProvider for MockDescriptionProvider {
    fn lookup(&self, command: &str, word: &str) -> Option<(&str, &str)> {
        self.entries
            .get(&(command.to_string(), word.to_string()))
            .map(|(g, d)| (g.as_str(), d.as_str()))
    }

    fn icon(&self, command: &str) -> Option<&str> {
        self.icons.get(command).map(String::as_str)
    }

    fn is_k8s_command(&self, cmd: &str) -> bool {
        self.k8s_commands.iter().any(|c| c == cmd)
    }
}

// ── Loader ────────────────────────────────────────────────────────────

/// Read all `.yaml` and `.yml` files from a directory and parse them as
/// completion specs.
pub fn load_specs_from_dir(path: &Path) -> Vec<CompletionSpec> {
    let mut specs = Vec::new();

    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return specs,
    };

    for entry in entries.flatten() {
        let file_path = entry.path();
        let ext = file_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        if ext != "yaml" && ext != "yml" {
            continue;
        }

        match std::fs::read_to_string(&file_path) {
            Ok(content) => match serde_yaml::from_str::<CompletionSpec>(&content) {
                Ok(spec) => specs.push(spec),
                Err(e) => {
                    eprintln!(
                        "skim-tab: failed to parse spec {}: {e}",
                        file_path.display()
                    );
                }
            },
            Err(e) => {
                eprintln!(
                    "skim-tab: failed to read spec {}: {e}",
                    file_path.display()
                );
            }
        }
    }

    specs
}

// ── Helpers ───────────────────────────────────────────────────────────

/// Expand `~` at the start of a path to `$HOME`.
#[must_use]
fn shellexpand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix('~') {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}{rest}");
        }
    }
    path.to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_specs_config() -> config::SpecsConfig {
        config::SpecsConfig {
            enable: true,
            dirs: vec![],
            project_specs: false,
        }
    }

    #[test]
    fn builtin_specs_load() {
        let reg = SpecRegistry::new(&default_specs_config());
        // Should have all 12 built-in specs
        assert!(reg.specs.len() >= 2);
    }

    #[test]
    fn lookup_docker_run() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = DescriptionProvider::lookup(&reg, "docker", "run");
        assert!(result.is_some());
        let (glyph, desc) = result.unwrap();
        assert_eq!(desc, "Run a container");
        assert!(!glyph.is_empty());
    }

    #[test]
    fn lookup_podman_alias() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = DescriptionProvider::lookup(&reg, "podman", "build");
        assert!(result.is_some());
        let (_, desc) = result.unwrap();
        assert_eq!(desc, "Build an image");
    }

    #[test]
    fn lookup_nix_build() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = DescriptionProvider::lookup(&reg, "nix", "build");
        assert!(result.is_some());
        let (glyph, desc) = result.unwrap();
        assert_eq!(desc, "Build a derivation");
        assert!(!glyph.is_empty());
    }

    #[test]
    fn is_k8s_command_via_trait() {
        let reg = SpecRegistry::new(&default_specs_config());
        assert!(reg.is_k8s_command("kubectl"));
        assert!(reg.is_k8s_command("kubecolor"));
        assert!(reg.is_k8s_command("k"));
        assert!(reg.is_k8s_command("helm"));
        assert!(reg.is_k8s_command("flux"));
        assert!(!reg.is_k8s_command("docker"));
        assert!(!reg.is_k8s_command("aws"));
        assert!(!reg.is_k8s_command("cd"));
    }

    #[test]
    fn lookup_unknown_command() {
        let reg = SpecRegistry::new(&default_specs_config());
        assert!(DescriptionProvider::lookup(&reg, "nonexistent-tool", "run").is_none());
    }

    #[test]
    fn lookup_unknown_word() {
        let reg = SpecRegistry::new(&default_specs_config());
        assert!(DescriptionProvider::lookup(&reg, "docker", "nonexistent-subcommand").is_none());
    }

    #[test]
    fn icon_docker() {
        let reg = SpecRegistry::new(&default_specs_config());
        let icon = DescriptionProvider::icon(&reg, "docker");
        assert!(icon.is_some());
    }

    #[test]
    fn icon_unknown_command() {
        let reg = SpecRegistry::new(&default_specs_config());
        assert!(DescriptionProvider::icon(&reg, "unknown-tool").is_none());
    }

    #[test]
    fn disabled_specs_empty() {
        let cfg = config::SpecsConfig {
            enable: false,
            dirs: vec![],
            project_specs: false,
        };
        let reg = SpecRegistry::new(&cfg);
        assert!(reg.specs.is_empty());
        assert!(DescriptionProvider::lookup(&reg, "docker", "run").is_none());
    }

    #[test]
    fn load_from_nonexistent_dir() {
        let specs = load_specs_from_dir(Path::new("/tmp/nonexistent-skim-tab-specs-dir"));
        assert!(specs.is_empty());
    }

    #[test]
    fn shellexpand_tilde_expands() {
        let expanded = shellexpand_tilde("~/foo/bar");
        assert!(!expanded.starts_with('~'));
        assert!(expanded.ends_with("/foo/bar"));
    }

    #[test]
    fn shellexpand_tilde_no_tilde() {
        assert_eq!(shellexpand_tilde("/absolute/path"), "/absolute/path");
    }

    #[test]
    fn parse_docker_yaml() {
        let spec: CompletionSpec =
            serde_yaml::from_str(BUILTIN_SPECS[0].1).expect("docker.yaml should parse");
        assert!(spec.commands.contains(&"docker".to_string()));
        assert!(spec.commands.contains(&"podman".to_string()));
        assert!(spec.subcommands.contains_key("run"));
        assert!(spec.subcommands.contains_key("compose"));
    }

    #[test]
    fn parse_nix_yaml() {
        let spec: CompletionSpec =
            serde_yaml::from_str(BUILTIN_SPECS[1].1).expect("nix.yaml should parse");
        assert!(spec.commands.contains(&"nix".to_string()));
        assert!(spec.subcommands.contains_key("flake"));
        // Nested subcommands
        let flake = &spec.subcommands["flake"];
        assert!(flake.subcommands.contains_key("update"));
        assert!(flake.subcommands.contains_key("check"));
    }

    #[test]
    fn lookup_aws_s3() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = DescriptionProvider::lookup(&reg, "aws", "s3");
        assert!(result.is_some());
        let (_, desc) = result.unwrap();
        assert_eq!(desc, "Object storage");
    }

    #[test]
    fn lookup_gcloud_compute() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = DescriptionProvider::lookup(&reg, "gcloud", "compute");
        assert!(result.is_some());
        let (_, desc) = result.unwrap();
        assert_eq!(desc, "Virtual machines & disks");
    }

    #[test]
    fn lookup_az_vm() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = DescriptionProvider::lookup(&reg, "az", "vm");
        assert!(result.is_some());
        let (_, desc) = result.unwrap();
        assert_eq!(desc, "Virtual machines");
    }

    // ── New YAML spec parsing tests ────────────────────────────────

    #[test]
    fn parse_kubectl_yaml() {
        let spec: CompletionSpec =
            serde_yaml::from_str(BUILTIN_SPECS[9].1).expect("kubectl.yaml should parse");
        assert!(spec.commands.contains(&"kubectl".to_string()));
        assert!(spec.commands.contains(&"kubecolor".to_string()));
        assert!(spec.commands.contains(&"k".to_string()));
        assert!(spec.subcommands.contains_key("get"));
        assert!(spec.subcommands.contains_key("apply"));
        assert!(spec.subcommands.contains_key("pods"));
        assert!(spec.subcommands.contains_key("deploy"));
        assert!(spec.subcommands.contains_key("crd"));
        assert!(spec.icon.is_some());
    }

    #[test]
    fn parse_helm_yaml() {
        let spec: CompletionSpec =
            serde_yaml::from_str(BUILTIN_SPECS[10].1).expect("helm.yaml should parse");
        assert!(spec.commands.contains(&"helm".to_string()));
        assert!(spec.subcommands.contains_key("install"));
        assert!(spec.subcommands.contains_key("upgrade"));
        assert!(spec.subcommands.contains_key("values"));
        assert!(spec.icon.is_some());
    }

    #[test]
    fn parse_flux_yaml() {
        let spec: CompletionSpec =
            serde_yaml::from_str(BUILTIN_SPECS[11].1).expect("flux.yaml should parse");
        assert!(spec.commands.contains(&"flux".to_string()));
        assert!(spec.subcommands.contains_key("reconcile"));
        assert!(spec.subcommands.contains_key("kustomizations"));
        assert!(spec.subcommands.contains_key("hr"));
        assert!(spec.icon.is_some());
    }

    #[test]
    fn lookup_kubectl_aliases() {
        let reg = SpecRegistry::new(&default_specs_config());
        // "kubectl" command
        let result = reg.lookup("kubectl", "pods");
        assert!(result.is_some());
        let (glyph, desc) = result.unwrap();
        assert_eq!(desc, "Pod workloads");
        assert!(!glyph.is_empty());

        // "k" alias
        let result_k = reg.lookup("k", "deploy");
        assert!(result_k.is_some());
        let (_, desc_k) = result_k.unwrap();
        assert_eq!(desc_k, "Managed replicas");

        // "kubecolor" alias
        let result_kc = reg.lookup("kubecolor", "get");
        assert!(result_kc.is_some());
        let (_, desc_kc) = result_kc.unwrap();
        assert_eq!(desc_kc, "Display resources");
    }

    #[test]
    fn all_12_specs_parse() {
        assert_eq!(BUILTIN_SPECS.len(), 12);
        for (name, content) in BUILTIN_SPECS {
            let result = serde_yaml::from_str::<CompletionSpec>(content);
            assert!(result.is_ok(), "failed to parse {name}: {:?}", result.err());
        }
    }

    #[test]
    fn icon_kubectl() {
        let reg = SpecRegistry::new(&default_specs_config());
        assert!(DescriptionProvider::icon(&reg, "kubectl").is_some());
        assert!(DescriptionProvider::icon(&reg, "k").is_some());
        assert!(DescriptionProvider::icon(&reg, "helm").is_some());
        assert!(DescriptionProvider::icon(&reg, "flux").is_some());
    }

    #[test]
    fn mock_provider_lookup() {
        let mut mock = MockDescriptionProvider::new();
        mock.entries.insert(
            ("test-cmd".to_string(), "sub".to_string()),
            ("G".to_string(), "A description".to_string()),
        );
        mock.icons
            .insert("test-cmd".to_string(), "T ".to_string());

        let result = DescriptionProvider::lookup(&mock, "test-cmd", "sub");
        assert_eq!(result, Some(("G", "A description")));
        assert!(DescriptionProvider::lookup(&mock, "test-cmd", "missing").is_none());

        assert_eq!(DescriptionProvider::icon(&mock, "test-cmd"), Some("T "));
        assert!(DescriptionProvider::icon(&mock, "other").is_none());
    }
}
