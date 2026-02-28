// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

use bitflags::bitflags;
use chiaki_sys as sys;

bitflags! {
    /// Bitmask of controller buttons, combining both digital (bits 0–15) and
    /// analog (bits 16–17) buttons into a single `u32`.
    ///
    /// Matches `ChiakiControllerButton` + `ChiakiControllerAnalogButton`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct ControllerButtons: u32 {
        /// ✕ button (Cross).
        const CROSS       = 1 << 0;
        /// ○ button (Moon/Circle).
        const MOON        = 1 << 1;
        /// □ button (Box/Square).
        const BOX         = 1 << 2;
        /// △ button (Pyramid/Triangle).
        const PYRAMID     = 1 << 3;
        /// D-Pad Left.
        const DPAD_LEFT   = 1 << 4;
        /// D-Pad Right.
        const DPAD_RIGHT  = 1 << 5;
        /// D-Pad Up.
        const DPAD_UP     = 1 << 6;
        /// D-Pad Down.
        const DPAD_DOWN   = 1 << 7;
        /// L1 shoulder button.
        const L1          = 1 << 8;
        /// R1 shoulder button.
        const R1          = 1 << 9;
        /// L3 (left stick click).
        const L3          = 1 << 10;
        /// R3 (right stick click).
        const R3          = 1 << 11;
        /// Options button.
        const OPTIONS     = 1 << 12;
        /// Share / Create button.
        const SHARE       = 1 << 13;
        /// Touchpad click.
        const TOUCHPAD    = 1 << 14;
        /// PlayStation button.
        const PS          = 1 << 15;
        /// L2 trigger (analog, bit 16).
        const L2          = 1 << 16;
        /// R2 trigger (analog, bit 17).
        const R2          = 1 << 17;
    }
}

/// A single touchpad contact point.
///
/// `id` is `-1` when the finger has been lifted.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Touch {
    pub x: u16,
    pub y: u16,
    /// Touch identifier; `-1` means the slot is empty / finger up.
    pub id: i8,
}

/// Safe, zero-overhead wrapper around `ChiakiControllerState`.
///
/// Uses `#[repr(transparent)]` so a `&ControllerState` can be cast directly
/// to `*mut ChiakiControllerState` when calling into the C library.
#[repr(transparent)]
#[derive(Clone)]
pub struct ControllerState(pub(crate) sys::chiaki_controller_state_t);

impl Default for ControllerState {
    fn default() -> Self {
        Self::idle()
    }
}

impl std::fmt::Debug for ControllerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControllerState")
            .field("buttons", &self.buttons())
            .field("l2", &self.l2())
            .field("r2", &self.r2())
            .field("left_stick", &self.left_stick())
            .field("right_stick", &self.right_stick())
            .field("touches", &self.touches())
            .field("gyro", &self.gyro())
            .field("accel", &self.accel())
            .field("orient", &self.orient())
            .finish()
    }
}

impl PartialEq for ControllerState {
    fn eq(&self, other: &Self) -> bool {
        // SAFETY: both references are valid `ChiakiControllerState` values.
        unsafe {
            sys::chiaki_controller_state_equals(
                &self.0 as *const _ as *mut _,
                &other.0 as *const _ as *mut _,
            )
        }
    }
}

impl ControllerState {
    /// Return a zeroed / idle controller state (all axes centered, no buttons).
    pub fn idle() -> Self {
        let mut inner = unsafe { std::mem::zeroed::<sys::chiaki_controller_state_t>() };
        // SAFETY: pointer is valid for the duration of the call.
        unsafe { sys::chiaki_controller_state_set_idle(&mut inner) };
        ControllerState(inner)
    }

    // ── Button access ──────────────────────────────────────────────────────

    /// Return all currently active buttons as a bitmask.
    #[inline]
    pub fn buttons(&self) -> ControllerButtons {
        ControllerButtons::from_bits_truncate(self.0.buttons)
    }

    /// Overwrite the button bitmask.
    #[inline]
    pub fn set_buttons(&mut self, buttons: ControllerButtons) {
        self.0.buttons = buttons.bits();
    }

    // ── Analog triggers ────────────────────────────────────────────────────

