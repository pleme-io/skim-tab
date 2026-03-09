//! blx-json — pretty-print JSON via jaq.
//!
//! Usage: blx-json '{"key": "value"}'
//!   or:  echo '{"key": "value"}' | blx-json

use std::os::unix::process::CommandExt;
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        // Pipe mode: stdin → jaq .
        let err = Command::new("jaq").arg(".").exec();
        eprintln!("blx-json: failed to exec jaq: {err}");
        std::process::exit(1);
    }

    // Args mode: echo args | jaq .
    let input = args.join(" ");
    let child = Command::new("jaq")
        .arg(".")
        .stdin(std::process::Stdio::piped())
        .spawn();

    match child {
        Ok(mut child) => {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(input.as_bytes());
            }
            let status = child.wait().unwrap_or_else(|e| {
                eprintln!("blx-json: {e}");
                std::process::exit(1);
            });
            std::process::exit(status.code().unwrap_or(1));
        }
        Err(e) => {
            eprintln!("blx-json: failed to spawn jaq: {e}");
            std::process::exit(1);
        }
    }
}
