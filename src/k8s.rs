//! Kubernetes context introspection for completion enrichment.
//!
//! Parses kubeconfig directly (no subprocess) for context/namespace/cluster.
//! Shells out to kubectl only for live resource counts.

use serde::Deserialize;
use std::borrow::Cow;
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

// ── Loader trait ─────────────────────────────────────────────────────

/// Abstraction over kubeconfig loading for testability.
/// Returns an `Option<KubeContext>` directly since `KubeConfig` is private.
pub trait KubeconfigLoader: Send + Sync {
    fn load(&self) -> Option<KubeContext>;
}

/// Loads kubeconfig from the filesystem (the production path).
pub struct FsKubeconfigLoader;

impl KubeconfigLoader for FsKubeconfigLoader {
    fn load(&self) -> Option<KubeContext> {
        let paths = kubeconfig_paths();
        let config = paths.iter().find_map(|p| {
            std::fs::read_to_string(p)
                .ok()
                .and_then(|s| serde_yaml::from_str::<KubeConfig>(&s).ok())
        })?;

        kube_context_from_config(&config)
    }
}

/// Extract a `KubeContext` from a parsed `KubeConfig`.
fn kube_context_from_config(config: &KubeConfig) -> Option<KubeContext> {
    let ctx_entry = config
        .contexts
        .iter()
        .find(|c| c.name == config.current_context)?;

    let namespace = if ctx_entry.context.namespace.is_empty() {
        "default".to_string()
    } else {
        ctx_entry.context.namespace.clone()
    };

    Some(KubeContext {
        context: config.current_context.clone(),
        namespace,
        cluster: ctx_entry.context.cluster.clone(),
    })
}

// ── Kubectl runner trait ─────────────────────────────────────────────

/// Abstraction over kubectl subprocess calls for testability.
pub trait KubectlRunner: Send + Sync {
    fn run(&self, args: &[&str]) -> Option<String>;
}

/// Runs kubectl as a real subprocess (the production path).
pub struct RealKubectlRunner;

impl KubectlRunner for RealKubectlRunner {
    fn run(&self, args: &[&str]) -> Option<String> {
        let output = Command::new("kubectl")
            .args(args)
            .stderr(std::process::Stdio::null())
            .output();

        match output {
            Ok(ref o) if o.status.success() => {
                Some(String::from_utf8_lossy(&o.stdout).into_owned())
            }
            _ => None,
        }
    }
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
    /// Delegates to `FsKubeconfigLoader`.
    pub fn current() -> Option<Self> {
        FsKubeconfigLoader.load()
    }

    /// Parse the current kubectl context using a custom loader (for testing).
    pub fn with_loader(loader: &dyn KubeconfigLoader) -> Option<Self> {
        loader.load()
    }

    /// Format header string for skim display.
    #[must_use]
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
    #[must_use]
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
#[must_use]
pub fn resource_counts(types: &[&str], namespace: Option<&str>) -> HashMap<String, usize> {
    resource_counts_with(&RealKubectlRunner, types, namespace)
}

/// Count resources by type using a custom `KubectlRunner`.
#[must_use]
pub fn resource_counts_with(
    runner: &dyn KubectlRunner,
    types: &[&str],
    namespace: Option<&str>,
) -> HashMap<String, usize> {
    if types.is_empty() {
        return HashMap::new();
    }

    let type_list = types.join(",");
    let mut args = vec!["get", &type_list, "--no-headers", "-o", "name"];
    if let Some(ns) = namespace {
        args.extend_from_slice(&["-n", ns]);
    }

    let stdout = match runner.run(&args) {
        Some(s) => s,
        None => return HashMap::new(),
    };

    parse_resource_counts(&stdout)
}

/// Parse `kubectl get -o name` output into resource type counts.
fn parse_resource_counts(stdout: &str) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for line in stdout.lines() {
        if let Some(api_type) = line.split('/').next() {
            let plural = api_type_to_plural(api_type);
            *counts.entry(plural.into_owned()).or_insert(0) += 1;
        }
    }
    counts
}

