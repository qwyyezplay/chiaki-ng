// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

use std::ffi::CStr;
use std::os::raw::c_void;
use std::sync::Arc;

use bitflags::bitflags;
use chiaki_sys as sys;

bitflags! {
    /// Bitmask controlling which log levels are emitted.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct LogLevel: u32 {
        /// Error messages only.
        const ERROR   = 1 << 0;
        /// Warnings and above.
        const WARNING = 1 << 1;
        /// Informational messages and above.
        const INFO    = 1 << 2;
        /// Verbose output and above.
        const VERBOSE = 1 << 3;
        /// All messages including debug.
        const DEBUG   = 1 << 4;
        /// All log levels enabled.
        const ALL     = (1 << 5) - 1;
    }
}

/// Boxed Rust logging closure stored as C `void *user`.
type LogClosure = Box<dyn Fn(LogLevel, &str) + Send + Sync + 'static>;

/// Outer box whose raw pointer is stored in `ChiakiLog.user`.
///
/// Double-boxing stabilises the fat-pointer address so the C side always
/// finds a valid `*const LogClosure` at the address given at init time.
type LogClosureOuter = Box<LogClosure>;

/// `extern "C"` trampoline forwarding C log calls to a Rust closure.
///
/// # Safety
/// `user` must point to a live `LogClosureOuter` for the duration of this
/// call, which is guaranteed by `Log` keeping `_callback_data` alive.
unsafe extern "C" fn log_callback_trampoline(
    level: sys::ChiakiLogLevel,
    msg: *const ::std::os::raw::c_char,
    user: *mut c_void,
) { unsafe {
    let closure = &*(user as *const LogClosure);
    let rust_level = LogLevel::from_bits_truncate(level);
    let msg_str = if msg.is_null() {
        ""
    } else {
        // SAFETY: C guarantees a valid, null-terminated string.
        CStr::from_ptr(msg).to_str().unwrap_or("")
    };
    closure(rust_level, msg_str);
}}

/// A chiaki logger.
///
/// Heap-allocates the underlying `ChiakiLog` so its address is stable across
/// moves. Pass `Arc<Log>` to sessions and other components that need shared
/// access to the same logger.
pub struct Log {
    /// Stable heap address — the C library stores `*mut ChiakiLog` internally.
    raw: Box<sys::ChiakiLog>,
    /// Keeps the boxed closure alive for the same lifetime as `raw`.
    _callback_data: Option<LogClosureOuter>,
}

impl Log {
    /// Create a logger that prints to stdout via chiaki's built-in printer.
    pub fn new_default(level_mask: LogLevel) -> Arc<Self> {
        let mut raw = Box::new(unsafe { std::mem::zeroed::<sys::ChiakiLog>() });
        unsafe {
            sys::chiaki_log_init(
                raw.as_mut(),
                level_mask.bits(),
                Some(sys::chiaki_log_cb_print),
                std::ptr::null_mut(),
            );
        }
        Arc::new(Log {
            raw,
            _callback_data: None,
        })
    }

    /// Create a logger with a custom Rust closure.
    ///
    /// The closure receives the [`LogLevel`] and the log message string.
    ///
    /// # Example
    /// ```no_run
    /// use chiaki::log::{Log, LogLevel};
    ///
    /// let log = Log::new(LogLevel::ALL, |level, msg| {
    ///     eprintln!("[{level:?}] {msg}");
    /// });
    /// ```
    pub fn new(
        level_mask: LogLevel,
        cb: impl Fn(LogLevel, &str) + Send + Sync + 'static,
    ) -> Arc<Self> {
        // Inner box is the fat pointer (ptr + vtable).
        let inner: LogClosure = Box::new(cb);
        // Outer box stabilises the address of the fat pointer.
        let outer: LogClosureOuter = Box::new(inner);
        let user_ptr: *mut c_void = Box::into_raw(outer) as *mut c_void;

        let mut raw = Box::new(unsafe { std::mem::zeroed::<sys::ChiakiLog>() });
        unsafe {
            sys::chiaki_log_init(
                raw.as_mut(),
                level_mask.bits(),
                Some(log_callback_trampoline),
                user_ptr,
            );
        }

        // Reclaim ownership so we can drop it when Log is dropped.
        // SAFETY: `user_ptr` was created by `Box::into_raw` above and has not
        // been freed.
        let callback_data: LogClosureOuter =
            unsafe { Box::from_raw(user_ptr as *mut LogClosure) };

        Arc::new(Log {
            raw,
            _callback_data: Some(callback_data),
        })
    }

