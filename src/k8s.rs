//! Kubernetes context introspection for completion enrichment.
//!
//! Parses kubeconfig directly (no subprocess) for context/namespace/cluster.
//! Shells out to kubectl only for live resource counts.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

// ── Kubeconfig types (serde_yaml) ────────────────────────────────────

#[derive(Deserialize)]
struct KubeConfig {
    #[serde(rename = "current-context", default)]
    current_context: String,
    #[serde(default)]
    contexts: Vec<ContextEntry>,
}

#[derive(Deserialize)]
struct ContextEntry {
    name: String,
    #[serde(default)]
    context: ContextData,
}

#[derive(Deserialize, Default)]
struct ContextData {
    #[serde(default)]
    cluster: String,
    #[serde(default)]
    namespace: String,
}

// ── Public context ───────────────────────────────────────────────────

/// Parsed kubeconfig — the active context, namespace, and cluster.
pub struct KubeContext {
    pub context: String,
    pub namespace: String,
    pub cluster: String,
}

impl KubeContext {
    /// Parse the current kubectl context directly from kubeconfig YAML.
    /// Zero subprocess calls — pure file I/O (~0ms).
    pub fn current() -> Option<Self> {
        let paths = kubeconfig_paths();
        let config = paths.iter().find_map(|p| {
            std::fs::read_to_string(p)
                .ok()
                .and_then(|s| serde_yaml::from_str::<KubeConfig>(&s).ok())
        })?;

        let ctx_entry = config
            .contexts
            .iter()
            .find(|c| c.name == config.current_context)?;

        let namespace = if ctx_entry.context.namespace.is_empty() {
            "default".to_string()
        } else {
            ctx_entry.context.namespace.clone()
        };

        Some(Self {
            context: config.current_context,
            namespace,
            cluster: ctx_entry.context.cluster.clone(),
        })
    }

    /// Format header string for skim display.
    pub fn header(&self) -> String {
        use crate::{ANSI_DIM, ANSI_FROST, ANSI_GREEN, ANSI_RESET};

        let mut parts = Vec::with_capacity(3);
        parts.push(format!(
            "{ANSI_DIM}ctx:{ANSI_RESET} {ANSI_FROST}{}{ANSI_RESET}",
            self.context
        ));
        parts.push(format!(
            "{ANSI_DIM}ns:{ANSI_RESET} {ANSI_GREEN}{}{ANSI_RESET}",
            self.namespace
        ));
        if !self.cluster.is_empty() && self.cluster != self.context {
            parts.push(format!(
                "{ANSI_DIM}cluster:{ANSI_RESET} {ANSI_FROST}{}{ANSI_RESET}",
                self.cluster
            ));
        }
        parts.join("  ")
    }

    /// Format prompt string with truncated context name.
    pub fn prompt(&self) -> String {
        let name = match self.context.char_indices().nth(15) {
            Some((idx, _)) => &self.context[..idx],
            None => &self.context,
        };
        format!("{}{name} ", crate::ICON_K8S)
    }
}

/// Resolve kubeconfig file paths from `$KUBECONFIG` or default location.
fn kubeconfig_paths() -> Vec<PathBuf> {
    if let Ok(val) = std::env::var("KUBECONFIG") {
        return val.split(':').map(PathBuf::from).collect();
    }
    if let Some(home) = std::env::var_os("HOME") {
        return vec![PathBuf::from(home).join(".kube/config")];
    }
    vec![]
}

// ── Resource counts ──────────────────────────────────────────────────

/// Count resources by type via a single `kubectl get` call.
/// Returns a map from plural type name (e.g., "pods") to count.
pub fn resource_counts(types: &[&str], namespace: Option<&str>) -> HashMap<String, usize> {
    if types.is_empty() {
        return HashMap::new();
    }

    let type_list = types.join(",");
    let mut args = vec!["get", &type_list, "--no-headers", "-o", "name"];
    if let Some(ns) = namespace {
        args.extend_from_slice(&["-n", ns]);
    }

    let output = Command::new("kubectl")
        .args(&args)
        .stderr(std::process::Stdio::null())
        .output();

    let stdout = match output {
        Ok(ref o) if o.status.success() => String::from_utf8_lossy(&o.stdout),
        _ => return HashMap::new(),
    };

    // Lines look like: "pod/nginx-xxx" or "deployment.apps/nginx"
    let mut counts: HashMap<String, usize> = HashMap::new();
    for line in stdout.lines() {
        if let Some(api_type) = line.split('/').next() {
            let plural = api_type_to_plural(api_type);
            *counts.entry(plural).or_insert(0) += 1;
        }
    }
    counts
}

