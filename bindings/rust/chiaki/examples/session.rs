// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! Example: Connect a streaming session to a PlayStation console.
//!
//! Usage:
//!   cargo run --example session -- \
//!       --host 192.168.1.10 \
//!       --regist-key <32 hex chars> \
//!       --morning <32 hex chars> \
//!       --psn-account-id <16 hex chars>
//!
//! Optional flags:
//!   --ps4                            Target is a PS4 (default: PS5)
//!   --resolution <360|540|720|1080>  Video resolution (default: 720)
//!   --fps <30|60>                    Frame rate (default: 60)
//!   --no-dualsense                   Disable DualSense features
//!
//! The credentials (`--regist-key` = rp_regist_key, `--morning` = rp_key) are
//! obtained by running the `regist` example first.
//!
//! The example streams for up to 30 seconds, or until the user presses Ctrl-C,
//! or until the console ends the session.  On exit a summary is printed.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use chiaki::prelude::*;

// ── Simple AudioSink implementation ──────────────────────────────────────────

/// Counts received audio frames and prints the stream header on first call.
struct CountingAudioSink {
    /// Human-readable label printed in log lines.
    label: &'static str,
    frame_count: Arc<AtomicU64>,
}

impl CountingAudioSink {
    fn new(label: &'static str) -> (Self, Arc<AtomicU64>) {
        let counter = Arc::new(AtomicU64::new(0));
        (CountingAudioSink { label, frame_count: Arc::clone(&counter) }, counter)
    }
}

impl AudioSink for CountingAudioSink {
    fn on_header(&mut self, header: AudioHeader) {
        println!(
            "[{}] audio header: {}ch  {}bit  {}Hz  frame_size={}",
            self.label, header.channels, header.bits, header.rate, header.frame_size,
        );
    }

    fn on_frame(&mut self, _opus_data: &[u8]) {
        self.frame_count.fetch_add(1, Ordering::Relaxed);
    }
}

// ── Quit signal ───────────────────────────────────────────────────────────────

/// Reason the main loop should exit.
#[derive(Debug)]
struct QuitSignal {
    reason: QuitReason,
    reason_str: Option<String>,
}

// ── CLI ───────────────────────────────────────────────────────────────────────

struct Args {
    host: String,
    regist_key: [u8; 16],
    morning: [u8; 16],
    psn_account_id: [u8; 8],
    ps5: bool,
    resolution: VideoResolutionPreset,
    fps: VideoFpsPreset,
    enable_dualsense: bool,
}

fn parse_hex16(hex: &str, flag: &str) -> [u8; 16] {
    if hex.len() != 32 {
        eprintln!("Error: {flag} requires exactly 32 hex characters (16 bytes), got {}", hex.len());
        std::process::exit(1);
    }
    let mut out = [0u8; 16];
    for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
        out[i] = (hex_nibble(pair[0], flag) << 4) | hex_nibble(pair[1], flag);
    }
    out
}

fn parse_hex8(hex: &str, flag: &str) -> [u8; 8] {
    if hex.len() != 16 {
        eprintln!("Error: {flag} requires exactly 16 hex characters (8 bytes), got {}", hex.len());
        std::process::exit(1);
    }
    let mut out = [0u8; 8];
    for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
        out[i] = (hex_nibble(pair[0], flag) << 4) | hex_nibble(pair[1], flag);
    }
    out
}

fn hex_nibble(c: u8, flag: &str) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => {
            eprintln!("Error: invalid hex character '{}' in {flag}", c as char);
            std::process::exit(1);
        }
    }
}

