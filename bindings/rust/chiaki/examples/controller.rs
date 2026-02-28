// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! Example: Build and inspect `ControllerState` values.
//!
//! This example demonstrates every accessor on `ControllerState`, including
//! buttons, analog triggers, thumbsticks, touchpad contacts, and IMU motion.
//! It also shows how to merge two independent sources of input (e.g. a keyboard
//! overlay on top of a gamepad) using `ControllerState::merge`.
//!
//! No network connection is required — the example runs entirely offline.
//!
//! Usage:
//!   cargo run --example controller

use chiaki::prelude::*;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Pretty-print a single [`ControllerState`].
fn print_state(label: &str, s: &ControllerState) {
    println!("  {label}");
    println!("    buttons      : {:?}", s.buttons());
    println!("    L2 / R2      : {} / {}", s.l2(), s.r2());
    println!("    left stick   : {:?}", s.left_stick());
    println!("    right stick  : {:?}", s.right_stick());
    println!("    touches      : {:?}", s.touches());
    println!("    gyro (rad/s) : {:?}", s.gyro());
    println!("    accel (m/s²) : {:?}", s.accel());
    println!("    orient (xyzw): {:?}", s.orient());
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    // ── 1. Idle / default state ───────────────────────────────────────────────
    println!("=== 1. Idle state ===");
    let idle = ControllerState::idle();
    assert_eq!(idle.buttons(), ControllerButtons::empty());
    assert_eq!(idle.l2(), 0);
    assert_eq!(idle.r2(), 0);
    assert_eq!(idle.left_stick(), (0, 0));
    assert_eq!(idle.right_stick(), (0, 0));
    print_state("idle", &idle);

    // ── 2. Buttons ───────────────────────────────────────────────────────────
    println!("\n=== 2. Buttons ===");
    let mut btn = ControllerState::idle();

    // Press △ + ○ + R1 simultaneously.
    btn.set_buttons(ControllerButtons::PYRAMID | ControllerButtons::MOON | ControllerButtons::R1);
    assert!(btn.buttons().contains(ControllerButtons::PYRAMID));
    assert!(btn.buttons().contains(ControllerButtons::MOON));
    assert!(btn.buttons().contains(ControllerButtons::R1));
    assert!(!btn.buttons().contains(ControllerButtons::CROSS));
    print_state("△ + ○ + R1", &btn);

    // Add D-Pad Up.
    let new_buttons = btn.buttons() | ControllerButtons::DPAD_UP;
    btn.set_buttons(new_buttons);
    print_state("△ + ○ + R1 + D-Pad Up", &btn);

    // Release everything.
    btn.set_buttons(ControllerButtons::empty());
    assert_eq!(btn.buttons(), ControllerButtons::empty());
    println!("  all buttons released — ok");

    // ── 3. Analog triggers ───────────────────────────────────────────────────
    println!("\n=== 3. Analog triggers ===");
    let mut trig = ControllerState::idle();
    trig.set_l2(128);
    trig.set_r2(255);
    // Setting trigger values does NOT automatically set the L2/R2 button bits
    // unless the caller also sets them explicitly.
    trig.set_buttons(ControllerButtons::L2 | ControllerButtons::R2);
    print_state("L2=128 R2=255 (fully pressed)", &trig);

    // ── 4. Analog sticks ─────────────────────────────────────────────────────
    println!("\n=== 4. Analog sticks ===");
    let mut sticks = ControllerState::idle();

    // Full-tilt left stick (up-left diagonal).
    sticks.set_left_stick(-32768, -32768);
    // Right stick tilted right.
    sticks.set_right_stick(32767, 0);
    print_state("sticks: left=↖ full, right=→ full", &sticks);

    // Centre sticks again.
    sticks.set_left_stick(0, 0);
    sticks.set_right_stick(0, 0);
    assert_eq!(sticks.left_stick(), (0, 0));

    // ── 5. Touchpad ──────────────────────────────────────────────────────────
    println!("\n=== 5. Touchpad ===");
    let mut touch = ControllerState::idle();

    // Single-finger tap at (800, 400).
    let id0 = touch
        .start_touch(800, 400)
        .expect("first slot must be free");
    println!("  finger 0 id={id0} pressed at (800, 400)");
    print_state("one finger", &touch);

    // Move the finger.
    touch.set_touch_pos(id0 as u8, 820, 410);
    println!("  finger 0 moved to (820, 410)");

    // Second finger at (200, 300).
    let id1 = touch
        .start_touch(200, 300)
        .expect("second slot must be free");
    println!("  finger 1 id={id1} pressed at (200, 300)");
    print_state("two fingers", &touch);

    // Third finger should fail — only two slots.
    assert!(
        touch.start_touch(500, 500).is_none(),
        "no third touch slot should exist"
    );
    println!("  start_touch with all slots full returned None — ok");

    // Lift both fingers.
    touch.stop_touch(id0 as u8);
    touch.stop_touch(id1 as u8);
    println!("  both fingers lifted");
    print_state("after stop", &touch);

    // ── 6. Motion sensors ────────────────────────────────────────────────────
    println!("\n=== 6. Motion sensors ===");
    let mut motion = ControllerState::idle();

    // Simulate a gentle rotation around the Y axis.
    motion.set_gyro(0.0, 0.05, 0.0);
    // Gravity pointing straight down along –Z.
    motion.set_accel(0.0, 0.0, -9.81);
    // Identity quaternion.
    motion.set_orient(0.0, 0.0, 0.0, 1.0);
    print_state("gyro + accel + orient", &motion);

    // ── 7. Equality ──────────────────────────────────────────────────────────
    println!("\n=== 7. Equality ===");
    let a = ControllerState::idle();
    let b = ControllerState::idle();
    assert_eq!(a, b, "two idle states must be equal");

    let mut c = ControllerState::idle();
    c.set_buttons(ControllerButtons::CROSS);
    assert_ne!(a, c, "idle vs CROSS must differ");
    println!("  idle == idle  : true");
    println!("  idle == CROSS : false");

    // ── 8. Merge (OR) ────────────────────────────────────────────────────────
    println!("\n=== 8. Merge two input sources ===");

    // Source A: gamepad — left stick tilted up, L1 held.
    let mut gamepad = ControllerState::idle();
    gamepad.set_left_stick(0, -20000);
    gamepad.set_buttons(ControllerButtons::L1);

    // Source B: keyboard overlay — D-Pad Right pressed, gyro data from IMU.
    let mut keyboard = ControllerState::idle();
    keyboard.set_buttons(ControllerButtons::DPAD_RIGHT);
    keyboard.set_gyro(0.0, 0.1, 0.0);

    let merged = ControllerState::merge(&gamepad, &keyboard);

    // Buttons from both sources are OR-ed together.
    assert!(merged.buttons().contains(ControllerButtons::L1));
    assert!(merged.buttons().contains(ControllerButtons::DPAD_RIGHT));
    // Left stick comes from source A (non-zero).
    assert_eq!(merged.left_stick(), (0, -20000));

    print_state("gamepad", &gamepad);
    print_state("keyboard overlay", &keyboard);
    print_state("merged", &merged);

    println!("\nAll assertions passed.");
}
