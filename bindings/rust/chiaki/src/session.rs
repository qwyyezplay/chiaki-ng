// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

use std::ffi::{CStr, CString};
use std::os::raw::c_void;
use std::ptr;
use std::sync::Arc;

use chiaki_sys as sys;

use crate::error::{ffi_result, Result};
use crate::controller::ControllerState;
use crate::log::Log;
use crate::regist::RegisteredHost;
use crate::stats::StreamStats;
use crate::types::{Codec, DualSenseEffectIntensity, QuitReason};

// ── Supporting types ──────────────────────────────────────────────────────────

/// Video stream resolution preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoResolutionPreset {
    P360,
    P540,
    P720,
    P1080,
}

impl VideoResolutionPreset {
    fn to_raw(self) -> sys::ChiakiVideoResolutionPreset {
        match self {
            VideoResolutionPreset::P360 => sys::ChiakiVideoResolutionPreset_CHIAKI_VIDEO_RESOLUTION_PRESET_360p,
            VideoResolutionPreset::P540 => sys::ChiakiVideoResolutionPreset_CHIAKI_VIDEO_RESOLUTION_PRESET_540p,
            VideoResolutionPreset::P720 => sys::ChiakiVideoResolutionPreset_CHIAKI_VIDEO_RESOLUTION_PRESET_720p,
            VideoResolutionPreset::P1080 => sys::ChiakiVideoResolutionPreset_CHIAKI_VIDEO_RESOLUTION_PRESET_1080p,
        }
    }
}

/// Video stream frame-rate preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoFpsPreset {
    Fps30,
    Fps60,
}

impl VideoFpsPreset {
    fn to_raw(self) -> sys::ChiakiVideoFPSPreset {
        match self {
            VideoFpsPreset::Fps30 => sys::ChiakiVideoFPSPreset_CHIAKI_VIDEO_FPS_PRESET_30,
            VideoFpsPreset::Fps60 => sys::ChiakiVideoFPSPreset_CHIAKI_VIDEO_FPS_PRESET_60,
        }
    }
}

/// Resolved video codec / resolution / FPS / bitrate profile.
#[derive(Debug, Clone)]
pub struct VideoProfile {
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
    pub bitrate: u32,
    pub codec: Codec,
}

impl VideoProfile {
    /// Build a profile from standard presets.
    pub fn preset(resolution: VideoResolutionPreset, fps: VideoFpsPreset) -> Self {
        let mut raw = unsafe { std::mem::zeroed::<sys::chiaki_connect_video_profile_t>() };
        unsafe {
            sys::chiaki_connect_video_profile_preset(
                &mut raw,
                resolution.to_raw(),
                fps.to_raw(),
            )
        };
        VideoProfile {
            width: raw.width,
            height: raw.height,
            max_fps: raw.max_fps,
            bitrate: raw.bitrate,
            codec: Codec::from_raw(raw.codec),
        }
    }

    pub(crate) fn to_raw(&self) -> sys::chiaki_connect_video_profile_t {
        sys::chiaki_connect_video_profile_t {
            width: self.width,
            height: self.height,
            max_fps: self.max_fps,
            bitrate: self.bitrate,
            codec: self.codec.to_raw(),
        }
    }
}

/// Audio stream header conveyed by [`AudioSink::on_header`].
#[derive(Debug, Clone, Copy)]
pub struct AudioHeader {
    pub channels: u8,
    pub bits: u8,
    pub rate: u32,
    pub frame_size: u32,
}

// ── Event ──────────────────────────────────────────────────────────────────────

/// Session event received by the event callback.
#[derive(Debug)]
pub enum Event {
    /// Stream is connected and video/audio are flowing.
    Connected,
    /// The console requires the user to enter a PIN.
    LoginPinRequest { pin_incorrect: bool },
    /// NAT hole-punching status (only for internet play).
    Holepunch { finished: bool },
    /// Registration completed during the session (auto-regist path).
    Regist(RegisteredHost),
    /// Console sent its nickname.
    NicknameReceived(String),
    /// Console opened the on-screen keyboard.
    KeyboardOpen,
    /// On-screen keyboard text changed.
    KeyboardTextChange(String),
    /// On-screen keyboard was closed by the remote side.
    KeyboardRemoteClose,
    /// DualShock / DualSense rumble request from the console.
    Rumble { unknown: u8, left: u8, right: u8 },
    /// Session has ended.
    Quit {
        reason: QuitReason,
        reason_str: Option<String>,
    },
    /// DualSense adaptive trigger effect parameters.
    TriggerEffects {
        type_left: u8,
        type_right: u8,
        left: [u8; 10],
        right: [u8; 10],
    },
    /// Reset all motion/orientation tracking state.
    MotionReset,
    /// DualSense LED colour (RGB).
    LedColor([u8; 3]),
    /// Controller player index assignment.
    PlayerIndex(u8),
    /// DualSense haptic motor intensity changed.
    HapticIntensity(DualSenseEffectIntensity),
    /// DualSense trigger resistance intensity changed.
    TriggerIntensity(DualSenseEffectIntensity),
}

