// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! Example: Connect to a PlayStation console, receive the video/audio stream,
//! and perform a simple sequence of controller inputs.
//!
//! Features:
//!   - Establishes a streaming session (discovery and registration not included)
//!   - Writes raw video frames (H.264/H.265 NAL units) to a file
//!   - Writes raw Opus audio frames in length-prefixed format to a file
//!   - After connecting, executes a timed control sequence:
//!       2.0 s  →  Press PS button (go to home screen)
//!       4.0 s  →  Press D-Pad Up
//!       4.8 s  →  Press D-Pad Down
//!       5.6 s  →  Press D-Pad Left
//!       6.4 s  →  Press D-Pad Right
//!       7.2 s  →  Press Cross (confirm)
//!
//! Usage:
//!   cargo run --example stream_and_control -- \
//!       --host 192.168.1.10 \
//!       --regist-key <32 hex chars> \
//!       --morning <32 hex chars> \
//!       --psn-account-id <16 hex chars>
//!
//! Optional flags:
//!   --ps4                              Target is a PS4 (default: PS5)
//!   --resolution 360|540|720|1080      Video resolution (default: 720)
//!   --fps 30|60                        Frame rate (default: 60)
//!   --no-dualsense                     Disable DualSense features
//!   --output-video <path>              Video output file (default: output.h264 / output.h265)
//!   --output-audio <path>              Audio output file (default: output.opus.bin)
//!   --duration <seconds>               Maximum recording duration (default: 30 s)
//!
//! The video file can be played back with:
//!   ffplay -f h264 output.h264
//!   ffplay -f hevc output.h265
//!
//! Audio file format (output.opus.bin):
//!   Per-frame layout → [4-byte big-endian length][Opus frame data]
//! Decode with:
//!   ffprobe -f data output.opus.bin

use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use chiaki::prelude::*;

// ── Video writer ───────────────────────────────────────────────────────────────

/// Writes raw H.264/H.265 NAL units to a file.
///
/// The resulting file can be played directly with
/// `ffplay -f h264 <file>` or `ffplay -f hevc <file>`.
struct VideoFileWriter {
    writer: BufWriter<File>,
    frame_count: Arc<AtomicU64>,
}

impl VideoFileWriter {
    fn open(path: &str) -> std::io::Result<(Self, Arc<AtomicU64>)> {
        let file = File::create(path)?;
        let counter = Arc::new(AtomicU64::new(0));
        Ok((
            VideoFileWriter {
                writer: BufWriter::with_capacity(256 * 1024, file),
                frame_count: Arc::clone(&counter),
            },
            counter,
        ))
    }

    /// Appends one NAL unit to the file; returns whether the write succeeded.
    fn write_frame(&mut self, data: &[u8]) -> bool {
        if let Err(e) = self.writer.write_all(data) {
            eprintln!("[video] Write failed: {e}");
            return false;
        }
        self.frame_count.fetch_add(1, Ordering::Relaxed);
        true
    }
}

// ── Audio writer ───────────────────────────────────────────────────────────────

/// AudioSink that writes Opus frames to a file in length-prefixed binary format.
///
/// File format: per frame → [4-byte big-endian u32 = frame length][frame data]
struct OpusFileAudioSink {
    writer: BufWriter<File>,
    label: &'static str,
    frame_count: Arc<AtomicU64>,
}

impl OpusFileAudioSink {
    fn open(path: &str, label: &'static str) -> std::io::Result<(Self, Arc<AtomicU64>)> {
        let file = File::create(path)?;
        let counter = Arc::new(AtomicU64::new(0));
        Ok((
            OpusFileAudioSink {
                writer: BufWriter::with_capacity(64 * 1024, file),
                label,
                frame_count: Arc::clone(&counter),
            },
            counter,
        ))
    }
}

impl AudioSink for OpusFileAudioSink {
    fn on_header(&mut self, header: AudioHeader) {
        println!(
            "[{}] audio header: {}ch  {}bit  {}Hz  frame_size={}",
            self.label, header.channels, header.bits, header.rate, header.frame_size,
        );
    }

