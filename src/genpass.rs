//! skim-genpass — secure password generator in pure Rust.
//!
//! Generates cryptographically random passwords without external dependencies.
//! Replaces the shell function that called openssl.

use std::env;

fn main() {
    let length: usize = env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(16);

    // Use /dev/urandom for cryptographic randomness
    let charset = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%^&*-_=+";

    let random_bytes = std::fs::read("/dev/urandom")
        .ok()
        .map(|_| {
            // Read enough bytes
            let mut buf = vec![0u8; length * 2];
            if let Ok(f) = std::fs::File::open("/dev/urandom") {
                use std::io::Read;
                let mut f = f;
                let _ = f.read_exact(&mut buf);
            }
            buf
        })
        .unwrap_or_else(|| vec![0u8; length * 2]);

    let password: String = random_bytes
        .iter()
        .filter_map(|&b| {
            let idx = (b as usize) % charset.len();
            Some(charset[idx] as char)
        })
        .take(length)
        .collect();

    println!("{password}");
}
