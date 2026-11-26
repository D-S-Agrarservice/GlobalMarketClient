//! Prints the digest of a new Extended (support) or Developer access key, in the format the build
//! expects (`GM_EXTENDED_KEY_DIGEST` / `GM_DEVELOPER_KEY_DIGEST`, stored as CI secrets).
//!
//! Usage: `cargo run --example access_key_digest`, then type the key and press Enter.
//! The key is read from stdin so it never ends up in the shell history.

use globalmarket_client_lib::config::{digest_access_key, parse_digest_pair_hex};
use std::io::{self, BufRead, Write};

/// Below this length a key can be guessed from the public digest algorithm. That is fine for the
/// support key, which is handed out to players anyway, but not for the developer key.
const GUESSABLE_BELOW: usize = 24;

fn main() {
    eprint!("Access key: ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).is_err() {
        eprintln!("Could not read the key.");
        std::process::exit(1);
    }
    let key = line.trim();
    if key.is_empty() || key.len() > 128 {
        eprintln!("The key needs 1 to 128 characters.");
        std::process::exit(1);
    }
    if key.len() < GUESSABLE_BELOW {
        eprintln!(
            "Note: keys under {GUESSABLE_BELOW} characters can be guessed. Fine for the support key, not for the developer key."
        );
    }
    let (a, b) = digest_access_key(key);
    let digest = format!("{a:016x}:{b:016x}");
    debug_assert_eq!(parse_digest_pair_hex(&digest), Some((a, b)));
    println!("{digest}");
}
