//! Configuration — shikumi-based YAML config with env overrides.
//!
//! Config file: `~/.config/skim-tab/skim-tab.yaml`
//! Env override: `SKIM_TAB_CONFIG=/path/to/config.yaml`
//! Env prefix:   `SKIM_TAB_` (e.g. `SKIM_TAB_COMPLETION__MODE=hybrid`)
//!
//! # Feature flags
//!
//! All new features are gated behind config flags, defaulting to off.
//! Enable them individually as they mature:
//!
//! ```yaml
//! completion:
//!   mode: direct
//!   in_picker_descent: false       # in-picker readdir loop (vs tab-dance)
//!   single_auto_select: true       # auto-select when 1 candidate (skip skim)
//!   preview:
//!     enable: true                 # show preview pane
//!     directories: true            # preview dir contents (eza/ls)
//!     files: true                  # preview file contents (bat/cat)
//!     max_lines: 20                # preview line limit
//!   picker:
//!     height: "40%"                # skim picker height
//!     cycle: true                  # wrap around at top/bottom
//!     sort: false                  # preserve completion order (no re-sort)
//!     group_colors: true           # colorize completion groups differently
//!     min_candidates: 2            # threshold to show picker (below = auto-insert all)
//!     multi_select: false          # enable tab multi-select in picker
//!     show_group_header: true      # show group count info in picker header
//!     continuous_trigger: "/"      # key to descend into dirs in-picker (empty=disable)
//!     accept_execute_key: ""       # key for select+execute (e.g. "ctrl-x", empty=disable)
//!   dir_handling:
//!     append_slash: true           # append / to directory words
//!     skip_trailing_space: true    # no space after dirs (enables tab-dance)
//!   enrichment:
//!     lscolors: true               # colorize file candidates via LS_COLORS
//!     descriptions: true           # add command/subcommand descriptions
//!     k8s_live: true               # live kubectl resource counts
//!     project_detection: true      # detect project type from CWD markers
//!     history_boost: false         # SQLite selection history (opt-in)
//!     frecency: false              # frecency-based candidate reordering (opt-in)
//! ```

use serde::{Deserialize, Serialize};
use shikumi::{ConfigDiscovery, Format, ProviderChain};

// ── Top-level config ────────────────────────────────────────────────

/// Root configuration for skim-tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub completion: CompletionConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            completion: CompletionConfig::default(),
        }
    }
}

// ── Completion config ───────────────────────────────────────────────

/// Controls how completion candidates are sourced, displayed, and applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompletionConfig {
    /// Completion source mode: `direct`, `service`, or `hybrid`.
    pub mode: CompletionMode,

    /// gRPC service settings (used in `service` and `hybrid` modes).
    pub service: ServiceConfig,

    /// Direct-mode settings (kubectl enrichment, etc.).
    pub direct: DirectConfig,

    /// In-picker directory descent: selecting a directory loops in-process
    /// with readdir + new skim session. When false (default), directories
    /// return to zsh with trailing / for the tab-dance pattern.
    pub in_picker_descent: bool,

    /// Auto-select when exactly one candidate matches (skip skim picker).
    /// When false, always show the picker even for single matches.
    pub single_auto_select: bool,

    /// Preview pane configuration.
    pub preview: PreviewConfig,

    /// Skim picker appearance and behavior.
    pub picker: PickerConfig,

    /// Directory handling in selections.
    pub dir_handling: DirHandlingConfig,

    /// Candidate enrichment (colors, descriptions, live data).
    pub enrichment: EnrichmentConfig,

    /// YAML completion spec configuration.
    pub specs: SpecsConfig,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            mode: CompletionMode::Direct,
            service: ServiceConfig::default(),
            direct: DirectConfig::default(),
            in_picker_descent: false,
            single_auto_select: true,
            preview: PreviewConfig::default(),
            picker: PickerConfig::default(),
            dir_handling: DirHandlingConfig::default(),
            enrichment: EnrichmentConfig::default(),
            specs: SpecsConfig::default(),
        }
    }
}

// ── Completion mode ─────────────────────────────────────────────────

