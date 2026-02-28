// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

use chiaki_sys as sys;
use thiserror::Error;

/// Errors returned by the chiaki library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum Error {
    #[error("unknown error")]
    Unknown,
    #[error("failed to parse address")]
    ParseAddr,
    #[error("thread error")]
    Thread,
    #[error("memory error")]
    Memory,
    #[error("overflow")]
    Overflow,
    #[error("network error")]
    Network,
    #[error("connection refused")]
    ConnectionRefused,
    #[error("host is down")]
    HostDown,
    #[error("host is unreachable")]
    HostUnreach,
    #[error("disconnected")]
    Disconnected,
    #[error("invalid data")]
    InvalidData,
    #[error("buffer too small")]
    BufTooSmall,
    #[error("mutex locked")]
    MutexLocked,
    #[error("canceled")]
    Canceled,
    #[error("timeout")]
    Timeout,
    #[error("invalid response")]
    InvalidResponse,
    #[error("invalid MAC")]
    InvalidMac,
    #[error("uninitialized")]
    Uninitialized,
    #[error("FEC failed")]
    FecFailed,
    #[error("version mismatch")]
    VersionMismatch,
    #[error("HTTP non-OK response")]
    HttpNonOk,
    /// A future error code not known at compile time.
    #[error("unrecognized error code: {0}")]
    UnrecognizedCode(u32),
}

/// Convenience `Result` alias using [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

impl From<sys::ChiakiErrorCode> for Error {
    fn from(code: sys::ChiakiErrorCode) -> Self {
        match code {
            sys::ChiakiErrorCode_CHIAKI_ERR_UNKNOWN => Error::Unknown,
            sys::ChiakiErrorCode_CHIAKI_ERR_PARSE_ADDR => Error::ParseAddr,
            sys::ChiakiErrorCode_CHIAKI_ERR_THREAD => Error::Thread,
            sys::ChiakiErrorCode_CHIAKI_ERR_MEMORY => Error::Memory,
            sys::ChiakiErrorCode_CHIAKI_ERR_OVERFLOW => Error::Overflow,
            sys::ChiakiErrorCode_CHIAKI_ERR_NETWORK => Error::Network,
            sys::ChiakiErrorCode_CHIAKI_ERR_CONNECTION_REFUSED => Error::ConnectionRefused,
            sys::ChiakiErrorCode_CHIAKI_ERR_HOST_DOWN => Error::HostDown,
            sys::ChiakiErrorCode_CHIAKI_ERR_HOST_UNREACH => Error::HostUnreach,
            sys::ChiakiErrorCode_CHIAKI_ERR_DISCONNECTED => Error::Disconnected,
            sys::ChiakiErrorCode_CHIAKI_ERR_INVALID_DATA => Error::InvalidData,
            sys::ChiakiErrorCode_CHIAKI_ERR_BUF_TOO_SMALL => Error::BufTooSmall,
            sys::ChiakiErrorCode_CHIAKI_ERR_MUTEX_LOCKED => Error::MutexLocked,
            sys::ChiakiErrorCode_CHIAKI_ERR_CANCELED => Error::Canceled,
            sys::ChiakiErrorCode_CHIAKI_ERR_TIMEOUT => Error::Timeout,
            sys::ChiakiErrorCode_CHIAKI_ERR_INVALID_RESPONSE => Error::InvalidResponse,
            sys::ChiakiErrorCode_CHIAKI_ERR_INVALID_MAC => Error::InvalidMac,
            sys::ChiakiErrorCode_CHIAKI_ERR_UNINITIALIZED => Error::Uninitialized,
            sys::ChiakiErrorCode_CHIAKI_ERR_FEC_FAILED => Error::FecFailed,
            sys::ChiakiErrorCode_CHIAKI_ERR_VERSION_MISMATCH => Error::VersionMismatch,
            sys::ChiakiErrorCode_CHIAKI_ERR_HTTP_NONOK => Error::HttpNonOk,
            other => Error::UnrecognizedCode(other),
        }
    }
}