/// Map kubectl API type prefix (from `-o name`) to the plural form
/// used by completion candidates.
#[must_use]
fn api_type_to_plural(api_type: &str) -> Cow<'static, str> {
    let base = api_type.split('.').next().unwrap_or(api_type);
    match base {
        "pod" => Cow::Borrowed("pods"),
        "service" => Cow::Borrowed("services"),
        "deployment" => Cow::Borrowed("deployments"),
        "replicaset" => Cow::Borrowed("replicasets"),
        "statefulset" => Cow::Borrowed("statefulsets"),
        "daemonset" => Cow::Borrowed("daemonsets"),
        "job" => Cow::Borrowed("jobs"),
        "cronjob" => Cow::Borrowed("cronjobs"),
        "configmap" => Cow::Borrowed("configmaps"),
        "secret" => Cow::Borrowed("secrets"),
        "ingress" => Cow::Borrowed("ingresses"),
        "namespace" => Cow::Borrowed("namespaces"),
        "node" => Cow::Borrowed("nodes"),
        "persistentvolumeclaim" => Cow::Borrowed("persistentvolumeclaims"),
        "persistentvolume" => Cow::Borrowed("persistentvolumes"),
        "serviceaccount" => Cow::Borrowed("serviceaccounts"),
        "role" => Cow::Borrowed("roles"),
        "clusterrole" => Cow::Borrowed("clusterroles"),
        "rolebinding" => Cow::Borrowed("rolebindings"),
        "clusterrolebinding" => Cow::Borrowed("clusterrolebindings"),
        "networkpolicy" => Cow::Borrowed("networkpolicies"),
        "storageclass" => Cow::Borrowed("storageclasses"),
        "event" => Cow::Borrowed("events"),
        "endpoints" => Cow::Borrowed("endpoints"),
        "horizontalpodautoscaler" => Cow::Borrowed("horizontalpodautoscalers"),
        "poddisruptionbudget" => Cow::Borrowed("poddisruptionbudgets"),
        "limitrange" => Cow::Borrowed("limitranges"),
        "resourcequota" => Cow::Borrowed("resourcequotas"),
        "customresourcedefinition" => Cow::Borrowed("customresourcedefinitions"),
        other => Cow::Owned(format!("{other}s")),
    }
}

// ── Namespace pod counts ─────────────────────────────────────────────

/// Count pods per namespace via a single `kubectl get pods -A` call.
#[must_use]
pub fn namespace_pod_counts() -> HashMap<String, usize> {
    namespace_pod_counts_with(&RealKubectlRunner)
}

/// Count pods per namespace using a custom `KubectlRunner`.
#[must_use]
pub fn namespace_pod_counts_with(runner: &dyn KubectlRunner) -> HashMap<String, usize> {
    let args = [
        "get",
        "pods",
        "-A",
        "--no-headers",
        "-o",
        "custom-columns=NS:.metadata.namespace",
    ];

    let stdout = match runner.run(&args) {
        Some(s) => s,
        None => return HashMap::new(),
    };

    parse_namespace_pod_counts(&stdout)
}