    /// L2 pressure value (0–255).
    #[inline]
    pub fn l2(&self) -> u8 {
        self.0.l2_state
    }

    /// Set L2 pressure value (0–255).
    #[inline]
    pub fn set_l2(&mut self, value: u8) {
        self.0.l2_state = value;
    }

    /// R2 pressure value (0–255).
    #[inline]
    pub fn r2(&self) -> u8 {
        self.0.r2_state
    }

    /// Set R2 pressure value (0–255).
    #[inline]
    pub fn set_r2(&mut self, value: u8) {
        self.0.r2_state = value;
    }

    // ── Analog sticks ──────────────────────────────────────────────────────

    /// Left stick `(x, y)` in range `[-32768, 32767]`.
    #[inline]
    pub fn left_stick(&self) -> (i16, i16) {
        (self.0.left_x, self.0.left_y)
    }

    /// Set left stick axes.
    #[inline]
    pub fn set_left_stick(&mut self, x: i16, y: i16) {
        self.0.left_x = x;
        self.0.left_y = y;
    }

    /// Right stick `(x, y)` in range `[-32768, 32767]`.
    #[inline]
    pub fn right_stick(&self) -> (i16, i16) {
        (self.0.right_x, self.0.right_y)
    }

    /// Set right stick axes.
    #[inline]
    pub fn set_right_stick(&mut self, x: i16, y: i16) {
        self.0.right_x = x;
        self.0.right_y = y;
    }

    // ── Touchpad ───────────────────────────────────────────────────────────

    /// Read-only view of both touch slots.
    ///
    /// # Safety
    /// `Touch` is `#[repr(C)]` with the same layout as `chiaki_controller_touch_t`.
    pub fn touches(&self) -> &[Touch; 2] {
        // SAFETY: `Touch` has identical layout to `chiaki_controller_touch_t`
        // (repr(C), same fields in the same order).
        unsafe { &*(self.0.touches.as_ptr() as *const [Touch; 2]) }
    }

    /// Start a new touchpad contact and return the allocated touch id.
    ///
    /// Returns `None` when all slots are occupied.
    pub fn start_touch(&mut self, x: u16, y: u16) -> Option<i8> {
        let id = unsafe { sys::chiaki_controller_state_start_touch(&mut self.0, x, y) };
        if id < 0 { None } else { Some(id) }
    }

    /// Stop a touchpad contact by `id`.
    pub fn stop_touch(&mut self, id: u8) {
        unsafe { sys::chiaki_controller_state_stop_touch(&mut self.0, id) };
    }

    /// Update the position of an active touch slot.
    pub fn set_touch_pos(&mut self, id: u8, x: u16, y: u16) {
        unsafe { sys::chiaki_controller_state_set_touch_pos(&mut self.0, id, x, y) };
    }

    // ── Motion ─────────────────────────────────────────────────────────────

    /// Gyroscope `(x, y, z)` in radians/second.
    #[inline]
    pub fn gyro(&self) -> (f32, f32, f32) {
        (self.0.gyro_x, self.0.gyro_y, self.0.gyro_z)
    }

    /// Set gyroscope values.
    #[inline]
    pub fn set_gyro(&mut self, x: f32, y: f32, z: f32) {
        self.0.gyro_x = x;
        self.0.gyro_y = y;
        self.0.gyro_z = z;
    }

    /// Accelerometer `(x, y, z)` in m/s².
    #[inline]
    pub fn accel(&self) -> (f32, f32, f32) {
        (self.0.accel_x, self.0.accel_y, self.0.accel_z)
    }

    /// Set accelerometer values.
    #[inline]
    pub fn set_accel(&mut self, x: f32, y: f32, z: f32) {
        self.0.accel_x = x;
        self.0.accel_y = y;
        self.0.accel_z = z;
    }

    /// Orientation quaternion `(x, y, z, w)`.
    #[inline]
    pub fn orient(&self) -> (f32, f32, f32, f32) {
        (self.0.orient_x, self.0.orient_y, self.0.orient_z, self.0.orient_w)
    }

    /// Set orientation quaternion.
    #[inline]
    pub fn set_orient(&mut self, x: f32, y: f32, z: f32, w: f32) {
        self.0.orient_x = x;
        self.0.orient_y = y;
        self.0.orient_z = z;
        self.0.orient_w = w;
    }

