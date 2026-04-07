//! skim-kpod — fuzzy Kubernetes pod selector.
//!
//! Lists pods via kubectl, presents them via skim, and prints the
//! selected pod name. Used by aliases for kexec/klog.

use std::io;
use std::process::Command;

use anyhow::{Context, Result};
use skim::prelude::SkimItemReader;
use skim::tui::options::PreviewLayout;
use skim::Skim;
use skim_tab::{base_options, build_options};

/// Icon for Kubernetes operations.
const ICON_K8S: &str = "\u{2388} "; // ⎈ (helm symbol)

/// List pods via kubectl.
fn pod_list() -> Result<String> {
    let output = Command::new("kubectl")
        .args(["get", "pods", "--no-headers", "-o", "wide"])
        .output()
        .context("failed to run kubectl — is it installed?")?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("kubectl failed: {err}");
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn main() -> Result<()> {
    let entries = pod_list()?;
    if entries.is_empty() {
        eprintln!("No pods found");
        return Ok(());
    }

    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(entries));

    let options = build_options(
        base_options("")
            .prompt(ICON_K8S.to_string())
            .preview(
                "kubectl describe pod {1} 2>/dev/null | head -40".to_string(),
            )
            .preview_window(PreviewLayout::from("down:6:wrap"))
            .header("Pods | CTRL-/: Toggle Preview | ESC: Cancel".to_string()),
    )?;

    match Skim::run_with(options, Some(items)) {
        Ok(out) if !out.is_abort => {
            if let Some(item) = out.selected_items.first() {
                let line = item.output().to_string();
                if let Some(pod) = line.split_whitespace().next() {
                    print!("{pod}");
                }
            }
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_is_valid() {
        assert!(!super::ICON_K8S.is_empty());
    }
}