    fn on_frame(&mut self, opus_data: &[u8]) {
        // Write a 4-byte big-endian length prefix followed by the frame data.
        let len = opus_data.len() as u32;
        if self.writer.write_all(&len.to_be_bytes()).is_err()
            || self.writer.write_all(opus_data).is_err()
        {
            eprintln!("[{}] Audio write failed", self.label);
            return;
        }
        self.frame_count.fetch_add(1, Ordering::Relaxed);
    }
}

// ── Control sequence ──────────────────────────────────────────────────────────

/// A timed input step executed after the session connects.
///
/// Each `ControlStep` means "hold `buttons` for `hold` milliseconds,
/// starting at `connected_at + offset`".
struct ControlStep {
    offset: Duration,    // time since connected_at when the press begins
    hold: Duration,      // how long to hold the button(s)
    buttons: ControllerButtons,
    label: &'static str,
}

/// Builds the demo control sequence: go home, navigate with D-Pad, confirm.
fn build_control_sequence() -> Vec<ControlStep> {
    vec![
        ControlStep {
            offset: Duration::from_millis(2000),
            hold:   Duration::from_millis(400),
            buttons: ControllerButtons::PS,
            label: "PS button (home screen)",
        },
        ControlStep {
            offset: Duration::from_millis(4000),
            hold:   Duration::from_millis(300),
            buttons: ControllerButtons::DPAD_UP,
            label: "D-Pad Up",
        },
        ControlStep {
            offset: Duration::from_millis(4800),
            hold:   Duration::from_millis(300),
            buttons: ControllerButtons::DPAD_DOWN,
            label: "D-Pad Down",
        },
        ControlStep {
            offset: Duration::from_millis(5600),
            hold:   Duration::from_millis(300),
            buttons: ControllerButtons::DPAD_LEFT,
            label: "D-Pad Left",
        },
        ControlStep {
            offset: Duration::from_millis(6400),
            hold:   Duration::from_millis(300),
            buttons: ControllerButtons::DPAD_RIGHT,
            label: "D-Pad Right",
        },
        ControlStep {
            offset: Duration::from_millis(7200),
            hold:   Duration::from_millis(300),
            buttons: ControllerButtons::CROSS,
            label: "Cross (confirm)",
        },
    ]
}

/// Returns the ControllerState that should be sent at the current instant.
fn compute_controller_state(
    connected_at: Instant,
    steps: &[ControlStep],
) -> ControllerState {
    let elapsed = connected_at.elapsed();
    let mut state = ControllerState::idle();

    for step in steps {
        let start = step.offset;
        let end = step.offset + step.hold;
        if elapsed >= start && elapsed < end {
            let current = state.buttons();
            state.set_buttons(current | step.buttons);
        }
    }

    state
}

/// Logs press/release transitions that occurred since the previous tick.
fn check_step_transitions(
    connected_at: Instant,
    steps: &[ControlStep],
    prev_elapsed: Duration,
) {
    let elapsed = connected_at.elapsed();
    for step in steps {
        if prev_elapsed < step.offset && elapsed >= step.offset {
            println!("[ctrl] > Press:   {}", step.label);
        }
        let end = step.offset + step.hold;
        if prev_elapsed < end && elapsed >= end {
            println!("[ctrl] . Release: {}", step.label);
        }
    }
}

// ── Quit signal ───────────────────────────────────────────────────────────────

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
    output_video: Option<String>,
    output_audio: Option<String>,
    duration: Duration,
}

fn parse_hex_bytes<const N: usize>(hex: &str, flag: &str) -> [u8; N] {
    let expected = N * 2;
    if hex.len() != expected {
        eprintln!("Error: {flag} requires exactly {expected} hex characters ({N} bytes), got {}", hex.len());
        std::process::exit(1);
    }
    let mut out = [0u8; N];
    for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
        out[i] = (nibble(pair[0], flag) << 4) | nibble(pair[1], flag);
    }
    out
}