/// Convert a raw `ChiakiEvent` pointer to an owned Rust [`Event`].
///
/// # Safety
/// `event` must point to a valid, fully-initialised `chiaki_event_t` for the
/// duration of this call.
unsafe fn convert_event(event: *const sys::chiaki_event_t) -> Event { unsafe {
    let e = &*event;
    let u = &e.__bindgen_anon_1;

    match e.type_ {
        sys::ChiakiEventType_CHIAKI_EVENT_CONNECTED => Event::Connected,

        sys::ChiakiEventType_CHIAKI_EVENT_LOGIN_PIN_REQUEST => Event::LoginPinRequest {
            pin_incorrect: u.login_pin_request.pin_incorrect,
        },

        sys::ChiakiEventType_CHIAKI_EVENT_HOLEPUNCH => Event::Holepunch {
            finished: u.data_holepunch.finished,
        },

        sys::ChiakiEventType_CHIAKI_EVENT_REGIST => {
            Event::Regist(RegisteredHost::from_raw(&u.host))
        }

        sys::ChiakiEventType_CHIAKI_EVENT_NICKNAME_RECEIVED => {
            let s = CStr::from_ptr(u.server_nickname.as_ptr())
                .to_string_lossy()
                .into_owned();
            Event::NicknameReceived(s)
        }

        sys::ChiakiEventType_CHIAKI_EVENT_KEYBOARD_OPEN => Event::KeyboardOpen,

        sys::ChiakiEventType_CHIAKI_EVENT_KEYBOARD_TEXT_CHANGE => {
            let text = if u.keyboard.text_str.is_null() {
                String::new()
            } else {
                CStr::from_ptr(u.keyboard.text_str)
                    .to_string_lossy()
                    .into_owned()
            };
            Event::KeyboardTextChange(text)
        }

        sys::ChiakiEventType_CHIAKI_EVENT_KEYBOARD_REMOTE_CLOSE => Event::KeyboardRemoteClose,

        sys::ChiakiEventType_CHIAKI_EVENT_RUMBLE => Event::Rumble {
            unknown: u.rumble.unknown,
            left: u.rumble.left,
            right: u.rumble.right,
        },

        sys::ChiakiEventType_CHIAKI_EVENT_QUIT => {
            let quit = &u.quit;
            let reason_str = if quit.reason_str.is_null() {
                None
            } else {
                Some(
                    CStr::from_ptr(quit.reason_str)
                        .to_string_lossy()
                        .into_owned(),
                )
            };
            Event::Quit {
                reason: QuitReason::from_raw(quit.reason),
                reason_str,
            }
        }

        sys::ChiakiEventType_CHIAKI_EVENT_TRIGGER_EFFECTS => Event::TriggerEffects {
            type_left: u.trigger_effects.type_left,
            type_right: u.trigger_effects.type_right,
            left: u.trigger_effects.left,
            right: u.trigger_effects.right,
        },

        sys::ChiakiEventType_CHIAKI_EVENT_MOTION_RESET => Event::MotionReset,

        sys::ChiakiEventType_CHIAKI_EVENT_LED_COLOR => Event::LedColor(u.led_state),

        sys::ChiakiEventType_CHIAKI_EVENT_PLAYER_INDEX => Event::PlayerIndex(u.player_index),

        sys::ChiakiEventType_CHIAKI_EVENT_HAPTIC_INTENSITY => {
            Event::HapticIntensity(DualSenseEffectIntensity::from_raw(u.intensity))
        }

        sys::ChiakiEventType_CHIAKI_EVENT_TRIGGER_INTENSITY => {
            Event::TriggerIntensity(DualSenseEffectIntensity::from_raw(u.intensity))
        }

        _ => Event::Connected, // Unknown event; treat as no-op
    }
}}

// ── AudioSink trait ───────────────────────────────────────────────────────────

/// Receiver for the decoded audio stream.
///
/// Implement this trait to consume Opus-encoded audio packets from the console.
pub trait AudioSink: Send + 'static {
    /// Called once with audio format details before the first frame.
    fn on_header(&mut self, header: AudioHeader);
    /// Called for each Opus-encoded audio frame.
    fn on_frame(&mut self, opus_data: &[u8]);
}