/// Map kubectl API type prefix (from `-o name`) to the plural form
/// used by completion candidates.
fn api_type_to_plural(api_type: &str) -> String {
    let base = api_type.split('.').next().unwrap_or(api_type);
    match base {
        "pod" => "pods",
        "service" => "services",
        "deployment" => "deployments",
        "replicaset" => "replicasets",
        "statefulset" => "statefulsets",
        "daemonset" => "daemonsets",
        "job" => "jobs",
        "cronjob" => "cronjobs",
        "configmap" => "configmaps",
        "secret" => "secrets",
        "ingress" => "ingresses",
        "namespace" => "namespaces",
        "node" => "nodes",
        "persistentvolumeclaim" => "persistentvolumeclaims",
        "persistentvolume" => "persistentvolumes",
        "serviceaccount" => "serviceaccounts",
        "role" => "roles",
        "clusterrole" => "clusterroles",
        "rolebinding" => "rolebindings",
        "clusterrolebinding" => "clusterrolebindings",
        "networkpolicy" => "networkpolicies",
        "storageclass" => "storageclasses",
        "event" => "events",
        "endpoints" => "endpoints",
        "horizontalpodautoscaler" => "horizontalpodautoscalers",
        "poddisruptionbudget" => "poddisruptionbudgets",
        "limitrange" => "limitranges",
        "resourcequota" => "resourcequotas",
        "customresourcedefinition" => "customresourcedefinitions",
        other => return format!("{other}s"),
    }
    .to_string()
}

// ── Namespace pod counts ─────────────────────────────────────────────

/// Count pods per namespace via a single `kubectl get pods -A` call.
pub fn namespace_pod_counts() -> HashMap<String, usize> {
    let output = Command::new("kubectl")
        .args([
            "get",
            "pods",
            "-A",
            "--no-headers",
            "-o",
            "custom-columns=NS:.metadata.namespace",
        ])
        .stderr(std::process::Stdio::null())
        .output();

    let stdout = match output {
        Ok(ref o) if o.status.success() => String::from_utf8_lossy(&o.stdout),
        _ => return HashMap::new(),
    };

    let mut counts: HashMap<String, usize> = HashMap::new();
    for line in stdout.lines() {
        let ns = line.trim();
        if !ns.is_empty() {
            *counts.entry(ns.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kubeconfig_paths_from_env() {
        std::env::set_var("KUBECONFIG", "/tmp/a:/tmp/b");
        let paths = kubeconfig_paths();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], PathBuf::from("/tmp/a"));
        assert_eq!(paths[1], PathBuf::from("/tmp/b"));
        std::env::remove_var("KUBECONFIG");
    }

    #[test]
    fn api_type_to_plural_common() {
        assert_eq!(api_type_to_plural("pod"), "pods");
        assert_eq!(api_type_to_plural("deployment.apps"), "deployments");
        assert_eq!(api_type_to_plural("service"), "services");
        assert_eq!(api_type_to_plural("ingress"), "ingresses");
        assert_eq!(api_type_to_plural("cronjob.batch"), "cronjobs");
    }

    #[test]
    fn api_type_to_plural_unknown() {
        assert_eq!(api_type_to_plural("widget"), "widgets");
        assert_eq!(api_type_to_plural("foobar.custom.io"), "foobars");
    }

    #[test]
    fn parse_kubeconfig_yaml() {
        let yaml = r#"
apiVersion: v1
kind: Config
current-context: plo
contexts:
  - name: plo
    context:
      cluster: plo-cluster
      namespace: lilitu
clusters:
  - name: plo-cluster
    cluster:
      server: https://10.0.0.1:6443
"#;
        let config: KubeConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.current_context, "plo");
        assert_eq!(config.contexts[0].context.namespace, "lilitu");
        assert_eq!(config.contexts[0].context.cluster, "plo-cluster");
    }

    #[test]
    fn parse_kubeconfig_default_namespace() {
        let yaml = r#"
apiVersion: v1
kind: Config
current-context: test
contexts:
  - name: test
    context:
      cluster: test-cluster
"#;
        let config: KubeConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.contexts[0].context.namespace.is_empty());
    }

    #[test]
    fn kube_context_header_format() {
        let ctx = KubeContext {
            context: "plo".to_string(),
            namespace: "default".to_string(),
            cluster: "plo-cluster".to_string(),
        };
        let header = ctx.header();
        let plain = crate::strip_ansi(&header);
        assert!(plain.contains("ctx: plo"));
        assert!(plain.contains("ns: default"));
        assert!(plain.contains("cluster: plo-cluster"));
    }

    #[test]
    fn kube_context_header_hides_redundant_cluster() {
        let ctx = KubeContext {
            context: "plo".to_string(),
            namespace: "default".to_string(),
            cluster: "plo".to_string(), // same as context
        };
        let header = ctx.header();
        let plain = crate::strip_ansi(&header);
        assert!(!plain.contains("cluster:"));
    }

    #[test]
    fn kube_context_prompt_truncates() {
        let ctx = KubeContext {
            context: "very-long-context-name-that-exceeds-limit".to_string(),
            namespace: "default".to_string(),
            cluster: String::new(),
        };
        let prompt = ctx.prompt();
        // Should contain the truncated name (15 chars)
        assert!(prompt.contains("very-long-conte"));
        assert!(!prompt.contains("very-long-context-name"));
    }

    #[test]
    fn kube_context_prompt_short_name() {
        let ctx = KubeContext {
            context: "plo".to_string(),
            namespace: "default".to_string(),
            cluster: String::new(),
        };
        let prompt = ctx.prompt();
        assert!(prompt.contains("plo"));
    }
}
