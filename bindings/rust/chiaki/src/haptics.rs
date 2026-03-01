// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! DualSense haptics processing: direct audio output and motor-rumble fallback.
//!
//! This module implements the two haptics paths also found in the C++ GUI's
//! `PushHapticsFrame`:
//!
//! **(a) Direct DualSense audio** — remixes the PS5 stereo haptics stream
//! (3 kHz, 2-channel int16) to 4-channel and resamples to 48 kHz, then queues
//! the raw waveform to the DualSense's voice-coil actuators via SDL2.
//!
//! **(b) Motor rumble fallback** — computes the mean absolute amplitude and
//! converts it to a u16 motor-rumble strength pushed via [`FeedbackCmd::HapticRumble`].

use std::ffi::CStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::feedback::FeedbackCmd;
use crate::session::{AudioHeader, AudioSink};

// ── Platform-dependent constants ─────────────────────────────────────────────

/// DualSense audio device name needle (platform-dependent, mirrors the C++ GUI).
#[cfg(target_os = "linux")]
pub const DUALSENSE_AUDIO_NEEDLE: &str = "DualSense";
/// DualSense audio device name needle (platform-dependent, mirrors the C++ GUI).
#[cfg(not(target_os = "linux"))]
pub const DUALSENSE_AUDIO_NEEDLE: &str = "Wireless Controller";

// ── Rumble approximation constants ───────────────────────────────────────────

/// Number of haptic audio frames to average before each rumble update (~30 ms).
pub const RUMBLE_HAPTICS_PACKETS_PER_RUMBLE: u32 = 3;
/// Minimum mean absolute amplitude (0–65535) required to trigger any rumble.
pub const HAPTIC_RUMBLE_MIN_STRENGTH: u32 = 100;
/// Minimum non-zero rumble value to ensure controllers that require a minimum
/// signal (>= 9-bit range) actually vibrate.
pub const HAPTIC_RUMBLE_FLOOR: u16 = 1 << 9; // 512

// ── Device discovery ─────────────────────────────────────────────────────────

/// Try to find and open the DualSense as an SDL audio output device for
/// direct haptics playback (48 kHz, 4-channel int16).
///
/// Returns the raw SDL audio device ID (> 0 on success, 0 when not found).
///
/// # Safety
///
/// Requires the SDL2 audio subsystem to be initialised before calling.
pub fn open_dualsense_haptics_device() -> u32 {
    let count = unsafe { sdl2::sys::SDL_GetNumAudioDevices(0) }; // 0 = playback
    for i in 0..count {
        let name_ptr = unsafe { sdl2::sys::SDL_GetAudioDeviceName(i, 0) };
        if name_ptr.is_null() {
            continue;
        }
        let name = unsafe { CStr::from_ptr(name_ptr) }.to_string_lossy();
        if !name.contains(DUALSENSE_AUDIO_NEEDLE) {
            continue;
        }

        let desired = sdl2::sys::SDL_AudioSpec {
            freq: 48000,
            format: 0x8010, // AUDIO_S16LSB (= AUDIO_S16SYS on little-endian)
            channels: 4,
            silence: 0,
            samples: 480,
            padding: 0,
            size: 0,
            callback: None,
            userdata: std::ptr::null_mut(),
        };
        let mut obtained = unsafe { std::mem::zeroed::<sdl2::sys::SDL_AudioSpec>() };
        let dev = unsafe {
            sdl2::sys::SDL_OpenAudioDevice(name_ptr, 0, &desired, &mut obtained, 0)
        };
        if dev == 0 {
            continue;
        }
        // Start accepting queued audio.
        unsafe { sdl2::sys::SDL_PauseAudioDevice(dev, 0) };
        return dev;
    }
    0
}

/// Close a previously opened DualSense haptics audio device.
pub fn close_dualsense_haptics_device(device_id: u32) {
    if device_id > 0 {
        unsafe { sdl2::sys::SDL_CloseAudioDevice(device_id) };
    }
}

// ── Resampler ────────────────────────────────────────────────────────────────

/// 16x linear-interpolation resampler for 4-channel int16 audio (3 kHz -> 48 kHz).
///
/// `input` must contain `N * 4` samples (N frames, 4 channels each).
/// `output` must have room for `N * 16 * 4` samples.
pub fn resample_16x_4ch(input: &[i16], output: &mut [i16]) {
    const CH: usize = 4;
    let n = input.len() / CH;
    for i in 0..n {
        let next = (i + 1).min(n - 1);
        for j in 0..16u32 {
            let base = (i * 16 + j as usize) * CH;
            for c in 0..CH {
                let a = input[i * CH + c] as i32;
                let b = input[next * CH + c] as i32;
                output[base + c] = (a + (b - a) * j as i32 / 16) as i16;
            }
        }
    }
}