impl AudioSink for Box<dyn AudioSink> {
    fn on_header(&mut self, header: AudioHeader) {
        (**self).on_header(header);
    }
    fn on_frame(&mut self, opus_data: &[u8]) {
        (**self).on_frame(opus_data);
    }
}

// ── ConnectInfo ───────────────────────────────────────────────────────────────

/// Parameters for establishing a streaming session.
///
/// Build using [`ConnectInfo::builder`].
pub struct ConnectInfo {
    pub(crate) ps5: bool,
    pub(crate) host: CString,
    pub(crate) regist_key: [u8; 16],
    pub(crate) morning: [u8; 16],
    pub(crate) video_profile: VideoProfile,
    pub(crate) video_profile_auto_downgrade: bool,
    pub(crate) enable_keyboard: bool,
    pub(crate) enable_dualsense: bool,
    pub(crate) psn_account_id: [u8; 8],
    pub(crate) packet_loss_max: f64,
    pub(crate) enable_idr_on_fec_failure: bool,
}

/// Builder for [`ConnectInfo`].
#[derive(Default)]
pub struct ConnectInfoBuilder {
    ps5: bool,
    host: String,
    regist_key: [u8; 16],
    morning: [u8; 16],
    video_profile: Option<VideoProfile>,
    video_profile_auto_downgrade: bool,
    enable_keyboard: bool,
    enable_dualsense: bool,
    psn_account_id: [u8; 8],
    packet_loss_max: f64,
    enable_idr_on_fec_failure: bool,
}

impl ConnectInfoBuilder {
    pub fn host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }
    pub fn ps5(mut self, ps5: bool) -> Self {
        self.ps5 = ps5;
        self
    }
    pub fn regist_key(mut self, key: [u8; 16]) -> Self {
        self.regist_key = key;
        self
    }
    pub fn morning(mut self, morning: [u8; 16]) -> Self {
        self.morning = morning;
        self
    }
    pub fn video_profile(mut self, profile: VideoProfile) -> Self {
        self.video_profile = Some(profile);
        self
    }
    pub fn video_profile_auto_downgrade(mut self, v: bool) -> Self {
        self.video_profile_auto_downgrade = v;
        self
    }
    pub fn enable_keyboard(mut self, v: bool) -> Self {
        self.enable_keyboard = v;
        self
    }
    pub fn enable_dualsense(mut self, v: bool) -> Self {
        self.enable_dualsense = v;
        self
    }
    pub fn psn_account_id(mut self, id: [u8; 8]) -> Self {
        self.psn_account_id = id;
        self
    }
    pub fn packet_loss_max(mut self, v: f64) -> Self {
        self.packet_loss_max = v;
        self
    }
    pub fn enable_idr_on_fec_failure(mut self, v: bool) -> Self {
        self.enable_idr_on_fec_failure = v;
        self
    }

    /// Finalise the builder.
    ///
    /// Returns `Err` only if the host string contains an interior null byte.
    pub fn build(self) -> crate::error::Result<ConnectInfo> {
        let host = CString::new(self.host).map_err(|_| crate::error::Error::InvalidData)?;
        let video_profile = self.video_profile.unwrap_or_else(|| {
            VideoProfile::preset(VideoResolutionPreset::P720, VideoFpsPreset::Fps60)
        });
        Ok(ConnectInfo {
            ps5: self.ps5,
            host,
            regist_key: self.regist_key,
            morning: self.morning,
            video_profile,
            video_profile_auto_downgrade: self.video_profile_auto_downgrade,
            enable_keyboard: self.enable_keyboard,
            enable_dualsense: self.enable_dualsense,
            psn_account_id: self.psn_account_id,
            packet_loss_max: self.packet_loss_max,
            enable_idr_on_fec_failure: self.enable_idr_on_fec_failure,
        })
    }
}

impl ConnectInfo {
    /// Start building a [`ConnectInfo`].
    pub fn builder() -> ConnectInfoBuilder {
        ConnectInfoBuilder::default()
    }
}

// ── Callback data structs ──────────────────────────────────────────────────────

struct EventCbData {
    callback: Box<dyn Fn(Event) + Send + 'static>,
}

struct VideoCbData {
    callback: Box<dyn Fn(&[u8], i32, bool) -> bool + Send + 'static>,
}

struct AudioSinkData {
    sink: Box<dyn AudioSink>,
}

// ── Trampolines ───────────────────────────────────────────────────────────────

unsafe extern "C" fn event_callback_trampoline(
    event: *mut sys::chiaki_event_t,
    user: *mut c_void,
) { unsafe {
    // SAFETY: user is a live EventCbData for the session's lifetime.
    let data = &*(user as *const EventCbData);
    // SAFETY: event is valid for this call duration (C guarantees it).
    let rust_event = convert_event(event as *const _);
    (data.callback)(rust_event);
}}

