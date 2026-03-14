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

    /// Look up (glyph, description) for a word under a command.
    ///
    /// Searches specs in reverse order so that later-loaded specs
    /// (project > user > built-in) take priority.
    pub fn lookup(&self, command: &str, word: &str) -> Option<(String, String)> {
        // Search in reverse so later specs (higher priority) win.
        for spec in self.specs.iter().rev() {
            if spec.commands.iter().any(|c| c == command) {
                if let Some(sub) = spec.subcommands.get(word) {
                    let glyph = if sub.glyph.is_empty() {
                        String::new()
                    } else {
                        sub.glyph.clone()
                    };
                    return Some((glyph, sub.description.clone()));
                }
            }
        }
        None
    }

    /// Get the prompt icon for a command from specs, or None.
    pub fn icon(&self, command: &str) -> Option<&str> {
        for spec in self.specs.iter().rev() {
            if spec.commands.iter().any(|c| c == command) {
                if let Some(ref icon) = spec.icon {
                    return Some(icon.as_str());
                }
            }
        }
        None
    }

    /// Get the global singleton registry.
    ///
    /// Initializes on first call using the provided config. Subsequent
    /// calls return the same registry regardless of config changes.
    pub fn global(specs_cfg: &config::SpecsConfig) -> &'static Self {
        GLOBAL_REGISTRY.get_or_init(|| Self::new(specs_cfg))
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
        // Should have at least the 2 built-in specs (docker + nix)
        assert!(reg.specs.len() >= 2);
    }

    #[test]
    fn lookup_docker_run() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = reg.lookup("docker", "run");
        assert!(result.is_some());
        let (glyph, desc) = result.unwrap();
        assert_eq!(desc, "Run a container");
        assert!(!glyph.is_empty());
    }

    #[test]
    fn lookup_podman_alias() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = reg.lookup("podman", "build");
        assert!(result.is_some());
        let (_, desc) = result.unwrap();
        assert_eq!(desc, "Build an image");
    }

    #[test]
    fn lookup_nix_build() {
        let reg = SpecRegistry::new(&default_specs_config());
        let result = reg.lookup("nix", "build");
        assert!(result.is_some());
        let (glyph, desc) = result.unwrap();
        assert_eq!(desc, "Build a derivation");
        assert!(!glyph.is_empty());
    }

    #[test]
    fn lookup_unknown_command() {
        let reg = SpecRegistry::new(&default_specs_config());
        assert!(reg.lookup("nonexistent-tool", "run").is_none());
    }

    #[test]
    fn lookup_unknown_word() {
        let reg = SpecRegistry::new(&default_specs_config());
        assert!(reg.lookup("docker", "nonexistent-subcommand").is_none());
    }

    #[test]
    fn icon_docker() {
        let reg = SpecRegistry::new(&default_specs_config());
        let icon = reg.icon("docker");
        assert!(icon.is_some());
    }

    #[test]
    fn icon_unknown_command() {
        let reg = SpecRegistry::new(&default_specs_config());
        assert!(reg.icon("unknown-tool").is_none());
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
        assert!(reg.lookup("docker", "run").is_none());
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
}
