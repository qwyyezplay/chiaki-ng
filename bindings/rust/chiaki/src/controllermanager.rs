// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! SDL2-backed controller manager.
//!
//! Translates SDL2 game-controller events into [`ControllerState`] values that
//! can be fed directly to [`crate::session::Session::set_controller_state`].
//!
//! # Feature flag
//!
//! This module is only available when the crate is compiled with the
//! `sdl-controller` feature flag, which enables the `sdl2` dependency
//! (with `hidapi` enabled for gyro/touchpad sensor support).
//!
//! # Usage
//!
//! ```no_run
//! use chiaki::controllermanager::{ControllerManager, ControllerEvent};
//!
//! let sdl_ctx = sdl2::init().unwrap();
//! let mut manager = ControllerManager::new(&sdl_ctx).unwrap();
//! let mut event_pump = sdl_ctx.event_pump().unwrap();
//!
//! // Open all initially available controllers.
//! for id in manager.available_controllers() {
//!     manager.open_controller(id);
//! }
//!
//! loop {
//!     for event in event_pump.poll_iter() {
//!         for ctrl_event in manager.process_event(&event) {
//!             if let ControllerEvent::StateChanged(id) = ctrl_event {
//!                 let state = manager.controller_state(id).unwrap();
//!                 // session.set_controller_state(state).unwrap();
//!             }
//!         }
//!     }
//!     std::thread::sleep(std::time::Duration::from_millis(4));
//! }
//! ```

use std::collections::HashMap;

use sdl2::controller::{Axis, Button, GameController};
use sdl2::event::Event;
use sdl2::sensor::SensorType;
// sdl2::GameControllerSubsystem is re-exported from sdl2::sdl via `pub use crate::sdl::*;`
use sdl2::GameControllerSubsystem;
use sdl2::sys as sdl_sys;

use chiaki_sys as sys;

use crate::controller::{ControllerButtons, ControllerState};

// ── PS Touchpad dimensions ────────────────────────────────────────────────────
const PS_TOUCHPAD_MAXX: f32 = 1920.0;
const PS_TOUCHPAD_MAXY: f32 = 1079.0;

/// SDL defines 1 g as 9.80665 m/s².
const SDL_STANDARD_GRAVITY: f32 = 9.80665;

// ── Known device VID/PID tables (matches C++ controllermanager.cpp) ───────────
const DUALSENSE_IDS: &[(u16, u16)] = &[
    (0x054c, 0x0ce6), // DualSense
];

const DUALSENSE_EDGE_IDS: &[(u16, u16)] = &[
    (0x054c, 0x0df2), // DualSense Edge
];

const HANDHELD_IDS: &[(u16, u16)] = &[
    (0x28de, 0x1205), // Steam Deck
    (0x0b05, 0x1abe), // Rog Ally
    (0x17ef, 0x6182), // Legion Go
    (0x0db0, 0x1901), // MSI Claw
];

/// On non-macOS: Steam Virtual Controller VID/PID.
/// On macOS: Steam presents a virtual Xbox 360 controller.
#[cfg(not(target_os = "macos"))]
const STEAM_VIRTUAL_IDS: &[(u16, u16)] = &[
    (0x28de, 0x11ff), // Steam Virtual Controller
];

#[cfg(target_os = "macos")]
const STEAM_VIRTUAL_IDS: &[(u16, u16)] = &[
    (0x045e, 0x028e), // Microsoft Xbox 360 (Steam virtual on macOS)
];

// ── DualSense 5 effects state ─────────────────────────────────────────────────

/// Raw DualSense haptic effect packet sent via `SDL_GameControllerSendEffect`.
///
/// Layout matches `DS5EffectsState_t` from the C++ implementation.
/// `#[repr(C, packed)]` is required for byte-exact layout.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Ds5EffectsState {
    enable_bits1: u8,               // 0
    enable_bits2: u8,               // 1
    rumble_right: u8,               // 2
    rumble_left: u8,                // 3
    headphone_volume: u8,           // 4
    speaker_volume: u8,             // 5
    microphone_volume: u8,          // 6
    audio_enable_bits: u8,          // 7
    mic_light_mode: u8,             // 8
    audio_mute_bits: u8,            // 9
    right_trigger_effect: [u8; 11], // 10
    left_trigger_effect: [u8; 11],  // 21
    unknown1: [u8; 6],              // 32
    enable_bits3: u8,               // 38
    unknown2: [u8; 2],              // 39
    led_anim: u8,                   // 41
    led_brightness: u8,             // 42
    pad_lights: u8,                 // 43
    led_red: u8,                    // 44
    led_green: u8,                  // 45
    led_blue: u8,                   // 46
}

impl Default for Ds5EffectsState {
    fn default() -> Self {
        // SAFETY: all-zero is valid for a packed-bytes struct.
        unsafe { std::mem::zeroed() }
    }
}

// ── Public types ──────────────────────────────────────────────────────────────

