// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! Microphone input: Opus encoding and transmission to the console.
//!
//! Wraps the C `chiaki_opus_encoder_t` to accept raw PCM audio, encode it
//! with Opus, and push the encoded data through the session to the console.
//!
//! # Usage
//!
//! ```no_run,ignore
//! // After session is connected:
//! let mut mic = MicEncoder::new(&session, &log, 2, 480)?;
//! MicEncoder::connect(&session)?;
//! MicEncoder::toggle_mute(&session, false)?;
//!
//! // In your audio capture callback:
//! mic.push_pcm(&pcm_samples);
//! ```

use std::sync::Arc;

use chiaki_sys as sys;

use crate::error::Result;
use crate::log::Log;
use crate::session::Session;

/// Default number of PCM samples per Opus frame (10 ms at 48 kHz).
pub const DEFAULT_FRAME_SIZE: u32 = 480;

/// Default number of audio channels for microphone input.
pub const DEFAULT_MIC_CHANNELS: u8 = 2;

/// Safe wrapper around `ChiakiOpusEncoder`.
///
/// Handles PCM buffering, Opus encoding, and transmission to the PS console
/// via the session's Takion protocol.  The caller is responsible for
/// capturing audio (e.g. via SDL2) and pushing PCM data through [`push_pcm`].
///
/// [`push_pcm`]: MicEncoder::push_pcm
pub struct MicEncoder {
    encoder: Box<sys::chiaki_opus_encoder_t>,
    /// Internal buffer for accumulating PCM samples until a complete frame.
    pcm_buf: Vec<i16>,
    /// Current write position in `pcm_buf` (in samples, not bytes).
    current_sample: usize,
    /// Number of samples in one complete frame (`frame_size * channels`).
    frame_samples: usize,
}

impl MicEncoder {
    /// Initialise the Opus encoder and link it to the given session.
    ///
    /// * `channels` — typically 2 (stereo).
    /// * `frame_size` — number of samples per frame per channel (typically 480
    ///   for 10 ms at 48 kHz).
    ///
    /// Must be called after [`Session::start`] and preferably after receiving
    /// [`Event::Connected`].
    ///
    /// [`Event::Connected`]: crate::session::Event::Connected
    pub fn new(
        session: &Session,
        log: &Arc<Log>,
        channels: u8,
        frame_size: u32,
    ) -> Result<Self> {
        let mut encoder: Box<sys::chiaki_opus_encoder_t> =
            Box::new(unsafe { std::mem::zeroed() });

        unsafe {
            sys::chiaki_opus_encoder_init(&mut *encoder, log.raw_ptr());
        }

        // Configure audio header: rate = frame_size * 100 (= 48000 for 480).
        let rate = frame_size * 100;
        let mut header: sys::chiaki_audio_header_t = unsafe { std::mem::zeroed() };
        unsafe {
            sys::chiaki_audio_header_set(
                &mut header,
                channels,
                16, // 16-bit PCM
                rate,
                frame_size,
            );
        }

        // Link encoder to session (initialises the internal audio sender).
        unsafe {
            sys::chiaki_opus_encoder_header(
                &mut header,
                &mut *encoder,
                session.raw_session_ptr(),
            );
        }

        let frame_samples = (frame_size as usize) * (channels as usize);

        Ok(Self {
            encoder,
            pcm_buf: vec![0i16; frame_samples],
            current_sample: 0,
            frame_samples,
        })
    }

    /// Push raw PCM audio data (interleaved int16 samples) into the encoder.
    ///
    /// Internally buffers samples until a complete frame is available, then
    /// encodes and sends it via the session.  This mirrors the GUI's `ReadMic`
    /// buffering logic.
    ///
    /// You may push any number of samples per call — partial frames are
    /// buffered, and multiple frames are processed if enough data is available.
    pub fn push_pcm(&mut self, pcm_data: &[i16]) {
        let mut offset = 0;
        while offset < pcm_data.len() {
            let remaining = self.frame_samples - self.current_sample;
            let available = pcm_data.len() - offset;
            let to_copy = remaining.min(available);

            self.pcm_buf[self.current_sample..self.current_sample + to_copy]
                .copy_from_slice(&pcm_data[offset..offset + to_copy]);
            self.current_sample += to_copy;
            offset += to_copy;

            if self.current_sample >= self.frame_samples {
                unsafe {
                    sys::chiaki_opus_encoder_frame(
                        self.pcm_buf.as_mut_ptr(),
                        &mut *self.encoder,
                    );
                }
                self.current_sample = 0;
            }
        }
    }

    /// Signal the console that a microphone channel is connected.
    ///
    /// Call this once before starting to push PCM data.
    pub fn connect(session: &Session) -> Result<()> {
        session.connect_microphone()
    }

    /// Toggle the microphone mute state on the session.
    ///
    /// Pass `false` to unmute (start transmitting), `true` to mute.
    pub fn toggle_mute(session: &Session, muted: bool) -> Result<()> {
        session.toggle_microphone(muted)
    }
}

impl Drop for MicEncoder {
    fn drop(&mut self) {
        unsafe {
            sys::chiaki_opus_encoder_fini(&mut *self.encoder);
        }
    }
}

// SAFETY: The opus encoder's internal audio sender uses the session's
// takion mutex for thread-safe sending.  MicEncoder exclusively owns the
// encoder struct and can be moved between threads.
unsafe impl Send for MicEncoder {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_constants() {
        assert_eq!(DEFAULT_FRAME_SIZE, 480);
        assert_eq!(DEFAULT_MIC_CHANNELS, 2);
    }
}
