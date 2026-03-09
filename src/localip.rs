//! skim-localip — get local IP address (cross-platform).
//!
//! Detects the local network IP without external dependencies.
//! Replaces the shell function with platform-specific commands.

use std::process::Command;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("ipconfig")
            .args(["getifaddr", "en0"])
            .output()
            .context("failed to run ipconfig")?;

        if output.status.success() {
            print!("{}", String::from_utf8_lossy(&output.stdout).trim());
            return Ok(());
        }

        // Fallback: try en1 (WiFi on some Macs)
        let output = Command::new("ipconfig")
            .args(["getifaddr", "en1"])
            .output()
            .context("failed to run ipconfig")?;

        print!("{}", String::from_utf8_lossy(&output.stdout).trim());
    }

    #[cfg(not(target_os = "macos"))]
    {
        let output = Command::new("hostname")
            .arg("-I")
            .output()
            .context("failed to run hostname -I")?;

        let ip = String::from_utf8_lossy(&output.stdout);
        if let Some(first) = ip.split_whitespace().next() {
            print!("{first}");
        }
    }

    println!();
    Ok(())
}
