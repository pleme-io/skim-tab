//! Context intelligence — project detection and environment awareness.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

/// Detected project type based on marker files in CWD.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectType {
    /// Rust project (`Cargo.toml`).
    Rust,
    /// Node.js project (`package.json`).
    Node,
    /// Python project (`pyproject.toml`, `setup.py`, `requirements.txt`).
    Python,
    /// Go project (`go.mod`).
    Go,
    /// Nix flake project (`flake.nix`).
    Nix,
    /// Zig project (`build.zig`).
    Zig,
    /// Ruby project (`Gemfile`).
    Ruby,
    /// Terraform project (`main.tf`).
    Terraform,
    /// Helm chart project (`Chart.yaml`).
    Helm,
    /// No recognized project markers found.
    Unknown,
}

impl fmt::Display for ProjectType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rust => write!(f, "rust"),
            Self::Node => write!(f, "node"),
            Self::Python => write!(f, "python"),
            Self::Go => write!(f, "go"),
            Self::Nix => write!(f, "nix"),
            Self::Zig => write!(f, "zig"),
            Self::Ruby => write!(f, "ruby"),
            Self::Terraform => write!(f, "terraform"),
            Self::Helm => write!(f, "helm"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

impl FromStr for ProjectType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "rust" => Ok(Self::Rust),
            "node" => Ok(Self::Node),
            "python" => Ok(Self::Python),
            "go" => Ok(Self::Go),
            "nix" => Ok(Self::Nix),
            "zig" => Ok(Self::Zig),
            "ruby" => Ok(Self::Ruby),
            "terraform" => Ok(Self::Terraform),
            "helm" => Ok(Self::Helm),
            "unknown" => Ok(Self::Unknown),
            other => Err(format!("unknown project type: {other}")),
        }
    }
}

/// Detect project type from marker files in the given directory.
#[must_use]
pub fn detect_project(dir: &Path) -> ProjectType {
    const MARKERS: &[(&str, ProjectType)] = &[
        ("Cargo.toml", ProjectType::Rust),
        ("flake.nix", ProjectType::Nix),
        ("go.mod", ProjectType::Go),
        ("package.json", ProjectType::Node),
        ("pyproject.toml", ProjectType::Python),
        ("setup.py", ProjectType::Python),
        ("build.zig", ProjectType::Zig),
        ("Gemfile", ProjectType::Ruby),
        ("Chart.yaml", ProjectType::Helm),
        ("main.tf", ProjectType::Terraform),
    ];
    MARKERS
        .iter()
        .find(|(file, _)| dir.join(file).exists())
        .map(|(_, proj_type)| proj_type.clone())
        .unwrap_or(ProjectType::Unknown)
}

/// Get the current project type (detect from CWD).
#[must_use]
pub fn current_project() -> ProjectType {
    std::env::current_dir()
        .map(|d| detect_project(&d))
        .unwrap_or(ProjectType::Unknown)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detect_rust_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Rust);
    }

    #[test]
    fn detect_nix_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("flake.nix"), "").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Nix);
    }

    #[test]
    fn detect_node_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Node);
    }

    #[test]
    fn detect_python_pyproject() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Python);
    }

    #[test]
    fn detect_python_setup_py() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("setup.py"), "").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Python);
    }

    #[test]
    fn detect_go_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("go.mod"), "").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Go);
    }

    #[test]
    fn detect_zig_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("build.zig"), "").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Zig);
    }

    #[test]
    fn detect_ruby_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Gemfile"), "").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Ruby);
    }

    #[test]
    fn detect_helm_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Chart.yaml"), "").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Helm);
    }

    #[test]
    fn detect_terraform_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.tf"), "").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Terraform);
    }

    #[test]
    fn detect_unknown_project() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Unknown);
    }

    #[test]
    fn rust_takes_priority_over_nix() {
        // If both Cargo.toml and flake.nix exist, Rust wins (most specific first)
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        fs::write(dir.path().join("flake.nix"), "").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Rust);
    }

    #[test]
    fn current_project_returns_something() {
        let _ = current_project();
    }

    // ── Priority tests ──────────────────────────────────────────────

    #[test]
    fn nix_takes_priority_over_node() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("flake.nix"), "").unwrap();
        fs::write(dir.path().join("package.json"), "").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Nix);
    }

    #[test]
    fn go_takes_priority_over_python() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("go.mod"), "").unwrap();
        fs::write(dir.path().join("pyproject.toml"), "").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Go);
    }

    // ── All variants have Debug ──────────────────────────────────────

    #[test]
    fn project_type_debug_formatting() {
        let types = vec![
            ProjectType::Rust,
            ProjectType::Node,
            ProjectType::Python,
            ProjectType::Go,
            ProjectType::Nix,
            ProjectType::Zig,
            ProjectType::Ruby,
            ProjectType::Terraform,
            ProjectType::Helm,
            ProjectType::Unknown,
        ];
        for t in types {
            let debug = format!("{t:?}");
            assert!(!debug.is_empty());
        }
    }

    // ── Nonexistent directory ────────────────────────────────────────

    #[test]
    fn detect_nonexistent_dir() {
        let result = detect_project(Path::new("/tmp/nonexistent-skim-tab-test-dir-xyz"));
        assert_eq!(result, ProjectType::Unknown);
    }

    // ── ProjectType Clone and Eq ─────────────────────────────────────

    #[test]
    fn project_type_clone_and_eq() {
        let a = ProjectType::Rust;
        let b = a.clone();
        assert_eq!(a, b);
        assert_ne!(ProjectType::Rust, ProjectType::Node);
    }

    // ── Display / FromStr round-trip ─────────────────────────────────

    #[test]
    fn project_type_display_fromstr_roundtrip() {
        let variants = [
            ProjectType::Rust,
            ProjectType::Node,
            ProjectType::Python,
            ProjectType::Go,
            ProjectType::Nix,
            ProjectType::Zig,
            ProjectType::Ruby,
            ProjectType::Terraform,
            ProjectType::Helm,
            ProjectType::Unknown,
        ];
        for v in &variants {
            let s = v.to_string();
            let parsed: ProjectType = s.parse().unwrap();
            assert_eq!(&parsed, v, "round-trip failed for {s}");
        }
    }

    #[test]
    fn project_type_fromstr_invalid() {
        let result = "invalid".parse::<ProjectType>();
        assert!(result.is_err());
    }
}
