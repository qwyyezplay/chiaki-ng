// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

use std::ffi::{CStr, CString};
use std::net::IpAddr;
use std::os::raw::c_void;
use std::ptr;
use std::sync::Arc;

use chiaki_sys as sys;
use chiaki_sys::libc;

use crate::error::{ffi_result, Result};
use crate::log::Log;

// ── DiscoveryHostState ────────────────────────────────────────────────────────

/// Power state of a discovered PlayStation console.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiscoveryHostState {
    Unknown,
    /// Console is on and ready to accept a connection.
    Ready,
    /// Console is in standby / rest mode (can be woken via [`DiscoveryService::wakeup`]).
    Standby,
}

impl DiscoveryHostState {
    fn from_raw(raw: sys::ChiakiDiscoveryHostState) -> Self {
        match raw {
            sys::chiaki_discovery_host_state_t_CHIAKI_DISCOVERY_HOST_STATE_READY => DiscoveryHostState::Ready,
            sys::chiaki_discovery_host_state_t_CHIAKI_DISCOVERY_HOST_STATE_STANDBY => DiscoveryHostState::Standby,
            _ => DiscoveryHostState::Unknown,
        }
    }
}

// ── DiscoveryHost ─────────────────────────────────────────────────────────────

/// Metadata for a PlayStation console discovered on the local network.
#[derive(Debug, Clone)]
pub struct DiscoveryHost {
    pub state: DiscoveryHostState,
    pub host_request_port: u16,
    pub host_addr: Option<String>,
    pub system_version: Option<String>,
    pub device_discovery_protocol_version: Option<String>,
    pub host_name: Option<String>,
    pub host_type: Option<String>,
    pub host_id: Option<String>,
    pub running_app_titleid: Option<String>,
    pub running_app_name: Option<String>,
}

/// Convert a nullable `*const c_char` to `Option<String>`.
///
/// # Safety
/// `ptr` must be either null or a valid null-terminated C string.
unsafe fn opt_cstr(ptr: *const ::std::os::raw::c_char) -> Option<String> { unsafe {
    if ptr.is_null() {
        None
    } else {
        Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
    }
}}

impl DiscoveryHost {
    /// Convert from the raw C struct.
    ///
    /// # Safety
    /// `raw` must point to a valid `chiaki_discovery_host_t` for the duration
    /// of this call (it is valid inside the C callback).
    unsafe fn from_raw(raw: &sys::chiaki_discovery_host_t) -> Self { unsafe {
        DiscoveryHost {
            state: DiscoveryHostState::from_raw(raw.state),
            host_request_port: raw.host_request_port,
            host_addr: opt_cstr(raw.host_addr),
            system_version: opt_cstr(raw.system_version),
            device_discovery_protocol_version: opt_cstr(
                raw.device_discovery_protocol_version,
            ),
            host_name: opt_cstr(raw.host_name),
            host_type: opt_cstr(raw.host_type),
            host_id: opt_cstr(raw.host_id),
            running_app_titleid: opt_cstr(raw.running_app_titleid),
            running_app_name: opt_cstr(raw.running_app_name),
        }
    }}
}

// ── DiscoveryServiceOptions ───────────────────────────────────────────────────

/// Configuration for a [`DiscoveryService`].
pub struct DiscoveryServiceOptions {
    /// Maximum number of consoles to track simultaneously.
    pub hosts_max: usize,
    /// Number of missed pings before a host is removed from the list.
    pub host_drop_pings: u64,
    /// Interval between pings, in milliseconds.
    pub ping_ms: u64,
    /// Delay before the first ping, in milliseconds.
    pub ping_initial_ms: u64,
    /// Destination address for search packets (typically the broadcast address
    /// `255.255.255.255` for IPv4 LAN discovery).
    pub send_addr: IpAddr,
    /// Additional broadcast addresses (leave empty for single-LAN setups).
    pub broadcast_addrs: Vec<IpAddr>,
}

impl Default for DiscoveryServiceOptions {
    fn default() -> Self {
        DiscoveryServiceOptions {
            hosts_max: 16,
            host_drop_pings: 3,
            ping_ms: 2_000,
            ping_initial_ms: 500,
            send_addr: "255.255.255.255".parse().unwrap(),
            broadcast_addrs: Vec::new(),
        }
    }
}

// ── sockaddr helpers ─────────────────────────────────────────────────────────

/// Build a zeroed `sockaddr_storage` filled for the given `IpAddr`.
///
/// The port is intentionally left as 0; `chiaki_discovery_service_init`
/// overwrites it with the correct discovery port.
fn ipaddr_to_sockaddr_storage(addr: IpAddr) -> sys::sockaddr_storage {
    let mut storage: sys::sockaddr_storage = unsafe { std::mem::zeroed() };
    match addr {
        IpAddr::V4(v4) => unsafe {
            let sin =
                &mut storage as *mut sys::sockaddr_storage as *mut libc::sockaddr_in;
            (*sin).sin_family = libc::AF_INET as _;
            (*sin).sin_addr.s_addr = u32::from_be_bytes(v4.octets());
        },
        IpAddr::V6(v6) => unsafe {
            let sin6 =
                &mut storage as *mut sys::sockaddr_storage as *mut libc::sockaddr_in6;
            (*sin6).sin6_family = libc::AF_INET6 as _;
            (*sin6).sin6_addr.s6_addr = v6.octets();
        },
    }
    storage
}

// ── Callback data ─────────────────────────────────────────────────────────────

struct DiscoveryCbData {
    callback: Box<dyn Fn(Vec<DiscoveryHost>) + Send + 'static>,
}

