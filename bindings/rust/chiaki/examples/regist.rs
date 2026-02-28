// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! Example: Register with a PlayStation console over LAN.
//!
//! Usage:
//!   cargo run --example regist -- --host <IP> --pin <8-digit-PIN>
//!   cargo run --example regist -- --host <IP> --pin <8-digit-PIN> --target ps4
//!   cargo run --example regist -- --host <IP> --pin <8-digit-PIN> --psn-online-id <name>
//!   cargo run --example regist -- --host <IP> --pin <8-digit-PIN> --psn-account-id <16-hex-chars>
//!
//! Navigate to Settings → Remote Play on your PS4/PS5, start Remote Play
//! Connection, and enter the 8-digit PIN shown on-screen as `--pin`.
//! On success the program prints the credentials needed to open a streaming
//! session.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chiaki::prelude::*;

// ── CLI ───────────────────────────────────────────────────────────────────────

struct Args {
    /// Console hostname or IP address (required).
    host: String,
    /// 8-digit PIN from the Remote Play screen (required).
    pin: u32,
    /// Target console family (default: Ps5_1).
    target: Target,
    /// PSN online ID (mutually exclusive with psn_account_id).
    psn_online_id: Option<String>,
    /// 8-byte PSN account ID supplied as 16 hex characters.
    psn_account_id: Option<[u8; 8]>,
    /// Seconds to wait for registration to complete (default 30).
    timeout_secs: u64,
}

fn parse_account_id(hex: &str) -> [u8; 8] {
    if hex.len() != 16 {
        eprintln!("Error: --psn-account-id must be exactly 16 hex characters (8 bytes)");
        std::process::exit(1);
    }
    let mut bytes = [0u8; 8];
    for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
        bytes[i] = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
    }
    bytes
}

fn hex_nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => {
            eprintln!("Error: invalid hex character '{}' in --psn-account-id", c as char);
            std::process::exit(1);
        }
    }
}

fn parse_args() -> Args {
    let mut host: Option<String> = None;
    let mut pin: Option<u32> = None;
    let mut target = Target::Ps5_1;
    let mut psn_online_id: Option<String> = None;
    let mut psn_account_id: Option<[u8; 8]> = None;
    let mut timeout_secs: u64 = 30;

    let mut iter = std::env::args().skip(1).peekable();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--host" | "-H" => {
                if let Some(val) = iter.next() {
                    host = Some(val);
                }
            }
            "--pin" | "-p" => {
                if let Some(val) = iter.next() {
                    pin = Some(
                        val.parse()
                            .expect("--pin requires a numeric 8-digit PIN"),
                    );
                }
            }
            "--target" | "-T" => {
                if let Some(val) = iter.next() {
                    target = match val.to_lowercase().as_str() {
                        "ps4" | "ps4_10" => Target::Ps4_10,
                        "ps4_9" => Target::Ps4_9,
                        "ps4_8" => Target::Ps4_8,
                        "ps5" | "ps5_1" => Target::Ps5_1,
                        other => {
                            eprintln!("Unknown target '{other}'; use ps4 or ps5");
                            std::process::exit(1);
                        }
                    };
                }
            }
            "--psn-online-id" | "-u" => {
                if let Some(val) = iter.next() {
                    psn_online_id = Some(val);
                }
            }
            "--psn-account-id" | "-a" => {
                if let Some(val) = iter.next() {
                    psn_account_id = Some(parse_account_id(&val));
                }
            }
            "--timeout" | "-t" => {
                if let Some(val) = iter.next() {
                    timeout_secs = val
                        .parse()
                        .expect("--timeout requires a positive integer (seconds)");
                }
            }
            "--help" | "-h" => {
                eprintln!(concat!(
                    "Usage: regist --host <IP> --pin <PIN> [options]\n",
                    "\n",
                    "Required:\n",
                    "  -H, --host <IP>                Console hostname or IP address\n",
                    "  -p, --pin <8-digit>            PIN shown on the Remote Play screen\n",
                    "\n",
                    "Options:\n",
                    "  -T, --target <ps4|ps5>         Console family (default: ps5)\n",
                    "  -u, --psn-online-id <name>     PSN username (PS4 < 7.0)\n",
                    "  -a, --psn-account-id <hex16>   PSN account ID as 16 hex chars (PS4 >= 7.0 / PS5)\n",
                    "  -t, --timeout <secs>           Seconds to wait for result (default 30)",
                ));
                std::process::exit(0);
            }
            other => eprintln!("Unknown argument: {other}"),
        }
    }

    let host = host.unwrap_or_else(|| {
        eprintln!("Error: --host is required");
        std::process::exit(1);
    });
    let pin = pin.unwrap_or_else(|| {
        eprintln!("Error: --pin is required");
        std::process::exit(1);
    });

    Args { host, pin, target, psn_online_id, psn_account_id, timeout_secs }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn fmt_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join("")
}

