// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! Example: comprehensive controller input detection via SDL2.
//!
//! Demonstrates every input category supported by [`ControllerManager`]:
//!
//! | Category             | Display fields                               | Print rate   |
//! |----------------------|----------------------------------------------|--------------|
//! | Digital buttons      | `buttons` bitmask                            | on-change    |
//! | Analog triggers      | `l2`, `r2` (0–255)                           | ≤ 30 Hz      |
//! | Analog sticks        | `left_stick`, `right_stick` (±32767)         | ≤ 30 Hz      |
//! | Gyroscope            | `gyro` (rad/s)                               | ≤ 10 Hz      |
//! | Accelerometer        | `accel` (g)                                  | ≤ 10 Hz      |
//! | Orientation          | `orient` quaternion                          | ≤ 10 Hz      |
//! | Touchpad contacts    | touch slot id + pixel coordinates            | ≤ 30 Hz      |
//! | DualSense mic button | dedicated event                              | on-change    |
//! | Hotplug              | connect / disconnect                         | on-change    |
//!
//! ## Why rate-limiting is required
//!
//! SDL2 sensor events (gyro + accelerometer) fire at 200–400 Hz each.  Without
//! throttling, `StateChanged` generates thousands of `println!` calls per second.
//! Each `println!` acquires the stdout mutex and performs a `write(2)` syscall,
//! saturating the terminal pipe buffer and causing visible lag or dropped output.
//!
//! Two complementary fixes are applied:
//!
//! 1. **[`Throttle`]** — per-(controller, category) `Instant`-based gate that
//!    suppresses prints until a minimum interval has elapsed.
//! 2. **[`BufWriter`]** — all `writeln!` calls go into a heap buffer; a single
//!    `flush()` at the end of every event-loop tick drains it with one syscall.
//!
//! # Build & run
//!
//! ```bash
//! cargo run --example controllermanager --features sdl-controller
//! ```
//!
//! Press Ctrl-C to exit.

use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chiaki::controllermanager::{ControllerEvent, ControllerManager};

// ── Rate-limit constants ──────────────────────────────────────────────────────

/// Maximum print rate for IMU data (gyro / accel / orient): 10 Hz.
const SENSOR_INTERVAL: Duration = Duration::from_millis(100);
/// Maximum print rate for analog inputs (sticks / triggers / touch motion): 30 Hz.
const ANALOG_INTERVAL: Duration = Duration::from_millis(33);

// ── Throttle ──────────────────────────────────────────────────────────────────

/// Input categories used as throttle-table keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Category {
    Triggers,
    Sticks,
    Gyro,
    Accel,
    Orient,
    Touch,
}

/// Per-(controller_id, [`Category`]) print-rate limiter.
///
/// Stores the `Instant` of the last allowed print for every pair.
/// [`Throttle::allow`] returns `true` at most once per `interval`.
struct Throttle {
    last: HashMap<(u32, Category), Instant>,
}

impl Throttle {
    fn new() -> Self {
        Self {
            last: HashMap::new(),
        }
    }