/// How completion candidates are sourced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompletionMode {
    /// Poll live sources directly (kubectl, fs, history). Default.
    Direct,
    /// Complete from gRPC indexing service only.
    Service,
    /// Try gRPC first, fall back to direct polling if unavailable.
    Hybrid,
}

impl CompletionMode {
    /// Whether direct (local) enrichment should run.
    pub fn use_direct(self) -> bool {
        matches!(self, Self::Direct | Self::Hybrid)
    }

    /// Whether the gRPC service should be queried.
    pub fn use_service(self) -> bool {
        matches!(self, Self::Service | Self::Hybrid)
    }
}

// ── Preview config ──────────────────────────────────────────────────

/// Preview pane for completion candidates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PreviewConfig {
    /// Enable preview pane in the skim picker.
    pub enable: bool,

    /// Preview directory contents (via eza or ls).
    pub directories: bool,

    /// Preview file contents (via bat or cat).
    pub files: bool,

    /// Maximum lines to show in preview.
    pub max_lines: usize,

    /// Preview window layout (e.g., "right:50%:wrap").
    pub layout: String,
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self {
            enable: true,
            directories: true,
            files: true,
            max_lines: 20,
            layout: "right:50%:wrap".to_string(),
        }
    }
}

// ── Picker config ───────────────────────────────────────────────────

/// Skim picker appearance and behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PickerConfig {
    /// Picker height (e.g., "40%", "20", "~50%").
    pub height: String,

    /// Wrap around at top/bottom of candidate list.
    pub cycle: bool,

    /// Preserve zsh completion order (no re-sorting by skim).
    pub no_sort: bool,

    /// Colorize completion groups with different accents.
    pub group_colors: bool,

    /// Minimum candidates required to show the skim picker.
    /// Below this threshold (but above 1), all candidates are auto-inserted
    /// (like single_auto_select but for small counts). Default: 2.
    pub min_candidates: usize,

    /// Enable multi-select in the skim picker (tab to mark items).
    /// When true, multiple selections are batch-inserted.
    pub multi_select: bool,

    /// Show group count info in the skim header when candidates have groups.
    /// e.g., "3 groups: files, flags, subcommands"
    pub show_group_header: bool,

    /// Trigger character for continuous directory descent inside the picker.
    /// When non-empty (default: "/"), pressing this key on a directory
    /// candidate immediately descends into that directory without leaving
    /// the picker. The key is bound to skim's `accept` action so that
    /// skim closes and skim-tab detects the trigger via `final_key`.
    /// Set to empty string to disable.
    pub continuous_trigger: String,

    /// Key that selects AND signals immediate execution (e.g., "ctrl-x").
    /// When the user presses this key, the selection is applied and the
    /// zsh widget calls `zle accept-line` to execute the command immediately.
    /// The eval output includes an "x" flag in the 8th field when triggered.
    /// Set to empty string to disable (default).
    pub accept_execute_key: String,
}

impl Default for PickerConfig {
    fn default() -> Self {
        Self {
            height: "40%".to_string(),
            cycle: true,
            no_sort: true,
            group_colors: true,
            min_candidates: 2,
            multi_select: false,
            show_group_header: true,
            continuous_trigger: "/".to_string(),
            accept_execute_key: String::new(),
        }
    }
}

// ── Directory handling config ───────────────────────────────────────

/// Controls how directory selections are handled.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DirHandlingConfig {
    /// Append `/` to directory words (enables path-aware tab-dance).
    pub append_slash: bool,

    /// Skip trailing space for directories (cursor stays after `/`).
    pub skip_trailing_space: bool,
}

impl Default for DirHandlingConfig {
    fn default() -> Self {
        Self {
            append_slash: true,
            skip_trailing_space: true,
        }
    }
}

// ── Enrichment config ───────────────────────────────────────────────

/// Controls candidate enrichment (colors, descriptions, live data).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EnrichmentConfig {
    /// Colorize file candidates via LS_COLORS / lscolors.
    pub lscolors: bool,

    /// Add command/subcommand descriptions from the built-in registry.
    pub descriptions: bool,

    /// Enable live kubectl resource counts for K8s completions.
    pub k8s_live: bool,

    /// Detect project type from CWD marker files (Cargo.toml, package.json, etc.).
    pub project_detection: bool,

    /// Record selections and boost candidates via SQLite history (opt-in).
    pub history_boost: bool,

    /// Reorder candidates by frecency score before display (opt-in, requires history_boost).
    pub frecency: bool,
}

