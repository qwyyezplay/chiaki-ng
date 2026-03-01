// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! High-level stream orchestrator tying [`Session`], [`ControllerManager`],
//! and DualSense haptics together.
//!
//! [`StreamController`] encapsulates:
//!
//! 1. Session creation, callback wiring, and lifecycle management.
//! 2. Automatic event -> feedback-command mapping and application to the physical controller.
//! 3. DualSense haptics sink creation (direct audio or motor-rumble fallback).
//! 4. Controller hotplug management and primary-controller election.
//! 5. Controller state forwarding at ~60 Hz via [`StreamController::tick`].
//!
//! # Example
//!
//! ```no_run,ignore
//! use chiaki::prelude::*;
//! use chiaki::stream::{StreamController, StreamControllerConfig};
//!
//! let sdl = sdl2::init().unwrap();
//! let mut event_pump = sdl.event_pump().unwrap();
//! let _ = sdl.audio(); // ensure SDL audio subsystem is initialised
//!
//! let config = StreamControllerConfig {
//!     connect_info,
//!     enable_dualsense: true,
//!     audio_sink: None,
//!     video_callback: None,
//!     event_callback: None,
//! };
//! let mut ctrl = StreamController::new(config, log, &sdl).unwrap();
//! ctrl.start().unwrap();
//!
//! loop {
//!     if let Some(notif) = ctrl.tick(&mut event_pump).unwrap() {
//!         match notif {
//!             StreamNotification::Quit { .. } => break,
//!             _ => {}
//!         }
//!     }
//!     std::thread::sleep(std::time::Duration::from_millis(16));
//! }
//! ctrl.stop().unwrap();
//! ```

use std::sync::atomic::AtomicU64;
use std::sync::{mpsc, Arc, Mutex};

use sdl2::event::Event as SdlEvent;

use crate::controllermanager::{ControllerEvent, ControllerManager};
use crate::error::Result;
use crate::feedback::{self, FeedbackCmd};
use crate::haptics::{self, HapticsSink};
use crate::log::Log;
use crate::session::{AudioSink, ConnectInfo, Event, Session};
use crate::stats::StreamStats;
use crate::types::QuitReason;

// ── Configuration ────────────────────────────────────────────────────────────

/// Configuration for a [`StreamController`].
pub struct StreamControllerConfig {
    /// Connection parameters for the streaming session.
    pub connect_info: ConnectInfo,
    /// Whether to enable DualSense features (haptics, adaptive triggers, LED).
    pub enable_dualsense: bool,
    /// Optional audio sink for decoded Opus audio from the console.
    pub audio_sink: Option<Box<dyn AudioSink>>,
    /// Optional video callback: `(frame_bytes, frames_lost, frame_recovered) -> continue?`
    pub video_callback: Option<Box<dyn Fn(&[u8], i32, bool) -> bool + Send + 'static>>,
    /// Optional callback for session lifecycle events.
    ///
    /// Events that produce [`FeedbackCmd`]s (Rumble, TriggerEffects, etc.) are
    /// handled internally by the `StreamController` — your callback can still
    /// observe them for logging or other purposes.
    pub event_callback: Option<Box<dyn Fn(&Event) + Send + Sync + 'static>>,
}

// ── Notification ─────────────────────────────────────────────────────────────

/// Notification produced by [`StreamController::tick`].
#[derive(Debug)]
pub enum StreamNotification {
    /// The session has connected; controller input forwarding is now active.
    Connected,
    /// The session has ended.
    Quit {
        reason: QuitReason,
        reason_str: Option<String>,
    },
    /// The primary (active) controller changed.
    ActiveControllerChanged(Option<u32>),
}

// ── Internal signal types ────────────────────────────────────────────────────

struct QuitSignal {
    reason: QuitReason,
    reason_str: Option<String>,
}

// ── StreamController ─────────────────────────────────────────────────────────

/// High-level orchestrator that ties [`Session`] + [`ControllerManager`] +
/// haptics + feedback together.
pub struct StreamController {
    session: Session,
    manager: ControllerManager,
    feedback_cmds: Arc<Mutex<Vec<FeedbackCmd>>>,
    active_controller_id: Option<u32>,
    haptics_device_id: u32,
    haptics_frame_count: Arc<AtomicU64>,
    connected: bool,
    quit_rx: mpsc::Receiver<QuitSignal>,
    connected_rx: mpsc::Receiver<()>,
}

