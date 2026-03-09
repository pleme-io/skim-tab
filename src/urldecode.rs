//! blx-urldecode — percent-decode a string (pure Rust, no deps).
//!
//! Usage: blx-urldecode <string>
//!   or:  echo "string" | blx-urldecode

use std::io::Read;

fn main() {
    let input = match std::env::args().nth(1) {
        Some(s) => s,
        None => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf).expect("failed to read stdin");
            if buf.ends_with('\n') {
                buf.pop();
            }
            buf
        }
    };

    let mut decoded = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(val) = u8::from_str_radix(
                &input[i + 1..i + 3], 16
            ) {
                decoded.push(val);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i]);
        i += 1;
    }

    let s = String::from_utf8_lossy(&decoded);
    println!("{s}");
}
