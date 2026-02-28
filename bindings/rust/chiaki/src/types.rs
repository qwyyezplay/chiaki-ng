// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! Common enumerations shared across multiple modules.

use chiaki_sys as sys;

/// PlayStation target device family and firmware generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Ps4Unknown,
    Ps4_8,
    Ps4_9,
    Ps4_10,
    Ps5Unknown,
    Ps5_1,
}

impl Target {
    /// Returns `true` if this target is a PS5 console.
    #[inline]
    pub fn is_ps5(self) -> bool {
        matches!(self, Target::Ps5Unknown | Target::Ps5_1)
    }

    /// Returns `true` if the exact firmware version is not known.
    #[inline]
    pub fn is_unknown(self) -> bool {
        matches!(self, Target::Ps4Unknown | Target::Ps5Unknown)
    }

    pub(crate) fn to_raw(self) -> sys::ChiakiTarget {
        match self {
            Target::Ps4Unknown => sys::ChiakiTarget_CHIAKI_TARGET_PS4_UNKNOWN,
            Target::Ps4_8 => sys::ChiakiTarget_CHIAKI_TARGET_PS4_8,
            Target::Ps4_9 => sys::ChiakiTarget_CHIAKI_TARGET_PS4_9,
            Target::Ps4_10 => sys::ChiakiTarget_CHIAKI_TARGET_PS4_10,
            Target::Ps5Unknown => sys::ChiakiTarget_CHIAKI_TARGET_PS5_UNKNOWN,
            Target::Ps5_1 => sys::ChiakiTarget_CHIAKI_TARGET_PS5_1,
        }
    }

    pub(crate) fn from_raw(raw: sys::ChiakiTarget) -> Self {
        match raw {
            sys::ChiakiTarget_CHIAKI_TARGET_PS4_8 => Target::Ps4_8,
            sys::ChiakiTarget_CHIAKI_TARGET_PS4_9 => Target::Ps4_9,
            sys::ChiakiTarget_CHIAKI_TARGET_PS4_10 => Target::Ps4_10,
            sys::ChiakiTarget_CHIAKI_TARGET_PS5_UNKNOWN => Target::Ps5Unknown,
            sys::ChiakiTarget_CHIAKI_TARGET_PS5_1 => Target::Ps5_1,
            _ => Target::Ps4Unknown,
        }
    }
}

/// Video stream codec selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    H264,
    H265,
    H265Hdr,
}

impl Codec {
    /// Returns `true` for H.265 (with or without HDR).
    #[inline]
    pub fn is_h265(self) -> bool {
        matches!(self, Codec::H265 | Codec::H265Hdr)
    }

    /// Returns `true` only for H.265 HDR.
    #[inline]
    pub fn is_hdr(self) -> bool {
        matches!(self, Codec::H265Hdr)
    }

    pub(crate) fn to_raw(self) -> sys::ChiakiCodec {
        match self {
            Codec::H264 => sys::ChiakiCodec_CHIAKI_CODEC_H264,
            Codec::H265 => sys::ChiakiCodec_CHIAKI_CODEC_H265,
            Codec::H265Hdr => sys::ChiakiCodec_CHIAKI_CODEC_H265_HDR,
        }
    }

    pub(crate) fn from_raw(raw: sys::ChiakiCodec) -> Self {
        match raw {
            sys::ChiakiCodec_CHIAKI_CODEC_H265 => Codec::H265,
            sys::ChiakiCodec_CHIAKI_CODEC_H265_HDR => Codec::H265Hdr,
            _ => Codec::H264,
        }
    }
}

/// Session termination reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuitReason {
    None,
    Stopped,
    SessionRequestUnknown,
    SessionRequestConnectionRefused,
    SessionRequestRpInUse,
    SessionRequestRpCrash,
    SessionRequestRpVersionMismatch,
    CtrlUnknown,
    CtrlConnectFailed,
    CtrlConnectionRefused,
    StreamConnectionUnknown,
    StreamConnectionRemoteDisconnected,
    /// Remote shutdown (e.g. PS button power-off from console side).
    StreamConnectionRemoteShutdown,
    PsnRegistFailed,
}

impl QuitReason {
    /// Returns `true` if the session ended with an error rather than a clean stop.
    #[inline]
    pub fn is_error(self) -> bool {
        !matches!(
            self,
            QuitReason::Stopped | QuitReason::StreamConnectionRemoteShutdown
        )
    }