/// `extern "C"` trampoline forwarding C discovery callbacks to Rust.
///
/// # Safety
/// `user` must point to a live `DiscoveryCbData`.
unsafe extern "C" fn discovery_callback_trampoline(
    hosts: *mut sys::chiaki_discovery_host_t,
    hosts_count: usize,
    user: *mut c_void,
) { unsafe {
    let data = &*(user as *const DiscoveryCbData);
    let rust_hosts: Vec<DiscoveryHost> = (0..hosts_count)
        .map(|i| DiscoveryHost::from_raw(&*hosts.add(i)))
        .collect();
    (data.callback)(rust_hosts);
}}

// ── DiscoveryService ──────────────────────────────────────────────────────────

/// RAII wrapper around a PS4/PS5 LAN discovery service.
///
/// Spawns a background thread that periodically broadcasts search packets.
/// The provided callback is invoked on that thread whenever the set of visible
/// consoles changes.
///
/// Dropping this value stops the background thread cleanly.
pub struct DiscoveryService {
    raw: Box<sys::chiaki_discovery_service_t>,
    _log: Arc<Log>,
    /// Callback memory — valid until after `chiaki_discovery_service_fini`.
    _callback_data: Box<DiscoveryCbData>,
}

impl DiscoveryService {
    /// Start the discovery service.
    ///
    /// `callback` is called on the internal discovery thread each time the
    /// known-host list is updated.  The argument is a snapshot of all currently
    /// visible consoles.
    pub fn start(
        options: DiscoveryServiceOptions,
        log: Arc<Log>,
        callback: impl Fn(Vec<DiscoveryHost>) + Send + 'static,
    ) -> Result<Self> {
        let callback_data = Box::new(DiscoveryCbData {
            callback: Box::new(callback),
        });
        let cb_ptr = &*callback_data as *const DiscoveryCbData as *mut c_void;

        // Build the send_addr sockaddr_storage (C will malloc+memcpy it).
        let mut send_addr = ipaddr_to_sockaddr_storage(options.send_addr);
        // IMPORTANT: send_addr_size must match the *actual* sockaddr variant,
        // not sockaddr_storage.  On macOS (and POSIX in general) sendto(2)
        // returns EINVAL if addrlen is larger than the real structure.
        let send_addr_size = match options.send_addr {
            IpAddr::V4(_) => std::mem::size_of::<libc::sockaddr_in>(),
            IpAddr::V6(_) => std::mem::size_of::<libc::sockaddr_in6>(),
        };

        // Build the broadcast_addrs array (C will malloc+memcpy).
        let broadcast_storages: Vec<sys::sockaddr_storage> = options
            .broadcast_addrs
            .iter()
            .map(|&a| ipaddr_to_sockaddr_storage(a))
            .collect();
        let broadcast_addrs_ptr = if broadcast_storages.is_empty() {
            ptr::null_mut()
        } else {
            broadcast_storages.as_ptr() as *mut sys::sockaddr_storage
        };

        let c_options = sys::chiaki_discovery_service_options_t {
            hosts_max: options.hosts_max,
            host_drop_pings: options.host_drop_pings,
            ping_ms: options.ping_ms,
            ping_initial_ms: options.ping_initial_ms,
            send_addr: &mut send_addr,
            send_addr_size,
            broadcast_addrs: broadcast_addrs_ptr,
            broadcast_num: broadcast_storages.len(),
            send_host: ptr::null_mut(),
            cb: Some(discovery_callback_trampoline),
            cb_user: cb_ptr,
        };

        // Heap-allocate so the C thread has a stable `*this` pointer.
        let mut raw = Box::new(unsafe {
            std::mem::zeroed::<sys::chiaki_discovery_service_t>()
        });

        // SAFETY: raw is valid zeroed memory; c_options and log are valid.
        // C copies send_addr and broadcast_addrs internally via malloc+memcpy.
        let err = unsafe {
            sys::chiaki_discovery_service_init(
                raw.as_mut(),
                &c_options as *const _ as *mut _,
                log.raw_ptr(),
            )
        };

        // broadcast_storages and send_addr are dropped here — safe because C
        // has already copied both inside chiaki_discovery_service_init.
        drop(broadcast_storages);
        // send_addr is on the stack so it drops automatically.

        ffi_result(err)?;

        Ok(DiscoveryService {
            raw,
            _log: log,
            _callback_data: callback_data,
        })
    }

    /// Send a wakeup packet to a console in standby mode.
    ///
    /// `user_credential` is the `rp_regist_key` from [`crate::regist::RegisteredHost`]
    /// interpreted as a 64-bit integer (use the bytes directly).
    pub fn wakeup(
        log: &Arc<Log>,
        host: &str,
        user_credential: u64,
        ps5: bool,
    ) -> Result<()> {
        let host_c = CString::new(host).unwrap_or_default();
        ffi_result(unsafe {
            sys::chiaki_discovery_wakeup(
                log.raw_ptr(),
                ptr::null_mut(), // create a temporary discovery internally
                host_c.as_ptr(),
                user_credential,
                ps5,
            )
        })
    }
}

impl Drop for DiscoveryService {
    fn drop(&mut self) {
        // Stops the C thread and joins it before our callback_data is freed.
        // SAFETY: raw is a valid, initialised ChiakiDiscoveryService.
        unsafe { sys::chiaki_discovery_service_fini(self.raw.as_mut()) };
        // _callback_data is dropped automatically after this point.
    }
}

// SAFETY: ChiakiDiscoveryService uses internal synchronisation.
// The Rust wrapper holds all referenced memory exclusively.
unsafe impl Send for DiscoveryService {}