fn parse_args() -> Args {
    let mut host: Option<String> = None;
    let mut regist_key: Option<[u8; 16]> = None;
    let mut morning: Option<[u8; 16]> = None;
    let mut psn_account_id: Option<[u8; 8]> = None;
    let mut ps5 = true;
    let mut resolution = VideoResolutionPreset::P720;
    let mut fps = VideoFpsPreset::Fps60;
    let mut enable_dualsense = true;

    let mut iter = std::env::args().skip(1).peekable();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--host" | "-H" => {
                host = iter.next();
            }
            "--regist-key" | "-k" => {
                if let Some(val) = iter.next() {
                    regist_key = Some(parse_hex16(&val, "--regist-key"));
                }
            }
            "--morning" | "-m" => {
                if let Some(val) = iter.next() {
                    morning = Some(parse_hex16(&val, "--morning"));
                }
            }
            "--psn-account-id" | "-a" => {
                if let Some(val) = iter.next() {
                    psn_account_id = Some(parse_hex8(&val, "--psn-account-id"));
                }
            }
            "--ps4" => {
                ps5 = false;
            }
            "--resolution" | "-r" => {
                if let Some(val) = iter.next() {
                    resolution = match val.as_str() {
                        "360" => VideoResolutionPreset::P360,
                        "540" => VideoResolutionPreset::P540,
                        "720" => VideoResolutionPreset::P720,
                        "1080" => VideoResolutionPreset::P1080,
                        other => {
                            eprintln!("Unknown resolution '{other}'; choose 360|540|720|1080");
                            std::process::exit(1);
                        }
                    };
                }
            }
            "--fps" | "-f" => {
                if let Some(val) = iter.next() {
                    fps = match val.as_str() {
                        "30" => VideoFpsPreset::Fps30,
                        "60" => VideoFpsPreset::Fps60,
                        other => {
                            eprintln!("Unknown fps '{other}'; choose 30|60");
                            std::process::exit(1);
                        }
                    };
                }
            }
            "--no-dualsense" => {
                enable_dualsense = false;
            }
            "--help" | "-h" => {
                eprintln!(concat!(
                    "Usage: session --host <IP> --regist-key <hex32> --morning <hex32>\n",
                    "               --psn-account-id <hex16> [options]\n",
                    "\n",
                    "Required:\n",
                    "  -H, --host <IP>              Console hostname or IP address\n",
                    "  -k, --regist-key <hex32>     rp_regist_key from registration (32 hex)\n",
                    "  -m, --morning <hex32>        rp_key from registration (32 hex)\n",
                    "  -a, --psn-account-id <hex16> PSN account ID (16 hex)\n",
                    "\n",
                    "Options:\n",
                    "      --ps4                    Target is a PS4 (default: PS5)\n",
                    "  -r, --resolution <res>       360|540|720|1080 (default: 720)\n",
                    "  -f, --fps <fps>              30|60 (default: 60)\n",
                    "      --no-dualsense           Disable DualSense features",
                ));
                std::process::exit(0);
            }
            other => eprintln!("Unknown argument: {other}"),
        }
    }

    Args {
        host: host.unwrap_or_else(|| {
            eprintln!("Error: --host is required");
            std::process::exit(1);
        }),
        regist_key: regist_key.unwrap_or_else(|| {
            eprintln!("Error: --regist-key is required");
            std::process::exit(1);
        }),
        morning: morning.unwrap_or_else(|| {
            eprintln!("Error: --morning is required");
            std::process::exit(1);
        }),
        psn_account_id: psn_account_id.unwrap_or_else(|| {
            eprintln!("Error: --psn-account-id is required");
            std::process::exit(1);
        }),
        ps5,
        resolution,
        fps,
        enable_dualsense,
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

/// Why the main loop exited.
#[derive(Debug)]
enum ExitReason {
    /// The console sent a Quit event.
    SessionQuit { reason: QuitReason, reason_str: Option<String> },
    /// 30-second auto-timeout elapsed.
    Timeout,
    /// User pressed Ctrl-C.
    CtrlC,
    /// The quit-signal channel closed unexpectedly.
    ChannelClosed,
}

const SESSION_TIMEOUT: Duration = Duration::from_secs(30);

fn main() {
    let args = parse_args();

    // ── 1. Initialise the C library ──────────────────────────────────────────
    chiaki::init().expect("chiaki_lib_init failed");

    // ── 2. Logger ────────────────────────────────────────────────────────────
    let log = Arc::new(Log::new(
        LogLevel::INFO | LogLevel::WARNING | LogLevel::ERROR,
        |level, msg| println!("[{level:?}] {msg}"),
    ));

    // ── 3. Video profile ─────────────────────────────────────────────────────
    let video_profile = VideoProfile::preset(args.resolution, args.fps);
    println!(
        "Video profile: {}x{}  {}fps  {:?}  {}kbps",
        video_profile.width,
        video_profile.height,
        video_profile.max_fps,
        video_profile.codec,
        video_profile.bitrate,
    );

    // ── 4. ConnectInfo ───────────────────────────────────────────────────────
    let connect_info = ConnectInfo::builder()
        .host(args.host.clone())
        .ps5(args.ps5)
        .regist_key(args.regist_key)
        .morning(args.morning)
        .psn_account_id(args.psn_account_id)
        .video_profile(video_profile)
        .video_profile_auto_downgrade(true)
        .enable_keyboard(true)
        .enable_dualsense(args.enable_dualsense)
        .packet_loss_max(0.05)           // tolerate up to 5% packet loss
        .enable_idr_on_fec_failure(true) // request keyframe on unrecoverable FEC failure
        .build()
        .expect("Invalid host string (contains null byte)");

    // ── 5. Quit channel ──────────────────────────────────────────────────────
    //
    // The event callback (C background thread) sends a QuitSignal here when
    // the session ends.  The main loop receives it and exits cleanly.
    let (quit_tx, quit_rx) = mpsc::channel::<QuitSignal>();

    // ── 6. Frame counters ────────────────────────────────────────────────────
    let video_frames = Arc::new(AtomicU64::new(0));
    let video_frames_cb = Arc::clone(&video_frames);

    let (audio_sink, audio_count) = CountingAudioSink::new("audio");
    let (haptics_sink, haptics_count) = CountingAudioSink::new("haptics");

    // ── 7. Create session ────────────────────────────────────────────────────
    let mut session = Session::new(connect_info, Arc::clone(&log))
        .expect("Failed to create session");

    // ── 8a. Event callback ───────────────────────────────────────────────────
    session.set_event_callback(move |event| {
        match &event {
            Event::Connected => {
                println!("[session] Connected — stream is live.");
            }

            Event::LoginPinRequest { pin_incorrect } => {
                if *pin_incorrect {
                    eprintln!("[session] Entered PIN was incorrect.");
                } else {
                    eprintln!("[session] Console is requesting a login PIN.");
                }
                // In a real application call session.set_login_pin() here.
                // This example simply waits for the session to time out.
                eprintln!("[session] Hint: call Session::set_login_pin() to supply the PIN.");
            }

            Event::NicknameReceived(name) => {
                println!("[session] Console nickname: {name}");
            }

            Event::Rumble { left, right, .. } => {
                if *left > 0 || *right > 0 {
                    println!("[session] Rumble  left={left}  right={right}");
                }
            }

            Event::TriggerEffects { type_left, type_right, .. } => {
                println!("[session] Trigger effects  left_type={type_left}  right_type={type_right}");
            }

            Event::LedColor(rgb) => {
                println!("[session] LED  #{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
            }

            Event::HapticIntensity(intensity) => {
                println!("[session] Haptic intensity: {intensity:?}");
            }

            Event::TriggerIntensity(intensity) => {
                println!("[session] Trigger intensity: {intensity:?}");
            }

            Event::PlayerIndex(idx) => {
                println!("[session] Player index: {idx}");
            }

            Event::MotionReset => {
                println!("[session] Motion reset requested.");
            }

            Event::KeyboardOpen => {
                println!("[session] On-screen keyboard opened.");
            }

            Event::KeyboardTextChange(text) => {
                println!("[session] Keyboard text: {text:?}");
            }

            Event::KeyboardRemoteClose => {
                println!("[session] On-screen keyboard closed by console.");
            }

            Event::Holepunch { finished } => {
                println!("[session] Holepunch finished={finished}");
            }

            Event::Regist(host) => {
                println!("[session] Auto-registration completed  target={:?}", host.target);
            }

            Event::Quit { reason, reason_str } => {
                println!(
                    "[session] Quit  reason={reason:?}  msg={:?}",
                    reason_str.as_deref().unwrap_or(""),
                );
                // Signal the main loop to exit.
                let _ = quit_tx.send(QuitSignal {
                    reason: *reason,
                    reason_str: reason_str.clone(),
                });
            }
        }
    });

    // ── 8b. Video callback ───────────────────────────────────────────────────
    //
    // Receives raw H.264 / H.265 NAL unit data on a C background thread.
    // Return `true` on success; return `false` to request a keyframe (IDR).
    session.set_video_callback(move |frame, frames_lost, frame_recovered| {
        let n = video_frames_cb.fetch_add(1, Ordering::Relaxed) + 1;
        if frames_lost > 0 {
            println!(
                "[video] #{n}  lost={frames_lost}  recovered={frame_recovered}  {}B",
                frame.len(),
            );
        }
        true
    });

    // ── 8c. Audio / haptics sinks ────────────────────────────────────────────
    session.set_audio_sink(audio_sink);
    session.set_haptics_sink(haptics_sink);

    // ── 9. Start streaming ───────────────────────────────────────────────────
    println!(
        "Connecting to {} ({})  resolution={:?}  fps={:?}  dualsense={}",
        args.host,
        if args.ps5 { "PS5" } else { "PS4" },
        args.resolution,
        args.fps,
        args.enable_dualsense,
    );
    session.start().expect("Failed to start session");
    println!(
        "Session started.  Will auto-exit after {}s or on Ctrl-C…",
        SESSION_TIMEOUT.as_secs(),
    );

    // ── 10. Ctrl-C handler ───────────────────────────────────────────────────
    let ctrlc_flag = Arc::new(AtomicBool::new(false));
    {
        let flag = Arc::clone(&ctrlc_flag);
        ctrlc::set_handler(move || {
            flag.store(true, Ordering::SeqCst);
        })
        .expect("Failed to set Ctrl-C handler");
    }

    // ── 11. Main loop — send controller state at ~60 Hz ─────────────────────
    //
    // In a real application you would poll a gamepad here (e.g. via SDL2) and
    // build a meaningful ControllerState from it.
    let state = ControllerState::idle();
    let mut last_stats = Instant::now();
    let start_time = Instant::now();
    let exit_reason: ExitReason;

    loop {
        // Check whether the session has ended.
        match quit_rx.try_recv() {
            Ok(sig) => {
                exit_reason = ExitReason::SessionQuit {
                    reason: sig.reason,
                    reason_str: sig.reason_str.clone(),
                };
                print!("[main] Quit: {:?}", sig.reason);
                if let Some(msg) = &sig.reason_str {
                    print!("  ({msg})");
                }
                println!();
                break;
            }
            Err(mpsc::TryRecvError::Empty) => {
                // Still running — send the current controller snapshot.
                if let Err(e) = session.set_controller_state(&state) {
                    eprintln!("[main] set_controller_state: {e}");
                    exit_reason = ExitReason::ChannelClosed;
                    break;
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                // The event callback was dropped — session ended unexpectedly.
                eprintln!("[main] Event channel closed unexpectedly.");
                exit_reason = ExitReason::ChannelClosed;
                break;
            }
        }

        // Check Ctrl-C.
        if ctrlc_flag.load(Ordering::SeqCst) {
            println!("[main] Ctrl-C received — stopping.");
            exit_reason = ExitReason::CtrlC;
            break;
        }

        // Check 30-second auto-timeout.
        let elapsed = start_time.elapsed();
        if elapsed >= SESSION_TIMEOUT {
            println!(
                "[main] {}s timeout reached — stopping.",
                SESSION_TIMEOUT.as_secs()
            );
            exit_reason = ExitReason::Timeout;
            break;
        }

        // Print brief statistics every 5 seconds.
        if last_stats.elapsed() >= Duration::from_secs(5) {
            println!(
                "[stats] elapsed={:.1}s  video={} frames  audio={} frames  haptics={} frames",
                elapsed.as_secs_f32(),
                video_frames.load(Ordering::Relaxed),
                audio_count.load(Ordering::Relaxed),
                haptics_count.load(Ordering::Relaxed),
            );
            last_stats = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(16)); // ~60 Hz tick
    }

    let session_duration = start_time.elapsed();

    // ── 12. Stop and join ────────────────────────────────────────────────────
    println!("Stopping session…");
    let _ = session.stop();
    session.join().expect("Failed to join session thread");

    // ── 13. Summary ──────────────────────────────────────────────────────────
    println!();
    println!("--- Session summary ---");
    println!(
        "  Exit reason  : {}",
        match &exit_reason {
            ExitReason::SessionQuit { reason, reason_str } => format!(
                "SessionQuit ({reason:?}){}",
                reason_str
                    .as_deref()
                    .map(|s| format!(" — {s}"))
                    .unwrap_or_default()
            ),
            ExitReason::Timeout => format!("Timeout ({}s)", SESSION_TIMEOUT.as_secs()),
            ExitReason::CtrlC => "Ctrl-C".to_string(),
            ExitReason::ChannelClosed => "ChannelClosed".to_string(),
        }
    );
    println!(
        "  Duration     : {:.1}s",
        session_duration.as_secs_f64()
    );
    println!("  Video frames : {}", video_frames.load(Ordering::Relaxed));
    println!("  Audio frames : {}", audio_count.load(Ordering::Relaxed));
    println!("  Haptic frames: {}", haptics_count.load(Ordering::Relaxed));
}
