//! skim-killport — kill process by port number.
//!
//! Finds the process listening on a given port and kills it.
//! Replaces the shell function that called lsof + kill.

use std::process::Command;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let port = std::env::args()
        .nth(1)
        .context("Usage: killport <port>")?;

    let output = Command::new("lsof")
        .args(["-ti", &format!(":{port}")])
        .output()
        .context("failed to run lsof")?;

    let pids = String::from_utf8_lossy(&output.stdout);
    let pids: Vec<&str> = pids.lines().filter(|l| !l.is_empty()).collect();

    if pids.is_empty() {
        eprintln!("No process found on port {port}");
        return Ok(());
    }

    for pid in &pids {
        let status = Command::new("kill")
            .args(["-9", pid])
            .status()
            .with_context(|| format!("failed to kill PID {pid}"))?;

        if status.success() {
            eprintln!("Killed process on port {port} (PID: {pid})");
        } else {
            eprintln!("Failed to kill PID {pid}");
        }
    }

    Ok(())
}