impl Default for EnrichmentConfig {
    fn default() -> Self {
        Self {
            lscolors: true,
            descriptions: true,
            k8s_live: true,
            project_detection: true,
            history_boost: false,
            frecency: false,
        }
    }
}

// ── Specs config ────────────────────────────────────────────────────

/// YAML completion spec loading configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpecsConfig {
    /// Enable YAML spec loading (built-in + user + project specs).
    pub enable: bool,

    /// Directories to load user spec YAML files from.
    /// Supports `~` expansion. Default: `["~/.config/skim-tab/specs"]`.
    pub dirs: Vec<String>,

    /// Load project-local specs from `.skim-tab/specs/` in CWD.
    pub project_specs: bool,
}

impl Default for SpecsConfig {
    fn default() -> Self {
        Self {
            enable: true,
            dirs: vec!["~/.config/skim-tab/specs".to_string()],
            project_specs: true,
        }
    }
}

// ── Service config ──────────────────────────────────────────────────

/// gRPC completion service connection settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceConfig {
    /// gRPC endpoint (e.g. `http://127.0.0.1:50051`).
    pub endpoint: String,

    /// Connection timeout in milliseconds.
    pub timeout_ms: u64,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:50051".to_string(),
            timeout_ms: 200,
        }
    }
}

// ── Direct config (legacy — preserved for backward compat) ──────────

/// Settings for direct (local subprocess) enrichment.
/// Prefer `enrichment.k8s_live` for new configs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DirectConfig {
    /// Enable live kubectl calls for resource count enrichment.
    pub k8s_enrichment: bool,
}

impl Default for DirectConfig {
    fn default() -> Self {
        Self {
            k8s_enrichment: true,
        }
    }
}

// ── Loading ─────────────────────────────────────────────────────────

