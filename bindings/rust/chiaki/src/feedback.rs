// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! Feedback command system for mapping PS5 session events to physical
//! controller actions (rumble, adaptive triggers, LED colour, etc.).

use std::sync::{Arc, Mutex};

use crate::session::Event;
use crate::types::DualSenseEffectIntensity;

// ── FeedbackCmd ──────────────────────────────────────────────────────────────

/// A command representing PS5 feedback that should be applied to a physical
/// controller.
#[derive(Debug, Clone)]
pub enum FeedbackCmd {
    /// Standard rumble motors (0–255 per side).
    Rumble { left: u8, right: u8 },
    /// DualSense adaptive trigger effect parameters.
    TriggerEffects {
        type_left: u8,
        type_right: u8,
        left: [u8; 10],
        right: [u8; 10],
    },
    /// Reset the orientation / motion-tracking state.
    MotionReset,
    /// Set the controller LED to an RGB colour.
    LedColor([u8; 3]),
    /// Set the player-index indicator.
    PlayerIndex(u32),
    /// Haptics-derived motor rumble strength (16-bit, from [`crate::haptics::HapticsSink`]).
    HapticRumble { left: u16, right: u16 },
    /// Update DualSense output-effect intensity register.
    SetDualSenseIntensity(u8),
}

// ── Event → FeedbackCmd mapping ──────────────────────────────────────────────

/// Map a [`DualSenseEffectIntensity`] to the byte value expected by the
/// DualSense controller output report.
///
/// Values mirror the C++ `ControllerManager::SetDualSenseIntensity` convention:
/// `0x00` = full/strong, `0x02` = medium, `0x03` = weak.
fn intensity_to_byte(intensity: &DualSenseEffectIntensity) -> u8 {
    match intensity {
        DualSenseEffectIntensity::Strong => 0x00,
        DualSenseEffectIntensity::Medium => 0x02,
        DualSenseEffectIntensity::Weak => 0x03,
        DualSenseEffectIntensity::Off => 0x00,
    }
}

/// Convert a session [`Event`] to a [`FeedbackCmd`].
///
/// Returns `None` for events that do not produce feedback (e.g. `Connected`,
/// `Quit`, keyboard events, holepunch status).
pub fn event_to_feedback(event: &Event) -> Option<FeedbackCmd> {
    match event {
        Event::Rumble { left, right, .. } => Some(FeedbackCmd::Rumble {
            left: *left,
            right: *right,
        }),
        Event::TriggerEffects {
            type_left,
            type_right,
            left,
            right,
        } => Some(FeedbackCmd::TriggerEffects {
            type_left: *type_left,
            type_right: *type_right,
            left: *left,
            right: *right,
        }),
        Event::LedColor(rgb) => Some(FeedbackCmd::LedColor(*rgb)),
        Event::HapticIntensity(intensity) => {
            Some(FeedbackCmd::SetDualSenseIntensity(intensity_to_byte(intensity)))
        }
        Event::TriggerIntensity(intensity) => {
            Some(FeedbackCmd::SetDualSenseIntensity(intensity_to_byte(intensity)))
        }
        Event::PlayerIndex(idx) => Some(FeedbackCmd::PlayerIndex(*idx as u32)),
        Event::MotionReset => Some(FeedbackCmd::MotionReset),
        // All other events (Connected, Quit, Keyboard*, Holepunch, Regist, LoginPinRequest, NicknameReceived)
        // do not produce controller feedback.
        _ => None,
    }
}

// ── Apply feedback to ControllerManager ──────────────────────────────────────

/// Apply a single [`FeedbackCmd`] to a [`ControllerManager`] for the given
/// controller instance ID.
#[cfg(feature = "sdl-controller")]
pub fn apply_feedback(
    cmd: &FeedbackCmd,
    manager: &mut crate::controllermanager::ControllerManager,
    instance_id: u32,
) {
    match cmd {
        FeedbackCmd::Rumble { left, right } => {
            manager.set_rumble(instance_id, *left, *right);
        }
        FeedbackCmd::TriggerEffects {
            type_left,
            type_right,
            left,
            right,
        } => {
            manager.set_trigger_effects(instance_id, *type_left, left, *type_right, right);
        }
        FeedbackCmd::MotionReset => {
            manager.reset_motion_controls(instance_id);
        }
        FeedbackCmd::LedColor(rgb) => {
            manager.change_led_color(instance_id, rgb[0], rgb[1], rgb[2]);
        }
        FeedbackCmd::PlayerIndex(idx) => {
            manager.change_player_index(instance_id, *idx as i32);
        }
        FeedbackCmd::HapticRumble { left, right } => {
            manager.set_haptic_rumble(instance_id, *left, *right);
        }
        FeedbackCmd::SetDualSenseIntensity(val) => {
            manager.set_dualsense_intensity(*val);
        }
    }
}

/// Drain all pending [`FeedbackCmd`]s from the shared queue and apply them to
/// the active controller.
///
/// If `active_id` is `None`, the queue is still drained (commands are
/// discarded).
#[cfg(feature = "sdl-controller")]
pub fn drain_and_apply(
    cmds: &Mutex<Vec<FeedbackCmd>>,
    manager: &mut crate::controllermanager::ControllerManager,
    active_id: Option<u32>,
) {
    if let Ok(mut locked) = cmds.lock() {
        for cmd in locked.drain(..) {
            if let Some(id) = active_id {
                apply_feedback(&cmd, manager, id);
            }
        }
    }
}