unsafe extern "C" fn video_sample_callback_trampoline(
    buf: *mut u8,
    buf_size: usize,
    frames_lost: i32,
    frame_recovered: bool,
    user: *mut c_void,
) -> bool { unsafe {
    // SAFETY: user is a live VideoCbData.
    let data = &*(user as *const VideoCbData);
    // SAFETY: buf is valid for buf_size bytes for this call.
    let slice = std::slice::from_raw_parts(buf, buf_size);
    (data.callback)(slice, frames_lost, frame_recovered)
}}

unsafe extern "C" fn audio_header_trampoline(
    header: *mut sys::chiaki_audio_header_t,
    user: *mut c_void,
) { unsafe {
    // SAFETY: user is a live AudioSinkData.
    let data = &mut *(user as *mut AudioSinkData);
    let h = &*header;
    data.sink.on_header(AudioHeader {
        channels: h.channels,
        bits: h.bits,
        rate: h.rate,
        frame_size: h.frame_size,
    });
}}

unsafe extern "C" fn audio_frame_trampoline(
    buf: *mut u8,
    buf_size: usize,
    user: *mut c_void,
) { unsafe {
    // SAFETY: user is a live AudioSinkData.
    let data = &mut *(user as *mut AudioSinkData);
    // SAFETY: buf is valid for buf_size bytes for this call.
    let slice = std::slice::from_raw_parts(buf, buf_size);
    data.sink.on_frame(slice);
}}

// ── Session ────────────────────────────────────────────────────────────────────

struct SessionInner {
    /// The C session struct — must not move after `chiaki_session_init`.
    raw: sys::chiaki_session_t,
    /// Shared logger whose stable heap address is stored inside `raw`.
    _log: Arc<Log>,
    /// Raw pointers to callback data — valid until after `chiaki_session_fini`.
    event_cb_data: *mut EventCbData,
    video_cb_data: *mut VideoCbData,
    audio_sink_data: *mut AudioSinkData,
    haptics_sink_data: *mut AudioSinkData,
}

/// RAII streaming session with a PlayStation console.
///
/// # Lifecycle
///
/// 1. Build a [`ConnectInfo`] with [`ConnectInfo::builder`].
/// 2. Create a `Session` with [`Session::new`].
/// 3. Register callbacks with `set_event_callback`, `set_video_callback`,
///    `set_audio_sink` *before* calling `start`.
/// 4. Call [`Session::start`] to begin streaming.
/// 5. Send controller state via [`Session::set_controller_state`] in a loop.
/// 6. Call [`Session::stop`] to request shutdown.
/// 7. Call [`Session::join`] to wait for the C thread to exit cleanly.
///
/// Dropping a `Session` without calling `join` will implicitly call `stop`
/// and then `join`, which may block briefly.
pub struct Session {
    inner: std::ptr::NonNull<SessionInner>,
}

impl Session {
    /// Create a new session from the given connection parameters.
    ///
    /// This calls `chiaki_session_init` but does **not** start the stream.
    /// Set callbacks and then call [`Session::start`].
    pub fn new(connect_info: ConnectInfo, log: Arc<Log>) -> Result<Self> {
        // Assemble the C connect_info.  host pointer is valid because
        // connect_info.host (CString) lives on the stack until after init.
        let regist_key: [std::os::raw::c_char; 16] = unsafe {
            std::mem::transmute(connect_info.regist_key)
        };
        let c_info = sys::chiaki_connect_info_t {
            ps5: connect_info.ps5,
            host: connect_info.host.as_ptr(),
            regist_key,
            morning: connect_info.morning,
            video_profile: connect_info.video_profile.to_raw(),
            video_profile_auto_downgrade: connect_info.video_profile_auto_downgrade,
            enable_keyboard: connect_info.enable_keyboard,
            enable_dualsense: connect_info.enable_dualsense,
            audio_video_disabled: sys::ChiakiDisableAudioVideo_CHIAKI_NONE_DISABLED,
            auto_regist: false,
            holepunch_session: ptr::null_mut(),
            rudp_sock: ptr::null_mut(),
            psn_account_id: connect_info.psn_account_id,
            packet_loss_max: connect_info.packet_loss_max,
            enable_idr_on_fec_failure: connect_info.enable_idr_on_fec_failure,
        };

        // Heap-allocate SessionInner so `raw` has a stable address.
        let inner = Box::into_raw(Box::new(SessionInner {
            raw: unsafe { std::mem::zeroed::<sys::chiaki_session_t>() },
            _log: log.clone(),
            event_cb_data: ptr::null_mut(),
            video_cb_data: ptr::null_mut(),
            audio_sink_data: ptr::null_mut(),
            haptics_sink_data: ptr::null_mut(),
        }));

        // SAFETY: inner was just allocated; c_info and log are valid.
        let err = unsafe {
            sys::chiaki_session_init(&mut (*inner).raw, &c_info as *const _ as *mut _, log.raw_ptr())
        };

        // connect_info.host (CString) and c_info are dropped after this point —
        // chiaki_session_init copies the host into session->connect_info.hostname.
        let _ = c_info;
        drop(connect_info);

        if err != sys::ChiakiErrorCode_CHIAKI_ERR_SUCCESS {
            // Init failed; free the allocation before returning the error.
            // SAFETY: inner was just allocated and init failed so no cleanup needed.
            unsafe { drop(Box::from_raw(inner)) };
            return Err(crate::error::Error::from(err));
        }

        Ok(Session {
            inner: unsafe { std::ptr::NonNull::new_unchecked(inner) },
        })
    }