    /// Returns a stable mutable pointer to the inner `ChiakiLog`.
    ///
    /// The returned pointer is valid for the lifetime of this `Log`.
    /// It is the caller's responsibility not to retain this pointer beyond
    /// the lifetime of the `Arc<Log>`.
    pub(crate) fn raw_ptr(&self) -> *mut sys::ChiakiLog {
        // Box::as_ref gives &ChiakiLog whose address is the heap allocation.
        self.raw.as_ref() as *const sys::ChiakiLog as *mut sys::ChiakiLog
    }
}

// SAFETY: `ChiakiLog` has no thread affinity; `chiaki_log()` only reads
// `level_mask` then calls the callback.  The callback closure is required
// `Send + Sync`.
unsafe impl Send for Log {}
unsafe impl Sync for Log {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn init() {
        crate::init().unwrap();
    }

    // ── LogLevel bitflags ──────────────────────────────────────────────────────

    #[test]
    fn log_level_individual_bit_values() {
        assert_eq!(LogLevel::ERROR.bits(), 1 << 0);
        assert_eq!(LogLevel::WARNING.bits(), 1 << 1);
        assert_eq!(LogLevel::INFO.bits(), 1 << 2);
        assert_eq!(LogLevel::VERBOSE.bits(), 1 << 3);
        assert_eq!(LogLevel::DEBUG.bits(), 1 << 4);
    }

    #[test]
    fn log_level_all_contains_every_level() {
        assert!(LogLevel::ALL.contains(LogLevel::ERROR));
        assert!(LogLevel::ALL.contains(LogLevel::WARNING));
        assert!(LogLevel::ALL.contains(LogLevel::INFO));
        assert!(LogLevel::ALL.contains(LogLevel::VERBOSE));
        assert!(LogLevel::ALL.contains(LogLevel::DEBUG));
    }

    #[test]
    fn log_level_all_equals_union_of_all_bits() {
        let manual_all = LogLevel::ERROR
            | LogLevel::WARNING
            | LogLevel::INFO
            | LogLevel::VERBOSE
            | LogLevel::DEBUG;
        assert_eq!(LogLevel::ALL, manual_all);
    }

    #[test]
    fn log_level_combination_only_contains_specified_levels() {
        let mask = LogLevel::ERROR | LogLevel::WARNING;
        assert!(mask.contains(LogLevel::ERROR));
        assert!(mask.contains(LogLevel::WARNING));
        assert!(!mask.contains(LogLevel::INFO));
        assert!(!mask.contains(LogLevel::VERBOSE));
        assert!(!mask.contains(LogLevel::DEBUG));
    }

    #[test]
    fn log_level_empty_contains_nothing() {
        let empty = LogLevel::empty();
        assert!(!empty.contains(LogLevel::ERROR));
        assert!(!empty.contains(LogLevel::DEBUG));
    }

    #[test]
    fn log_level_is_copy_and_eq() {
        let a = LogLevel::INFO;
        let b = a;
        assert_eq!(a, b);
        assert_ne!(LogLevel::INFO, LogLevel::DEBUG);
    }

    // ── Log construction ──────────────────────────────────────────────────────

    #[test]
    fn log_new_default_returns_non_null_ptr() {
        init();
        let log = Log::new_default(LogLevel::ERROR);
        assert!(!log.raw_ptr().is_null());
    }

    #[test]
    fn log_new_with_noop_closure_returns_non_null_ptr() {
        init();
        let log = Log::new(LogLevel::ALL, |_level, _msg| {});
        assert!(!log.raw_ptr().is_null());
    }

    #[test]
    fn log_arc_clone_shares_same_raw_ptr() {
        init();
        let log = Log::new_default(LogLevel::INFO);
        let log2 = Arc::clone(&log);
        // Both Arc references point to the same Log, so same raw_ptr.
        assert_eq!(log.raw_ptr(), log2.raw_ptr());
    }

    #[test]
    fn log_closure_captures_external_state() {
        init();
        let captured = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_clone = Arc::clone(&captured);
        // Create logger with closure that records messages.
        let _log = Log::new(LogLevel::ALL, move |_level, msg| {
            captured_clone.lock().unwrap().push(msg.to_string());
        });
        // We can't easily trigger the C callback without a session, but we
        // verify the logger was created successfully and the closure compiles.
        assert!(!_log.raw_ptr().is_null());
    }

    #[test]
    fn multiple_logs_have_distinct_raw_ptrs() {
        init();
        let log1 = Log::new_default(LogLevel::ERROR);
        let log2 = Log::new_default(LogLevel::DEBUG);
        // Each Log heap-allocates a separate ChiakiLog.
        assert_ne!(log1.raw_ptr(), log2.raw_ptr());
    }
}