/// Events produced by [`ControllerManager::process_event`].
#[derive(Debug, Clone)]
pub enum ControllerEvent {
    /// The set of available (connected but not necessarily open) controllers changed.
    AvailableControllersUpdated,
    /// The [`ControllerState`] for a controller changed.  Contains the instance ID.
    StateChanged(u32),
    /// The microphone button on a DualSense was pressed and released.  Contains the instance ID.
    MicButtonPush(u32),
    /// A raw button-mapping string was observed while mapping mode was active.
    ///
    /// The second field encodes the input as `"bN"` (joystick button N),
    /// `"aN"` (axis N) or `"hN.V"` (hat N, value V).
    NewButtonMapping(u32, String),
}

/// Static information about a connected SDL game controller.
#[derive(Debug, Clone)]
pub struct ControllerInfo {
    pub instance_id: u32,
    /// Human-readable name reported by SDL.
    pub name: String,
    /// SDL GUID string (32 hex characters).
    pub guid: String,
    /// `"0xVVVV:0xPPPP"` vendor:product string.
    pub vid_pid: String,
    pub is_dualsense: bool,
    pub is_dualsense_edge: bool,
    pub is_handheld: bool,
    /// Whether the controller is a Steam Virtual Controller.
    pub is_steam_virtual: bool,
    /// Whether the raw (unmasked) VID/PID also identifies a Steam Virtual Controller.
    ///
    /// On macOS, Steam presents a virtual Xbox 360 controller and the GUID-embedded
    /// VID/PID is zeroed, so `is_steam_virtual` checks the GUID while this field
    /// checks the actual hardware VID/PID.  Off macOS the two values are always equal.
    pub is_steam_virtual_unmasked: bool,
    /// Whether SDL reports the controller type as PS3, PS4 or PS5.
    pub is_ps: bool,
    /// Whether the controller has an RGB LED that can be set.
    pub has_led: bool,
}

// ── Internal per-controller state ────────────────────────────────────────────

struct ControllerInner {
    /// Safe SDL2 wrapper.  Owns the SDL reference count; dropped when closed.
    #[allow(dead_code)]
    sdl_controller: GameController,
    info: ControllerInfo,
    state: ControllerState,
    orientation_tracker: sys::chiaki_orientation_tracker_t,
    accel_zero: sys::chiaki_accel_new_zero,
    /// Tracks the latest real-accelerometer reading for zero-calibration resets.
    real_accel: sys::chiaki_accel_new_zero,
    last_motion_timestamp_us: u64,
    /// Maps `(touchpad_index, finger_index)` to an allocated Chiaki touch slot id.
    touch_ids: HashMap<(u32, u32), u8>,
    micbutton_push: bool,
    firmware_version: u16,
    // Controller-mapping state
    updating_mapping_button: bool,
    enable_analog_stick_mapping: bool,
}

impl ControllerInner {
    /// Obtain a non-owning raw `SDL_GameController*` for the currently open controller.
    ///
    /// # Safety
    /// The returned pointer is valid only while `self.sdl_controller` is alive,
    /// i.e. for the duration of any `&self` or `&mut self` method call.
    #[inline]
    fn raw_ptr(&self) -> *mut sdl_sys::SDL_GameController {
        // SDL_GameControllerFromInstanceID returns a non-owning pointer to the
        // already-open controller.  Since self.sdl_controller holds the controller
        // open, this pointer is valid for the duration of any call on self.
        unsafe { sdl_sys::SDL_GameControllerFromInstanceID(self.info.instance_id as i32) }
    }

    /// Returns `true` if the underlying SDL controller is currently attached.
    pub fn is_connected(&self) -> bool {
        let raw = self.raw_ptr();
        if raw.is_null() {
            return false;
        }
        unsafe { sdl_sys::SDL_GameControllerGetAttached(raw) == sdl_sys::SDL_bool::SDL_TRUE }
    }