// ── HapticsSink ──────────────────────────────────────────────────────────────

/// Builder for [`HapticsSink`].
pub struct HapticsSinkBuilder {
    feedback_cmds: Option<Arc<Mutex<Vec<FeedbackCmd>>>>,
    frame_counter: Option<Arc<AtomicU64>>,
    haptics_device_id: u32,
}

impl HapticsSinkBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            feedback_cmds: None,
            frame_counter: None,
            haptics_device_id: 0,
        }
    }

    /// Supply a shared feedback-command queue for the motor-rumble fallback path.
    pub fn feedback_cmds(mut self, cmds: Arc<Mutex<Vec<FeedbackCmd>>>) -> Self {
        self.feedback_cmds = Some(cmds);
        self
    }

    /// Supply an atomic frame counter for statistics.
    pub fn frame_counter(mut self, counter: Arc<AtomicU64>) -> Self {
        self.frame_counter = Some(counter);
        self
    }

    /// Set the SDL audio device ID for direct DualSense haptics.
    ///
    /// Pass `0` (or omit) to use the motor-rumble fallback.
    pub fn haptics_device_id(mut self, id: u32) -> Self {
        self.haptics_device_id = id;
        self
    }

    /// Build the [`HapticsSink`].
    ///
    /// # Panics
    ///
    /// Panics if `feedback_cmds` was not set (required for the rumble fallback
    /// path even when direct audio is available, to handle edge cases).
    pub fn build(self) -> HapticsSink {
        HapticsSink {
            feedback_cmds: self
                .feedback_cmds
                .expect("HapticsSinkBuilder: feedback_cmds is required"),
            frame_count: self
                .frame_counter
                .unwrap_or_else(|| Arc::new(AtomicU64::new(0))),
            accumulated: 0,
            num_frames: 0,
            last_rumble_on: false,
            haptics_device_id: self.haptics_device_id,
            remix_buf: vec![0i16; 30 * 4],         // 30 samples * 4 channels
            resample_buf: vec![0i16; 30 * 16 * 4],  // 480 samples * 4 channels
        }
    }
}

impl Default for HapticsSinkBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// [`AudioSink`] for DualSense haptics with two paths:
///
/// **(a) Direct audio** — when `haptics_device_id > 0`, remixes stereo to
/// 4-channel, resamples 3 kHz to 48 kHz, and queues to the DualSense via
/// `SDL_QueueAudio`.
///
/// **(b) Motor rumble fallback** — when `haptics_device_id == 0`, computes
/// the mean absolute amplitude, applies thresholding and a 9-bit floor, then
/// pushes [`FeedbackCmd::HapticRumble`] through the shared feedback queue.
pub struct HapticsSink {
    feedback_cmds: Arc<Mutex<Vec<FeedbackCmd>>>,
    frame_count: Arc<AtomicU64>,
    // Motor rumble approximation state (fallback path b).
    accumulated: u32,
    num_frames: u32,
    last_rumble_on: bool,
    // Direct haptics audio output (path a).
    // 0 = not available -> use motor rumble fallback.
    haptics_device_id: u32,
    remix_buf: Vec<i16>,
    resample_buf: Vec<i16>,
}

impl HapticsSink {
    /// Create a new builder.
    pub fn builder() -> HapticsSinkBuilder {
        HapticsSinkBuilder::new()
    }

    /// Whether this sink is using the direct DualSense audio path.
    pub fn is_direct_audio(&self) -> bool {
        self.haptics_device_id > 0
    }
}

impl AudioSink for HapticsSink {
    fn on_header(&mut self, _h: AudioHeader) {
        // No action needed; buffers are lazily resized in on_frame.
    }