/// Create a thread-safe sender closure backed by an `Arc<Mutex<Vec<FeedbackCmd>>>`.
///
/// The returned closure can be called from any thread (e.g. a C callback
/// trampoline) to push feedback commands into the shared queue.
pub fn feedback_sender(
    cmds: Arc<Mutex<Vec<FeedbackCmd>>>,
) -> Arc<dyn Fn(FeedbackCmd) + Send + Sync> {
    Arc::new(move |cmd| {
        if let Ok(mut locked) = cmds.lock() {
            locked.push(cmd);
        }
    })
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_rumble_maps_to_feedback() {
        let event = Event::Rumble {
            unknown: 0,
            left: 128,
            right: 64,
        };
        let cmd = event_to_feedback(&event).unwrap();
        match cmd {
            FeedbackCmd::Rumble { left, right } => {
                assert_eq!(left, 128);
                assert_eq!(right, 64);
            }
            _ => panic!("Expected FeedbackCmd::Rumble"),
        }
    }

    #[test]
    fn event_trigger_effects_maps_to_feedback() {
        let left = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let right = [10, 9, 8, 7, 6, 5, 4, 3, 2, 1];
        let event = Event::TriggerEffects {
            type_left: 0x01,
            type_right: 0x02,
            left,
            right,
        };
        let cmd = event_to_feedback(&event).unwrap();
        match cmd {
            FeedbackCmd::TriggerEffects {
                type_left,
                type_right,
                left: l,
                right: r,
            } => {
                assert_eq!(type_left, 0x01);
                assert_eq!(type_right, 0x02);
                assert_eq!(l, left);
                assert_eq!(r, right);
            }
            _ => panic!("Expected FeedbackCmd::TriggerEffects"),
        }
    }

    #[test]
    fn event_led_color_maps_to_feedback() {
        let event = Event::LedColor([0xFF, 0x00, 0x80]);
        let cmd = event_to_feedback(&event).unwrap();
        match cmd {
            FeedbackCmd::LedColor(rgb) => assert_eq!(rgb, [0xFF, 0x00, 0x80]),
            _ => panic!("Expected FeedbackCmd::LedColor"),
        }
    }

    #[test]
    fn event_player_index_maps_to_feedback() {
        let event = Event::PlayerIndex(2);
        let cmd = event_to_feedback(&event).unwrap();
        match cmd {
            FeedbackCmd::PlayerIndex(idx) => assert_eq!(idx, 2),
            _ => panic!("Expected FeedbackCmd::PlayerIndex"),
        }
    }

    #[test]
    fn event_motion_reset_maps_to_feedback() {
        let event = Event::MotionReset;
        let cmd = event_to_feedback(&event).unwrap();
        assert!(matches!(cmd, FeedbackCmd::MotionReset));
    }

    #[test]
    fn haptic_intensity_strong_maps_to_0x00() {
        let event = Event::HapticIntensity(DualSenseEffectIntensity::Strong);
        let cmd = event_to_feedback(&event).unwrap();
        match cmd {
            FeedbackCmd::SetDualSenseIntensity(v) => assert_eq!(v, 0x00),
            _ => panic!("Expected SetDualSenseIntensity"),
        }
    }

    #[test]
    fn haptic_intensity_medium_maps_to_0x02() {
        let event = Event::HapticIntensity(DualSenseEffectIntensity::Medium);
        let cmd = event_to_feedback(&event).unwrap();
        match cmd {
            FeedbackCmd::SetDualSenseIntensity(v) => assert_eq!(v, 0x02),
            _ => panic!("Expected SetDualSenseIntensity"),
        }
    }

    #[test]
    fn haptic_intensity_weak_maps_to_0x03() {
        let event = Event::HapticIntensity(DualSenseEffectIntensity::Weak);
        let cmd = event_to_feedback(&event).unwrap();
        match cmd {
            FeedbackCmd::SetDualSenseIntensity(v) => assert_eq!(v, 0x03),
            _ => panic!("Expected SetDualSenseIntensity"),
        }
    }

    #[test]
    fn trigger_intensity_maps_same_as_haptic() {
        let event = Event::TriggerIntensity(DualSenseEffectIntensity::Medium);
        let cmd = event_to_feedback(&event).unwrap();
        match cmd {
            FeedbackCmd::SetDualSenseIntensity(v) => assert_eq!(v, 0x02),
            _ => panic!("Expected SetDualSenseIntensity"),
        }
    }

    #[test]
    fn event_connected_returns_none() {
        assert!(event_to_feedback(&Event::Connected).is_none());
    }

    #[test]
    fn event_quit_returns_none() {
        let event = Event::Quit {
            reason: crate::types::QuitReason::Stopped,
            reason_str: None,
        };
        assert!(event_to_feedback(&event).is_none());
    }

    #[test]
    fn event_keyboard_open_returns_none() {
        assert!(event_to_feedback(&Event::KeyboardOpen).is_none());
    }

    #[test]
    fn feedback_sender_pushes_to_vec() {
        let cmds = Arc::new(Mutex::new(Vec::new()));
        let sender = feedback_sender(Arc::clone(&cmds));
        sender(FeedbackCmd::MotionReset);
        sender(FeedbackCmd::LedColor([1, 2, 3]));
        let locked = cmds.lock().unwrap();
        assert_eq!(locked.len(), 2);
        assert!(matches!(locked[0], FeedbackCmd::MotionReset));
        assert!(matches!(locked[1], FeedbackCmd::LedColor([1, 2, 3])));
    }
}