    /// Open a game controller by device index and build its [`ControllerInner`].
    fn open(
        gc_subsystem: &GameControllerSubsystem,
        device_index: u32,
        instance_id: u32,
    ) -> Result<Self, String> {
        let sdl_controller = gc_subsystem
            .open(device_index)
            .map_err(|e| e.to_string())?;

        // ── Identify the device (safe API) ───────────────────────────────────
        let vendor = sdl_controller.vendor_id().unwrap_or(0);
        let product = sdl_controller.product_id().unwrap_or(0);
        let vid_pid = (vendor, product);

        let is_dualsense = DUALSENSE_IDS.contains(&vid_pid);
        let is_dualsense_edge = DUALSENSE_EDGE_IDS.contains(&vid_pid);
        let is_handheld = HANDHELD_IDS.contains(&vid_pid);
        let has_led = sdl_controller.has_led();

        // ── Unsafe operations that need the raw pointer ──────────────────────
        // SAFETY: sdl_controller was just opened (refcount >= 1); raw is valid.
        let raw = unsafe { sdl_sys::SDL_GameControllerFromInstanceID(instance_id as i32) };

        let firmware_version =
            unsafe { sdl_sys::SDL_GameControllerGetFirmwareVersion(raw) };

        // Steam virtual detection via GUID-embedded VID/PID and version.
        let (is_steam_virtual, is_steam_virtual_unmasked) = {
            let js = unsafe { sdl_sys::SDL_GameControllerGetJoystick(raw) };
            let guid = unsafe { sdl_sys::SDL_JoystickGetGUID(js) };
            let mut guid_vendor: u16 = 0;
            let mut guid_product: u16 = 0;
            let mut guid_version: u16 = 0;
            unsafe {
                sdl_sys::SDL_GetJoystickGUIDInfo(
                    guid,
                    &mut guid_vendor,
                    &mut guid_product,
                    &mut guid_version,
                    std::ptr::null_mut(),
                )
            };
            let guid_vid_pid = (guid_vendor, guid_product);

            #[cfg(target_os = "macos")]
            {
                let masked = guid_version == 0 && STEAM_VIRTUAL_IDS.contains(&guid_vid_pid);
                let unmasked = guid_version == 0 && STEAM_VIRTUAL_IDS.contains(&vid_pid);
                (masked, unmasked)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let matched = STEAM_VIRTUAL_IDS.contains(&guid_vid_pid);
                let unmasked = STEAM_VIRTUAL_IDS.contains(&vid_pid);
                (matched, unmasked)
            }
        };

        // PS-family controller type detection.
        let ctrl_type = unsafe { sdl_sys::SDL_GameControllerGetType(raw) };
        let is_ps = matches!(
            ctrl_type,
            sdl_sys::SDL_GameControllerType::SDL_CONTROLLER_TYPE_PS3
                | sdl_sys::SDL_GameControllerType::SDL_CONTROLLER_TYPE_PS4
                | sdl_sys::SDL_GameControllerType::SDL_CONTROLLER_TYPE_PS5
        );

        // GUID string (for display / mapping lookup).
        let guid_str = {
            let js = unsafe { sdl_sys::SDL_GameControllerGetJoystick(raw) };
            let guid = unsafe { sdl_sys::SDL_JoystickGetGUID(js) };
            let mut buf = [0u8; 256];
            unsafe {
                sdl_sys::SDL_JoystickGetGUIDString(
                    guid,
                    buf.as_mut_ptr() as *mut i8,
                    buf.len() as i32,
                )
            };
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(&buf[..end]).into_owned()
        };

        let name_str = sdl_controller.name();

        let info = ControllerInfo {
            instance_id,
            name: name_str,
            guid: guid_str,
            vid_pid: format!("0x{vendor:04x}:0x{product:04x}"),
            is_dualsense,
            is_dualsense_edge,
            is_handheld,
            is_steam_virtual,
            is_steam_virtual_unmasked,
            is_ps,
            has_led,
        };

        // ── Enable motion sensors via the hidapi safe API ────────────────────
        if sdl_controller.has_sensor(SensorType::Accelerometer) {
            let _ = unsafe {
                sdl_sys::SDL_GameControllerSetSensorEnabled(
                    raw,
                    sdl_sys::SDL_SensorType::SDL_SENSOR_ACCEL,
                    sdl_sys::SDL_bool::SDL_TRUE,
                )
            };
        }
        if sdl_controller.has_sensor(SensorType::Gyroscope) {
            let _ = unsafe {
                sdl_sys::SDL_GameControllerSetSensorEnabled(
                    raw,
                    sdl_sys::SDL_SensorType::SDL_SENSOR_GYRO,
                    sdl_sys::SDL_bool::SDL_TRUE,
                )
            };
        }

        // ── Initialise orientation tracker ───────────────────────────────────
        let mut orientation_tracker =
            unsafe { std::mem::zeroed::<sys::chiaki_orientation_tracker_t>() };
        let mut accel_zero = unsafe { std::mem::zeroed::<sys::chiaki_accel_new_zero>() };
        let mut real_accel = unsafe { std::mem::zeroed::<sys::chiaki_accel_new_zero>() };
        unsafe {
            sys::chiaki_orientation_tracker_init(&mut orientation_tracker);
            sys::chiaki_accel_new_zero_set_inactive(&mut accel_zero, false);
            sys::chiaki_accel_new_zero_set_inactive(&mut real_accel, true);
        }

        Ok(Self {
            sdl_controller,
            info,
            state: ControllerState::idle(),
            orientation_tracker,
            accel_zero,
            real_accel,
            last_motion_timestamp_us: 0,
            touch_ids: HashMap::new(),
            micbutton_push: false,
            firmware_version,
            updating_mapping_button: false,
            enable_analog_stick_mapping: false,
        })
    }

    // ── SDL event handlers ────────────────────────────────────────────────────

    fn handle_button_down(&mut self, button: Button) -> Option<ControllerEvent> {
        if button == Button::Misc1 {
            self.micbutton_push = true;
            return None;
        }
        // Paddle buttons are not forwarded to the PS protocol.
        if matches!(
            button,
            Button::Paddle1 | Button::Paddle2 | Button::Paddle3 | Button::Paddle4
        ) {
            return None;
        }
        let ps_btn = sdl_button_to_ps(button)?;
        let mut buttons = self.state.buttons();
        buttons |= ps_btn;
        self.state.set_buttons(buttons);
        Some(ControllerEvent::StateChanged(self.info.instance_id))
    }