    /// Register a callback to receive [`Event`]s from the session thread.
    ///
    /// Must be called before [`Session::start`].
    pub fn set_event_callback(&mut self, f: impl Fn(Event) + Send + 'static) {
        let inner = unsafe { self.inner.as_mut() };

        // Drop any previous callback data first.
        if !inner.event_cb_data.is_null() {
            unsafe { drop(Box::from_raw(inner.event_cb_data)) };
        }

        let data = Box::new(EventCbData {
            callback: Box::new(f),
        });
        inner.event_cb_data = Box::into_raw(data);

        // Set directly — chiaki_session_set_event_cb is static inline.
        inner.raw.event_cb = Some(event_callback_trampoline);
        inner.raw.event_cb_user = inner.event_cb_data as *mut c_void;
    }

    /// Register a callback for raw H.264/H.265 video frames.
    ///
    /// The closure receives `(frame_bytes, frames_lost, frame_recovered)` and
    /// should return `true` on success, or `false` to request a keyframe (IDR).
    ///
    /// Must be called before [`Session::start`].
    pub fn set_video_callback(
        &mut self,
        f: impl Fn(&[u8], i32, bool) -> bool + Send + 'static,
    ) {
        let inner = unsafe { self.inner.as_mut() };

        if !inner.video_cb_data.is_null() {
            unsafe { drop(Box::from_raw(inner.video_cb_data)) };
        }

        let data = Box::new(VideoCbData {
            callback: Box::new(f),
        });
        inner.video_cb_data = Box::into_raw(data);

        // Set directly — chiaki_session_set_video_sample_cb is static inline.
        inner.raw.video_sample_cb = Some(video_sample_callback_trampoline);
        inner.raw.video_sample_cb_user = inner.video_cb_data as *mut c_void;
    }

    /// Set an audio sink to receive Opus-encoded audio frames.
    ///
    /// Must be called before [`Session::start`].
    pub fn set_audio_sink(&mut self, sink: impl AudioSink) {
        let inner = unsafe { self.inner.as_mut() };

        if !inner.audio_sink_data.is_null() {
            unsafe { drop(Box::from_raw(inner.audio_sink_data)) };
        }

        let data = Box::new(AudioSinkData { sink: Box::new(sink) });
        inner.audio_sink_data = Box::into_raw(data);

        // Set directly — chiaki_session_set_audio_sink is static inline.
        inner.raw.audio_sink = sys::chiaki_audio_sink_t {
            user: inner.audio_sink_data as *mut c_void,
            header_cb: Some(audio_header_trampoline),
            frame_cb: Some(audio_frame_trampoline),
        };
    }

    /// Set a haptics audio sink (DualSense haptic feedback audio).
    ///
    /// Must be called before [`Session::start`].
    pub fn set_haptics_sink(&mut self, sink: impl AudioSink) {
        let inner = unsafe { self.inner.as_mut() };

        if !inner.haptics_sink_data.is_null() {
            unsafe { drop(Box::from_raw(inner.haptics_sink_data)) };
        }

        let data = Box::new(AudioSinkData { sink: Box::new(sink) });
        inner.haptics_sink_data = Box::into_raw(data);

        // Set directly — chiaki_session_set_haptics_sink is static inline.
        inner.raw.haptics_sink = sys::chiaki_audio_sink_t {
            user: inner.haptics_sink_data as *mut c_void,
            header_cb: Some(audio_header_trampoline),
            frame_cb: Some(audio_frame_trampoline),
        };
    }

    // ── Session control ─────────────────────────────────────────────────────

