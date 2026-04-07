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

    // ── Spec priority (later specs override earlier) ─────────────────

    #[test]
    fn later_spec_overrides_earlier() {
        let spec1 = CompletionSpec {
            commands: vec!["test-cmd".to_string()],
            icon: Some("A ".to_string()),
            subcommands: {
                let mut m = HashMap::new();
                m.insert("sub1".to_string(), SubcommandSpec {
                    description: "original".to_string(),
                    glyph: "".to_string(),
                    subcommands: HashMap::new(),
                    flags: HashMap::new(),
                });
                m
            },
            flags: HashMap::new(),
        };
        let spec2 = CompletionSpec {
            commands: vec!["test-cmd".to_string()],
            icon: Some("B ".to_string()),
            subcommands: {
                let mut m = HashMap::new();
                m.insert("sub1".to_string(), SubcommandSpec {
                    description: "overridden".to_string(),
                    glyph: "".to_string(),
                    subcommands: HashMap::new(),
                    flags: HashMap::new(),
                });
                m
            },
            flags: HashMap::new(),
        };
        let reg = SpecRegistry { specs: vec![spec1, spec2] };
        let (_, desc) = reg.lookup("test-cmd", "sub1").unwrap();
        assert_eq!(desc, "overridden");
        assert_eq!(reg.icon("test-cmd"), Some("B "));
    }

    // ── load_specs_from_dir ──────────────────────────────────────────

    #[test]
    fn load_specs_from_dir_valid_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
commands: [mytest]
icon: "T "
subcommands:
  run:
    description: Run it
    glyph: ">"