fn fmt_mac(mac: &[u8; 6]) -> String {
    mac.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();

    // 1. Initialise the C library (idempotent, must be called first).
    chiaki::init().expect("chiaki_lib_init failed");

    // 2. Create a logger that captures messages into a Vec so we can print
    //    them after registration has completed.
    let log_entries: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let log_entries_clone = Arc::clone(&log_entries);

    let log = Log::new(LogLevel::INFO | LogLevel::WARNING | LogLevel::ERROR, move |level, msg| {
        let entry = format!("[{level:?}] {msg}");
        log_entries_clone.lock().unwrap().push(entry);
    });

    // 3. Build registration info.
    let info = RegistInfo {
        target: args.target,
        host: args.host.clone(),
        broadcast: false,
        psn_online_id: args.psn_online_id,
        psn_account_id: args.psn_account_id,
        pin: args.pin,
        console_pin: 0,
    };

    println!(
        "Registering with {} (target: {:?}, timeout: {} s)...",
        args.host, args.target, args.timeout_secs
    );

    // 4. Start registration. The C thread runs in the background and sends
    //    exactly one RegistResult to `rx` when it completes.
    let (_regist, rx) = Regist::start(info, log)
        .expect("Failed to start registration");

    // 5. Block until the result arrives or the timeout expires.
    let result = rx
        .recv_timeout(Duration::from_secs(args.timeout_secs))
        .unwrap_or(RegistResult::Failed);

    // 6. Print any log messages collected during registration.
    let entries = log_entries.lock().unwrap();
    if !entries.is_empty() {
        println!("\n--- Library log ---");
        for e in entries.iter() {
            println!("  {e}");
        }
    }
    drop(entries);

    // 7. Print the outcome.
    println!();
    match result {
        RegistResult::Canceled => {
            println!("Registration canceled.");
        }
        RegistResult::Failed => {
            println!("Registration failed.");
            println!("Check that the PIN is correct and that the console is in Remote Play pairing mode.");
            std::process::exit(1);
        }
        RegistResult::Success(host) => {
            println!("Registration successful!");
            println!();
            println!("  Target          : {:?}", host.target);
            println!("  Server nickname : {}", host.server_nickname);
            println!("  Server MAC      : {}", fmt_mac(&host.server_mac));
            println!("  RP regist key   : {}", fmt_hex(&host.rp_regist_key));
            println!("  RP key type     : {}", host.rp_key_type);
            println!("  RP key          : {}", fmt_hex(&host.rp_key));
            if !host.ap_ssid.is_empty() {
                println!("  AP SSID         : {}", host.ap_ssid);
            }
            if !host.ap_bssid.is_empty() {
                println!("  AP BSSID        : {}", host.ap_bssid);
            }
            if !host.ap_name.is_empty() {
                println!("  AP name         : {}", host.ap_name);
            }
            if host.console_pin != 0 {
                println!("  Console PIN     : {}", host.console_pin);
            }
        }
    }
}