/// Load config using shikumi discovery + provider chain.
///
/// Layers (later wins): defaults → config file → env vars.
/// Missing config file is fine — defaults are always valid.
pub fn load() -> Config {
    let path = ConfigDiscovery::new("skim-tab")
        .env_override("SKIM_TAB_CONFIG")
        .formats(&[Format::Yaml])
        .discover();

    let mut chain = ProviderChain::new().with_defaults(&Config::default());

    if let Ok(ref p) = path {
        chain = chain.with_file(p);
    }

    chain = chain.with_env("SKIM_TAB_");

    chain.extract().unwrap_or_default()
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_direct_mode() {
        let cfg = Config::default();
        assert_eq!(cfg.completion.mode, CompletionMode::Direct);
        assert!(cfg.completion.mode.use_direct());
        assert!(!cfg.completion.mode.use_service());
    }

    #[test]
    fn hybrid_mode_uses_both() {
        assert!(CompletionMode::Hybrid.use_direct());
        assert!(CompletionMode::Hybrid.use_service());
    }

    #[test]
    fn service_mode_no_direct() {
        assert!(!CompletionMode::Service.use_direct());
        assert!(CompletionMode::Service.use_service());
    }

    #[test]
    fn default_feature_flags() {
        let cfg = Config::default();
        assert!(cfg.completion.single_auto_select);
        assert!(!cfg.completion.in_picker_descent);
        assert!(cfg.completion.preview.enable);
        assert!(cfg.completion.dir_handling.append_slash);
        assert!(cfg.completion.dir_handling.skip_trailing_space);
        assert!(cfg.completion.enrichment.lscolors);
        assert!(cfg.completion.enrichment.descriptions);
        assert!(cfg.completion.enrichment.k8s_live);
        assert!(cfg.completion.picker.cycle);
        assert!(cfg.completion.picker.no_sort);
        assert_eq!(cfg.completion.picker.min_candidates, 2);
        assert!(!cfg.completion.picker.multi_select);
        assert!(cfg.completion.picker.show_group_header);
        assert_eq!(cfg.completion.picker.continuous_trigger, "/");
        assert!(cfg.completion.picker.accept_execute_key.is_empty());
    }

    #[test]
    fn deserialize_yaml_mode() {
        let yaml = "completion:\n  mode: hybrid\n";
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.completion.mode, CompletionMode::Hybrid);
    }

    #[test]
    fn deserialize_yaml_service() {
        let yaml = r#"
completion:
  mode: service
  service:
    endpoint: "http://10.0.0.1:9090"
    timeout_ms: 500
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.completion.mode, CompletionMode::Service);
        assert_eq!(cfg.completion.service.endpoint, "http://10.0.0.1:9090");
        assert_eq!(cfg.completion.service.timeout_ms, 500);
    }

    #[test]
    fn deserialize_feature_flags() {
        let yaml = r#"
completion:
  single_auto_select: false
  in_picker_descent: true
  preview:
    enable: false
    max_lines: 50
  picker:
    height: "60%"
    cycle: false
  dir_handling:
    append_slash: false
  enrichment:
    k8s_live: false
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(!cfg.completion.single_auto_select);
        assert!(cfg.completion.in_picker_descent);
        assert!(!cfg.completion.preview.enable);
        assert_eq!(cfg.completion.preview.max_lines, 50);
        assert_eq!(cfg.completion.picker.height, "60%");
        assert!(!cfg.completion.picker.cycle);
        assert!(!cfg.completion.dir_handling.append_slash);
        assert!(!cfg.completion.enrichment.k8s_live);
        // Unset fields preserve defaults
        assert!(cfg.completion.dir_handling.skip_trailing_space);
        assert!(cfg.completion.enrichment.lscolors);
        // New picker fields preserve defaults when unset
        assert_eq!(cfg.completion.picker.min_candidates, 2);
        assert!(!cfg.completion.picker.multi_select);
        assert!(cfg.completion.picker.show_group_header);
    }

    #[test]
    fn deserialize_yaml_direct_disable_k8s() {
        let yaml = "completion:\n  direct:\n    k8s_enrichment: false\n";
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(!cfg.completion.direct.k8s_enrichment);
    }

    #[test]
    fn load_returns_defaults_without_config_file() {
        let cfg = load();
        assert_eq!(cfg.completion.mode, CompletionMode::Direct);
    }

    #[test]
    fn partial_yaml_preserves_defaults() {
        let yaml = "completion:\n  mode: hybrid\n";
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.completion.service.endpoint, "http://127.0.0.1:50051");
        assert!(cfg.completion.direct.k8s_enrichment);
        assert!(cfg.completion.single_auto_select);
        assert!(cfg.completion.preview.enable);
    }

    #[test]
    fn deserialize_picker_new_fields() {
        let yaml = r#"
completion:
  picker:
    min_candidates: 5
    multi_select: true
    show_group_header: false
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.completion.picker.min_candidates, 5);
        assert!(cfg.completion.picker.multi_select);
        assert!(!cfg.completion.picker.show_group_header);
        // Other picker defaults preserved
        assert!(cfg.completion.picker.cycle);
        assert!(cfg.completion.picker.no_sort);
        assert_eq!(cfg.completion.picker.height, "40%");
        // R2a/R6 defaults preserved when not set
        assert_eq!(cfg.completion.picker.continuous_trigger, "/");
        assert!(cfg.completion.picker.accept_execute_key.is_empty());
    }

    #[test]
    fn deserialize_picker_r2a_r6_fields() {
        let yaml = r#"
completion:
  picker:
    continuous_trigger: ""
    accept_execute_key: "ctrl-x"
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.completion.picker.continuous_trigger.is_empty());
        assert_eq!(cfg.completion.picker.accept_execute_key, "ctrl-x");
        assert!(cfg.completion.picker.cycle);
        assert_eq!(cfg.completion.picker.min_candidates, 2);
    }

    // ── CompletionMode exhaustive ────────────────────────────────────

    #[test]
    fn direct_mode_properties() {
        assert!(CompletionMode::Direct.use_direct());
        assert!(!CompletionMode::Direct.use_service());
    }

    #[test]
    fn completion_mode_serialization_roundtrip() {
        for mode_str in ["direct", "service", "hybrid"] {
            let yaml = format!("completion:\n  mode: {mode_str}\n");
            let cfg: Config = serde_yaml::from_str(&yaml).unwrap();
            let serialized = serde_yaml::to_string(&cfg).unwrap();
            assert!(serialized.contains(mode_str));
        }
    }

    // ── Default values for all nested configs ────────────────────────

    #[test]
    fn preview_config_defaults() {
        let cfg = PreviewConfig::default();
        assert!(cfg.enable);
        assert!(cfg.directories);
        assert!(cfg.files);
        assert_eq!(cfg.max_lines, 20);
        assert_eq!(cfg.layout, "right:50%:wrap");
    }

    #[test]
    fn service_config_defaults() {
        let cfg = ServiceConfig::default();
        assert_eq!(cfg.endpoint, "http://127.0.0.1:50051");
        assert_eq!(cfg.timeout_ms, 200);
    }

    #[test]
    fn specs_config_defaults() {
        let cfg = SpecsConfig::default();
        assert!(cfg.enable);
        assert!(cfg.project_specs);
        assert!(!cfg.dirs.is_empty());
        assert!(cfg.dirs[0].contains("skim-tab"));
    }

    #[test]
    fn enrichment_config_defaults() {
        let cfg = EnrichmentConfig::default();
        assert!(cfg.lscolors);
        assert!(cfg.descriptions);
        assert!(cfg.k8s_live);
        assert!(cfg.project_detection);
        assert!(!cfg.history_boost);
        assert!(!cfg.frecency);
    }

    #[test]
    fn dir_handling_config_defaults() {
        let cfg = DirHandlingConfig::default();
        assert!(cfg.append_slash);
        assert!(cfg.skip_trailing_space);
    }

    #[test]
    fn direct_config_defaults() {
        let cfg = DirectConfig::default();
        assert!(cfg.k8s_enrichment);
    }

    // ── Deserialize enrichment fields ────────────────────────────────

    #[test]
    fn deserialize_enrichment_all_flags() {
        let yaml = r#"
completion:
  enrichment:
    lscolors: false
    descriptions: false
    k8s_live: false
    project_detection: false
    history_boost: true
    frecency: true
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(!cfg.completion.enrichment.lscolors);
        assert!(!cfg.completion.enrichment.descriptions);
        assert!(!cfg.completion.enrichment.k8s_live);
        assert!(!cfg.completion.enrichment.project_detection);
        assert!(cfg.completion.enrichment.history_boost);
        assert!(cfg.completion.enrichment.frecency);
    }

    // ── Deserialize specs config ─────────────────────────────────────

    #[test]
    fn deserialize_specs_config() {
        let yaml = r#"
completion:
  specs:
    enable: false
    dirs: ["/custom/specs"]
    project_specs: false
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(!cfg.completion.specs.enable);
        assert_eq!(cfg.completion.specs.dirs, vec!["/custom/specs"]);
        assert!(!cfg.completion.specs.project_specs);
    }

    // ── Deserialize preview layout ───────────────────────────────────

    #[test]
    fn deserialize_preview_config() {
        let yaml = r#"
completion:
  preview:
    enable: false
    directories: false
    files: false
    max_lines: 100
    layout: "down:30%"
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(!cfg.completion.preview.enable);
        assert!(!cfg.completion.preview.directories);
        assert!(!cfg.completion.preview.files);
        assert_eq!(cfg.completion.preview.max_lines, 100);
        assert_eq!(cfg.completion.preview.layout, "down:30%");
    }

    // ── Empty YAML deserializes to defaults ──────────────────────────

    #[test]
    fn empty_yaml_uses_defaults() {
        let cfg: Config = serde_yaml::from_str("{}").unwrap();
        assert_eq!(cfg.completion.mode, CompletionMode::Direct);
        assert!(cfg.completion.single_auto_select);
    }

    // ── Invalid mode should fail ─────────────────────────────────────

    #[test]
    fn deserialize_invalid_mode_fails() {
        let yaml = "completion:\n  mode: invalid_mode\n";
        let result = serde_yaml::from_str::<Config>(yaml);
        assert!(result.is_err());
    }
}
