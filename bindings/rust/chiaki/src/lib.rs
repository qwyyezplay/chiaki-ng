// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! Safe Rust bindings for the chiaki-ng PlayStation Remote Play library.
//!
//! # Quick start
//!
//! ```no_run
//! use chiaki::prelude::*;
//!
//! // 1. Initialise the C library (idempotent).
//! chiaki::init().unwrap();
//!
//! // 2. Create a logger.
//! let log = Log::new_default(LogLevel::INFO | LogLevel::WARNING | LogLevel::ERROR);
//!
//! // 3. Discover consoles on the LAN.
//! let svc = DiscoveryService::start(Default::default(), log.clone(), |hosts| {
//!     for h in &hosts {
//!         println!("{:?} @ {:?}", h.host_name, h.host_addr);
//!     }
//! }).unwrap();
//!
//! std::thread::sleep(std::time::Duration::from_secs(3));
//! drop(svc);
//! ```

pub mod controller;
#[cfg(feature = "sdl-controller")]
pub mod controllermanager;
pub mod discovery;
pub mod error;
#[cfg(feature = "sdl-controller")]
pub mod feedback;
#[cfg(feature = "sdl-controller")]
pub mod haptics;
pub mod log;
pub mod mic;
pub mod regist;
pub mod session;
pub mod stats;
#[cfg(feature = "sdl-controller")]
pub mod stream;
pub mod types;

pub use error::{Error, Result};
pub use types::{Codec, DualSenseEffectIntensity, QuitReason, Target};

use std::sync::OnceLock;

static INIT: OnceLock<Result<()>> = OnceLock::new();

/// Initialise the chiaki C library.
///
/// This function is idempotent — safe to call multiple times.
/// Should be called once before using any other API.
pub fn init() -> Result<()> {
    INIT.get_or_init(|| error::ffi_result(unsafe { chiaki_sys::chiaki_lib_init() }))
        .clone()
}

/// Convenience re-exports for the most commonly used types.
pub mod prelude {
    pub use crate::controller::{ControllerButtons, ControllerState, Touch};
    #[cfg(feature = "sdl-controller")]
    pub use crate::controllermanager::{ControllerEvent, ControllerInfo, ControllerManager};
    pub use crate::discovery::{
        DiscoveryHost, DiscoveryHostState, DiscoveryService, DiscoveryServiceOptions,
    };
    pub use crate::error::{Error, Result};
    #[cfg(feature = "sdl-controller")]
    pub use crate::feedback::FeedbackCmd;
    #[cfg(feature = "sdl-controller")]
    pub use crate::haptics::HapticsSink;
    pub use crate::log::{Log, LogLevel};
    pub use crate::mic::MicEncoder;
    pub use crate::regist::{RegisteredHost, RegistInfo, RegistResult, Regist};
    pub use crate::session::{
        AudioHeader, AudioSink, ConnectInfo, Event, Session, VideoFpsPreset,
        VideoProfile, VideoResolutionPreset,
    };
    pub use crate::stats::StreamStats;
    #[cfg(feature = "sdl-controller")]
    pub use crate::stream::{StreamController, StreamControllerConfig, StreamNotification};
    pub use crate::types::{Codec, DualSenseEffectIntensity, QuitReason, Target};
}