    /// Start the streaming session (spawns the C session thread).
    pub fn start(&mut self) -> Result<()> {
        ffi_result(unsafe { sys::chiaki_session_start(&mut self.inner.as_mut().raw) })
    }

    /// Signal the session to stop.
    ///
    /// Non-blocking.  Call [`Session::join`] afterwards to wait for the C
    /// thread to exit.  Safe to call from a different thread than the one
    /// that owns the `Session` only when behind a `Mutex`.
    pub fn stop(&self) -> Result<()> {
        ffi_result(unsafe {
            sys::chiaki_session_stop(&mut (*self.inner.as_ptr()).raw)
        })
    }

    /// Wait for the session thread to exit and consume `self`.
    ///
    /// Call [`Session::stop`] first to avoid blocking indefinitely.
    pub fn join(mut self) -> Result<()> {
        let inner = unsafe { self.inner.as_mut() };
        let err = unsafe { sys::chiaki_session_join(&mut inner.raw) };
        // Prevent Drop from calling join again.
        unsafe { Self::cleanup_inner(self.inner) };
        // Forget self so Drop doesn't run.
        std::mem::forget(self);
        ffi_result(err)
    }

    // ── Controller / input ──────────────────────────────────────────────────

    /// Send a controller state snapshot to the console.
    ///
    /// Typically called at 60 Hz from the game loop.
    pub fn set_controller_state(&self, state: &ControllerState) -> Result<()> {
        ffi_result(unsafe {
            sys::chiaki_session_set_controller_state(
                &mut (*self.inner.as_ptr()).raw,
                &state.0 as *const _ as *mut _,
            )
        })
    }

    /// Set the login PIN displayed during PIN-based remote play.
    pub fn set_login_pin(&self, pin: &[u8]) -> Result<()> {
        ffi_result(unsafe {
            sys::chiaki_session_set_login_pin(
                &mut (*self.inner.as_ptr()).raw,
                pin.as_ptr(),
                pin.len(),
            )
        })
    }

    // ── Keyboard ────────────────────────────────────────────────────────────

    /// Set on-screen keyboard text.
    pub fn keyboard_set_text(&self, text: &str) -> Result<()> {
        let c = CString::new(text).unwrap_or_default();
        ffi_result(unsafe {
            sys::chiaki_session_keyboard_set_text(
                &mut (*self.inner.as_ptr()).raw,
                c.as_ptr(),
            )
        })
    }

    /// Confirm the on-screen keyboard input.
    pub fn keyboard_accept(&self) -> Result<()> {
        ffi_result(unsafe {
            sys::chiaki_session_keyboard_accept(
                &mut (*self.inner.as_ptr()).raw,
            )
        })
    }

    /// Dismiss the on-screen keyboard without confirming.
    pub fn keyboard_reject(&self) -> Result<()> {
        ffi_result(unsafe {
            sys::chiaki_session_keyboard_reject(
                &mut (*self.inner.as_ptr()).raw,
            )
        })
    }

    // ── System control ──────────────────────────────────────────────────────

    /// Send a "Go to bed" (put console in standby) command.
    pub fn goto_bed(&self) -> Result<()> {
        ffi_result(unsafe {
            sys::chiaki_session_goto_bed(
                &mut (*self.inner.as_ptr()).raw,
            )
        })
    }

    /// Send a "Go to home screen" command.
    pub fn go_home(&self) -> Result<()> {
        ffi_result(unsafe {
            sys::chiaki_session_go_home(
                &mut (*self.inner.as_ptr()).raw,
            )
        })
    }

    /// Toggle microphone mute state.
    pub fn toggle_microphone(&self, muted: bool) -> Result<()> {
        ffi_result(unsafe {
            sys::chiaki_session_toggle_microphone(
                &mut (*self.inner.as_ptr()).raw,
                muted,
            )
        })
    }

    /// Signal the console that a microphone channel is connected.
    ///
    /// Call this once before pushing microphone audio data.
    pub fn connect_microphone(&self) -> Result<()> {
        ffi_result(unsafe {
            sys::chiaki_session_connect_microphone(
                &mut (*self.inner.as_ptr()).raw,
            )
        })
    }

    /// Read current network statistics from the active stream connection.
    ///
    /// The values are updated by the C library's congestion-control thread and
    /// are eventually consistent.
    pub fn stream_stats(&self) -> StreamStats {
        let inner = unsafe { &*self.inner.as_ptr() };
        StreamStats {
            measured_bitrate: inner.raw.stream_connection.measured_bitrate,
            packet_loss: inner.raw.stream_connection.congestion_control.packet_loss,
        }
    }