"#;
        std::fs::write(dir.path().join("test.yaml"), yaml).unwrap();
        let specs = load_specs_from_dir(dir.path());
        assert_eq!(specs.len(), 1);
        assert!(specs[0].commands.contains(&"mytest".to_string()));
    }

    #[test]
    fn load_specs_from_dir_ignores_non_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "not yaml").unwrap();
        std::fs::write(dir.path().join("test.json"), "{}").unwrap();
        let specs = load_specs_from_dir(dir.path());
        assert!(specs.is_empty());
    }

    #[test]
    fn load_specs_from_dir_handles_yml_extension() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = "commands: [ymltest]\nsubcommands: {}\n";
        std::fs::write(dir.path().join("test.yml"), yaml).unwrap();
        let specs = load_specs_from_dir(dir.path());
        assert_eq!(specs.len(), 1);
    }

    #[test]
    fn load_specs_from_dir_skips_invalid_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.yaml"), "{{invalid yaml").unwrap();
        let valid = "commands: [good]\nsubcommands: {}\n";
        std::fs::write(dir.path().join("good.yaml"), valid).unwrap();
        let specs = load_specs_from_dir(dir.path());
        assert_eq!(specs.len(), 1);
        assert!(specs[0].commands.contains(&"good".to_string()));
    }

    // ── SpecRegistry::new with user dirs ─────────────────────────────

    #[test]
    fn registry_with_user_dir_overrides_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
commands: [docker, podman]
subcommands:
  run:
    description: Custom run override
    glyph: ">"
"#;
        std::fs::write(dir.path().join("docker.yaml"), yaml).unwrap();
        let cfg = config::SpecsConfig {
            enable: true,
            dirs: vec![dir.path().to_str().unwrap().to_string()],
            project_specs: false,
        };
        let reg = SpecRegistry::new(&cfg);
        let (_, desc) = reg.lookup("docker", "run").unwrap();
        assert_eq!(desc, "Custom run override");
    }

    // ── is_k8s_command edge cases ────────────────────────────────────

    #[test]
    fn is_k8s_command_false_for_no_icon() {
        let spec = CompletionSpec {
            commands: vec!["noicon".to_string()],
            icon: None,
            subcommands: HashMap::new(),
            flags: HashMap::new(),
        };
        let reg = SpecRegistry { specs: vec![spec] };
        assert!(!reg.is_k8s_command("noicon"));
    }

    #[test]
    fn is_k8s_command_false_for_wrong_icon() {
        let spec = CompletionSpec {
            commands: vec!["docker".to_string()],
            icon: Some("\u{1F40B} ".to_string()),
            subcommands: HashMap::new(),
            flags: HashMap::new(),
        };
        let reg = SpecRegistry { specs: vec![spec] };
        assert!(!reg.is_k8s_command("docker"));
    }

    #[test]
    fn is_k8s_command_true_for_helm_wheel() {
        let spec = CompletionSpec {
            commands: vec!["myk8s".to_string()],
            icon: Some("\u{2388} ".to_string()),
            subcommands: HashMap::new(),
            flags: HashMap::new(),
        };
        let reg = SpecRegistry { specs: vec![spec] };
        assert!(reg.is_k8s_command("myk8s"));
    }

    // ── Mock provider k8s detection ──────────────────────────────────

    #[test]
    fn mock_provider_is_k8s() {
        let mut mock = MockDescriptionProvider::new();
        mock.k8s_commands.push("kubectl".to_string());
        assert!(mock.is_k8s_command("kubectl"));
        assert!(!mock.is_k8s_command("docker"));
    }

    // ── lookup with empty subcommands ────────────────────────────────

    #[test]
    fn lookup_empty_word() {
        let reg = SpecRegistry::new(&default_specs_config());
        assert!(reg.lookup("kubectl", "").is_none());
    }

    #[test]
    fn lookup_empty_command() {
        let reg = SpecRegistry::new(&default_specs_config());
        assert!(reg.lookup("", "run").is_none());
    }

    // ── shellexpand edge cases ───────────────────────────────────────

    #[test]
    fn shellexpand_tilde_only() {
        let result = shellexpand_tilde("~");
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(result, home);
    }

    #[test]
    fn shellexpand_tilde_with_user_unsupported() {
        // ~user/foo — we only expand leading ~, not ~user
        let result = shellexpand_tilde("~otheruser/foo");
        // Should still expand since it starts with ~
        assert!(!result.starts_with('~') || std::env::var("HOME").is_err());
    }

    // ── All specs have commands field ────────────────────────────────

    #[test]
    fn all_builtin_specs_have_commands() {
        for (name, content) in BUILTIN_SPECS {
            let spec: CompletionSpec = serde_yaml::from_str(content)
                .unwrap_or_else(|e| panic!("failed to parse {name}: {e}"));
            assert!(!spec.commands.is_empty(), "{name} has no commands");
        }
    }

    #[test]
    fn all_builtin_specs_have_subcommands() {
        for (name, content) in BUILTIN_SPECS {
            let spec: CompletionSpec = serde_yaml::from_str(content)
                .unwrap_or_else(|e| panic!("failed to parse {name}: {e}"));
            assert!(!spec.subcommands.is_empty(), "{name} has no subcommands");
        }
    }

    // ── Per-spec YAML parser tests ────────────────────────────────────

    #[test]
    fn parse_git_yaml() {
        let spec: CompletionSpec =
            serde_yaml::from_str(BUILTIN_SPECS[3].1).expect("git.yaml should parse");
        assert!(spec.commands.contains(&"git".to_string()));
        assert!(spec.subcommands.contains_key("commit"));
        assert!(spec.subcommands.contains_key("push"));
        assert!(spec.subcommands.contains_key("stash"));
        assert!(spec.subcommands.contains_key("cherry-pick"));
        assert!(spec.icon.is_some());
    }

    #[test]
    fn parse_cargo_yaml() {
        let spec: CompletionSpec =
            serde_yaml::from_str(BUILTIN_SPECS[2].1).expect("cargo.yaml should parse");
        assert!(spec.commands.contains(&"cargo".to_string()));
        assert!(spec.subcommands.contains_key("build"));
        assert!(spec.subcommands.contains_key("test"));
        assert!(spec.subcommands.contains_key("clippy"));
        assert!(spec.subcommands.contains_key("nextest"));
    }

    #[test]
    fn parse_npm_yaml() {
        let spec: CompletionSpec =
            serde_yaml::from_str(BUILTIN_SPECS[4].1).expect("npm.yaml should parse");
        assert!(spec.commands.contains(&"npm".to_string()));
        assert!(spec.commands.contains(&"pnpm".to_string()));
        assert!(spec.commands.contains(&"yarn".to_string()));
        assert!(spec.subcommands.contains_key("install"));
        assert!(spec.subcommands.contains_key("run"));
    }

    #[test]
    fn parse_terraform_yaml() {
        let spec: CompletionSpec =
            serde_yaml::from_str(BUILTIN_SPECS[5].1).expect("terraform.yaml should parse");
        assert!(spec.commands.contains(&"terraform".to_string()));
        assert!(spec.commands.contains(&"tofu".to_string()));
        assert!(spec.subcommands.contains_key("plan"));
        assert!(spec.subcommands.contains_key("apply"));
        assert!(spec.subcommands.contains_key("destroy"));
    }

    #[test]
    fn parse_aws_yaml() {
        let spec: CompletionSpec =
            serde_yaml::from_str(BUILTIN_SPECS[6].1).expect("aws.yaml should parse");
        assert!(spec.commands.contains(&"aws".to_string()));
        assert!(spec.subcommands.contains_key("s3"));
        assert!(spec.subcommands.contains_key("ec2"));
        assert!(spec.subcommands.contains_key("iam"));
        assert!(spec.subcommands.contains_key("lambda"));
    }

    #[test]
    fn parse_gcloud_yaml() {
        let spec: CompletionSpec =
            serde_yaml::from_str(BUILTIN_SPECS[7].1).expect("gcloud.yaml should parse");
        assert!(spec.commands.contains(&"gcloud".to_string()));
        assert!(spec.subcommands.contains_key("compute"));
        assert!(spec.subcommands.contains_key("container"));
    }

    #[test]
    fn parse_az_yaml() {
        let spec: CompletionSpec =
            serde_yaml::from_str(BUILTIN_SPECS[8].1).expect("az.yaml should parse");
        assert!(spec.commands.contains(&"az".to_string()));
        assert!(spec.subcommands.contains_key("vm"));
        assert!(spec.subcommands.contains_key("aks"));
    }

    // ── Nested subcommand lookup ──────────────────────────────────────

    #[test]
    fn nix_flake_nested_subcommand_structure() {
        let spec: CompletionSpec =
            serde_yaml::from_str(BUILTIN_SPECS[1].1).expect("nix.yaml should parse");
        let flake = spec.subcommands.get("flake").expect("should have flake");
        assert!(!flake.description.is_empty());
        assert!(flake.subcommands.contains_key("update"));
        assert!(flake.subcommands.contains_key("check"));
        assert!(flake.subcommands.contains_key("show"));
        let update = flake.subcommands.get("update").unwrap();
        assert!(!update.description.is_empty());
    }

    // ── Spec subcommand field validation ──────────────────────────────

    #[test]
    fn all_builtin_subcommands_have_descriptions() {
        for (name, content) in BUILTIN_SPECS {
            let spec: CompletionSpec = serde_yaml::from_str(content)
                .unwrap_or_else(|e| panic!("failed to parse {name}: {e}"));
            for (sub_name, sub) in &spec.subcommands {
                assert!(
                    !sub.description.is_empty(),
                    "{name}: subcommand '{sub_name}' has no description"
                );
            }
        }
    }

    #[test]
    fn all_builtin_subcommands_have_glyphs() {
        for (name, content) in BUILTIN_SPECS {
            let spec: CompletionSpec = serde_yaml::from_str(content)
                .unwrap_or_else(|e| panic!("failed to parse {name}: {e}"));
            for (sub_name, sub) in &spec.subcommands {
                assert!(
                    !sub.glyph.is_empty(),
                    "{name}: subcommand '{sub_name}' has no glyph"
                );
            }
        }
    }

    // ── Lookup across all non-K8s specs ────────────────────────────────

    #[test]
    fn lookup_git_commit() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = reg.lookup("git", "commit");
        assert!(result.is_some());
        let (glyph, desc) = result.unwrap();
        assert_eq!(desc, "Record changes");
        assert!(!glyph.is_empty());
    }

    #[test]
    fn lookup_cargo_test() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = reg.lookup("cargo", "test");
        assert!(result.is_some());
        let (_, desc) = result.unwrap();
        assert_eq!(desc, "Run the tests");
    }

    #[test]
    fn lookup_npm_install() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = reg.lookup("npm", "install");
        assert!(result.is_some());
    }

    #[test]
    fn lookup_npm_alias_pnpm() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = reg.lookup("pnpm", "install");
        assert!(result.is_some());
    }

    #[test]
    fn lookup_npm_alias_yarn() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = reg.lookup("yarn", "install");
        assert!(result.is_some());
    }

    #[test]
    fn lookup_terraform_plan() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = reg.lookup("terraform", "plan");
        assert!(result.is_some());
        let (_, desc) = result.unwrap();
        assert_eq!(desc, "Show execution plan");
    }

    #[test]
    fn lookup_tofu_alias() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = reg.lookup("tofu", "apply");
        assert!(result.is_some());
        let (_, desc) = result.unwrap();
        assert_eq!(desc, "Apply changes");
    }

    // ── load_specs_from_dir: multiple valid files ──────────────────────

    #[test]
    fn load_specs_from_dir_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let yaml1 = "commands: [tool1]\nsubcommands:\n  sub1:\n    description: desc1\n    glyph: \">\"\n";
        let yaml2 = "commands: [tool2]\nsubcommands:\n  sub2:\n    description: desc2\n    glyph: \"<\"\n";
        std::fs::write(dir.path().join("a.yaml"), yaml1).unwrap();
        std::fs::write(dir.path().join("b.yml"), yaml2).unwrap();
        let specs = load_specs_from_dir(dir.path());
        assert_eq!(specs.len(), 2);
        let all_cmds: Vec<&str> = specs
            .iter()
            .flat_map(|s| s.commands.iter().map(String::as_str))
            .collect();
        assert!(all_cmds.contains(&"tool1"));
        assert!(all_cmds.contains(&"tool2"));
    }

    // ── Spec with flags ───────────────────────────────────────────────

    #[test]
    fn parse_spec_with_flags() {
        let yaml = r#"
commands: [mytool]
subcommands:
  run:
    description: Run something
    glyph: "▸"
    flags:
      --verbose: "Increase verbosity"
      --dry-run: "Preview without executing"
flags:
  --help: "Show help"
  --version: "Show version"
"#;
        let spec: CompletionSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.flags.len(), 2);
        assert!(spec.flags.contains_key("--help"));
        let run = spec.subcommands.get("run").unwrap();
        assert_eq!(run.flags.len(), 2);
        assert!(run.flags.contains_key("--verbose"));
    }

    // ── Empty/minimal YAML parses ─────────────────────────────────────

    #[test]
    fn parse_minimal_spec() {
        let yaml = "commands: [x]\n";
        let spec: CompletionSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.commands, vec!["x"]);
        assert!(spec.subcommands.is_empty());
        assert!(spec.flags.is_empty());
        assert!(spec.icon.is_none());
    }

    // ── Registry with empty specs ─────────────────────────────────────

    #[test]
    fn empty_registry_returns_none_for_all() {
        let reg = SpecRegistry { specs: vec![] };
        assert!(reg.lookup("anything", "anything").is_none());
        assert!(reg.icon("anything").is_none());
        assert!(!reg.is_k8s_command("kubectl"));
    }

    // ── Spec count validation ─────────────────────────────────────────

    #[test]
    fn builtin_specs_count_is_12() {
        assert_eq!(BUILTIN_SPECS.len(), 12);
    }

    #[test]
    fn builtin_spec_filenames_end_with_yaml() {
        for (name, _) in BUILTIN_SPECS {
            assert!(
                name.ends_with(".yaml"),
                "spec name {name} should end with .yaml"
            );
        }
    }
}
