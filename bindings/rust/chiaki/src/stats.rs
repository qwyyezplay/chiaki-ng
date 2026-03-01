// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! Network statistics from an active streaming session.

/// Snapshot of network quality metrics from the stream connection.
///
/// These values are updated by the C library's congestion-control thread and
/// are eventually consistent (no lock is taken when reading individual `f64`
/// fields on 64-bit platforms).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamStats {
    /// Measured video bitrate in bits per second.
    pub measured_bitrate: f64,
    /// Current packet-loss ratio (0.0 = no loss, 1.0 = total loss).
    pub packet_loss: f64,
}

impl Default for StreamStats {
    fn default() -> Self {
        Self {
            measured_bitrate: 0.0,
            packet_loss: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_stats_default_is_zero() {
        let s = StreamStats::default();
        assert_eq!(s.measured_bitrate, 0.0);
        assert_eq!(s.packet_loss, 0.0);
    }

    #[test]
    fn stream_stats_clone_eq() {
        let s = StreamStats {
            measured_bitrate: 15_000_000.0,
            packet_loss: 0.02,
        };
        assert_eq!(s, s.clone());
    }
}