/// Convert a raw `ChiakiErrorCode` into `Result<()>`.
///
/// `CHIAKI_ERR_SUCCESS` becomes `Ok(())`, everything else becomes `Err`.
#[inline]
pub(crate) fn ffi_result(code: sys::ChiakiErrorCode) -> Result<()> {
    if code == sys::ChiakiErrorCode_CHIAKI_ERR_SUCCESS {
        Ok(())
    } else {
        Err(Error::from(code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chiaki_sys as sys;

    #[test]
    fn success_code_returns_ok() {
        assert!(ffi_result(sys::ChiakiErrorCode_CHIAKI_ERR_SUCCESS).is_ok());
    }

    #[test]
    fn ffi_result_errors_on_non_success() {
        let result = ffi_result(sys::ChiakiErrorCode_CHIAKI_ERR_TIMEOUT);
        assert_eq!(result, Err(Error::Timeout));
    }

    #[test]
    fn all_known_error_codes_map_correctly() {
        let cases: &[(sys::ChiakiErrorCode, Error)] = &[
            (sys::ChiakiErrorCode_CHIAKI_ERR_UNKNOWN, Error::Unknown),
            (sys::ChiakiErrorCode_CHIAKI_ERR_PARSE_ADDR, Error::ParseAddr),
            (sys::ChiakiErrorCode_CHIAKI_ERR_THREAD, Error::Thread),
            (sys::ChiakiErrorCode_CHIAKI_ERR_MEMORY, Error::Memory),
            (sys::ChiakiErrorCode_CHIAKI_ERR_OVERFLOW, Error::Overflow),
            (sys::ChiakiErrorCode_CHIAKI_ERR_NETWORK, Error::Network),
            (sys::ChiakiErrorCode_CHIAKI_ERR_CONNECTION_REFUSED, Error::ConnectionRefused),
            (sys::ChiakiErrorCode_CHIAKI_ERR_HOST_DOWN, Error::HostDown),
            (sys::ChiakiErrorCode_CHIAKI_ERR_HOST_UNREACH, Error::HostUnreach),
            (sys::ChiakiErrorCode_CHIAKI_ERR_DISCONNECTED, Error::Disconnected),
            (sys::ChiakiErrorCode_CHIAKI_ERR_INVALID_DATA, Error::InvalidData),
            (sys::ChiakiErrorCode_CHIAKI_ERR_BUF_TOO_SMALL, Error::BufTooSmall),
            (sys::ChiakiErrorCode_CHIAKI_ERR_MUTEX_LOCKED, Error::MutexLocked),
            (sys::ChiakiErrorCode_CHIAKI_ERR_CANCELED, Error::Canceled),
            (sys::ChiakiErrorCode_CHIAKI_ERR_TIMEOUT, Error::Timeout),
            (sys::ChiakiErrorCode_CHIAKI_ERR_INVALID_RESPONSE, Error::InvalidResponse),
            (sys::ChiakiErrorCode_CHIAKI_ERR_INVALID_MAC, Error::InvalidMac),
            (sys::ChiakiErrorCode_CHIAKI_ERR_UNINITIALIZED, Error::Uninitialized),
            (sys::ChiakiErrorCode_CHIAKI_ERR_FEC_FAILED, Error::FecFailed),
            (sys::ChiakiErrorCode_CHIAKI_ERR_VERSION_MISMATCH, Error::VersionMismatch),
            (sys::ChiakiErrorCode_CHIAKI_ERR_HTTP_NONOK, Error::HttpNonOk),
        ];
        for &(code, expected) in cases {
            assert_eq!(Error::from(code), expected, "code {code} did not map to {expected:?}");
        }
    }

    #[test]
    fn unrecognized_code_wraps_value() {
        let raw: sys::ChiakiErrorCode = 0xDEAD;
        assert_eq!(Error::from(raw), Error::UnrecognizedCode(0xDEAD));
    }

    #[test]
    fn error_display_messages() {
        assert_eq!(Error::Unknown.to_string(), "unknown error");
        assert_eq!(Error::Timeout.to_string(), "timeout");
        assert_eq!(Error::InvalidMac.to_string(), "invalid MAC");
        assert_eq!(Error::FecFailed.to_string(), "FEC failed");
        assert_eq!(
            Error::UnrecognizedCode(42).to_string(),
            "unrecognized error code: 42"
        );
    }

    #[test]
    fn error_is_copy_and_clone() {
        let e1 = Error::Network;
        let e2 = e1; // copy
        let e3 = e1; // copy again
        assert_eq!(e1, e2);
        assert_eq!(e2, e3);
    }

    #[test]
    fn error_equality_and_inequality() {
        assert_eq!(Error::Timeout, Error::Timeout);
        assert_ne!(Error::Timeout, Error::Network);
        assert_eq!(Error::UnrecognizedCode(1), Error::UnrecognizedCode(1));
        assert_ne!(Error::UnrecognizedCode(1), Error::UnrecognizedCode(2));
    }
}
