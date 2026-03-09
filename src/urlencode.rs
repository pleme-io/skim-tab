//! blx-urlencode — percent-encode a string (pure Rust, no deps).
//!
//! Usage: blx-urlencode <string>
//!   or:  echo "string" | blx-urlencode

use std::io::Read;

fn main() {
    let input = match std::env::args().nth(1) {
        Some(s) => s,
        None => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf).expect("failed to read stdin");
            // Trim trailing newline from pipe
            if buf.ends_with('\n') {
                buf.pop();
            }
            buf
        }
    };

    let encoded: String = input.bytes().map(|b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            String::from(b as char)
        } else {
            format!("%{b:02X}")
        }
    }).collect();

    println!("{encoded}");
}