fn nibble(c: u8, flag: &str) -> u8 {
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
    let mut output_video: Option<String> = None;
    let mut output_audio: Option<String> = None;
    let mut duration = Duration::from_secs(30);

    let mut iter = std::env::args().skip(1).peekable();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--host" | "-H" => {
                host = iter.next();
            }
            "--regist-key" | "-k" => {
                if let Some(val) = iter.next() {
                    regist_key = Some(parse_hex_bytes::<16>(&val, "--regist-key"));
                }
            }
            "--morning" | "-m" => {
                if let Some(val) = iter.next() {
                    morning = Some(parse_hex_bytes::<16>(&val, "--morning"));
                }
            }
            "--psn-account-id" | "-a" => {
                if let Some(val) = iter.next() {
                    psn_account_id = Some(parse_hex_bytes::<8>(&val, "--psn-account-id"));
                }
            }
            "--ps4" => {
                ps5 = false;
            }
            "--resolution" | "-r" => {
                if let Some(val) = iter.next() {
                    resolution = match val.as_str() {
                        "360"  => VideoResolutionPreset::P360,
                        "540"  => VideoResolutionPreset::P540,
                        "720"  => VideoResolutionPreset::P720,
                        "1080" => VideoResolutionPreset::P1080,
                        other  => {
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
            "--output-video" | "-V" => {
                output_video = iter.next();
            }
            "--output-audio" | "-A" => {
                output_audio = iter.next();
            }
            "--duration" | "-d" => {
                if let Some(val) = iter.next() {
                    match val.parse::<u64>() {
                        Ok(secs) => duration = Duration::from_secs(secs),
                        Err(_) => {
                            eprintln!("Error: --duration requires a positive integer (seconds)");
                            std::process::exit(1);
                        }
                    }
                }
            }
            "--help" | "-h" => {
                println!(concat!(
                    "Usage: stream_and_control --host <IP> --regist-key <hex32> --morning <hex32>\n",
                    "                          --psn-account-id <hex16> [options]\n",
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
                    "      --no-dualsense           Disable DualSense features\n",
                    "  -V, --output-video <path>    Video output file\n",
                    "  -A, --output-audio <path>    Audio output file (length-prefixed Opus frames)\n",
                    "  -d, --duration <secs>        Maximum recording duration (default: 30 s)\n",
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
        output_video,
        output_audio,
        duration,
    }
}

// ── Exit reason ───────────────────────────────────────────────────────────────

#[derive(Debug)]
enum ExitReason {
    /// The console sent a Quit event.
    SessionQuit { reason: QuitReason, reason_str: Option<String> },
    /// The configured duration elapsed.
    Timeout,
    /// User pressed Ctrl-C.
    CtrlC,
    /// The quit-signal channel closed unexpectedly.
    ChannelClosed,
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();

    // ── 1. Initialise the C library ──────────────────────────────────────────
    chiaki::init().expect("chiaki_lib_init failed");

    // ── 2. Logger ────────────────────────────────────────────────────────────
    let log = Log::new(
        LogLevel::INFO | LogLevel::WARNING | LogLevel::ERROR,
        |level, msg| println!("[{level:?}] {msg}"),
    );

    // ── 3. Video output file ─────────────────────────────────────────────────
    //
    // The video codec is only known after connecting (H.264 → .h264,
    // H.265 → .h265).  For simplicity we create the file up-front; rename it
    // after the first frame if needed.
    //
    // Note: the video callback runs on a C background thread, so the writer is
    // protected by a Mutex.
    let default_video_path = "output.h264".to_string();
    let video_path = args.output_video.unwrap_or(default_video_path);
    let (video_writer, video_frame_count) =
        VideoFileWriter::open(&video_path).expect("Failed to create video output file");
    let video_byte_count = Arc::new(AtomicU64::new(0));
    let video_byte_count_cb = Arc::clone(&video_byte_count);

    // Move video_writer into the callback closure; wrap in Mutex for thread safety.
    let video_writer = Mutex::new(video_writer);

    // ── 4. Audio output file ─────────────────────────────────────────────────
    let default_audio_path = "output.opus.bin".to_string();
    let audio_path = args.output_audio.unwrap_or(default_audio_path);
    let (audio_sink, audio_frame_count) =
        OpusFileAudioSink::open(&audio_path, "audio").expect("Failed to create audio output file");
    // The haptics channel is not written to a file — just counted.
    let haptics_counter = Arc::new(AtomicU64::new(0));
    let haptics_counter_sink = {
        struct CountSink(Arc<AtomicU64>);
        impl AudioSink for CountSink {
            fn on_header(&mut self, h: AudioHeader) {
                println!("[haptics] audio header: {}ch  {}Hz", h.channels, h.rate);
            }
            fn on_frame(&mut self, _data: &[u8]) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        CountSink(Arc::clone(&haptics_counter))
    };

    // ── 5. Video profile ─────────────────────────────────────────────────────
    let video_profile = VideoProfile::preset(args.resolution, args.fps);
    println!(
        "Video profile: {}x{}  {}fps  {:?}  {}kbps",
        video_profile.width, video_profile.height,
        video_profile.max_fps, video_profile.codec,
        video_profile.bitrate,
    );

    // ── 6. ConnectInfo ───────────────────────────────────────────────────────
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
        .packet_loss_max(0.05)
        .enable_idr_on_fec_failure(true)
        .build()
        .expect("Invalid host string (contains null byte)");

    // ── 7. Channels ──────────────────────────────────────────────────────────
    let (quit_tx, quit_rx) = mpsc::channel::<QuitSignal>();
    // connected_tx fires when the Connected event arrives, signalling the main
    // loop to start sending controller inputs.
    let (connected_tx, connected_rx) = mpsc::channel::<()>();

    // ── 8. Create session ────────────────────────────────────────────────────
    let mut session = Session::new(connect_info, Arc::clone(&log))
        .expect("Failed to create session");

    // ── 9a. Event callback ───────────────────────────────────────────────────
    session.set_event_callback(move |event| {
        match &event {
            Event::Connected => {
                println!("[session] Connected — stream is live.");
                // Signal the main loop to begin the control sequence.
                let _ = connected_tx.send(());
            }
            Event::LoginPinRequest { pin_incorrect } => {
                if *pin_incorrect {
                    eprintln!("[session] Entered PIN was incorrect.");
                } else {
                    eprintln!("[session] Console is requesting a login PIN.");
                }
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
                let _ = quit_tx.send(QuitSignal {
                    reason: *reason,
                    reason_str: reason_str.clone(),
                });
            }
        }
    });

    // ── 9b. Video callback ───────────────────────────────────────────────────
    //
    // Receives one H.264/H.265 NAL unit per call on a C background thread.
    // Return `true` on success; return `false` to request a keyframe (IDR).
    session.set_video_callback(move |frame, frames_lost, frame_recovered| {
        // Append the NAL data directly to the output file.
        let mut w = video_writer.lock().unwrap();
        let ok = w.write_frame(frame);
        video_byte_count_cb.fetch_add(frame.len() as u64, Ordering::Relaxed);

        if frames_lost > 0 {
            println!(
                "[video] #{n}  lost={frames_lost}  recovered={frame_recovered}  {}B",
                frame.len(),
                n = w.frame_count.load(Ordering::Relaxed),
            );
        }

        ok // return false on write failure to request a keyframe
    });

    // ── 9c. Audio / haptics sinks ────────────────────────────────────────────
    session.set_audio_sink(audio_sink);
    session.set_haptics_sink(haptics_counter_sink);

    // ── 10. Start streaming ──────────────────────────────────────────────────
    println!(
        "Connecting to {} ({})  resolution={:?}  fps={:?}  dualsense={}",
        args.host,
        if args.ps5 { "PS5" } else { "PS4" },
        args.resolution,
        args.fps,
        args.enable_dualsense,
    );
    session.start().expect("Failed to start session");
    println!("Session started.  Will auto-exit after {}s or on Ctrl-C…", args.duration.as_secs());
    println!("Video output: {video_path}");
    println!("Audio output: {audio_path}");

    // ── 11. Ctrl-C handler ───────────────────────────────────────────────────
    let ctrlc_flag = Arc::new(AtomicBool::new(false));
    {
        let flag = Arc::clone(&ctrlc_flag);
        ctrlc::set_handler(move || {
            flag.store(true, Ordering::SeqCst);
        })
        .expect("Failed to set Ctrl-C handler");
    }

    // ── 12. Control sequence ─────────────────────────────────────────────────
    let control_steps = build_control_sequence();
    // connected_at is recorded when the Connected event is received.
    let mut connected_at: Option<Instant> = None;

    // ── 13. Main loop (~60 Hz) ───────────────────────────────────────────────
    let start_time = Instant::now();
    let mut last_stats = Instant::now();
    let mut prev_ctrl_elapsed = Duration::ZERO;
    let exit_reason: ExitReason;

    loop {
        // Check whether the session has ended.
        match quit_rx.try_recv() {
            Ok(sig) => {
                exit_reason = ExitReason::SessionQuit {
                    reason: sig.reason,
                    reason_str: sig.reason_str,
                };
                break;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                eprintln!("[main] Event channel closed unexpectedly.");
                exit_reason = ExitReason::ChannelClosed;
                break;
            }
        }

        // Check whether the Connected event has just arrived.
        if connected_at.is_none() {
            if connected_rx.try_recv().is_ok() {
                connected_at = Some(Instant::now());
                println!("[main] Starting control sequence…");
            }
        }

        // Compute and send the current controller state.
        let controller_state = if let Some(ca) = connected_at {
            // Log any press/release transitions since the last tick.
            check_step_transitions(ca, &control_steps, prev_ctrl_elapsed);
            prev_ctrl_elapsed = ca.elapsed();
            compute_controller_state(ca, &control_steps)
        } else {
            // Not yet connected — send an idle state (all buttons released).
            ControllerState::idle()
        };

        if let Err(e) = session.set_controller_state(&controller_state) {
            eprintln!("[main] set_controller_state: {e}");
            exit_reason = ExitReason::ChannelClosed;
            break;
        }

        // Check Ctrl-C.
        if ctrlc_flag.load(Ordering::SeqCst) {
            println!("[main] Ctrl-C received — stopping.");
            exit_reason = ExitReason::CtrlC;
            break;
        }

        // Check timeout.
        let elapsed = start_time.elapsed();
        if elapsed >= args.duration {
            println!("[main] {}s timeout reached — stopping.", args.duration.as_secs());
            exit_reason = ExitReason::Timeout;
            break;
        }

        // Print brief statistics every 5 seconds.
        if last_stats.elapsed() >= Duration::from_secs(5) {
            println!(
                "[stats] elapsed={:.1}s  video={} frames ({} KB)  audio={} frames  haptics={} frames",
                elapsed.as_secs_f32(),
                video_frame_count.load(Ordering::Relaxed),
                video_byte_count.load(Ordering::Relaxed) / 1024,
                audio_frame_count.load(Ordering::Relaxed),
                haptics_counter.load(Ordering::Relaxed),
            );
            last_stats = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(16)); // ~60 Hz
    }

    let session_duration = start_time.elapsed();

    // ── 14. Stop and join ────────────────────────────────────────────────────
    println!("Stopping session…");
    let _ = session.stop();
    session.join().expect("Failed to join session thread");

    // ── 15. Session summary ──────────────────────────────────────────────────
    println!();
    println!("--- Session summary ---");
    println!(
        "  Exit reason  : {}",
        match &exit_reason {
            ExitReason::SessionQuit { reason, reason_str } => format!(
                "SessionQuit ({reason:?}){}",
                reason_str.as_deref().map(|s| format!(" — {s}")).unwrap_or_default()
            ),
            ExitReason::Timeout      => format!("Timeout ({}s)", args.duration.as_secs()),
            ExitReason::CtrlC        => "Ctrl-C".to_string(),
            ExitReason::ChannelClosed => "ChannelClosed".to_string(),
        }
    );
    println!("  Duration     : {:.1}s", session_duration.as_secs_f64());
    println!("  Video frames : {} ({} KB)", video_frame_count.load(Ordering::Relaxed), video_byte_count.load(Ordering::Relaxed) / 1024);
    println!("  Audio frames : {}", audio_frame_count.load(Ordering::Relaxed));
    println!("  Haptic frames: {}", haptics_counter.load(Ordering::Relaxed));
    println!("  Video file   : {video_path}");
    println!("  Audio file   : {audio_path}");
    println!();
    println!("Playback commands:");
    println!("  ffplay -f h264 {video_path}    # H.264");
    println!("  ffplay -f hevc {video_path}    # H.265 (if --no-dualsense may be hevc)");
    println!("---");
}