    fn handle_button_up(&mut self, button: Button) -> Option<ControllerEvent> {
        if button == Button::Misc1 {
            if self.micbutton_push {
                self.micbutton_push = false;
                return Some(ControllerEvent::MicButtonPush(self.info.instance_id));
            }
            return None;
        }
        if matches!(
            button,
            Button::Paddle1 | Button::Paddle2 | Button::Paddle3 | Button::Paddle4
        ) {
            return None;
        }
        let ps_btn = sdl_button_to_ps(button)?;
        let mut buttons = self.state.buttons();
        buttons -= ps_btn;
        self.state.set_buttons(buttons);
        Some(ControllerEvent::StateChanged(self.info.instance_id))
    }

    fn handle_axis(&mut self, axis: Axis, value: i16) -> Option<ControllerEvent> {
        match axis {
            Axis::TriggerLeft => self.state.set_l2((value >> 7) as u8),
            Axis::TriggerRight => self.state.set_r2((value >> 7) as u8),
            Axis::LeftX => {
                let (_, y) = self.state.left_stick();
                self.state.set_left_stick(value, y);
            }
            Axis::LeftY => {
                let (x, _) = self.state.left_stick();
                self.state.set_left_stick(x, value);
            }
            Axis::RightX => {
                let (_, y) = self.state.right_stick();
                self.state.set_right_stick(value, y);
            }
            Axis::RightY => {
                let (x, _) = self.state.right_stick();
                self.state.set_right_stick(x, value);
            }
        }
        Some(ControllerEvent::StateChanged(self.info.instance_id))
    }

    /// Process a `SDL_CONTROLLERSENSORUPDATE` event.
    ///
    /// SDL timestamps are in milliseconds; chiaki expects microseconds.
    fn handle_sensor(
        &mut self,
        sensor_type: SensorType,
        data: [f32; 3],
        timestamp_ms: u32,
    ) -> Option<ControllerEvent> {
        let timestamp_us = timestamp_ms as u64 * 1000;

        match sensor_type {
            SensorType::Accelerometer => {
                let ax = data[0] / SDL_STANDARD_GRAVITY;
                let ay = data[1] / SDL_STANDARD_GRAVITY;
                let az = data[2] / SDL_STANDARD_GRAVITY;

                unsafe {
                    sys::chiaki_accel_new_zero_set_active(
                        &mut self.real_accel,
                        ax, ay, az,
                        true,
                    );
                    let (gx, gy, gz) = self.state.gyro();
                    sys::chiaki_orientation_tracker_update(
                        &mut self.orientation_tracker,
                        gx, gy, gz,
                        ax, ay, az,
                        &mut self.accel_zero,
                        false,
                        timestamp_us as u32,
                    );
                }
            }
            SensorType::Gyroscope => {
                let (ax, ay, az) = self.state.accel();
                unsafe {
                    sys::chiaki_orientation_tracker_update(
                        &mut self.orientation_tracker,
                        data[0], data[1], data[2],
                        ax, ay, az,
                        &mut self.accel_zero,
                        true,
                        timestamp_us as u32,
                    );
                }
            }
            _ => return None,
        }

        self.last_motion_timestamp_us = timestamp_us;
        unsafe {
            sys::chiaki_orientation_tracker_apply_to_controller_state(
                &mut self.orientation_tracker,
                &mut self.state.0,
            );
        }
        Some(ControllerEvent::StateChanged(self.info.instance_id))
    }

    fn handle_touchpad_down(
        &mut self,
        touchpad: u32,
        finger: u32,
        x: f32,
        y: f32,
    ) -> Option<ControllerEvent> {
        let tx = (x * PS_TOUCHPAD_MAXX) as u16;
        let ty = (y * PS_TOUCHPAD_MAXY) as u16;
        let id = self.state.start_touch(tx, ty)?;
        self.touch_ids.insert((touchpad, finger), id as u8);
        Some(ControllerEvent::StateChanged(self.info.instance_id))
    }

    fn handle_touchpad_motion(
        &mut self,
        touchpad: u32,
        finger: u32,
        x: f32,
        y: f32,
    ) -> Option<ControllerEvent> {
        let chiaki_id = *self.touch_ids.get(&(touchpad, finger))?;
        let tx = (x * PS_TOUCHPAD_MAXX) as u16;
        let ty = (y * PS_TOUCHPAD_MAXY) as u16;
        self.state.set_touch_pos(chiaki_id, tx, ty);
        Some(ControllerEvent::StateChanged(self.info.instance_id))
    }

    fn handle_touchpad_up(
        &mut self,
        touchpad: u32,
        finger: u32,
    ) -> Option<ControllerEvent> {
        let chiaki_id = self.touch_ids.remove(&(touchpad, finger))?;
        self.state.stop_touch(chiaki_id);
        Some(ControllerEvent::StateChanged(self.info.instance_id))
    }

    // ── Raw joystick events for controller mapping ────────────────────────────

    fn handle_joy_button_down(&mut self, button: u8) -> Option<ControllerEvent> {
        if self.updating_mapping_button {
            self.updating_mapping_button = false;
            return Some(ControllerEvent::NewButtonMapping(
                self.info.instance_id,
                format!("b{button}"),
            ));
        }
        None
    }

    fn handle_joy_axis(&mut self, axis: u8) -> Option<ControllerEvent> {
        if self.updating_mapping_button && self.enable_analog_stick_mapping {
            self.updating_mapping_button = false;
            return Some(ControllerEvent::NewButtonMapping(
                self.info.instance_id,
                format!("a{axis}"),
            ));
        }
        None
    }