    /// Get a raw pointer to the underlying `chiaki_session_t`.
    ///
    /// # Safety
    ///
    /// The returned pointer is valid for the lifetime of this `Session`.
    /// The caller must not call `chiaki_session_fini` or otherwise invalidate
    /// the session through this pointer.
    pub(crate) fn raw_session_ptr(&self) -> *mut sys::chiaki_session_t {
        &mut unsafe { &mut *self.inner.as_ptr() }.raw
    }

    // ── Internal cleanup ─────────────────────────────────────────────────────

    /// Full teardown: stop → join (ignoring errors) → fini → free callbacks.
    ///
    /// # Safety
    /// `ptr` must be a valid, live `SessionInner`.  Caller must not use `ptr`
    /// after this function returns.
    unsafe fn cleanup_inner(ptr: std::ptr::NonNull<SessionInner>) {
        let inner = ptr.as_ptr();
        unsafe {
            // 1. Stop (no-op / error if not running — safe to ignore).
            let _ = sys::chiaki_session_stop(&mut (*inner).raw);
            // 2. Join the C thread.
            let _ = sys::chiaki_session_join(&mut (*inner).raw);
            // 3. C-level resource cleanup.
            sys::chiaki_session_fini(&mut (*inner).raw);
            // 4. Free callback boxes — safe because the C thread is now dead.
            if !(*inner).event_cb_data.is_null() {
                drop(Box::from_raw((*inner).event_cb_data));
            }
            if !(*inner).video_cb_data.is_null() {
                drop(Box::from_raw((*inner).video_cb_data));
            }
            if !(*inner).audio_sink_data.is_null() {
                drop(Box::from_raw((*inner).audio_sink_data));
            }
            if !(*inner).haptics_sink_data.is_null() {
                drop(Box::from_raw((*inner).haptics_sink_data));
            }
            // 5. Free the SessionInner box itself.
            drop(Box::from_raw(inner));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: inner is always a valid, live SessionInner until Drop.
        unsafe { Self::cleanup_inner(self.inner) };
    }
}

// SAFETY: ChiakiSession uses internal mutexes for thread-safe access from
// its own internal thread.  Session holds all referenced memory exclusively
// and can therefore safely be sent to another thread.
unsafe impl Send for Session {}

#[cfg(test)]
mod tests {
    use super::*;

    fn init() {
        crate::init().unwrap();
    }

    // ── VideoResolutionPreset / VideoFpsPreset ─────────────────────────────────

    #[test]
    fn video_profile_preset_360p_30fps() {
        init();
        let p = VideoProfile::preset(VideoResolutionPreset::P360, VideoFpsPreset::Fps30);
        assert_eq!(p.width, 640);
        assert_eq!(p.height, 360);
        assert_eq!(p.max_fps, 30);
    }

    #[test]
    fn video_profile_preset_540p_30fps() {
        init();
        let p = VideoProfile::preset(VideoResolutionPreset::P540, VideoFpsPreset::Fps30);
        assert_eq!(p.width, 960);
        assert_eq!(p.height, 540);
        assert_eq!(p.max_fps, 30);
    }

    #[test]
    fn video_profile_preset_720p_60fps() {
        init();
        let p = VideoProfile::preset(VideoResolutionPreset::P720, VideoFpsPreset::Fps60);
        assert_eq!(p.width, 1280);
        assert_eq!(p.height, 720);
        assert_eq!(p.max_fps, 60);
    }

    #[test]
    fn video_profile_preset_1080p_60fps() {
        init();
        let p = VideoProfile::preset(VideoResolutionPreset::P1080, VideoFpsPreset::Fps60);
        assert_eq!(p.width, 1920);
        assert_eq!(p.height, 1080);
        assert_eq!(p.max_fps, 60);
    }

    #[test]
    fn video_profile_preset_fps30_vs_60() {
        init();
        let p30 = VideoProfile::preset(VideoResolutionPreset::P720, VideoFpsPreset::Fps30);
        let p60 = VideoProfile::preset(VideoResolutionPreset::P720, VideoFpsPreset::Fps60);
        assert_eq!(p30.max_fps, 30);
        assert_eq!(p60.max_fps, 60);
        // Same resolution regardless of FPS
        assert_eq!(p30.width, p60.width);
        assert_eq!(p30.height, p60.height);
    }

    #[test]
    fn video_profile_preset_has_nonzero_bitrate() {
        init();
        let p = VideoProfile::preset(VideoResolutionPreset::P720, VideoFpsPreset::Fps60);
        assert!(p.bitrate > 0, "bitrate should be non-zero");
    }

