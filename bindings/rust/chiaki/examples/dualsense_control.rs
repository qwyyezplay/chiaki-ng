// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! Example: PS5 remote play driven by a physical DualSense controller.
//!
//! Reads all inputs from a connected DualSense (or any SDL2 game controller)
//! and streams them to a PS5 console in real time.  Audio and video output
//! are written to files.
//!
//! This example uses [`StreamController`] to orchestrate the session,
//! controller management, DualSense haptics, and feedback command dispatch.
//! The `main` function only needs to:
//!
//! 1. Build a [`StreamControllerConfig`] with callbacks for video and audio.
//! 2. Call [`StreamController::tick`] at ~60 Hz to drive the control loop.
//! 3. Handle [`StreamNotification`]s (Connected, Quit, controller changes).
//!
//! DualSense inputs forwarded to the console
//! ──────────────────────────────────────────
//! • Face buttons (✕/○/□/△), shoulder (L1/R1), triggers (L2/R2)
//! • D-Pad, L3/R3 stick clicks, Options, Share/Create, PS, Touchpad click
//! • Left / right analog sticks  (i16, full-range)
//! • Analog triggers              (u8 0–255, L2 / R2 bits auto-synced)
//! • Touchpad multi-touch         (up to 2 fingers; mapped to 1920 × 1079 space)
//! • Gyroscope                    (rad/s, via SDL2 sensor API)
//! • Accelerometer                (m/s², via SDL2 sensor API)
//!
//! DualSense feedback received from the console
//! ─────────────────────────────────────────────
//! • Rumble            → `manager.set_rumble()`
//! • Adaptive triggers → `manager.set_trigger_effects()`
//! • LED colour        → `manager.change_led_color()`
//! • Player index      → `manager.change_player_index()`
//! • Motion reset      → `manager.reset_motion_controls()`
//!
//! Prerequisite: SDL2 ≥ 2.0.16 installed (send_effect API for DualSense feedback)
//!   macOS : brew install sdl2
//!   Ubuntu: apt-get install libsdl2-dev
//!
//! Usage:
//!   cargo run --example dualsense_control --features sdl-controller -- \
//!       --host 192.168.1.10 \
//!       --regist-key <32 hex chars> \
//!       --morning    <32 hex chars> \
//!       --psn-account-id <16 hex chars>
//!
//! Optional flags:
//!   --ps4                         Target is a PS4 (default: PS5)
//!   --resolution 360|540|720|1080 Video resolution (default: 720)
//!   --fps 30|60                   Frame rate (default: 60)
//!   --no-dualsense                Disable DualSense features
//!   --output-video <path>         Video file  (default: output.h264 / .h265)
//!   --output-audio <path>         Audio file  (default: output.opus.bin)
//!   --duration <secs>             Auto-stop after N seconds (default: run until Ctrl-C)
//!   --no-log-input                Suppress per-event input logging
//!
//! Playback:
//!   ffplay -f h264 output.h264
//!   ffplay -f hevc output.h265

use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chiaki::controller::ControllerButtons;
use chiaki::prelude::*;

// ── Video writer ───────────────────────────────────────────────────────────────

/// Writes raw H.264/H.265 NAL units to a file.
struct VideoFileWriter {
    writer: BufWriter<File>,
    frame_count: Arc<AtomicU64>,
    byte_count: Arc<AtomicU64>,
}

impl VideoFileWriter {
    fn open(path: &str) -> std::io::Result<(Self, Arc<AtomicU64>, Arc<AtomicU64>)> {
        let file = File::create(path)?;
        let frames = Arc::new(AtomicU64::new(0));
        let bytes = Arc::new(AtomicU64::new(0));
        Ok((
            VideoFileWriter {
                writer: BufWriter::with_capacity(256 * 1024, file),
                frame_count: Arc::clone(&frames),
                byte_count: Arc::clone(&bytes),
            },
            frames,
            bytes,
        ))
    }

    fn write_frame(&mut self, data: &[u8]) -> bool {
        if let Err(e) = self.writer.write_all(data) {
            eprintln!("[video] Write failed: {e}");
            return false;
        }
        self.frame_count.fetch_add(1, Ordering::Relaxed);
        self.byte_count.fetch_add(data.len() as u64, Ordering::Relaxed);
        true
    }
}

// ── Audio writer ───────────────────────────────────────────────────────────────