    fn handle_joy_hat(&mut self, hat: u8, value: u8) -> Option<ControllerEvent> {
        if self.updating_mapping_button {
            self.updating_mapping_button = false;
            return Some(ControllerEvent::NewButtonMapping(
                self.info.instance_id,
                format!("h{hat}.{value}"),
            ));
        }
        None
    }

    // ── Haptic / LED output ───────────────────────────────────────────────────

    /// Send a raw DualSense effect packet via SDL.
    fn send_ds5_effect(&self, state: &Ds5EffectsState) {
        // SAFETY: raw_ptr() is valid for &self calls; state is repr(C,packed).
        let data = unsafe {
            std::slice::from_raw_parts(
                state as *const _ as *const u8,
                std::mem::size_of::<Ds5EffectsState>(),
            )
        };
        unsafe {
            sdl_sys::SDL_GameControllerSendEffect(
                self.raw_ptr(),
                data.as_ptr() as *const std::ffi::c_void,
                data.len() as i32,
            );
        }
    }

    fn set_dualsense_rumble(&self, left: u8, right: u8, intensity: u8) {
        let mut s = Ds5EffectsState::default();
        if self.firmware_version < 0x0224 {
            // Older firmware: legacy rumble path (half-scale).
            s.enable_bits1 |= 0x01;
            s.rumble_left = left >> 1;
            s.rumble_right = right >> 1;
        } else {
            // Newer firmware: full-scale compatible audio-haptic rumble.
            s.enable_bits3 |= 0x04;
            s.rumble_left = left;
            s.rumble_right = right;
        }
        s.unknown1[4] = intensity;
        s.enable_bits2 |= 0x40;
        s.enable_bits1 |= 0x02;
        self.send_ds5_effect(&s);
    }

    /// Set motor rumble.  Automatically selects the DualSense or generic path.
    pub fn set_rumble(&self, left: u8, right: u8, dualsense_intensity: u8) {
        if self.info.is_dualsense || self.info.is_dualsense_edge {
            self.set_dualsense_rumble(left, right, dualsense_intensity);
        } else {
            // SAFETY: raw_ptr() valid for &self.
            unsafe {
                sdl_sys::SDL_GameControllerRumble(
                    self.raw_ptr(),
                    (left as u16) << 8,
                    (right as u16) << 8,
                    5000,
                );
            }
        }
    }

    /// Set haptic rumble (16-bit values, used by the Chiaki feedback protocol).
    pub fn set_haptic_rumble(&self, left: u16, right: u16, dualsense_intensity: u8) {
        if self.info.is_dualsense || self.info.is_dualsense_edge {
            self.set_dualsense_rumble((left >> 8) as u8, (right >> 8) as u8, dualsense_intensity);
        } else {
            unsafe {
                sdl_sys::SDL_GameControllerRumble(self.raw_ptr(), left, right, 5000);
            }
        }
    }

    /// Send DualSense adaptive trigger effects.
    pub fn set_trigger_effects(
        &self,
        type_left: u8,
        data_left: &[u8; 10],
        type_right: u8,
        data_right: &[u8; 10],
        intensity: u8,
    ) {
        if !self.info.is_dualsense && !self.info.is_dualsense_edge {
            return;
        }
        let mut s = Ds5EffectsState::default();
        s.unknown1[4] = intensity;
        s.enable_bits2 |= 0x40;
        s.enable_bits1 |= 0x04 | 0x08; // left + right trigger
        s.left_trigger_effect[0] = type_left;
        s.left_trigger_effect[1..11].copy_from_slice(data_left);
        s.right_trigger_effect[0] = type_right;
        s.right_trigger_effect[1..11].copy_from_slice(data_right);
        self.send_ds5_effect(&s);
    }

    /// Clear all adaptive trigger effects (used on controller close).
    fn clear_trigger_effects(&self, intensity: u8) {
        let clear = [0u8; 10];
        self.set_trigger_effects(0x05, &clear, 0x05, &clear, intensity);
    }

    /// Set the RGB LED colour.  No-op if the controller has no LED.
    pub fn change_led_color(&self, r: u8, g: u8, b: u8) {
        if self.info.has_led {
            unsafe {
                sdl_sys::SDL_GameControllerSetLED(self.raw_ptr(), r, g, b);
            }
        }
    }

    /// Set the player index indicator LED.
    pub fn change_player_index(&self, index: i32) {
        unsafe {
            sdl_sys::SDL_GameControllerSetPlayerIndex(self.raw_ptr(), index);
        }
    }

    /// Control the DualSense built-in microphone and its mute LED.
    pub fn set_dualsense_mic(&self, muted: bool) {
        if !self.info.is_dualsense && !self.info.is_dualsense_edge {
            return;
        }
        let mut s = Ds5EffectsState::default();
        s.enable_bits2 |= 0x01 | 0x02; // mic light + mic
        if muted {
            s.mic_light_mode = 0x01;
            s.audio_mute_bits = 0x08;
        }
        self.send_ds5_effect(&s);
    }

