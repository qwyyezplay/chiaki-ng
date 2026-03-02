// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! PSN Account ID retrieval example.
//!
//! Mirrors the behaviour of the `scripts/psn-account-id.py` helper script:
//! it prints the PSN login URL, prompts the user to paste the resulting
//! redirect URL, and then exchanges the embedded authorization code for an
//! access token before fetching the numeric PSN Account ID.
//!
//! # Usage
//!
//! Interactive (browser login flow):
//! ```
//! cargo run -p chiaki --example psn_auth --features psn-auth
//! ```
//!
//! Non-interactive (supply the `code` directly):
//! ```
//! cargo run -p chiaki --example psn_auth --features psn-auth -- --code <CODE>
//! ```

use std::io::{self, BufRead, Write};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chiaki::psn_auth::{login_url, PsnAccountId};

// ── CLI ───────────────────────────────────────────────────────────────────────

struct Args {
    /// Authorization code obtained from the PSN redirect URL.
    /// When `None` the program runs the interactive browser-login flow.
    code: Option<String>,
}

impl Default for Args {
    fn default() -> Self {
        Args { code: None }
    }
}

fn parse_args() -> Args {
    let mut args = Args::default();
    let mut iter = std::env::args().skip(1).peekable();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--code" | "-c" => {
                if let Some(val) = iter.next() {
                    args.code = Some(val);
                }
            }
            "--help" | "-h" => {
                eprintln!(concat!(
                    "Usage: psn_auth [--code <CODE>]\n",
                    "\n",
                    "Options:\n",
                    "  -c, --code <CODE>   Skip the interactive login flow and use\n",
                    "                      a previously captured authorization code",
                ));
                std::process::exit(0);
            }
            other => eprintln!("Unknown argument: {other}"),
        }
    }
    args
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract the `code` query parameter from a PSN redirect URL.
///
/// The redirect URL has the form:
/// `https://remoteplay.dl.playstation.net/remoteplay/redirect?code=<CODE>&…`
///
/// Returns `None` when the input does not contain a `code` parameter, which
/// lets the caller fall back to treating the whole string as a raw code.
fn parse_code_from_url(url: &str) -> Option<String> {
    let query = url.splitn(2, '?').nth(1)?;
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        if kv.next() == Some("code") {
            return kv.next().map(str::to_owned);
        }
    }
    None
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();

    println!();
    println!("########################################################");
    println!("           Script to determine PSN AccountID");
    println!("########################################################");
    println!();

    // ------------------------------------------------------------------
    // Step 1 — acquire the authorization code
    // ------------------------------------------------------------------
    let redirect_code = match args.code {
        Some(code) => code,
        None => {
            println!("Open the following URL in your browser and log in:");
            println!();
            println!("{}", login_url());
            println!();
            println!(
                "After logging in, when the page shows \"redirect\", copy the URL\n\
                 from the address bar and paste it here:"
            );
            print!("> ");
            io::stdout().flush().expect("flush stdout");

            let mut line = String::new();
            io::stdin()
                .lock()
                .read_line(&mut line)
                .expect("failed to read from stdin");
            let line = line.trim().to_owned();

            if line.is_empty() {
                eprintln!("No input provided.");
                std::process::exit(1);
            }

            // Accept either the full redirect URL or a raw code string.
            parse_code_from_url(&line).unwrap_or(line)
        }
    };

    // ------------------------------------------------------------------
    // Step 2 — exchange the code for tokens and fetch the Account ID
    // ------------------------------------------------------------------
    println!("Requesting OAuth Token...");
    println!("Requesting Account Info...");

    match PsnAccountId::get(&redirect_code) {
        Ok((id_bytes, tokens)) => {
            let id_b64 = B64.encode(id_bytes);
            let id_hex: String = id_bytes.iter().map(|b| format!("{b:02x}")).collect();
            println!();
            println!("This is your AccountID:");
            println!("  base64 : {id_b64}");
            println!("  hex    : {id_hex}");
            println!();
            println!(
                "access_token  : {}…",
                &tokens.access_token[..20.min(tokens.access_token.len())]
            );
            println!(
                "refresh_token : {}…",
                &tokens.refresh_token[..20.min(tokens.refresh_token.len())]
            );
            println!("expires_in    : {}s", tokens.expires_in);
        }
        Err(e) => {
            eprintln!("Failed: {e}");
            std::process::exit(1);
        }
    }
}