    #[test]
    fn video_profile_to_raw_roundtrip_preserves_fields() {
        init();
        let p = VideoProfile::preset(VideoResolutionPreset::P1080, VideoFpsPreset::Fps30);
        let raw = p.to_raw();
        assert_eq!(raw.width, p.width);
        assert_eq!(raw.height, p.height);
        assert_eq!(raw.max_fps, p.max_fps);
        assert_eq!(raw.bitrate, p.bitrate);
    }

    // ── ConnectInfoBuilder ────────────────────────────────────────────────────

    #[test]
    fn connect_info_builder_builds_with_valid_host() {
        init();
        let result = ConnectInfo::builder().host("192.168.1.100").build();
        assert!(result.is_ok());
    }

    #[test]
    fn connect_info_builder_fails_on_null_byte_in_host() {
        init();
        // CString::new fails on interior null bytes.
        let result = ConnectInfo::builder().host("192.168.\x00.1").build();
        assert_eq!(result.err().unwrap(), crate::error::Error::InvalidData);
    }

    #[test]
    fn connect_info_builder_default_video_profile_is_720p_60fps() {
        init();
        let info = ConnectInfo::builder().host("10.0.0.1").build().unwrap();
        assert_eq!(info.video_profile.width, 1280);
        assert_eq!(info.video_profile.height, 720);
        assert_eq!(info.video_profile.max_fps, 60);
    }

    #[test]
    fn connect_info_builder_stores_ps5_flag() {
        init();
        let info = ConnectInfo::builder()
            .host("10.0.0.1")
            .ps5(true)
            .build()
            .unwrap();
        assert!(info.ps5);
    }

    #[test]
    fn connect_info_builder_ps5_defaults_to_false() {
        init();
        let info = ConnectInfo::builder().host("10.0.0.1").build().unwrap();
        assert!(!info.ps5);
    }

    #[test]
    fn connect_info_builder_stores_regist_key() {
        init();
        let key = [
            0x01u8, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
            0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
        ];
        let info = ConnectInfo::builder()
            .host("10.0.0.1")
            .regist_key(key)
            .build()
            .unwrap();
        assert_eq!(info.regist_key, key);
    }

    #[test]
    fn connect_info_builder_stores_morning_key() {
        init();
        let morning = [0xAAu8; 16];
        let info = ConnectInfo::builder()
            .host("10.0.0.1")
            .morning(morning)
            .build()
            .unwrap();
        assert_eq!(info.morning, morning);
    }

    #[test]
    fn connect_info_builder_stores_psn_account_id() {
        init();
        let id = [0xBBu8; 8];
        let info = ConnectInfo::builder()
            .host("10.0.0.1")
            .psn_account_id(id)
            .build()
            .unwrap();
        assert_eq!(info.psn_account_id, id);
    }

    #[test]
    fn connect_info_builder_stores_boolean_flags() {
        init();
        let info = ConnectInfo::builder()
            .host("10.0.0.1")
            .enable_keyboard(true)
            .enable_dualsense(true)
            .video_profile_auto_downgrade(true)
            .enable_idr_on_fec_failure(true)
            .build()
            .unwrap();
        assert!(info.enable_keyboard);
        assert!(info.enable_dualsense);
        assert!(info.video_profile_auto_downgrade);
        assert!(info.enable_idr_on_fec_failure);
    }

    #[test]
    fn connect_info_builder_boolean_flags_default_to_false() {
        init();
        let info = ConnectInfo::builder().host("10.0.0.1").build().unwrap();
        assert!(!info.enable_keyboard);
        assert!(!info.enable_dualsense);
        assert!(!info.video_profile_auto_downgrade);
        assert!(!info.enable_idr_on_fec_failure);
    }

    #[test]
    fn connect_info_builder_stores_packet_loss_max() {
        init();
        let info = ConnectInfo::builder()
            .host("10.0.0.1")
            .packet_loss_max(0.05)
            .build()
            .unwrap();
        assert!((info.packet_loss_max - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn connect_info_builder_custom_video_profile_overrides_default() {
        init();
        let custom = VideoProfile::preset(VideoResolutionPreset::P1080, VideoFpsPreset::Fps30);
        let info = ConnectInfo::builder()
            .host("10.0.0.1")
            .video_profile(custom)
            .build()
            .unwrap();
        assert_eq!(info.video_profile.width, 1920);
        assert_eq!(info.video_profile.height, 1080);
        assert_eq!(info.video_profile.max_fps, 30);
    }

    #[test]
    fn connect_info_builder_is_default_constructible() {
        init();
        // ConnectInfoBuilder derives Default; an empty builder with host should be buildable.
        let builder = ConnectInfoBuilder::default();
        // Empty host becomes an empty CString — valid because it has no null bytes.
        let result = builder.build();
        assert!(result.is_ok());
    }
}