    /// Reset the orientation/gyro calibration to the current gravity vector.
    pub fn reset_motion_controls(&mut self) {
        unsafe {
            sys::chiaki_accel_new_zero_set_active(
                &mut self.accel_zero,
                self.real_accel.accel_x,
                self.real_accel.accel_y,
                self.real_accel.accel_z,
                false,
            );
            sys::chiaki_orientation_tracker_init(&mut self.orientation_tracker);
            let (gx, gy, gz) = self.state.gyro();
            sys::chiaki_orientation_tracker_update(
                &mut self.orientation_tracker,
                gx, gy, gz,
                self.real_accel.accel_x,
                self.real_accel.accel_y,
                self.real_accel.accel_z,
                &mut self.accel_zero,
                false,
                self.last_motion_timestamp_us as u32,
            );
            sys::chiaki_orientation_tracker_apply_to_controller_state(
                &mut self.orientation_tracker,
                &mut self.state.0,
            );
        }
    }
}

impl Drop for ControllerInner {
    fn drop(&mut self) {
        // Clear trigger effects before closing (SDL doesn't do this automatically).
        self.clear_trigger_effects(0);
        // Zero rumble.
        unsafe {
            sdl_sys::SDL_GameControllerRumble(self.raw_ptr(), 0, 0, 0);
        }
    }
}

// ── Button mapping helper ─────────────────────────────────────────────────────

fn sdl_button_to_ps(button: Button) -> Option<ControllerButtons> {
    Some(match button {
        Button::A => ControllerButtons::CROSS,
        Button::B => ControllerButtons::MOON,
        Button::X => ControllerButtons::BOX,
        Button::Y => ControllerButtons::PYRAMID,
        Button::DPadLeft => ControllerButtons::DPAD_LEFT,
        Button::DPadRight => ControllerButtons::DPAD_RIGHT,
        Button::DPadUp => ControllerButtons::DPAD_UP,
        Button::DPadDown => ControllerButtons::DPAD_DOWN,
        Button::LeftShoulder => ControllerButtons::L1,
        Button::RightShoulder => ControllerButtons::R1,
        Button::LeftStick => ControllerButtons::L3,
        Button::RightStick => ControllerButtons::R3,
        Button::Start => ControllerButtons::OPTIONS,
        Button::Back => ControllerButtons::SHARE,
        Button::Guide => ControllerButtons::PS,
        Button::Touchpad => ControllerButtons::TOUCHPAD,
        _ => return None,
    })
}

// ── ControllerManager ─────────────────────────────────────────────────────────

/// Manages SDL2 game controllers and translates their input into
/// [`ControllerState`] values for use with a chiaki streaming session.
///
/// Call [`process_event`](Self::process_event) for every SDL event from your
/// event pump.  Open specific controllers with
/// [`open_controller`](Self::open_controller) before expecting state updates.
pub struct ControllerManager {
    gc_subsystem: GameControllerSubsystem,
    /// Maps SDL joystick *instance ID* → device *index* for available controllers.
    available: HashMap<u32, u32>,
    /// Maps SDL joystick *instance ID* → open controller state.
    open: HashMap<u32, ControllerInner>,
    /// Haptic intensity for DualSense controllers (0 = off).
    dualsense_intensity: u8,
    /// Set to `true` whenever any controller state changed.  Cleared by
    /// [`take_moved`](Self::take_moved).
    moved: bool,
    /// When `true`, the next raw joystick input is captured as a new mapping.
    creating_mapping: bool,
}

impl ControllerManager {
    /// Create a new `ControllerManager` using the given SDL context.
    ///
    /// SDL is configured to:
    /// - Enable PS4 and PS5 HIDAPI rumble so DualShock/DualSense haptics work.
    /// - Allow background joystick events.
    /// - Disable the Steam Deck HIDAPI driver (it conflicts with the generic HID path).
    pub fn new(sdl_ctx: &sdl2::Sdl) -> Result<Self, String> {
        // ── Configure SDL hints ──────────────────────────────────────────────
        sdl2::hint::set("SDL_JOYSTICK_HIDAPI_PS4_RUMBLE", "1");
        sdl2::hint::set("SDL_JOYSTICK_HIDAPI_PS5_RUMBLE", "1");
        sdl2::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
        // Disable Steam Deck HIDAPI driver to avoid conflicts with generic HID.
        sdl2::hint::set("SDL_JOYSTICK_HIDAPI_STEAMDECK", "0");

        let gc_subsystem = sdl_ctx.game_controller()?;

        let mut mgr = Self {
            gc_subsystem,
            available: HashMap::new(),
            open: HashMap::new(),
            dualsense_intensity: 0,
            moved: false,
            creating_mapping: false,
        };

        mgr.refresh_available();
        Ok(mgr)
    }

    // ── Discovery ─────────────────────────────────────────────────────────────

    /// Rescan all joysticks and rebuild the set of available game controllers.
    ///
    /// Called automatically on construction and on every
    /// `JoyDeviceAdded`/`JoyDeviceRemoved` event.
    fn refresh_available(&mut self) {
        let n = match self.gc_subsystem.num_joysticks() {
            Ok(n) => n,
            Err(_) => return,
        };

        let mut new_available: HashMap<u32, u32> = HashMap::new();

        for idx in 0..n {
            if !self.gc_subsystem.is_game_controller(idx) {
                continue;
            }
            let instance_id =
                unsafe { sdl_sys::SDL_JoystickGetDeviceInstanceID(idx as i32) };
            if instance_id < 0 {
                continue;
            }
            new_available.insert(instance_id as u32, idx);
        }

        self.available = new_available;
    }