impl StreamController {
    /// Create and wire up all components but do **not** start streaming yet.
    ///
    /// Call [`start`](Self::start) after construction to begin the session.
    pub fn new(
        config: StreamControllerConfig,
        log: Arc<Log>,
        sdl_ctx: &sdl2::Sdl,
    ) -> Result<Self> {
        // 1. Controller manager.
        let mut manager =
            ControllerManager::new(sdl_ctx).map_err(|_| crate::error::Error::Unknown)?;
        manager.set_dualsense_intensity(0x00); // full haptic intensity

        // 2. Open initially available controllers; elect primary.
        let mut active_id: Option<u32> = None;
        for id in manager.available_controllers() {
            if manager.open_controller(id) && active_id.is_none() {
                active_id = Some(id);
            }
        }

        // 3. DualSense haptics audio device.
        let haptics_device_id = if config.enable_dualsense {
            haptics::open_dualsense_haptics_device()
        } else {
            0
        };

        // 4. Shared feedback-command queue.
        let feedback_cmds: Arc<Mutex<Vec<FeedbackCmd>>> = Arc::new(Mutex::new(Vec::new()));

        // 5. Haptics sink.
        let haptics_frame_count = Arc::new(AtomicU64::new(0));
        let haptics_sink = HapticsSink::builder()
            .feedback_cmds(Arc::clone(&feedback_cmds))
            .frame_counter(Arc::clone(&haptics_frame_count))
            .haptics_device_id(haptics_device_id)
            .build();

        // 6. Create session.
        let mut session = Session::new(config.connect_info, Arc::clone(&log))?;

        // 7. Channels for quit / connected signals.
        let (quit_tx, quit_rx) = mpsc::channel::<QuitSignal>();
        let (connected_tx, connected_rx) = mpsc::channel::<()>();

        // 8. Event callback — dispatches feedback + forwards to user callback.
        let fb_cmds_for_cb = Arc::clone(&feedback_cmds);
        let user_event_cb: Option<Arc<dyn Fn(&Event) + Send + Sync + 'static>> =
            config.event_callback.map(Arc::from);

        session.set_event_callback(move |event| {
            // a) Forward feedback-producing events to the feedback queue.
            if let Some(cmd) = feedback::event_to_feedback(&event) {
                if let Ok(mut cmds) = fb_cmds_for_cb.lock() {
                    cmds.push(cmd);
                }
            }

            // b) Internal signals.
            match &event {
                Event::Connected => {
                    let _ = connected_tx.send(());
                }
                Event::Quit {
                    reason,
                    reason_str,
                } => {
                    let _ = quit_tx.send(QuitSignal {
                        reason: *reason,
                        reason_str: reason_str.clone(),
                    });
                }
                _ => {}
            }

            // c) User callback.
            if let Some(ref cb) = user_event_cb {
                cb(&event);
            }
        });

        // 9. Video callback.
        if let Some(video_cb) = config.video_callback {
            session.set_video_callback(move |frame, lost, recovered| {
                video_cb(frame, lost, recovered)
            });
        }

        // 10. Audio sink.
        if let Some(audio_sink) = config.audio_sink {
            session.set_audio_sink(audio_sink);
        }

        // 11. Haptics sink.
        session.set_haptics_sink(haptics_sink);

        Ok(Self {
            session,
            manager,
            feedback_cmds,
            active_controller_id: active_id,
            haptics_device_id,
            haptics_frame_count,
            connected: false,
            quit_rx,
            connected_rx,
        })
    }

    /// Start the streaming session (spawns the C session thread).
    pub fn start(&mut self) -> Result<()> {
        self.session.start()
    }

