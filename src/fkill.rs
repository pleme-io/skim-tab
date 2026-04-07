//! skim-fkill — fuzzy process killer with preview.
//!
//! Lists running processes, presents them via skim with multi-select,
//! and sends the specified signal to selected PIDs.

use std::io;
use std::process::Command;

use anyhow::{Context, Result};
use skim::prelude::SkimItemReader;
use skim::tui::options::PreviewLayout;
use skim::Skim;
use skim_tab::{base_options, ICON_MARKER, ICON_SEARCH};

/// List processes via ps.
fn process_list() -> Result<String> {
    let output = Command::new("ps")
        .args(["ax", "-o", "pid,user,%cpu,%mem,start,command"])
        .output()
        .context("failed to run ps")?;

    let raw = String::from_utf8_lossy(&output.stdout);
    // Skip the header line
    let lines: Vec<&str> = raw.lines().skip(1).collect();
    Ok(lines.join("\n"))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let signal = args.first().map(|s| s.as_str()).unwrap_or("9");

    let entries = process_list()?;
    if entries.is_empty() {
        return Ok(());
    }

    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(entries));

    let options = base_options("")
        .prompt(ICON_SEARCH.to_string())
        .multi(true)
        .multi_select_icon(ICON_MARKER.to_string())
        .preview(
            "ps -p {1} -o pid,user,comm,%cpu,%mem,start,time,command 2>/dev/null || echo 'Process not found'"
                .to_string(),
        )
        .preview_window(PreviewLayout::from("down:4:wrap"))
        .header(format!(
            "Processes (signal: {signal}) | TAB: Multi-select | ESC: Cancel"
        ))
        .build()
        .expect("failed to build skim options");

    match Skim::run_with(options, Some(items)) {
        Ok(out) if !out.is_abort => {
            for item in &out.selected_items {
                let line = item.output().to_string();
                if let Some(pid) = line.split_whitespace().next() {
                    let status = Command::new("kill")
                        .args([&format!("-{signal}"), pid])
                        .status();
                    match status {
                        Ok(s) if s.success() => eprintln!("killed PID {pid}"),
                        _ => eprintln!("failed to kill PID {pid}"),
                    }
                }
            }
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_list_runs() {
        let result = process_list();
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn process_list_skips_header() {
        let result = process_list().unwrap();
        // The header line (PID USER ...) should be stripped
        assert!(!result.starts_with("PID") && !result.starts_with("  PID"));
    }

    #[test]
    fn process_list_contains_current_process() {
        let result = process_list().unwrap();
        let current_pid = std::process::id().to_string();
        // Our own process should appear in the list
        assert!(
            result.contains(&current_pid),
            "process list should contain current PID {current_pid}"
        );
    }
}