    /// Return the instance IDs of all detected (but not necessarily open) game controllers.
    pub fn available_controllers(&self) -> Vec<u32> {
        self.available.keys().copied().collect()
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    /// Open a controller by its SDL instance ID.
    ///
    /// Returns `true` on success.  The controller must be in
    /// [`available_controllers`](Self::available_controllers).
    /// If the controller is already open this is a no-op.
    pub fn open_controller(&mut self, instance_id: u32) -> bool {
        if self.open.contains_key(&instance_id) {
            return true; // already open
        }
        let device_index = match self.available.get(&instance_id) {
            Some(&idx) => idx,
            None => return false,
        };
        match ControllerInner::open(&self.gc_subsystem, device_index, instance_id) {
            Ok(inner) => {
                self.open.insert(instance_id, inner);
                true
            }
            Err(_) => false,
        }
    }

    /// Close a previously opened controller.
    ///
    /// Trigger effects and rumble are cleared automatically (via `Drop`).
    pub fn close_controller(&mut self, instance_id: u32) {
        if let Some(inner) = self.open.get(&instance_id) {
            inner.clear_trigger_effects(self.dualsense_intensity);
        }
        self.open.remove(&instance_id);
    }

    // ── Configuration ─────────────────────────────────────────────────────────

    /// Set the haptic intensity used for DualSense trigger effects and motors.
    ///
    /// `0x00` = off, `0xff` = maximum.
    pub fn set_dualsense_intensity(&mut self, intensity: u8) {
        self.dualsense_intensity = intensity;
    }

    /// Current DualSense haptic intensity.
    pub fn dualsense_intensity(&self) -> u8 {
        self.dualsense_intensity
    }

    /// Use *button position* (ABXY by position) instead of *button label*.
    pub fn set_buttons_by_position(&self) {
        sdl2::hint::set("SDL_GAMECONTROLLER_USE_BUTTON_LABELS", "0");
    }

    /// Arm the controller-mapping mode.  The next raw joystick button, axis or
    /// hat event will emit [`ControllerEvent::NewButtonMapping`].
    pub fn start_creating_mapping(&mut self, enable_analog_stick_mapping: bool) {
        self.creating_mapping = true;
        for c in self.open.values_mut() {
            c.updating_mapping_button = true;
            c.enable_analog_stick_mapping = enable_analog_stick_mapping;
        }
    }

    /// Returns `true` if any controller moved since the last call to this method.
    pub fn take_moved(&mut self) -> bool {
        let v = self.moved;
        self.moved = false;
        v
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    /// Static information for an open controller.
    pub fn controller_info(&self, instance_id: u32) -> Option<&ControllerInfo> {
        self.open.get(&instance_id).map(|c| &c.info)
    }

    /// Current [`ControllerState`] for an open controller.
    pub fn controller_state(&self, instance_id: u32) -> Option<&ControllerState> {
        self.open.get(&instance_id).map(|c| &c.state)
    }

    /// Returns `true` if the controller is open and still physically attached.
    pub fn is_controller_connected(&self, instance_id: u32) -> bool {
        self.open
            .get(&instance_id)
            .map(|c| c.is_connected())
            .unwrap_or(false)
    }

    /// Returns the instance IDs of all currently open controllers.
    pub fn open_controllers(&self) -> Vec<u32> {
        self.open.keys().copied().collect()
    }

    // ── Haptic / LED forwarding ───────────────────────────────────────────────

    /// Set rumble motors.  `left`/`right` are 0–255.
    pub fn set_rumble(&self, instance_id: u32, left: u8, right: u8) {
        if let Some(c) = self.open.get(&instance_id) {
            c.set_rumble(left, right, self.dualsense_intensity);
        }
    }

    /// Set rumble using 16-bit values (as delivered by the Chiaki feedback protocol).
    pub fn set_haptic_rumble(&self, instance_id: u32, left: u16, right: u16) {
        if let Some(c) = self.open.get(&instance_id) {
            c.set_haptic_rumble(left, right, self.dualsense_intensity);
        }
    }

    /// Send DualSense adaptive trigger effects.
    pub fn set_trigger_effects(
        &self,
        instance_id: u32,
        type_left: u8,
        data_left: &[u8; 10],
        type_right: u8,
        data_right: &[u8; 10],
    ) {
        if let Some(c) = self.open.get(&instance_id) {
            c.set_trigger_effects(
                type_left,
                data_left,
                type_right,
                data_right,
                self.dualsense_intensity,
            );
        }
    }

    /// Set the RGB LED colour on the controller.
    pub fn change_led_color(&self, instance_id: u32, r: u8, g: u8, b: u8) {
        if let Some(c) = self.open.get(&instance_id) {
            c.change_led_color(r, g, b);
        }
    }

    /// Set the player index indicator.
    pub fn change_player_index(&self, instance_id: u32, index: i32) {
        if let Some(c) = self.open.get(&instance_id) {
            c.change_player_index(index);
        }
    }

    /// Mute or unmute the built-in DualSense microphone LED.
    pub fn set_dualsense_mic(&self, instance_id: u32, muted: bool) {
        if let Some(c) = self.open.get(&instance_id) {
            c.set_dualsense_mic(muted);
        }
    }

    /// Recalibrate orientation by resetting the zero-gravity reference.
    pub fn reset_motion_controls(&mut self, instance_id: u32) {
        if let Some(c) = self.open.get_mut(&instance_id) {
            c.reset_motion_controls();
        }
    }

    // ── Event processing ──────────────────────────────────────────────────────

    /// Process a single SDL event.
    ///
    /// Returns a (possibly empty) list of [`ControllerEvent`]s produced by
    /// this event.  Must be called from the same thread that owns the SDL
    /// event pump.
    pub fn process_event(&mut self, event: &Event) -> Vec<ControllerEvent> {
        let mut out = Vec::new();

        match *event {
            // ── Device hotplug ───────────────────────────────────────────────
            // JoyDeviceAdded.which = device INDEX; JoyDeviceRemoved.which = instance ID.
            Event::JoyDeviceAdded { .. } => {
                self.refresh_available();
                out.push(ControllerEvent::AvailableControllersUpdated);
            }

            Event::JoyDeviceRemoved { which, .. } => {
                self.refresh_available();
                out.push(ControllerEvent::AvailableControllersUpdated);
                self.open.remove(&which);
            }

            // ── Controller button events ─────────────────────────────────────
            Event::ControllerButtonDown { which, button, .. } => {
                if let Some(c) = self.open.get_mut(&which) {
                    if let Some(e) = c.handle_button_down(button) {
                        self.moved = true;
                        out.push(e);
                    }
                }
            }

            Event::ControllerButtonUp { which, button, .. } => {
                if let Some(c) = self.open.get_mut(&which) {
                    if let Some(e) = c.handle_button_up(button) {
                        if matches!(e, ControllerEvent::StateChanged(_)) {
                            self.moved = true;
                        }
                        out.push(e);
                    }
                }
            }

            // ── Axis motion ──────────────────────────────────────────────────
            Event::ControllerAxisMotion { which, axis, value, .. } => {
                if let Some(c) = self.open.get_mut(&which) {
                    if let Some(e) = c.handle_axis(axis, value) {
                        self.moved = true;
                        out.push(e);
                    }
                }
            }

            // ── Sensor (gyro / accelerometer) ────────────────────────────────
            Event::ControllerSensorUpdated { which, sensor, data, timestamp, .. } => {
                if let Some(c) = self.open.get_mut(&which) {
                    if let Some(e) = c.handle_sensor(sensor, data, timestamp) {
                        self.moved = true;
                        out.push(e);
                    }
                }
            }

            // ── Touchpad ─────────────────────────────────────────────────────
            Event::ControllerTouchpadDown { which, touchpad, finger, x, y, .. } => {
                if let Some(c) = self.open.get_mut(&which) {
                    if let Some(e) = c.handle_touchpad_down(touchpad, finger, x, y) {
                        self.moved = true;
                        out.push(e);
                    }
                }
            }

            Event::ControllerTouchpadMotion { which, touchpad, finger, x, y, .. } => {
                if let Some(c) = self.open.get_mut(&which) {
                    if let Some(e) = c.handle_touchpad_motion(touchpad, finger, x, y) {
                        self.moved = true;
                        out.push(e);
                    }
                }
            }

            Event::ControllerTouchpadUp { which, touchpad, finger, .. } => {
                if let Some(c) = self.open.get_mut(&which) {
                    if let Some(e) = c.handle_touchpad_up(touchpad, finger) {
                        self.moved = true;
                        out.push(e);
                    }
                }
            }

            // ── Raw joystick events (used only during button-mapping) ─────────
            Event::JoyAxisMotion { which, axis_idx, .. } => {
                if let Some(c) = self.open.get_mut(&which) {
                    if let Some(e) = c.handle_joy_axis(axis_idx) {
                        out.push(e);
                        self.creating_mapping = false;
                    }
                }
            }

            Event::JoyButtonDown { which, button_idx, .. } => {
                if let Some(c) = self.open.get_mut(&which) {
                    if let Some(e) = c.handle_joy_button_down(button_idx) {
                        out.push(e);
                        self.creating_mapping = false;
                    }
                }
            }

            Event::JoyHatMotion { which, hat_idx, state, .. } => {
                if let Some(c) = self.open.get_mut(&which) {
                    let hat_val: u8 = match state {
                        sdl2::joystick::HatState::LeftUp => 9,
                        sdl2::joystick::HatState::Up => 1,
                        sdl2::joystick::HatState::RightUp => 5,
                        sdl2::joystick::HatState::Left => 8,
                        sdl2::joystick::HatState::Centered => 0,
                        sdl2::joystick::HatState::Right => 2,
                        sdl2::joystick::HatState::LeftDown => 10,
                        sdl2::joystick::HatState::Down => 4,
                        sdl2::joystick::HatState::RightDown => 6,
                    };
                    if let Some(e) = c.handle_joy_hat(hat_idx, hat_val) {
                        out.push(e);
                        self.creating_mapping = false;
                    }
                }
            }

            _ => {}
        }

        out
    }
}