/// Parse `kubectl get pods -A` custom-columns output into namespace counts.
fn parse_namespace_pod_counts(stdout: &str) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for line in stdout.lines() {
        let ns = line.trim();
        if !ns.is_empty() {
            *counts.entry(ns.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

// ── Test-only mock implementations ──────────────────────────────────

#[cfg(test)]
pub struct MockKubeconfigLoader {
    pub yaml: String,
}

#[cfg(test)]
impl KubeconfigLoader for MockKubeconfigLoader {
    fn load(&self) -> Option<KubeContext> {
        let config = serde_yaml::from_str::<KubeConfig>(&self.yaml).ok()?;
        kube_context_from_config(&config)
    }
}

#[cfg(test)]
pub struct MockKubectlRunner {
    pub output: Option<String>,
}

#[cfg(test)]
impl KubectlRunner for MockKubectlRunner {
    fn run(&self, _args: &[&str]) -> Option<String> {
        self.output.clone()
    }
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

    // ── New trait-based tests ────────────────────────────────────────

    #[test]
    fn kube_context_with_mock_loader() {
        let loader = MockKubeconfigLoader {
            yaml: r#"
apiVersion: v1
kind: Config
current-context: staging
contexts:
  - name: staging
    context:
      cluster: staging-cluster
      namespace: apps
"#
            .to_string(),
        };

        let ctx = KubeContext::with_loader(&loader).unwrap();
        assert_eq!(ctx.context, "staging");
        assert_eq!(ctx.namespace, "apps");
        assert_eq!(ctx.cluster, "staging-cluster");
    }

    #[test]
    fn kube_context_with_mock_loader_default_namespace() {
        let loader = MockKubeconfigLoader {
            yaml: r#"
apiVersion: v1
kind: Config
current-context: dev
contexts:
  - name: dev
    context:
      cluster: dev-cluster
"#
            .to_string(),
        };

        let ctx = KubeContext::with_loader(&loader).unwrap();
        assert_eq!(ctx.context, "dev");
        assert_eq!(ctx.namespace, "default");
        assert_eq!(ctx.cluster, "dev-cluster");
    }

    #[test]
    fn resource_counts_with_mock_kubectl() {
        let runner = MockKubectlRunner {
            output: Some(
                "pod/nginx-abc\npod/redis-xyz\ndeployment.apps/web\nservice/api\nservice/db\n"
                    .to_string(),
            ),
        };

        let counts = resource_counts_with(&runner, &["pods", "deployments", "services"], None);
        assert_eq!(counts.get("pods"), Some(&2));
        assert_eq!(counts.get("deployments"), Some(&1));
        assert_eq!(counts.get("services"), Some(&2));
    }

    #[test]
    fn resource_counts_with_mock_kubectl_empty() {
        let runner = MockKubectlRunner { output: None };
        let counts = resource_counts_with(&runner, &["pods"], None);
        assert!(counts.is_empty());
    }

    #[test]
    fn resource_counts_with_mock_kubectl_empty_types() {
        let runner = MockKubectlRunner {
            output: Some("pod/nginx\n".to_string()),
        };
        let counts = resource_counts_with(&runner, &[], None);
        assert!(counts.is_empty());
    }

    #[test]
    fn namespace_pod_counts_with_mock_kubectl() {
        let runner = MockKubectlRunner {
            output: Some(
                "kube-system\nkube-system\nkube-system\ndefault\ndefault\napps\n".to_string(),
            ),
        };

        let counts = namespace_pod_counts_with(&runner);
        assert_eq!(counts.get("kube-system"), Some(&3));
        assert_eq!(counts.get("default"), Some(&2));
        assert_eq!(counts.get("apps"), Some(&1));
    }

    #[test]
    fn namespace_pod_counts_with_mock_kubectl_empty() {
        let runner = MockKubectlRunner { output: None };
        let counts = namespace_pod_counts_with(&runner);
        assert!(counts.is_empty());
    }

    // ── parse_resource_counts edge cases ─────────────────────────────

    #[test]
    fn parse_resource_counts_empty() {
        let counts = parse_resource_counts("");
        assert!(counts.is_empty());
    }

    #[test]
    fn parse_resource_counts_trailing_newline() {
        let counts = parse_resource_counts("pod/nginx\npod/redis\n");
        assert_eq!(counts.get("pods"), Some(&2));
    }

    #[test]
    fn parse_resource_counts_mixed_types() {
        let input = "pod/a\nservice/b\npod/c\ndeployment.apps/d\n";
        let counts = parse_resource_counts(input);
        assert_eq!(counts.get("pods"), Some(&2));
        assert_eq!(counts.get("services"), Some(&1));
        assert_eq!(counts.get("deployments"), Some(&1));
    }

    #[test]
    fn parse_resource_counts_no_slash_in_line() {
        // Lines without / should use the full line as the type
        let counts = parse_resource_counts("standalone\n");
        assert_eq!(counts.get("standalones"), Some(&1));
    }

    // ── parse_namespace_pod_counts edge cases ────────────────────────

    #[test]
    fn parse_namespace_pod_counts_empty() {
        let counts = parse_namespace_pod_counts("");
        assert!(counts.is_empty());
    }

    #[test]
    fn parse_namespace_pod_counts_whitespace_lines() {
        let counts = parse_namespace_pod_counts("  default  \n  kube-system  \n   \n");
        assert_eq!(counts.get("default"), Some(&1));
        assert_eq!(counts.get("kube-system"), Some(&1));
        assert!(!counts.contains_key(""));
    }

    #[test]
    fn parse_namespace_pod_counts_single_ns() {
        let counts = parse_namespace_pod_counts("myns\nmyns\nmyns\n");
        assert_eq!(counts.get("myns"), Some(&3));
    }

    // ── api_type_to_plural comprehensive ─────────────────────────────

    #[test]
    fn api_type_to_plural_all_known_types() {
        let known = vec![
            ("configmap", "configmaps"),
            ("secret", "secrets"),
            ("namespace", "namespaces"),
            ("node", "nodes"),
            ("persistentvolumeclaim", "persistentvolumeclaims"),
            ("persistentvolume", "persistentvolumes"),
            ("serviceaccount", "serviceaccounts"),
            ("role", "roles"),
            ("clusterrole", "clusterroles"),
            ("rolebinding", "rolebindings"),
            ("clusterrolebinding", "clusterrolebindings"),
            ("networkpolicy", "networkpolicies"),
            ("storageclass", "storageclasses"),
            ("event", "events"),
            ("endpoints", "endpoints"),
            ("horizontalpodautoscaler", "horizontalpodautoscalers"),
            ("poddisruptionbudget", "poddisruptionbudgets"),
            ("limitrange", "limitranges"),
            ("resourcequota", "resourcequotas"),
            ("customresourcedefinition", "customresourcedefinitions"),
            ("replicaset", "replicasets"),
            ("statefulset", "statefulsets"),
            ("daemonset", "daemonsets"),
            ("job", "jobs"),
            ("cronjob", "cronjobs"),
        ];
        for (input, expected) in known {
            assert_eq!(
                api_type_to_plural(input).as_ref(),
                expected,
                "failed for input: {input}"
            );
        }
    }

    #[test]
    fn api_type_to_plural_with_api_group_suffix() {
        assert_eq!(api_type_to_plural("statefulset.apps"), "statefulsets");
        assert_eq!(api_type_to_plural("job.batch"), "jobs");
        assert_eq!(api_type_to_plural("ingress.networking.k8s.io"), "ingresses");
    }

    // ── KubeContext header edge cases ────────────────────────────────

    #[test]
    fn kube_context_header_empty_cluster() {
        let ctx = KubeContext {
            context: "dev".to_string(),
            namespace: "default".to_string(),
            cluster: "".to_string(),
        };
        let header = ctx.header();
        let plain = crate::strip_ansi(&header);
        assert!(plain.contains("ctx: dev"));
        assert!(plain.contains("ns: default"));
        assert!(!plain.contains("cluster:"));
    }

    #[test]
    fn kube_context_prompt_empty_context() {
        let ctx = KubeContext {
            context: "".to_string(),
            namespace: "default".to_string(),
            cluster: "".to_string(),
        };
        let prompt = ctx.prompt();
        assert!(prompt.contains(crate::ICON_K8S));
    }

    #[test]
    fn kube_context_prompt_exactly_15_chars() {
        let ctx = KubeContext {
            context: "012345678901234".to_string(), // exactly 15 chars
            namespace: "default".to_string(),
            cluster: "".to_string(),
        };
        let prompt = ctx.prompt();
        assert!(prompt.contains("012345678901234"));
    }

    // ── MockKubeconfigLoader edge cases ──────────────────────────────

    #[test]
    fn mock_loader_invalid_yaml() {
        let loader = MockKubeconfigLoader {
            yaml: "{{invalid".to_string(),
        };
        assert!(KubeContext::with_loader(&loader).is_none());
    }

    #[test]
    fn mock_loader_no_matching_context() {
        let loader = MockKubeconfigLoader {
            yaml: r#"
apiVersion: v1
kind: Config
current-context: nonexistent
contexts:
  - name: actual
    context:
      cluster: cluster1
"#.to_string(),
        };
        assert!(KubeContext::with_loader(&loader).is_none());
    }

    #[test]
    fn mock_loader_empty_contexts() {
        let loader = MockKubeconfigLoader {
            yaml: r#"
apiVersion: v1
kind: Config
current-context: test
contexts: []
"#.to_string(),
        };
        assert!(KubeContext::with_loader(&loader).is_none());
    }

    // ── resource_counts_with namespace param ─────────────────────────

    #[test]
    fn resource_counts_with_namespace() {
        let runner = MockKubectlRunner {
            output: Some("pod/nginx\n".to_string()),
        };
        let counts = resource_counts_with(&runner, &["pods"], Some("myns"));
        assert_eq!(counts.get("pods"), Some(&1));
    }

    // ── kubeconfig_paths ─────────────────────────────────────────────

    #[test]
    fn kubeconfig_paths_single() {
        std::env::set_var("KUBECONFIG", "/tmp/single");
        let paths = kubeconfig_paths();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], PathBuf::from("/tmp/single"));
        std::env::remove_var("KUBECONFIG");
    }
}