    /// Process SDL events and perform one tick of the control loop.
    ///
    /// Should be called at ~60 Hz from your main loop.  It:
    ///
    /// 1. Checks for quit / connected signals from the session thread.
    /// 2. Processes all SDL events through the [`ControllerManager`].
    /// 3. Handles controller hotplug (open / close / elect primary).
    /// 4. Drains and applies pending feedback commands to the physical controller.
    /// 5. Forwards the latest controller state to the session.
    ///
    /// Returns `Ok(None)` normally, `Ok(Some(notification))` when a notable
    /// event occurs, or `Err` on fatal error.
    pub fn tick(
        &mut self,
        event_pump: &mut sdl2::EventPump,
    ) -> Result<Option<StreamNotification>> {
        // 1. Check quit signal.
        match self.quit_rx.try_recv() {
            Ok(sig) => {
                return Ok(Some(StreamNotification::Quit {
                    reason: sig.reason,
                    reason_str: sig.reason_str,
                }));
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                return Ok(Some(StreamNotification::Quit {
                    reason: QuitReason::Stopped,
                    reason_str: Some("Event channel closed".into()),
                }));
            }
        }

        // 2. Check connected signal.
        let mut notification: Option<StreamNotification> = None;
        if !self.connected && self.connected_rx.try_recv().is_ok() {
            self.connected = true;
            if let Some(id) = self.active_controller_id {
                self.manager.change_led_color(id, 0, 200, 0);
            }
            notification = Some(StreamNotification::Connected);
        }

        // 3. Poll SDL events.
        for sdl_event in event_pump.poll_iter() {
            if let SdlEvent::Quit { .. } = sdl_event {
                return Ok(Some(StreamNotification::Quit {
                    reason: QuitReason::Stopped,
                    reason_str: Some("SDL quit".into()),
                }));
            }

            for ctrl_event in self.manager.process_event(&sdl_event) {
                match ctrl_event {
                    ControllerEvent::AvailableControllersUpdated => {
                        self.handle_hotplug();
                        notification =
                            Some(StreamNotification::ActiveControllerChanged(
                                self.active_controller_id,
                            ));
                    }
                    // StateChanged, MicButtonPush, NewButtonMapping — no
                    // special handling needed at this level; the
                    // ControllerManager updates its internal state.
                    _ => {}
                }
            }
        }

        // 4. Drain and apply feedback commands.
        feedback::drain_and_apply(
            &self.feedback_cmds,
            &mut self.manager,
            self.active_controller_id,
        );

        // 5. Forward controller state to session.
        if let Some(id) = self.active_controller_id {
            if let Some(state) = self.manager.controller_state(id) {
                self.session.set_controller_state(state)?;
            }
        }

        Ok(notification)
    }

    // ── Hotplug logic ────────────────────────────────────────────────────────

    fn handle_hotplug(&mut self) {
        // Open any newly connected controllers.
        for id in self.manager.available_controllers() {
            if !self.manager.open_controllers().contains(&id) {
                if self.manager.open_controller(id) {
                    self.manager.change_led_color(id, 0, 0, 255);
                    if self.active_controller_id.is_none() {
                        self.active_controller_id = Some(id);
                    }
                }
            }
        }

        // Detect disconnected controllers.
        for id in self.manager.open_controllers() {
            if !self.manager.is_controller_connected(id) {
                self.manager.close_controller(id);
                if self.active_controller_id == Some(id) {
                    self.active_controller_id =
                        self.manager.open_controllers().into_iter().next();
                }
            }
        }
    }

    // ── Accessors ────────────────────────────────────────────────────────────

    /// Get the current active (primary) controller's instance ID.
    pub fn active_controller(&self) -> Option<u32> {
        self.active_controller_id
    }

    /// Whether the session has connected.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Get a reference to the underlying [`ControllerManager`].
    pub fn manager(&self) -> &ControllerManager {
        &self.manager
    }

    /// Get a mutable reference to the underlying [`ControllerManager`].
    pub fn manager_mut(&mut self) -> &mut ControllerManager {
        &mut self.manager
    }

    /// Get a reference to the underlying [`Session`].
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Read current network statistics from the active stream connection.
    pub fn stream_stats(&self) -> StreamStats {
        self.session.stream_stats()
    }

    /// Get the haptics frame counter (for statistics).
    pub fn haptics_frame_count(&self) -> &Arc<AtomicU64> {
        &self.haptics_frame_count
    }

    /// Stop the session and clean up.
    ///
    /// This calls `Session::stop` and then `Session::join`, which may block
    /// briefly.
    pub fn stop(self) -> Result<()> {
        let _ = self.session.stop();
        // Drop handles cleanup: session join + haptics device close.
        Ok(())
    }
}

impl Drop for StreamController {
    fn drop(&mut self) {
        if self.haptics_device_id > 0 {
            haptics::close_dualsense_haptics_device(self.haptics_device_id);
        }
    }
}

// SAFETY: StreamController owns all its sub-components exclusively.
// Session and ControllerManager are both Send (Session has internal mutexes,
// ControllerManager must be used on a single thread — the caller is
// responsible for ensuring the SDL event pump is on the same thread).
unsafe impl Send for StreamController {}