    pub(crate) fn from_raw(raw: sys::ChiakiQuitReason) -> Self {
        match raw {
            sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_STOPPED => QuitReason::Stopped,
            sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_SESSION_REQUEST_UNKNOWN => {
                QuitReason::SessionRequestUnknown
            }
            sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_SESSION_REQUEST_CONNECTION_REFUSED => {
                QuitReason::SessionRequestConnectionRefused
            }
            sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_SESSION_REQUEST_RP_IN_USE => {
                QuitReason::SessionRequestRpInUse
            }
            sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_SESSION_REQUEST_RP_CRASH => {
                QuitReason::SessionRequestRpCrash
            }
            sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_SESSION_REQUEST_RP_VERSION_MISMATCH => {
                QuitReason::SessionRequestRpVersionMismatch
            }
            sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_CTRL_UNKNOWN => QuitReason::CtrlUnknown,
            sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_CTRL_CONNECT_FAILED => QuitReason::CtrlConnectFailed,
            sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_CTRL_CONNECTION_REFUSED => {
                QuitReason::CtrlConnectionRefused
            }
            sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_STREAM_CONNECTION_UNKNOWN => {
                QuitReason::StreamConnectionUnknown
            }
            sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_STREAM_CONNECTION_REMOTE_DISCONNECTED => {
                QuitReason::StreamConnectionRemoteDisconnected
            }
            sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_STREAM_CONNECTION_REMOTE_SHUTDOWN => {
                QuitReason::StreamConnectionRemoteShutdown
            }
            sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_PSN_REGIST_FAILED => QuitReason::PsnRegistFailed,
            _ => QuitReason::None,
        }
    }
}

/// DualSense adaptive trigger / haptic motor intensity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DualSenseEffectIntensity {
    Off,
    Weak,
    Medium,
    Strong,
}

