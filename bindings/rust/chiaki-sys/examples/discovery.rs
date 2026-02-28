use chiaki_sys::*;
use std::mem;

// Callback: called whenever the host list is discovered/updated
unsafe extern "C" fn discovery_cb(
    hosts: *mut ChiakiDiscoveryHost,
    hosts_count: usize,
    _user: *mut std::ffi::c_void,
) {
    for i in 0..hosts_count {
        unsafe {
            let host = &*hosts.add(i);
            let name = if host.host_name.is_null() {
                "<unknown>".to_string()
            } else {
                std::ffi::CStr::from_ptr(host.host_name)
                    .to_str()
                    .unwrap_or("<invalid>")
                    .to_string()
            };
            let addr = if host.host_addr.is_null() {
                "<unknown>".to_string()
            } else {
                std::ffi::CStr::from_ptr(host.host_addr)
                    .to_str()
                    .unwrap_or("<invalid>")
                    .to_string()
            };
            println!("Host: {} @ {}  state={}", name, addr, host.state);
        }
    }
}

fn main() {
    unsafe {
        // 1. Initialize log
        let mut log: ChiakiLog = mem::zeroed();
        chiaki_log_init(&mut log, CHIAKI_LOG_ALL, None, std::ptr::null_mut());

        // 2. Primary send address: 255.255.255.255 (IPv4 broadcast)
        //    send_addr is a required field; the service uses it to determine the address family and send target
        //    The port is filled in dynamically by the service when sending (PS4=987, PS5=9302)
        let mut send_addr: chiaki_sys::sockaddr_storage = mem::zeroed();
        {
            let sin = &mut send_addr as *mut _ as *mut chiaki_sys::sockaddr_in;
            (*sin).sin_family = libc::AF_INET as _;
            (*sin).sin_addr.s_addr = u32::MAX; // 255.255.255.255 = INADDR_BROADCAST
        }

        // 3. Configure DiscoveryService options
        let mut options: ChiakiDiscoveryServiceOptions = mem::zeroed();
        options.hosts_max = 16;
        options.host_drop_pings = 3;
        options.ping_ms = 2000;
        options.ping_initial_ms = 500;
        options.send_addr = &mut send_addr;
        options.send_addr_size = std::mem::size_of::<chiaki_sys::sockaddr_in>();
        // broadcast_addrs is an optional list of additional broadcast addresses (if not set, only send_addr is used)
        options.broadcast_addrs = std::ptr::null_mut();
        options.broadcast_num = 0;
        options.cb = Some(discovery_cb);
        options.cb_user = std::ptr::null_mut();

        // 4. Start the service (internally spawns a background thread to periodically broadcast and receive responses)
        let mut service: ChiakiDiscoveryService = mem::zeroed();
        let err = chiaki_discovery_service_init(&mut service, &mut options, &mut log);
        assert_eq!(
            err,
            ChiakiErrorCode_CHIAKI_ERR_SUCCESS,
            "chiaki_discovery_service_init failed: {}",
            err
        );

        println!("Scanning LAN for PS4/PS5 hosts (5 seconds)...");
        std::thread::sleep(std::time::Duration::from_secs(5));

        // 5. Cleanup
        chiaki_discovery_service_fini(&mut service);
    }
}