    fn on_frame(&mut self, data: &[u8]) {
        self.frame_count.fetch_add(1, Ordering::Relaxed);

        let sample_size = 4; // 2 * i16 (stereo)
        let sample_count = data.len() / sample_size;

        // ── (a) Direct DualSense haptics audio ──────────────────────────────
        if self.haptics_device_id > 0 {
            if sample_count == 0 {
                return;
            }
            let remix_len = sample_count * 4;
            let resample_len = sample_count * 16 * 4;

            // Ensure buffers are large enough.
            if self.remix_buf.len() < remix_len {
                self.remix_buf.resize(remix_len, 0);
            }
            if self.resample_buf.len() < resample_len {
                self.resample_buf.resize(resample_len, 0);
            }

            // Remix stereo -> 4ch: [0, 0, L, R] per sample.
            for i in 0..sample_count {
                let off = i * sample_size;
                let l = i16::from_le_bytes([data[off], data[off + 1]]);
                let r = i16::from_le_bytes([data[off + 2], data[off + 3]]);
                self.remix_buf[i * 4] = 0;
                self.remix_buf[i * 4 + 1] = 0;
                self.remix_buf[i * 4 + 2] = l;
                self.remix_buf[i * 4 + 3] = r;
            }

            // Resample 3 kHz -> 48 kHz (x16 linear interpolation).
            resample_16x_4ch(
                &self.remix_buf[..remix_len],
                &mut self.resample_buf[..resample_len],
            );

            // Queue resampled audio to the DualSense audio device.
            unsafe {
                sdl2::sys::SDL_QueueAudio(
                    self.haptics_device_id,
                    self.resample_buf.as_ptr() as *const std::ffi::c_void,
                    (resample_len * std::mem::size_of::<i16>()) as u32,
                );
            }
            return;
        }

        // ── (b) Motor rumble approximation fallback ─────────────────────────

        // Compute per-channel mean absolute amplitude for this frame.
        let (mut temp_left, mut temp_right): (u32, u32) = if sample_count > 0 {
            let mut suml: u64 = 0;
            let mut sumr: u64 = 0;
            for i in 0..sample_count {
                let off = i * sample_size;
                let l = i16::from_le_bytes([data[off], data[off + 1]]);
                let r = i16::from_le_bytes([data[off + 2], data[off + 3]]);
                // x2 matches the C++ `qFabs(amplitude) * 2` scaling.
                suml += (l.unsigned_abs() as u64) * 2;
                sumr += (r.unsigned_abs() as u64) * 2;
            }
            (
                (suml / sample_count as u64) as u32,
                (sumr / sample_count as u64) as u32,
            )
        } else {
            (0, 0)
        };

        // Per-channel threshold.
        temp_left = if temp_left > HAPTIC_RUMBLE_MIN_STRENGTH {
            temp_left
        } else {
            0
        };
        temp_right = if temp_right > HAPTIC_RUMBLE_MIN_STRENGTH {
            temp_right
        } else {
            0
        };

        // Per-frame strength with 9-bit floor.
        let frame_strength: u16 = if temp_left == 0 && temp_right == 0 {
            0
        } else {
            let left = temp_left.min(u16::MAX as u32) as u16;
            let right = temp_right.min(u16::MAX as u32) as u16;
            let left = if left > 0 && (left as u32) < HAPTIC_RUMBLE_FLOOR as u32 {
                HAPTIC_RUMBLE_FLOOR
            } else {
                left
            };
            let right = if right > 0 && (right as u32) < HAPTIC_RUMBLE_FLOOR as u32 {
                HAPTIC_RUMBLE_FLOOR
            } else {
                right
            };
            left.max(right)
        };

        self.accumulated += frame_strength as u32;
        self.num_frames += 1;

        if self.num_frames >= RUMBLE_HAPTICS_PACKETS_PER_RUMBLE {
            let strength = (self.accumulated / RUMBLE_HAPTICS_PACKETS_PER_RUMBLE) as u16;
            if let Ok(mut cmds) = self.feedback_cmds.lock() {
                if strength > 0 || self.last_rumble_on {
                    cmds.push(FeedbackCmd::HapticRumble {
                        left: strength,
                        right: strength,
                    });
                }
            }
            self.last_rumble_on = strength > 0;
            self.accumulated = 0;
            self.num_frames = 0;
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_16x_4ch_output_length() {
        let input = vec![0i16; 30 * 4]; // 30 frames, 4 channels
        let mut output = vec![0i16; 30 * 16 * 4]; // 480 frames, 4 channels
        resample_16x_4ch(&input, &mut output);
        // All zeros in -> all zeros out
        assert!(output.iter().all(|&v| v == 0));
    }

    #[test]
    fn resample_16x_4ch_constant_input() {
        // Constant value should remain constant after interpolation.
        let input = vec![1000i16; 8 * 4]; // 8 frames, 4 channels, all 1000
        let mut output = vec![0i16; 8 * 16 * 4];
        resample_16x_4ch(&input, &mut output);
        for &v in &output {
            assert_eq!(v, 1000);
        }
    }

    #[test]
    fn resample_16x_4ch_linear_interpolation() {
        // Two frames: ch0 goes from 0 to 1600.
        let mut input = vec![0i16; 2 * 4];
        input[0] = 0;    // frame 0, ch 0
        input[4] = 1600; // frame 1, ch 0
        let mut output = vec![0i16; 2 * 16 * 4];
        resample_16x_4ch(&input, &mut output);

        // Frame 0's 16 interpolated values for ch0 should be 0, 100, 200, ...
        for j in 0..16 {
            let expected = (1600 * j as i32) / 16;
            assert_eq!(output[(j * 4) as usize], expected as i16);
        }
    }

    #[test]
    fn haptics_sink_motor_rumble_below_threshold() {
        let cmds = Arc::new(Mutex::new(Vec::new()));
        let mut sink = HapticsSink::builder()
            .feedback_cmds(Arc::clone(&cmds))
            .haptics_device_id(0)
            .build();

        // Very quiet audio — below HAPTIC_RUMBLE_MIN_STRENGTH.
        // Sample: L=10, R=10 (mean abs * 2 = 20, below 100 threshold)
        let mut frame = Vec::new();
        for _ in 0..30 {
            frame.extend_from_slice(&10i16.to_le_bytes());
            frame.extend_from_slice(&10i16.to_le_bytes());
        }

        // Push 3 frames to trigger evaluation.
        for _ in 0..3 {
            sink.on_frame(&frame);
        }

        let locked = cmds.lock().unwrap();
        // Should produce no rumble commands (below threshold).
        assert!(locked.is_empty());
    }

    #[test]
    fn haptics_sink_motor_rumble_above_threshold() {
        let cmds = Arc::new(Mutex::new(Vec::new()));
        let mut sink = HapticsSink::builder()
            .feedback_cmds(Arc::clone(&cmds))
            .haptics_device_id(0)
            .build();

        // Loud audio — above threshold.
        // Sample: L=5000, R=5000 (mean abs * 2 = 10000, well above 100)
        let mut frame = Vec::new();
        for _ in 0..30 {
            frame.extend_from_slice(&5000i16.to_le_bytes());
            frame.extend_from_slice(&5000i16.to_le_bytes());
        }

        // Push 3 frames to trigger evaluation.
        for _ in 0..3 {
            sink.on_frame(&frame);
        }

        let locked = cmds.lock().unwrap();
        assert_eq!(locked.len(), 1);
        match &locked[0] {
            FeedbackCmd::HapticRumble { left, right } => {
                assert!(*left > 0);
                assert_eq!(left, right);
            }
            _ => panic!("Expected HapticRumble"),
        }
    }

    #[test]
    fn haptics_sink_accumulates_over_packets() {
        let cmds = Arc::new(Mutex::new(Vec::new()));
        let mut sink = HapticsSink::builder()
            .feedback_cmds(Arc::clone(&cmds))
            .haptics_device_id(0)
            .build();

        let mut frame = Vec::new();
        for _ in 0..30 {
            frame.extend_from_slice(&5000i16.to_le_bytes());
            frame.extend_from_slice(&5000i16.to_le_bytes());
        }

        // After 1 and 2 frames — no command emitted yet.
        sink.on_frame(&frame);
        assert!(cmds.lock().unwrap().is_empty());
        sink.on_frame(&frame);
        assert!(cmds.lock().unwrap().is_empty());

        // After 3rd frame — command emitted.
        sink.on_frame(&frame);
        assert_eq!(cmds.lock().unwrap().len(), 1);
    }

    #[test]
    fn haptics_sink_floor_applied() {
        let cmds = Arc::new(Mutex::new(Vec::new()));
        let mut sink = HapticsSink::builder()
            .feedback_cmds(Arc::clone(&cmds))
            .haptics_device_id(0)
            .build();

        // Audio just above threshold but below floor (mean*2 ~ 200, < 512).
        let mut frame = Vec::new();
        for _ in 0..30 {
            frame.extend_from_slice(&100i16.to_le_bytes());
            frame.extend_from_slice(&100i16.to_le_bytes());
        }

        for _ in 0..3 {
            sink.on_frame(&frame);
        }

        let locked = cmds.lock().unwrap();
        assert_eq!(locked.len(), 1);
        match &locked[0] {
            FeedbackCmd::HapticRumble { left, .. } => {
                // Floor should be applied: result >= HAPTIC_RUMBLE_FLOOR
                assert!(*left >= HAPTIC_RUMBLE_FLOOR);
            }
            _ => panic!("Expected HapticRumble"),
        }
    }
}