impl DualSenseEffectIntensity {
    pub(crate) fn from_raw(raw: sys::ChiakiDualSenseEffectIntensity) -> Self {
        match raw {
            sys::chiaki_dualsense_effect_intensity_t_Weak => DualSenseEffectIntensity::Weak,
            sys::chiaki_dualsense_effect_intensity_t_Medium => DualSenseEffectIntensity::Medium,
            sys::chiaki_dualsense_effect_intensity_t_Strong => DualSenseEffectIntensity::Strong,
            _ => DualSenseEffectIntensity::Off,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chiaki_sys as sys;

    // ── Target ─────────────────────────────────────────────────────────────────

    #[test]
    fn target_is_ps5_true_for_ps5_variants() {
        assert!(Target::Ps5Unknown.is_ps5());
        assert!(Target::Ps5_1.is_ps5());
    }

    #[test]
    fn target_is_ps5_false_for_ps4_variants() {
        assert!(!Target::Ps4Unknown.is_ps5());
        assert!(!Target::Ps4_8.is_ps5());
        assert!(!Target::Ps4_9.is_ps5());
        assert!(!Target::Ps4_10.is_ps5());
    }

    #[test]
    fn target_is_unknown_true_for_unknown_variants() {
        assert!(Target::Ps4Unknown.is_unknown());
        assert!(Target::Ps5Unknown.is_unknown());
    }

    #[test]
    fn target_is_unknown_false_for_known_variants() {
        assert!(!Target::Ps4_8.is_unknown());
        assert!(!Target::Ps4_9.is_unknown());
        assert!(!Target::Ps4_10.is_unknown());
        assert!(!Target::Ps5_1.is_unknown());
    }

    #[test]
    fn target_roundtrip_to_from_raw() {
        let variants = [
            Target::Ps4_8,
            Target::Ps4_9,
            Target::Ps4_10,
            Target::Ps5Unknown,
            Target::Ps5_1,
        ];
        for v in variants {
            assert_eq!(Target::from_raw(v.to_raw()), v, "{v:?} roundtrip failed");
        }
    }

    #[test]
    fn target_ps4_unknown_roundtrip() {
        // Ps4Unknown maps to CHIAKI_TARGET_PS4_UNKNOWN; from_raw falls through to Ps4Unknown.
        let raw = sys::ChiakiTarget_CHIAKI_TARGET_PS4_UNKNOWN;
        assert_eq!(Target::from_raw(raw), Target::Ps4Unknown);
    }

    #[test]
    fn target_from_raw_unrecognized_maps_to_ps4_unknown() {
        assert_eq!(Target::from_raw(0xFFFF), Target::Ps4Unknown);
    }

    // ── Codec ──────────────────────────────────────────────────────────────────

    #[test]
    fn codec_is_h265_true_for_h265_variants() {
        assert!(Codec::H265.is_h265());
        assert!(Codec::H265Hdr.is_h265());
    }

    #[test]
    fn codec_is_h265_false_for_h264() {
        assert!(!Codec::H264.is_h265());
    }

    #[test]
    fn codec_is_hdr_true_only_for_h265_hdr() {
        assert!(Codec::H265Hdr.is_hdr());
        assert!(!Codec::H265.is_hdr());
        assert!(!Codec::H264.is_hdr());
    }

    #[test]
    fn codec_roundtrip_to_from_raw() {
        for codec in [Codec::H264, Codec::H265, Codec::H265Hdr] {
            assert_eq!(Codec::from_raw(codec.to_raw()), codec, "{codec:?} roundtrip failed");
        }
    }

    #[test]
    fn codec_from_raw_unrecognized_maps_to_h264() {
        assert_eq!(Codec::from_raw(0xFFFF), Codec::H264);
    }

    // ── QuitReason ────────────────────────────────────────────────────────────

    #[test]
    fn quit_reason_stopped_is_not_error() {
        assert!(!QuitReason::Stopped.is_error());
    }

    #[test]
    fn quit_reason_remote_shutdown_is_not_error() {
        assert!(!QuitReason::StreamConnectionRemoteShutdown.is_error());
    }

    #[test]
    fn quit_reason_none_is_error() {
        assert!(QuitReason::None.is_error());
    }

    #[test]
    fn all_error_quit_reasons_report_is_error_true() {
        let error_variants = [
            QuitReason::SessionRequestUnknown,
            QuitReason::SessionRequestConnectionRefused,
            QuitReason::SessionRequestRpInUse,
            QuitReason::SessionRequestRpCrash,
            QuitReason::SessionRequestRpVersionMismatch,
            QuitReason::CtrlUnknown,
            QuitReason::CtrlConnectFailed,
            QuitReason::CtrlConnectionRefused,
            QuitReason::StreamConnectionUnknown,
            QuitReason::StreamConnectionRemoteDisconnected,
            QuitReason::PsnRegistFailed,
        ];
        for r in error_variants {
            assert!(r.is_error(), "{r:?} should report is_error() == true");
        }
    }

    #[test]
    fn quit_reason_from_raw_unrecognized_maps_to_none() {
        assert_eq!(QuitReason::from_raw(0xFFFF), QuitReason::None);
    }

    #[test]
    fn quit_reason_stopped_from_raw() {
        assert_eq!(
            QuitReason::from_raw(sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_STOPPED),
            QuitReason::Stopped
        );
    }

    #[test]
    fn quit_reason_remote_shutdown_from_raw() {
        assert_eq!(
            QuitReason::from_raw(
                sys::ChiakiQuitReason_CHIAKI_QUIT_REASON_STREAM_CONNECTION_REMOTE_SHUTDOWN
            ),
            QuitReason::StreamConnectionRemoteShutdown
        );
    }

    // ── DualSenseEffectIntensity ───────────────────────────────────────────────

    #[test]
    fn dualsense_intensity_known_values_from_raw() {
        assert_eq!(
            DualSenseEffectIntensity::from_raw(sys::chiaki_dualsense_effect_intensity_t_Weak),
            DualSenseEffectIntensity::Weak
        );
        assert_eq!(
            DualSenseEffectIntensity::from_raw(sys::chiaki_dualsense_effect_intensity_t_Medium),
            DualSenseEffectIntensity::Medium
        );
        assert_eq!(
            DualSenseEffectIntensity::from_raw(sys::chiaki_dualsense_effect_intensity_t_Strong),
            DualSenseEffectIntensity::Strong
        );
    }

    #[test]
    fn dualsense_intensity_unrecognized_maps_to_off() {
        assert_eq!(
            DualSenseEffectIntensity::from_raw(0xFFFF),
            DualSenseEffectIntensity::Off
        );
    }

    #[test]
    fn dualsense_intensity_is_copy_and_eq() {
        let a = DualSenseEffectIntensity::Strong;
        let b = a;
        assert_eq!(a, b);
        assert_ne!(DualSenseEffectIntensity::Weak, DualSenseEffectIntensity::Strong);
    }
}
