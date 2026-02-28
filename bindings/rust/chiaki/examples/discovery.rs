// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! Example: Discover PlayStation consoles on the local network.
//!
//! Usage:
//!   cargo run --example discovery
//!   cargo run --example discovery -- --timeout 5
//!
//! The program broadcasts UDP discovery packets for `--timeout` seconds
//! (default 3) and prints every console it finds, then exits cleanly.

use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chiaki::prelude::*;

// ── CLI ───────────────────────────────────────────────────────────────────────

struct Args {
    /// How many seconds to wait for responses (default 3).
    timeout_secs: u64,
    /// Broadcast address to send discovery packets to (default 255.255.255.255).
    broadcast: IpAddr,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            timeout_secs: 3,
            broadcast: "255.255.255.255".parse().unwrap(),
        }
    }
}

fn parse_args() -> Args {
    let mut args = Args::default();
    let mut iter = std::env::args().skip(1).peekable();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--timeout" | "-t" => {
                if let Some(val) = iter.next() {
                    args.timeout_secs = val
                        .parse()
                        .expect("--timeout requires a positive integer (seconds)");
                }
            }
            "--broadcast" | "-b" => {
                if let Some(val) = iter.next() {
                    args.broadcast = val
                        .parse()
                        .expect("--broadcast requires a valid IP address");
                }
            }
            "--help" | "-h" => {
                eprintln!(concat!(
                    "Usage: discovery [--timeout <secs>] [--broadcast <ip>]\n",
                    "\n",
                    "Options:\n",
                    "  -t, --timeout <secs>   Seconds to listen (default 3)\n",
                    "  -b, --broadcast <ip>   Broadcast address (default 255.255.255.255)",
                ));
                std::process::exit(0);
            }
            other => eprintln!("Unknown argument: {other}"),
        }
    }
    args
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();

    // 1. Initialise the C library (idempotent, must be called first).
    chiaki::init().expect("chiaki_lib_init failed");

    // 2. Create a logger that captures messages into a Vec so we can print
    //    them after the discovery service has stopped.
    let log_entries: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let log_entries_clone = Arc::clone(&log_entries);

    let log = Log::new(LogLevel::INFO | LogLevel::WARNING | LogLevel::ERROR, move |level, msg| {
        let entry = format!("[{level:?}] {msg}");
        log_entries_clone.lock().unwrap().push(entry);
    });

    // 3. Shared state: the callback runs on a C background thread, so we use
    //    Arc<Mutex<_>> to pass discovered hosts back to the main thread.
    let found: Arc<Mutex<Vec<DiscoveryHost>>> = Arc::new(Mutex::new(Vec::new()));
    let found_clone = Arc::clone(&found);

    // 4. Configure and start the discovery service.
    let options = DiscoveryServiceOptions {
        hosts_max: 16,
        host_drop_pings: 3,
        ping_ms: 500,
        ping_initial_ms: 0,
        send_addr: args.broadcast,
        broadcast_addrs: Vec::new(),
    };

    println!(
        "Scanning for PlayStation consoles ({} seconds, broadcast {})...",
        args.timeout_secs, args.broadcast
    );

    let svc = DiscoveryService::start(options, log, move |hosts| {
        // Called by the C thread every time the host list changes.
        *found_clone.lock().unwrap() = hosts;
    })
    .expect("Failed to start discovery service");

    // 5. Wait for the configured timeout, then stop the service.
    std::thread::sleep(Duration::from_secs(args.timeout_secs));
    drop(svc); // joins the C background thread

    // 6. Print any log messages collected during discovery.
    let entries = log_entries.lock().unwrap();
    if !entries.is_empty() {
        println!("\n--- Library log ---");
        for e in entries.iter() {
            println!("  {e}");
        }
    }

    // 7. Print every discovered console.
    let hosts = found.lock().unwrap();
    if hosts.is_empty() {
        println!("\nNo PlayStation consoles found.");
        println!("Make sure your PS4/PS5 is on and on the same LAN segment.");
    } else {
        println!("\nFound {} console(s):", hosts.len());
        for (i, h) in hosts.iter().enumerate() {
            println!();
            println!("  [{i}] ----------------------------------------");
            println!("  State        : {:?}", h.state);
            println!(
                "  Address      : {}",
                h.host_addr.as_deref().unwrap_or("<unknown>")
            );
            println!(
                "  Name         : {}",
                h.host_name.as_deref().unwrap_or("<unknown>")
            );
            println!(
                "  Type         : {}",
                h.host_type.as_deref().unwrap_or("<unknown>")
            );
            println!(
                "  Host ID      : {}",
                h.host_id.as_deref().unwrap_or("<unknown>")
            );
            println!(
                "  System ver.  : {}",
                h.system_version.as_deref().unwrap_or("<unknown>")
            );
            println!(
                "  Protocol ver.: {}",
                h.device_discovery_protocol_version
                    .as_deref()
                    .unwrap_or("<unknown>")
            );
            println!("  Request port : {}", h.host_request_port);
            if let Some(app) = &h.running_app_name {
                println!("  Running app  : {app}");
                if let Some(tid) = &h.running_app_titleid {
                    println!("  Title ID     : {tid}");
                }
            }
        }
    }
}
