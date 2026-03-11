//! Configuration — shikumi-based YAML config with env overrides.
//!
//! Config file: `~/.config/skim-tab/skim-tab.yaml`
//! Env override: `SKIM_TAB_CONFIG=/path/to/config.yaml`
//! Env prefix:   `SKIM_TAB_` (e.g. `SKIM_TAB_COMPLETION__MODE=hybrid`)
//!
//! # Completion modes
//!
//! - **direct** (default): Polls live sources (kubectl, history, fs) on each
//!   completion. This is the current behavior — zero external dependencies.
//!
//! - **service**: Completes exclusively from a gRPC indexing service. If the
//!   service is unavailable, completions are empty (no fallback). Use when
//!   the service is guaranteed to be running.
//!
//! - **hybrid**: Tries the gRPC service first; falls back to direct polling
//!   if the service is unreachable or times out. Best of both worlds.

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

/// Controls how completion candidates are sourced and enriched.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompletionConfig {
    /// Completion source mode: `direct`, `service`, or `hybrid`.
    pub mode: CompletionMode,

    /// gRPC service settings (used in `service` and `hybrid` modes).
    pub service: ServiceConfig,

    /// Direct-mode settings (kubectl enrichment, etc.).
    pub direct: DirectConfig,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            mode: CompletionMode::Direct,
            service: ServiceConfig::default(),
            direct: DirectConfig::default(),
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

// ── Service config ──────────────────────────────────────────────────

/// gRPC completion service connection settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceConfig {
    /// gRPC endpoint (e.g. `http://127.0.0.1:50051`).
    pub endpoint: String,

    /// Connection timeout in milliseconds. In hybrid mode, exceeding
    /// this timeout triggers fallback to direct polling.
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

// ── Direct config ───────────────────────────────────────────────────

/// Settings for direct (local subprocess) enrichment.
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
    fn default_service_endpoint() {
        let cfg = Config::default();
        assert_eq!(cfg.completion.service.endpoint, "http://127.0.0.1:50051");
        assert_eq!(cfg.completion.service.timeout_ms, 200);
    }

    #[test]
    fn default_direct_k8s_enabled() {
        let cfg = Config::default();
        assert!(cfg.completion.direct.k8s_enrichment);
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
        // Service config should still have defaults
        assert_eq!(cfg.completion.service.endpoint, "http://127.0.0.1:50051");
        assert!(cfg.completion.direct.k8s_enrichment);
    }
}