/// Writes Opus frames in length-prefixed binary format.
///
/// Per-frame layout: [4-byte big-endian u32 = frame length][frame data]
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
    duration: Option<Duration>,
    log_input: bool,
}

fn parse_hex_bytes<const N: usize>(hex: &str, flag: &str) -> [u8; N] {
    let expected = N * 2;
    if hex.len() != expected {
        eprintln!(
            "Error: {flag} requires exactly {expected} hex characters ({N} bytes), got {}",
            hex.len()
        );
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
    let mut duration: Option<Duration> = None;
    let mut log_input = true;

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
                        Ok(secs) => duration = Some(Duration::from_secs(secs)),
                        Err(_) => {
                            eprintln!("Error: --duration requires a positive integer (seconds)");
                            std::process::exit(1);
                        }
                    }
                }
            }
            "--log-input"    => { log_input = true;  }
            "--no-log-input" => { log_input = false; }
            "--help" | "-h" => {
                println!(concat!(
                    "Usage: dualsense_control --host <IP> --regist-key <hex32> --morning <hex32>\n",
                    "                         --psn-account-id <hex16> [options]\n",
                    "\n",
                    "Required:\n",
                    "  -H, --host <IP>              Console hostname or IP address\n",
                    "  -k, --regist-key <hex32>     rp_regist_key (32 hex chars)\n",
                    "  -m, --morning <hex32>        rp_key (32 hex chars)\n",
                    "  -a, --psn-account-id <hex16> PSN account ID (16 hex chars)\n",
                    "\n",
                    "Options:\n",
                    "      --ps4                    Target is a PS4 (default: PS5)\n",
                    "  -r, --resolution <res>       360|540|720|1080 (default: 720)\n",
                    "  -f, --fps <fps>              30|60 (default: 60)\n",
                    "      --no-dualsense           Disable DualSense features\n",
                    "  -V, --output-video <path>    Video output file\n",
                    "  -A, --output-audio <path>    Audio output file (length-prefixed Opus frames)\n",
                    "  -d, --duration <secs>        Auto-stop after N seconds\n",
                    "      --no-log-input           Suppress per-event input logging\n",
                ));
                std::process::exit(0);
            }
            other => eprintln!("[warn] Unknown argument: {other}"),
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
        log_input,
    }
}

// ── Exit reason ───────────────────────────────────────────────────────────────

#[derive(Debug)]
enum ExitReason {
    /// The console sent a Quit event.
    SessionQuit { reason: QuitReason, reason_str: Option<String> },
    /// Configured duration elapsed.
    Timeout,
    /// User pressed Ctrl-C.
    CtrlC,
}

// ── Input-log helpers ─────────────────────────────────────────────────────────

/// Throttle intervals for input logging (to avoid flooding stdout with IMU data).
const IMU_LOG_INTERVAL: Duration = Duration::from_millis(250);
const ANALOG_LOG_INTERVAL: Duration = Duration::from_millis(100);

/// Simple per-category timestamp throttle.
struct LogThrottle {
    last_imu: Instant,
    last_analog: Instant,
}

impl LogThrottle {
    fn new() -> Self {
        let far_past = Instant::now() - Duration::from_secs(10);
        Self { last_imu: far_past, last_analog: far_past }
    }

    fn imu(&mut self) -> bool {
        if self.last_imu.elapsed() >= IMU_LOG_INTERVAL {
            self.last_imu = Instant::now();
            true
        } else {
            false
        }
    }

