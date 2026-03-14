//! Context intelligence — project detection and environment awareness.

use std::path::Path;

/// Detected project type based on marker files in CWD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectType {
    Rust,      // Cargo.toml
    Node,      // package.json
    Python,    // pyproject.toml, setup.py, requirements.txt
    Go,        // go.mod
    Nix,       // flake.nix
    Zig,       // build.zig
    Ruby,      // Gemfile
    Terraform, // main.tf
    Helm,      // Chart.yaml
    Unknown,
}

/// Detect project type from marker files in the given directory.
pub fn detect_project(dir: &Path) -> ProjectType {
    // Check for marker files, most specific first
    let markers = [
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
    for (file, proj_type) in &markers {
        if dir.join(file).exists() {
            return proj_type.clone();
        }
    }
    ProjectType::Unknown
}

/// Get the current project type (detect from CWD).
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
        // Just verifying it doesn't panic
        let _ = current_project();
    }
}