    /// Returns `true` if at least `interval` has elapsed since the last allowed
    /// print for `(id, cat)`, updating the timestamp when it does.
    fn allow(&mut self, id: u32, cat: Category, interval: Duration) -> bool {
        let now = Instant::now();
        // Initialise to a time far enough in the past so the first call always passes.
        let entry = self
            .last
            .entry((id, cat))
            .or_insert_with(|| now - interval - Duration::from_millis(1));
        if now.duration_since(*entry) >= interval {
            *entry = now;
            true
        } else {
            false
        }
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

fn main() {
    // ── Ctrl-C handler ────────────────────────────────────────────────────────
    let running = Arc::new(AtomicBool::new(true));
    {
        let r = running.clone();
        ctrlc::set_handler(move || r.store(false, Ordering::SeqCst))
            .expect("failed to set Ctrl-C handler");
    }

    // ── Initialise SDL2 ───────────────────────────────────────────────────────
    let sdl_ctx = sdl2::init().expect("SDL2 init failed");

    // ── Create ControllerManager ──────────────────────────────────────────────
    let mut manager = ControllerManager::new(&sdl_ctx).expect("ControllerManager init failed");
    manager.set_dualsense_intensity(0x40);

    // BufWriter wraps the stdout lock so multiple writeln!s within one tick are
    // coalesced into one write(2) syscall on flush().
    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());

    let mut throttle = Throttle::new();

    // Open all controllers that are already connected.
    for id in manager.available_controllers() {
        open_controller(&mut manager, id, &mut out);
    }
    if manager.open_controllers().is_empty() {
        let _ = writeln!(out, "No game controllers found.  Plug one in and press a button.");
    }
    let _ = out.flush();

    // ── Event loop ────────────────────────────────────────────────────────────
    let mut event_pump = sdl_ctx.event_pump().expect("event pump failed");

    while running.load(Ordering::SeqCst) {
        for sdl_event in event_pump.poll_iter() {
            // Collect first to satisfy the borrow checker (manager + out both needed later).
            let ctrl_events: Vec<ControllerEvent> = manager.process_event(&sdl_event);
            for ctrl_event in ctrl_events {
                handle_event(&mut manager, &mut throttle, &mut out, ctrl_event);
            }
        }
        // One flush per tick: drain the buffer in a single syscall.
        let _ = out.flush();
        std::thread::sleep(Duration::from_millis(4));
    }

    let _ = writeln!(out, "exiting.");
    let _ = out.flush();
}

// ── Open helper ───────────────────────────────────────────────────────────────

fn open_controller(manager: &mut ControllerManager, id: u32, out: &mut impl Write) {
    if !manager.open_controller(id) {
        return;
    }
    let info = manager.controller_info(id).unwrap();
    let _ = writeln!(
        out,
        "[+] opened #{id}  \"{}\"  {}  \
         DS={} DS-Edge={} handheld={} steam_virtual={}(unmasked={}) PS={} LED={}",
        info.name,
        info.vid_pid,
        info.is_dualsense,
        info.is_dualsense_edge,
        info.is_handheld,
        info.is_steam_virtual,
        info.is_steam_virtual_unmasked,
        info.is_ps,
        info.has_led,
    );
    manager.change_led_color(id, 0, 0, 255); // blue = idle
    manager.change_player_index(id, 0);
}

// ── Event handler ─────────────────────────────────────────────────────────────

fn handle_event(
    manager: &mut ControllerManager,
    throttle: &mut Throttle,
    out: &mut impl Write,
    evt: ControllerEvent,
) {
    match evt {
        // ── Hotplug ───────────────────────────────────────────────────────────
        ControllerEvent::AvailableControllersUpdated => {
            let _ = writeln!(out, "[i] available controllers changed");
            // Open newly connected controllers.
            for id in manager.available_controllers() {
                if !manager.open_controllers().contains(&id) {
                    open_controller(manager, id, out);
                }
            }
            // Report disconnected controllers.
            for id in manager.open_controllers() {
                if !manager.is_controller_connected(id) {
                    let _ = writeln!(out, "[-] controller #{id} disconnected");
                }
            }
        }

        // ── Input state changed ───────────────────────────────────────────────
        ControllerEvent::StateChanged(id) => {
            let state = match manager.controller_state(id) {
                Some(s) => s.clone(),
                None => return,
            };

            let btns    = state.buttons();
            let l2      = state.l2();
            let r2      = state.r2();
            let lstk    = state.left_stick();
            let rstk    = state.right_stick();
            let gyro    = state.gyro();
            let accel   = state.accel();
            let orient  = state.orient();
            let touches = state.touches();

            // ── Buttons (low-frequency, always print) ─────────────────────────
            if !btns.is_empty() {
                let _ = writeln!(out, "[#{id}] buttons  = {btns:?}");
                manager.set_rumble(id, 80, 80);
                if btns.contains(chiaki::controller::ControllerButtons::L1) {
                    let data_left  = [0x00, 0x4f, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
                    let data_right = [0x00; 10];
                    manager.set_trigger_effects(id, 0x21, &data_left, 0x00, &data_right);
                    let _ = writeln!(out, "[#{id}] adaptive trigger effect sent (L1 held)");
                }
                manager.change_led_color(id, 0, 200, 0); // green = active
            } else {
                manager.set_rumble(id, 0, 0);
                manager.change_led_color(id, 0, 0, 255); // blue = idle
                let clear = [0u8; 10];
                manager.set_trigger_effects(id, 0x00, &clear, 0x00, &clear);
            }

            // ── Analog triggers (≤ 30 Hz) ─────────────────────────────────────
            if (l2 > 0 || r2 > 0)
                && throttle.allow(id, Category::Triggers, ANALOG_INTERVAL)
            {
                let _ = writeln!(out, "[#{id}] triggers  L2={l2:>3}  R2={r2:>3}  (0–255)");
            }

            // ── Analog sticks (≤ 30 Hz) ───────────────────────────────────────
            let deadzone: i16 = 2000;
            if (lstk.0.abs() > deadzone
                || lstk.1.abs() > deadzone
                || rstk.0.abs() > deadzone
                || rstk.1.abs() > deadzone)
                && throttle.allow(id, Category::Sticks, ANALOG_INTERVAL)
            {
                let _ = writeln!(
                    out,
                    "[#{id}] sticks    L=({:>+6},{:>+6})  R=({:>+6},{:>+6})  (±32767)",
                    lstk.0, lstk.1, rstk.0, rstk.1,
                );
            }

            // ── Gyroscope (≤ 10 Hz) ───────────────────────────────────────────
            let gyro_thr: f32 = 0.05; // rad/s — filter out resting noise
            if (gyro.0.abs() > gyro_thr || gyro.1.abs() > gyro_thr || gyro.2.abs() > gyro_thr)
                && throttle.allow(id, Category::Gyro, SENSOR_INTERVAL)
            {
                let _ = writeln!(
                    out,
                    "[#{id}] gyro      x={:>+7.3}  y={:>+7.3}  z={:>+7.3}  (rad/s)",
                    gyro.0, gyro.1, gyro.2,
                );
            }

            // ── Accelerometer (≤ 10 Hz) ───────────────────────────────────────
            // Idle reads ~(0, 1, 0) — 1 g along Y.  Only print when moved.
            let accel_delta =
                (accel.0.powi(2) + (accel.1 - 1.0_f32).powi(2) + accel.2.powi(2)).sqrt();
            if accel_delta > 0.1 && throttle.allow(id, Category::Accel, SENSOR_INTERVAL) {
                let _ = writeln!(
                    out,
                    "[#{id}] accel     x={:>+7.3}  y={:>+7.3}  z={:>+7.3}  (g)",
                    accel.0, accel.1, accel.2,
                );
            }

            // ── Orientation quaternion (≤ 10 Hz) ──────────────────────────────
            // Identity = (0,0,0,1).  Print only when rotated away from rest.
            let orient_delta =
                (orient.0.powi(2) + orient.1.powi(2) + orient.2.powi(2)).sqrt();
            if orient_delta > 0.05 && throttle.allow(id, Category::Orient, SENSOR_INTERVAL) {
                let _ = writeln!(
                    out,
                    "[#{id}] orient    x={:.3}  y={:.3}  z={:.3}  w={:.3}",
                    orient.0, orient.1, orient.2, orient.3,
                );
            }

            // ── Touchpad contacts (≤ 30 Hz for motion) ────────────────────────
            for touch in touches.iter() {
                if touch.id >= 0 && throttle.allow(id, Category::Touch, ANALOG_INTERVAL) {
                    let _ = writeln!(
                        out,
                        "[#{id}] touch     id={}  x={:>4}  y={:>4}  (0–1920 × 0–1079)",
                        touch.id, touch.x, touch.y,
                    );
                }
            }
        }

        // ── DualSense microphone button (low-frequency, always print) ─────────
        ControllerEvent::MicButtonPush(id) => {
            let _ = writeln!(out, "[#{id}] DualSense mic button pushed");
            manager.set_dualsense_mic(id, true);
        }

        // ── Controller mapping mode ───────────────────────────────────────────
        ControllerEvent::NewButtonMapping(id, mapping) => {
            let _ = writeln!(out, "[#{id}] button mapping captured: {mapping}");
        }
    }
}