    fn analog(&mut self) -> bool {
        if self.last_analog.elapsed() >= ANALOG_LOG_INTERVAL {
            self.last_analog = Instant::now();
            true
        } else {
            false
        }
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();

    // ── 1. Initialise chiaki C library ────────────────────────────────────────
    chiaki::init().expect("chiaki_lib_init failed");

    // ── 2. Logger ─────────────────────────────────────────────────────────────
    let log = Log::new(
        LogLevel::INFO | LogLevel::WARNING | LogLevel::ERROR,
        |level, msg| println!("[{level:?}] {msg}"),
    );

    // ── 3. SDL2 ───────────────────────────────────────────────────────────────
    let sdl_ctx = sdl2::init().expect("SDL2 init failed");
    let mut event_pump = sdl_ctx.event_pump().expect("SDL2 event_pump failed");

    // Ensure SDL audio subsystem is initialised (required for haptics device).
    let _audio_subsystem = sdl_ctx.audio().ok();

    // ── 4. Video output file ──────────────────────────────────────────────────
    let video_path = args.output_video.unwrap_or_else(|| "output.h264".to_string());
    let (video_writer, video_frame_count, video_byte_count) =
        VideoFileWriter::open(&video_path).expect("Failed to create video output file");
    let video_writer = Mutex::new(video_writer);

    // ── 5. Audio output file ──────────────────────────────────────────────────
    let audio_path = args.output_audio.unwrap_or_else(|| "output.opus.bin".to_string());
    let (audio_sink, audio_frame_count) =
        OpusFileAudioSink::open(&audio_path, "audio").expect("Failed to create audio output file");

    // ── 6. Video profile ──────────────────────────────────────────────────────
    let video_profile = VideoProfile::preset(args.resolution, args.fps);
    println!(
        "[session] Video: {}x{}  {}fps  {:?}  {}kbps",
        video_profile.width, video_profile.height,
        video_profile.max_fps, video_profile.codec,
        video_profile.bitrate,
    );

    // ── 7. ConnectInfo ────────────────────────────────────────────────────────
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

    // ── 8. StreamController config ────────────────────────────────────────────
    let config = StreamControllerConfig {
        connect_info,
        enable_dualsense: args.enable_dualsense,
        video_callback: Some(Box::new(move |frame, frames_lost, frame_recovered| {
            let mut w = video_writer.lock().unwrap();
            let ok = w.write_frame(frame);
            if frames_lost > 0 {
                println!(
                    "[video] #{n}  lost={frames_lost}  recovered={frame_recovered}  {}B",
                    frame.len(),
                    n = w.frame_count.load(Ordering::Relaxed),
                );
            }
            ok
        })),
        audio_sink: Some(Box::new(audio_sink)),
        event_callback: Some(Box::new(|event| {
            match event {
                Event::Connected => {
                    println!("[session] Connected — stream is live.");
                }
                Event::LoginPinRequest { pin_incorrect } => {
                    if *pin_incorrect {
                        eprintln!("[session] PIN was incorrect.");
                    } else {
                        eprintln!("[session] Console requests a login PIN.");
                    }
                    eprintln!("[session] Call Session::set_login_pin() to supply the PIN.");
                }
                Event::NicknameReceived(name) => {
                    println!("[session] Console nickname: {name}");
                }
                Event::Rumble { left, right, .. } => {
                    println!("[session] Rumble  left={left}  right={right}");
                }
                Event::TriggerEffects { type_left, type_right, left, right } => {
                    println!(
                        "[session] TriggerEffects  type_left={type_left}  type_right={type_right}\n\
                         [session]   left_data ={left:02x?}\n\
                         [session]   right_data={right:02x?}",
                    );
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
                }
            }
        })),
    };

    // ── 9. Create StreamController ────────────────────────────────────────────
    let mut ctrl = StreamController::new(config, Arc::clone(&log), &sdl_ctx)
        .expect("Failed to create StreamController");

    // Log initially opened controllers.
    if let Some(id) = ctrl.active_controller() {
        if let Some(info) = ctrl.manager().controller_info(id) {
            println!(
                "[ctrl] Primary controller: #{id} \"{}\"  {}  DS={} DS-Edge={}",
                info.name, info.vid_pid, info.is_dualsense, info.is_dualsense_edge,
            );
        }
    } else {
        println!("[ctrl] No controller found at start-up.");
        println!("[ctrl] Connect a DualSense via USB or Bluetooth — it will be detected automatically.");
    }

    // ── 10. Start streaming ───────────────────────────────────────────────────
    let duration_str = args
        .duration
        .map(|d| format!("{}s", d.as_secs()))
        .unwrap_or_else(|| "unlimited (Ctrl-C to stop)".to_string());

    println!(
        "[session] Connecting to {} ({})  resolution={:?}  fps={:?}  dualsense={}",
        args.host,
        if args.ps5 { "PS5" } else { "PS4" },
        args.resolution,
        args.fps,
        args.enable_dualsense,
    );
    ctrl.start().expect("Failed to start session");
    println!("[session] Started.  Duration: {duration_str}");
    println!("[output]  Video: {video_path}");
    println!("[output]  Audio: {audio_path}");

    // ── 11. Ctrl-C handler ────────────────────────────────────────────────────
    let ctrlc_flag = Arc::new(AtomicBool::new(false));
    {
        let flag = Arc::clone(&ctrlc_flag);
        ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))
            .expect("Failed to install Ctrl-C handler");
    }

    // ── 12. Per-frame state ───────────────────────────────────────────────────
    let log_input = args.log_input;
    let mut last_buttons = ControllerButtons::empty();
    let mut log_throttle = LogThrottle::new();

    // ── 13. Main loop (~60 Hz) ────────────────────────────────────────────────
    let start_time = Instant::now();
    let mut last_stats = Instant::now();
    let mut connected = false;
    let exit_reason: ExitReason;

    'main: loop {
        // ── Tick StreamController ─────────────────────────────────────────────
        match ctrl.tick(&mut event_pump) {
            Ok(Some(StreamNotification::Quit { reason, reason_str })) => {
                exit_reason = ExitReason::SessionQuit { reason, reason_str };
                break 'main;
            }
            Ok(Some(StreamNotification::Connected)) => {
                connected = true;
                println!("[main] Stream connected — DualSense input is now active.");
                println!(
                    "[main] Input logging: {}",
                    if log_input { "ON" } else { "OFF (--no-log-input)" },
                );
            }
            Ok(Some(StreamNotification::ActiveControllerChanged(new_id))) => {
                match new_id {
                    Some(id) => {
                        if let Some(info) = ctrl.manager().controller_info(id) {
                            println!(
                                "[ctrl] Active controller changed: #{id} \"{}\"  DS={}",
                                info.name, info.is_dualsense,
                            );
                        }
                    }
                    None => println!("[ctrl] No active controller."),
                }
                last_buttons = ControllerButtons::empty();
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("[main] tick error: {e}");
                exit_reason = ExitReason::SessionQuit {
                    reason: QuitReason::Stopped,
                    reason_str: Some(format!("tick error: {e}")),
                };
                break 'main;
            }
        }

        // ── Input logging ─────────────────────────────────────────────────────
        if log_input && connected {
            if let Some(id) = ctrl.active_controller() {
                if let Some(state) = ctrl.manager().controller_state(id) {
                    let state = state.clone();

                    // Button press / release (diff-based, always immediate).
                    let btns = state.buttons();
                    if btns != last_buttons {
                        let pressed  = btns & !last_buttons;
                        let released = last_buttons & !btns;
                        if !pressed.is_empty() {
                            println!("[input] PRESS    {pressed:?}  buttons={btns:?}");
                        }
                        if !released.is_empty() {
                            println!("[input] RELEASE  {released:?}  buttons={btns:?}");
                        }
                        last_buttons = btns;
                    }

                    // Analog triggers (≤ 10 Hz).
                    let (l2, r2) = (state.l2(), state.r2());
                    if (l2 > 0 || r2 > 0) && log_throttle.analog() {
                        println!("[input] Triggers  L2={l2:>3}  R2={r2:>3}  (0–255)");
                    }

                    // Analog sticks (≤ 10 Hz).
                    let (lx, ly) = state.left_stick();
                    let (rx, ry) = state.right_stick();
                    const DEADZONE: i16 = 2000;
                    if (lx.saturating_abs() > DEADZONE || ly.saturating_abs() > DEADZONE
                        || rx.saturating_abs() > DEADZONE || ry.saturating_abs() > DEADZONE)
                        && log_throttle.analog()
                    {
                        println!(
                            "[input] Sticks    L=({lx:+6},{ly:+6})  R=({rx:+6},{ry:+6})  (±32767)",
                        );
                    }

                    // IMU (gyro / accel / orientation) — ≤ 4 Hz.
                    let (gx, gy, gz) = state.gyro();
                    let (ax, ay, az) = state.accel();
                    let (ox, oy, oz, ow) = state.orient();
                    if log_throttle.imu() {
                        if gx.abs() > 0.05 || gy.abs() > 0.05 || gz.abs() > 0.05 {
                            println!(
                                "[input] Gyro      x={gx:+.4}  y={gy:+.4}  z={gz:+.4}  rad/s",
                            );
                        }
                        let accel_delta =
                            (ax.powi(2) + (ay - 1.0_f32).powi(2) + az.powi(2)).sqrt();
                        if accel_delta > 0.1 {
                            println!(
                                "[input] Accel     x={ax:+.4}  y={ay:+.4}  z={az:+.4}  g",
                            );
                        }
                        let orient_delta =
                            (ox.powi(2) + oy.powi(2) + oz.powi(2)).sqrt();
                        if orient_delta > 0.05 {
                            println!(
                                "[input] Orient    x={ox:.3}  y={oy:.3}  z={oz:.3}  w={ow:.3}",
                            );
                        }
                    }

                    // Touchpad contacts.
                    for touch in state.touches().iter() {
                        if touch.id >= 0 {
                            println!(
                                "[input] Touchpad  id={}  x={:>4}  y={:>4}",
                                touch.id, touch.x, touch.y,
                            );
                        }
                    }
                }
            }
        }

        // ── Ctrl-C ────────────────────────────────────────────────────────────
        if ctrlc_flag.load(Ordering::SeqCst) {
            println!("[main] Ctrl-C received — stopping.");
            exit_reason = ExitReason::CtrlC;
            break 'main;
        }

        // ── Optional timeout ──────────────────────────────────────────────────
        let elapsed = start_time.elapsed();
        if let Some(max_dur) = args.duration {
            if elapsed >= max_dur {
                println!("[main] {}s timeout reached — stopping.", max_dur.as_secs());
                exit_reason = ExitReason::Timeout;
                break 'main;
            }
        }

        // ── Periodic statistics (every 5 s) ───────────────────────────────────
        if last_stats.elapsed() >= Duration::from_secs(5) {
            println!(
                "[stats] elapsed={:.1}s  video={} frames ({} KB)  audio={} frames  haptics={} frames",
                elapsed.as_secs_f32(),
                video_frame_count.load(Ordering::Relaxed),
                video_byte_count.load(Ordering::Relaxed) / 1024,
                audio_frame_count.load(Ordering::Relaxed),
                ctrl.haptics_frame_count().load(Ordering::Relaxed),
            );
            if let Some(id) = ctrl.active_controller() {
                if let Some(state) = ctrl.manager().controller_state(id) {
                    let (ls_x, ls_y) = state.left_stick();
                    let (rs_x, rs_y) = state.right_stick();
                    let (gx, gy, gz) = state.gyro();
                    let (ax, ay, az) = state.accel();
                    let (ox, oy, oz, ow) = state.orient();
                    println!(
                        "[stats] buttons={:?}  L2={}  R2={}  LS=({ls_x},{ls_y})  RS=({rs_x},{rs_y})",
                        state.buttons(), state.l2(), state.r2(),
                    );
                    println!(
                        "[stats] gyro=({gx:.3},{gy:.3},{gz:.3}) rad/s  accel=({ax:.3},{ay:.3},{az:.3}) g",
                    );
                    println!(
                        "[stats] orient=({ox:.3},{oy:.3},{oz:.3},{ow:.3})",
                    );
                }
            }
            last_stats = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(16)); // ~60 Hz
    }

    let session_duration = start_time.elapsed();

    // ── 14. Stop ──────────────────────────────────────────────────────────────
    println!("[main] Stopping session…");
    ctrl.stop().expect("Failed to stop session");

    // ── 15. Session summary ───────────────────────────────────────────────────
    println!();
    println!("--- Session summary ---");
    println!(
        "  Exit reason  : {}",
        match &exit_reason {
            ExitReason::SessionQuit { reason, reason_str } => format!(
                "SessionQuit ({reason:?}){}",
                reason_str.as_deref().map(|s| format!(" — {s}")).unwrap_or_default()
            ),
            ExitReason::Timeout => format!(
                "Timeout ({}s)",
                args.duration.unwrap_or_default().as_secs()
            ),
            ExitReason::CtrlC => "Ctrl-C".to_string(),
        }
    );
    println!("  Duration     : {:.1}s", session_duration.as_secs_f64());
    println!(
        "  Video frames : {} ({} KB)",
        video_frame_count.load(Ordering::Relaxed),
        video_byte_count.load(Ordering::Relaxed) / 1024,
    );
    println!("  Audio frames : {}", audio_frame_count.load(Ordering::Relaxed));
    println!("  Video file   : {video_path}");
    println!("  Audio file   : {audio_path}");
    println!();
    println!("Playback commands:");
    println!("  ffplay -f h264 {video_path}   # H.264");
    println!("  ffplay -f hevc {video_path}   # H.265");
    println!("---");
}
