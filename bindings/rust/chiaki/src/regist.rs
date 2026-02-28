// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::ptr;
use std::sync::{mpsc, Arc};

use chiaki_sys as sys;

use crate::error::{ffi_result, Result};
use crate::log::Log;
use crate::types::Target;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Convert a null-terminated C char array (as used in C structs) to a `String`.
///
/// # Safety
/// `chars` must be null-terminated within its length, which is the case for
/// all fixed-size character arrays in `ChiakiRegisteredHost`.
fn c_char_slice_to_string(chars: &[c_char]) -> String {
    // SAFETY: chars.as_ptr() points to a C char array that is contained in a
    // valid C struct; it is null-terminated within its declared length.
    unsafe { CStr::from_ptr(chars.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

// ── RegisteredHost ────────────────────────────────────────────────────────────

/// Credentials and metadata for a successfully registered PlayStation console.
///
/// Returned from a successful [`Regist`] operation. Store these to reconnect
/// without going through registration again.
#[derive(Debug, Clone)]
pub struct RegisteredHost {
    pub target: Target,
    pub ap_ssid: String,
    pub ap_bssid: String,
    pub ap_key: String,
    pub ap_name: String,
    /// Console MAC address (6 bytes).
    pub server_mac: [u8; 6],
    pub server_nickname: String,
    /// 16-byte binary remote play registration key (not a C string).
    pub rp_regist_key: [u8; 16],
    pub rp_key_type: u32,
    /// 16-byte binary remote play key.
    pub rp_key: [u8; 16],
    pub console_pin: u32,
}

impl RegisteredHost {
    /// Convert from the raw C struct.
    ///
    /// # Safety
    /// `raw` must point to a fully initialised `chiaki_registered_host_t`.
    pub(crate) unsafe fn from_raw(raw: &sys::chiaki_registered_host_t) -> Self {
        // rp_regist_key is binary key material, not a C string — copy as bytes.
        let rp_regist_key: [u8; 16] = raw
            .rp_regist_key
            .iter()
            .map(|&c| c as u8)
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();

        RegisteredHost {
            target: Target::from_raw(raw.target),
            ap_ssid: c_char_slice_to_string(&raw.ap_ssid),
            ap_bssid: c_char_slice_to_string(&raw.ap_bssid),
            ap_key: c_char_slice_to_string(&raw.ap_key),
            ap_name: c_char_slice_to_string(&raw.ap_name),
            server_mac: raw.server_mac,
            server_nickname: c_char_slice_to_string(&raw.server_nickname),
            rp_regist_key,
            rp_key_type: raw.rp_key_type,
            rp_key: raw.rp_key,
            console_pin: raw.console_pin,
        }
    }
}

// ── RegistResult ──────────────────────────────────────────────────────────────

/// Outcome of a registration attempt.
#[derive(Debug)]
pub enum RegistResult {
    /// The user (or code) canceled the registration before it completed.
    Canceled,
    /// Registration failed (e.g. wrong PIN, network error).
    Failed,
    /// Registration succeeded — the host credentials are in the payload.
    Success(RegisteredHost),
}

// ── RegistInfo ────────────────────────────────────────────────────────────────

/// Parameters for registering with a PlayStation console directly over LAN.
///
/// PSN-based remote registration (requiring holepunch/RUDP) is not exposed
/// in this safe wrapper; use `chiaki-sys` directly for that path.
#[derive(Debug, Clone)]
pub struct RegistInfo {
    /// Target console family.
    pub target: Target,
    /// Console hostname or IP address.
    pub host: String,
    /// If `true`, a UDP broadcast is sent instead of a unicast packet.
    pub broadcast: bool,
    /// PSN online ID (username).  Mutually exclusive with `psn_account_id`;
    /// set this to `None` to use the account-id path (PS4 firmware ≥ 7.0).
    pub psn_online_id: Option<String>,
    /// 8-byte PSN account ID.  Used when `psn_online_id` is `None`.
    pub psn_account_id: Option<[u8; 8]>,
    /// 8-digit PIN shown on the PS4/PS5 "Remote Play" screen.
    pub pin: u32,
    /// Console PIN for PS4 < 7.0.  Usually 0.
    pub console_pin: u32,
}

// ── Callback data ──────────────────────────────────────────────────────────

struct RegistCbData {
    /// Consumed on first callback invocation; set to `None` afterwards.
    tx: Option<mpsc::Sender<RegistResult>>,
}

/// `extern "C"` trampoline forwarding C registration callbacks to Rust.
///
/// # Safety
/// `user` must point to a live `RegistCbData` for the duration of this call.
unsafe extern "C" fn regist_callback_trampoline(
    event: *mut sys::chiaki_regist_event_t,
    user: *mut c_void,
) { unsafe {
    let data = &mut *(user as *mut RegistCbData);

    let result = match (*event).type_ {
        sys::chiaki_regist_event_type_t_CHIAKI_REGIST_EVENT_TYPE_FINISHED_CANCELED => RegistResult::Canceled,
        sys::chiaki_regist_event_type_t_CHIAKI_REGIST_EVENT_TYPE_FINISHED_SUCCESS => {
            let host_ptr = (*event).registered_host;
            if host_ptr.is_null() {
                RegistResult::Failed
            } else {
                // SAFETY: C guarantees registered_host is valid on success.
                RegistResult::Success(RegisteredHost::from_raw(&*host_ptr))
            }
        }
        _ => RegistResult::Failed,
    };

    if let Some(tx) = data.tx.take() {
        let _ = tx.send(result);
    }
}}

// ── Regist ────────────────────────────────────────────────────────────────────

/// RAII wrapper around a registration attempt with a PlayStation console.
///
/// # Channel-based result
///
/// `Regist::start` returns `(Self, Receiver<RegistResult>)`. Block on
/// `receiver.recv()` to get the outcome or use `recv_timeout` for a timeout.
///
/// # Drop behaviour
/// Dropping `Regist` calls `chiaki_regist_stop` (signals cancellation) and
/// then `chiaki_regist_fini` (joins the C thread).
pub struct Regist {
    raw: Box<sys::chiaki_regist_t>,
    _log: Arc<Log>,
    /// Keeps callback memory live until after `chiaki_regist_fini`.
    _callback_data: Box<RegistCbData>,
}

impl Regist {
    /// Start a registration attempt.
    ///
    /// Returns the RAII handle and a [`mpsc::Receiver`] that yields exactly
    /// one [`RegistResult`] when registration completes (success, failure, or
    /// cancellation).
    pub fn start(info: RegistInfo, log: Arc<Log>) -> Result<(Self, mpsc::Receiver<RegistResult>)> {
        let (tx, rx) = mpsc::channel();
        let callback_data = Box::new(RegistCbData { tx: Some(tx) });
        let cb_ptr = &*callback_data as *const RegistCbData as *mut c_void;

        // Build C-compatible strings (live for the duration of this function
        // call only; C strdups them in chiaki_regist_start).
        let host_c = CString::new(info.host.as_str()).unwrap_or_default();
        let psn_online_id_c = info
            .psn_online_id
            .as_deref()
            .map(|s| CString::new(s).unwrap_or_default());

        let c_info = sys::chiaki_regist_info_t {
            target: info.target.to_raw(),
            host: host_c.as_ptr(),
            broadcast: info.broadcast,
            psn_online_id: psn_online_id_c
                .as_ref()
                .map_or(ptr::null(), |s| s.as_ptr()),
            psn_account_id: info.psn_account_id.unwrap_or([0u8; 8]),
            pin: info.pin,
            console_pin: info.console_pin,
            // PSN / holepunch registration is not supported in this wrapper.
            holepunch_info: ptr::null_mut(),
            rudp: ptr::null_mut(),
        };

        // Heap-allocate the C struct so its address is stable across moves.
        let mut raw = Box::new(unsafe { std::mem::zeroed::<sys::chiaki_regist_t>() });

        // SAFETY: raw is a valid zeroed ChiakiRegist; c_info and all its
        // pointer fields are valid for this call; log.raw_ptr() is stable.
        let err = unsafe {
            sys::chiaki_regist_start(
                raw.as_mut(),
                log.raw_ptr(),
                &c_info,
                Some(regist_callback_trampoline),
                cb_ptr,
            )
        };

        // host_c and psn_online_id_c are dropped here — safe because C
        // has already strdup'd them inside chiaki_regist_start.
        drop(psn_online_id_c);
        drop(host_c);

        ffi_result(err)?;

        Ok((
            Regist {
                raw,
                _log: log,
                _callback_data: callback_data,
            },
            rx,
        ))
    }
}

impl Drop for Regist {
    fn drop(&mut self) {
        // Signal stop (no-op if callback already fired).
        unsafe { sys::chiaki_regist_stop(self.raw.as_mut()) };
        // Join the C thread and free internal resources.
        // SAFETY: raw is a valid, initialised ChiakiRegist.
        unsafe { sys::chiaki_regist_fini(self.raw.as_mut()) };
        // _callback_data is dropped automatically after this point — safe
        // because chiaki_regist_fini has fully joined the C thread.
    }
}

// SAFETY: ChiakiRegist's thread uses internal synchronisation.  The outer
// Rust type owns all referenced memory exclusively.
unsafe impl Send for Regist {}