    // ── Utility ────────────────────────────────────────────────────────────

    /// Merge two controller states using boolean OR on buttons/triggers and
    /// choosing the first state that has non-zero motion data for the IMU.
    pub fn merge(a: &Self, b: &Self) -> Self {
        let mut out = unsafe { std::mem::zeroed::<sys::chiaki_controller_state_t>() };
        unsafe {
            sys::chiaki_controller_state_or(
                &mut out,
                &a.0 as *const _ as *mut _,
                &b.0 as *const _ as *mut _,
            )
        };
        ControllerState(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init() {
        crate::init().unwrap();
    }

    // ── idle / default ─────────────────────────────────────────────────────────

    #[test]
    fn idle_state_has_no_buttons_pressed() {
        init();
        let s = ControllerState::idle();
        assert_eq!(s.buttons(), ControllerButtons::empty());
    }

    #[test]
    fn idle_triggers_are_zero() {
        init();
        let s = ControllerState::idle();
        assert_eq!(s.l2(), 0);
        assert_eq!(s.r2(), 0);
    }

    #[test]
    fn idle_sticks_are_zero() {
        init();
        let s = ControllerState::idle();
        assert_eq!(s.left_stick(), (0, 0));
        assert_eq!(s.right_stick(), (0, 0));
    }

    #[test]
    fn default_equals_idle() {
        init();
        let idle = ControllerState::idle();
        let default = ControllerState::default();
        assert_eq!(idle, default);
    }

    // ── Buttons ────────────────────────────────────────────────────────────────

    #[test]
    fn set_and_get_buttons_roundtrip() {
        init();
        let mut s = ControllerState::idle();
        let buttons = ControllerButtons::CROSS | ControllerButtons::R1 | ControllerButtons::OPTIONS;
        s.set_buttons(buttons);
        assert_eq!(s.buttons(), buttons);
    }

    #[test]
    fn set_buttons_replaces_previous_value() {
        init();
        let mut s = ControllerState::idle();
        s.set_buttons(ControllerButtons::CROSS);
        s.set_buttons(ControllerButtons::MOON);
        assert!(!s.buttons().contains(ControllerButtons::CROSS));
        assert!(s.buttons().contains(ControllerButtons::MOON));
    }

    #[test]
    fn set_all_buttons() {
        init();
        let mut s = ControllerState::idle();
        s.set_buttons(ControllerButtons::all());
        assert_eq!(s.buttons(), ControllerButtons::all());
    }

    #[test]
    fn clear_buttons() {
        init();
        let mut s = ControllerState::idle();
        s.set_buttons(ControllerButtons::PS | ControllerButtons::TOUCHPAD);
        s.set_buttons(ControllerButtons::empty());
        assert_eq!(s.buttons(), ControllerButtons::empty());
    }

    // ── Analog triggers ────────────────────────────────────────────────────────

    #[test]
    fn set_and_get_l2_roundtrip() {
        init();
        let mut s = ControllerState::idle();
        s.set_l2(200);
        assert_eq!(s.l2(), 200);
    }

    #[test]
    fn set_and_get_r2_roundtrip() {
        init();
        let mut s = ControllerState::idle();
        s.set_r2(255);
        assert_eq!(s.r2(), 255);
    }

    #[test]
    fn l2_r2_independent() {
        init();
        let mut s = ControllerState::idle();
        s.set_l2(100);
        s.set_r2(50);
        assert_eq!(s.l2(), 100);
        assert_eq!(s.r2(), 50);
    }

    // ── Analog sticks ──────────────────────────────────────────────────────────

    #[test]
    fn set_and_get_left_stick_roundtrip() {
        init();
        let mut s = ControllerState::idle();
        s.set_left_stick(-32768, 32767);
        assert_eq!(s.left_stick(), (-32768, 32767));
    }

    #[test]
    fn set_and_get_right_stick_roundtrip() {
        init();
        let mut s = ControllerState::idle();
        s.set_right_stick(100, -200);
        assert_eq!(s.right_stick(), (100, -200));
    }

    #[test]
    fn sticks_are_independent() {
        init();
        let mut s = ControllerState::idle();
        s.set_left_stick(1000, -1000);
        s.set_right_stick(-500, 500);
        assert_eq!(s.left_stick(), (1000, -1000));
        assert_eq!(s.right_stick(), (-500, 500));
    }

    // ── Motion ─────────────────────────────────────────────────────────────────

    #[test]
    fn set_and_get_gyro_roundtrip() {
        init();
        let mut s = ControllerState::idle();
        s.set_gyro(1.0, -2.5, 0.5);
        let (x, y, z) = s.gyro();
        assert!((x - 1.0).abs() < f32::EPSILON);
        assert!((y - (-2.5)).abs() < f32::EPSILON);
        assert!((z - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn set_and_get_accel_roundtrip() {
        init();
        let mut s = ControllerState::idle();
        s.set_accel(0.0, 9.8, 0.0);
        let (x, y, z) = s.accel();
        assert!((x - 0.0).abs() < f32::EPSILON);
        assert!((y - 9.8).abs() < 0.0001);
        assert!((z - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn set_and_get_orient_roundtrip() {
        init();
        let mut s = ControllerState::idle();
        s.set_orient(0.0, 0.0, 0.0, 1.0); // identity quaternion
        assert_eq!(s.orient(), (0.0, 0.0, 0.0, 1.0));
    }

    #[test]
    fn idle_motion_values_are_zero() {
        init();
        let s = ControllerState::idle();
        assert_eq!(s.gyro(), (0.0, 0.0, 0.0));
        // chiaki_controller_state_set_idle initialises accel to (0, 1, 0) —
        // a standard 1g gravity vector along the Y axis, not all-zero.
        assert_eq!(s.accel(), (0.0, 1.0, 0.0));
        // orient is initialised to the identity quaternion (0, 0, 0, 1).
        assert_eq!(s.orient(), (0.0, 0.0, 0.0, 1.0));
    }

    // ── Equality ───────────────────────────────────────────────────────────────

    #[test]
    fn same_state_is_equal() {
        init();
        let mut a = ControllerState::idle();
        a.set_buttons(ControllerButtons::PS);
        a.set_l2(128);
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn different_buttons_are_not_equal() {
        init();
        let mut a = ControllerState::idle();
        a.set_buttons(ControllerButtons::CROSS);
        let b = ControllerState::idle();
        assert_ne!(a, b);
    }

    #[test]
    fn different_triggers_are_not_equal() {
        init();
        let mut a = ControllerState::idle();
        a.set_l2(100);
        let b = ControllerState::idle();
        assert_ne!(a, b);
    }

    #[test]
    fn different_sticks_are_not_equal() {
        init();
        let mut a = ControllerState::idle();
        a.set_left_stick(500, 0);
        let b = ControllerState::idle();
        assert_ne!(a, b);
    }

    // ── Clone ──────────────────────────────────────────────────────────────────

    #[test]
    fn clone_creates_equal_independent_copy() {
        init();
        let mut s = ControllerState::idle();
        s.set_right_stick(-100, 100);
        s.set_r2(50);
        s.set_buttons(ControllerButtons::L1 | ControllerButtons::R1);
        let clone = s.clone();
        assert_eq!(s, clone);
    }

    #[test]
    fn clone_is_independent_mutation() {
        init();
        let s = ControllerState::idle();
        let mut clone = s.clone();
        clone.set_buttons(ControllerButtons::CROSS);
        // Mutating clone must not affect original
        assert_eq!(s.buttons(), ControllerButtons::empty());
        assert!(clone.buttons().contains(ControllerButtons::CROSS));
    }

    // ── Merge ──────────────────────────────────────────────────────────────────

    #[test]
    fn merge_combines_buttons_with_or() {
        init();
        let mut a = ControllerState::idle();
        a.set_buttons(ControllerButtons::CROSS | ControllerButtons::L1);
        let mut b = ControllerState::idle();
        b.set_buttons(ControllerButtons::MOON | ControllerButtons::R1);

        let merged = ControllerState::merge(&a, &b);
        let btn = merged.buttons();
        assert!(btn.contains(ControllerButtons::CROSS));
        assert!(btn.contains(ControllerButtons::L1));
        assert!(btn.contains(ControllerButtons::MOON));
        assert!(btn.contains(ControllerButtons::R1));
    }

    #[test]
    fn merge_combines_triggers_with_or() {
        init();
        let mut a = ControllerState::idle();
        a.set_l2(100);
        let mut b = ControllerState::idle();
        b.set_r2(200);

        let merged = ControllerState::merge(&a, &b);
        assert_eq!(merged.l2(), 100);
        assert_eq!(merged.r2(), 200);
    }

    #[test]
    fn merge_of_two_idle_states_is_idle() {
        init();
        let a = ControllerState::idle();
        let b = ControllerState::idle();
        let merged = ControllerState::merge(&a, &b);
        assert_eq!(merged, ControllerState::idle());
    }

    // ── Touch ─────────────────────────────────────────────────────────────────

    #[test]
    fn start_touch_returns_valid_id() {
        init();
        let mut s = ControllerState::idle();
        let id = s.start_touch(500, 300);
        assert!(id.is_some());
        assert!(id.unwrap() >= 0);
    }

    #[test]
    fn two_touch_slots_available() {
        init();
        let mut s = ControllerState::idle();
        let id0 = s.start_touch(100, 100);
        let id1 = s.start_touch(200, 200);
        assert!(id0.is_some());
        assert!(id1.is_some());
    }

    #[test]
    fn third_touch_returns_none_when_slots_full() {
        init();
        let mut s = ControllerState::idle();
        s.start_touch(100, 100);
        s.start_touch(200, 200);
        // All slots occupied — a third start should fail.
        let id2 = s.start_touch(300, 300);
        assert!(id2.is_none());
    }

    #[test]
    fn stop_touch_frees_slot_for_reuse() {
        init();
        let mut s = ControllerState::idle();
        let id = s.start_touch(100, 100).unwrap() as u8;
        s.stop_touch(id);
        // After freeing, a new touch can be started.
        let new_id = s.start_touch(500, 500);
        assert!(new_id.is_some());
    }

    #[test]
    fn set_touch_pos_updates_coordinates() {
        init();
        let mut s = ControllerState::idle();
        let id = s.start_touch(100, 100).unwrap() as u8;
        s.set_touch_pos(id, 750, 250);
        let touches = s.touches();
        let touch = touches.iter().find(|t| t.id == id as i8).unwrap();
        assert_eq!(touch.x, 750);
        assert_eq!(touch.y, 250);
    }

    // ── ControllerButtons bitflags ────────────────────────────────────────────

    #[test]
    fn buttons_bitflags_bitwise_ops() {
        let face = ControllerButtons::CROSS
            | ControllerButtons::MOON
            | ControllerButtons::BOX
            | ControllerButtons::PYRAMID;
        assert!(face.contains(ControllerButtons::CROSS));
        assert!(face.contains(ControllerButtons::MOON));
        assert!(!face.contains(ControllerButtons::L1));

        let without_moon = face - ControllerButtons::MOON;
        assert!(!without_moon.contains(ControllerButtons::MOON));
        assert!(without_moon.contains(ControllerButtons::CROSS));
    }

    #[test]
    fn buttons_empty_has_no_bits_set() {
        assert_eq!(ControllerButtons::empty().bits(), 0);
    }

    #[test]
    fn buttons_copy_and_eq() {
        let a = ControllerButtons::L1 | ControllerButtons::R1;
        let b = a;
        assert_eq!(a, b);
        assert_ne!(ControllerButtons::L1, ControllerButtons::R1);
    }

    // ── Debug formatting ──────────────────────────────────────────────────────

    #[test]
    fn controller_state_debug_format_contains_field_names() {
        init();
        let s = ControllerState::idle();
        let dbg = format!("{s:?}");
        assert!(dbg.contains("ControllerState"));
        assert!(dbg.contains("buttons"));
        assert!(dbg.contains("l2"));
        assert!(dbg.contains("r2"));
        assert!(dbg.contains("left_stick"));
        assert!(dbg.contains("right_stick"));
        assert!(dbg.contains("gyro"));
        assert!(dbg.contains("accel"));
        assert!(dbg.contains("orient"));
    }
}
