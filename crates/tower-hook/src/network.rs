//! Tower network compatibility hooks.

#[cfg(windows)]
mod adapter;

#[cfg(windows)]
mod windows_hook {
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::net::Ipv4Addr;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{HANDLE, HWND};

    use super::adapter::{
        broadcast, log_adapter_snapshot, query_default_gateways, query_dns_servers,
        query_primary_adapter, write_ipv4_sockaddr,
    };
    use crate::config::{AdapterLookup, DnsTarget, NetworkConfig, RouterCheckMode};
    use crate::hook::HookInstaller;
    use crate::{HookFailure, platform};

    const LEGACY_IPV4_READER_RVA: usize = 0x000a_3d00;
    const LEGACY_INTERFACE_QUERY_RVA: usize = 0x0009_8830;
    const PLAIN_HTTP_CONNECT_RVA: usize = 0x000a_a840;
    const ASYNC_HOST_LOOKUP_IAT_RVA: usize = 0x000d_336c;
    const TENPO_ROUTER_CHECK_RVA: usize = 0x0009_9aa0;
    const CONTENTS_ROUTER_CHECK_RVA: usize = 0x0009_9b50;
    const INTERFACE_INFO_SIZE: usize = 0x4c;
    const IP_REQ_TIMED_OUT: i32 = 11010;

    type LegacyIpv4ReaderFn =
        unsafe extern "fastcall" fn(*mut c_void, usize, *mut u8, i32, *const c_char) -> i32;
    type AsyncHostLookupFn =
        unsafe extern "system" fn(HWND, u32, *const u8, *mut u8, i32) -> HANDLE;
    type LegacyInterfaceQueryFn = unsafe extern "system" fn(*mut u8, u32) -> u32;
    type PlainHttpConnectFn = unsafe extern "C" fn(*mut u8) -> i32;
    type RouterCheckFn = unsafe extern "fastcall" fn(*mut u8, usize) -> u8;

    static ORIGINAL_LEGACY_IPV4_READER: OnceLock<LegacyIpv4ReaderFn> = OnceLock::new();
    static ORIGINAL_ASYNC_HOST_LOOKUP: OnceLock<AsyncHostLookupFn> = OnceLock::new();
    static ORIGINAL_LEGACY_INTERFACE_QUERY: OnceLock<LegacyInterfaceQueryFn> = OnceLock::new();
    static ORIGINAL_PLAIN_HTTP_CONNECT: OnceLock<PlainHttpConnectFn> = OnceLock::new();
    static NETWORK_CONFIG: OnceLock<RuntimeNetworkConfig> = OnceLock::new();

    #[derive(Debug)]
    struct RuntimeNetworkConfig {
        adapter: AdapterLookup,
        router_checks: RouterCheckMode,
        power_on_port: Option<std::num::NonZeroU16>,
        dns_overrides: Vec<RuntimeDnsOverride>,
    }

    #[derive(Debug)]
    struct RuntimeDnsOverride {
        source: Vec<u8>,
        target: CString,
    }

    pub(crate) fn configure(config: NetworkConfig) -> Result<(), HookFailure> {
        let runtime = RuntimeNetworkConfig::new(config)?;
        NETWORK_CONFIG
            .set(runtime)
            .map_err(|_| HookFailure::RuntimeState("network configuration"))?;
        if NETWORK_CONFIG
            .get()
            .is_some_and(|config| config.adapter == AdapterLookup::Dynamic)
        {
            log_adapter_snapshot("DefaultGateway", query_default_gateways());
            log_adapter_snapshot("NameServer", query_dns_servers());
            match query_primary_adapter() {
                Ok(adapter) => platform::log(&format!(
                    "network-adapter-primary address={} netmask={} gateway={}",
                    adapter.address,
                    adapter.netmask,
                    adapter
                        .gateway
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "none".to_owned())
                )),
                Err(error) => {
                    platform::log(&format!("network-adapter-primary-failed error={error}"))
                }
            }
        }

        Ok(())
    }

    pub(crate) fn queue_hooks(installer: &mut HookInstaller) -> Result<(), HookFailure> {
        queue_legacy_ipv4_reader(installer)?;
        queue_legacy_interface_query(installer)?;
        queue_plain_http_connect(installer)?;
        queue_async_host_lookup(installer)?;
        queue_router_checks(installer)
    }

    impl RuntimeNetworkConfig {
        fn new(config: NetworkConfig) -> Result<Self, HookFailure> {
            let mut dns_overrides = Vec::with_capacity(config.dns_overrides.len());
            for entry in config.dns_overrides {
                let target = match entry.target {
                    DnsTarget::Domain(domain) => domain,
                    DnsTarget::Ipv4(ip) => ip.to_string(),
                };
                let target = CString::new(target).map_err(|_| {
                    HookFailure::Config("a DNS replacement contains a null byte".to_owned())
                })?;
                dns_overrides.push(RuntimeDnsOverride {
                    source: entry.source.into_bytes(),
                    target,
                });
            }
            Ok(Self {
                adapter: config.adapter,
                router_checks: config.router_checks,
                power_on_port: config.power_on_port,
                dns_overrides,
            })
        }

        fn replacement_for(&self, source: &CStr) -> Option<&CStr> {
            let source = source.to_bytes();
            self.dns_overrides
                .iter()
                .find(|entry| entry.source.eq_ignore_ascii_case(source))
                .map(|entry| entry.target.as_c_str())
        }
    }

    fn queue_router_checks(installer: &mut HookInstaller) -> Result<(), HookFailure> {
        if NETWORK_CONFIG
            .get()
            .is_none_or(|config| config.router_checks == RouterCheckMode::Original)
        {
            return Ok(());
        }
        queue_router_check(
            installer,
            "tenpo-router-check",
            TENPO_ROUTER_CHECK_RVA,
            hooked_tenpo_router_check,
        )?;
        queue_router_check(
            installer,
            "contents-router-check",
            CONTENTS_ROUTER_CHECK_RVA,
            hooked_contents_router_check,
        )
    }

    fn queue_router_check(
        installer: &mut HookInstaller,
        name: &'static str,
        target_rva: usize,
        hook: RouterCheckFn,
    ) -> Result<(), HookFailure> {
        let target_address = installer
            .image_base()
            .checked_add(target_rva)
            .ok_or_else(|| HookFailure::HookPrepare {
                hook: name,
                detail: "target address overflow".to_owned(),
            })?;
        // SAFETY: The selected Tower image has the confirmed thiscall wrapper at this RVA.
        let target = unsafe { std::mem::transmute::<usize, RouterCheckFn>(target_address) };
        // SAFETY: The fastcall shim carries the this pointer in ECX and the unused EDX value.
        let _original = unsafe { installer.inline(name, target, hook)? };
        Ok(())
    }

    unsafe extern "fastcall" fn hooked_tenpo_router_check(this: *mut u8, _edx: usize) -> u8 {
        complete_router_check(this, "tenpo")
    }

    unsafe extern "fastcall" fn hooked_contents_router_check(this: *mut u8, _edx: usize) -> u8 {
        complete_router_check(this, "contents")
    }

    fn complete_router_check(this: *mut u8, name: &str) -> u8 {
        if this.is_null() {
            platform::log(&format!("network-router-check-invalid name={name}"));
            return 0;
        }
        let result = match NETWORK_CONFIG.get().map(|config| config.router_checks) {
            Some(RouterCheckMode::Disconnected) => IP_REQ_TIMED_OUT,
            Some(RouterCheckMode::Emulated) => 0,
            Some(RouterCheckMode::Original) | None => {
                platform::log(&format!("network-router-check-invalid-mode name={name}"));
                return 0;
            }
        };
        // SAFETY: Both confirmed wrappers receive the network manager as their this pointer.
        // Offset 0x194 is the signed ICMP result field. Zero is success. 11010 is a timeout.
        unsafe { this.add(0x194).cast::<i32>().write(result) };
        platform::log(&format!(
            "network-router-check-emulated name={name} result={result}"
        ));
        1
    }

    fn queue_legacy_ipv4_reader(installer: &mut HookInstaller) -> Result<(), HookFailure> {
        let target_address = installer
            .image_base()
            .checked_add(LEGACY_IPV4_READER_RVA)
            .ok_or_else(|| HookFailure::HookPrepare {
                hook: "legacy-ipv4-reader",
                detail: "target address overflow".to_owned(),
            })?;
        // SAFETY: The selected Tower image has the confirmed thiscall method at this RVA.
        let target = unsafe { std::mem::transmute::<usize, LegacyIpv4ReaderFn>(target_address) };
        // SAFETY: The fastcall shim carries ECX, EDX, and the three stack arguments correctly.
        let original =
            unsafe { installer.inline("legacy-ipv4-reader", target, hooked_legacy_ipv4_reader)? };
        ORIGINAL_LEGACY_IPV4_READER
            .set(original)
            .map_err(|_| HookFailure::RuntimeState("legacy IPv4 reader original"))
    }

    fn queue_async_host_lookup(installer: &mut HookInstaller) -> Result<(), HookFailure> {
        let target_address = installer.ordinal_iat(
            "wsa-async-get-host-by-name",
            ASYNC_HOST_LOOKUP_IAT_RVA,
            hooked_async_host_lookup as *const u8,
        )?;
        // SAFETY: The IAT slot contains WSAAsyncGetHostByName with the documented system ABI.
        let original = unsafe { std::mem::transmute::<usize, AsyncHostLookupFn>(target_address) };
        ORIGINAL_ASYNC_HOST_LOOKUP
            .set(original)
            .map_err(|_| HookFailure::RuntimeState("async host lookup original"))
    }

    fn queue_legacy_interface_query(installer: &mut HookInstaller) -> Result<(), HookFailure> {
        let target_address = installer
            .image_base()
            .checked_add(LEGACY_INTERFACE_QUERY_RVA)
            .ok_or_else(|| HookFailure::HookPrepare {
                hook: "legacy-interface-query",
                detail: "target address overflow".to_owned(),
            })?;
        // SAFETY: The selected Tower image has the confirmed stdcall query at this RVA.
        let target =
            unsafe { std::mem::transmute::<usize, LegacyInterfaceQueryFn>(target_address) };
        // SAFETY: The target and replacement use the same two-argument stdcall ABI.
        let original = unsafe {
            installer.inline(
                "legacy-interface-query",
                target,
                hooked_legacy_interface_query,
            )?
        };
        ORIGINAL_LEGACY_INTERFACE_QUERY
            .set(original)
            .map_err(|_| HookFailure::RuntimeState("legacy interface query original"))
    }

    fn queue_plain_http_connect(installer: &mut HookInstaller) -> Result<(), HookFailure> {
        if NETWORK_CONFIG
            .get()
            .and_then(|config| config.power_on_port)
            .is_none()
        {
            return Ok(());
        }
        let target_address = installer
            .image_base()
            .checked_add(PLAIN_HTTP_CONNECT_RVA)
            .ok_or_else(|| HookFailure::HookPrepare {
                hook: "plain-http-connect",
                detail: "target address overflow".to_owned(),
            })?;
        // SAFETY: The selected Tower image has the confirmed cdecl function at this RVA.
        let target = unsafe { std::mem::transmute::<usize, PlainHttpConnectFn>(target_address) };
        // SAFETY: The target and replacement use the same one-argument cdecl ABI.
        let original =
            unsafe { installer.inline("plain-http-connect", target, hooked_plain_http_connect)? };
        ORIGINAL_PLAIN_HTTP_CONNECT
            .set(original)
            .map_err(|_| HookFailure::RuntimeState("plain HTTP connect original"))
    }

    unsafe extern "fastcall" fn hooked_legacy_ipv4_reader(
        this: *mut c_void,
        edx: usize,
        output: *mut u8,
        capacity: i32,
        value_name: *const c_char,
    ) -> i32 {
        let Some(original) = ORIGINAL_LEGACY_IPV4_READER.get() else {
            return 0;
        };
        let Some(config) = NETWORK_CONFIG.get() else {
            // SAFETY: The arguments are unchanged from the selected Tower call.
            return unsafe { original(this, edx, output, capacity, value_name) };
        };
        if config.adapter == AdapterLookup::Original
            || output.is_null()
            || value_name.is_null()
            || capacity <= 0
        {
            // SAFETY: The arguments are unchanged from the selected Tower call.
            return unsafe { original(this, edx, output, capacity, value_name) };
        }

        // SAFETY: The selected Tower method requires a null-terminated registry value name.
        let value_name = unsafe { CStr::from_ptr(value_name) };
        let addresses = if value_name
            .to_bytes()
            .eq_ignore_ascii_case(b"DefaultGateway")
        {
            query_default_gateways()
        } else if value_name.to_bytes().eq_ignore_ascii_case(b"NameServer") {
            query_dns_servers()
        } else {
            // SAFETY: The arguments are unchanged from the selected Tower call.
            return unsafe { original(this, edx, output, capacity, value_name.as_ptr()) };
        };

        let addresses = match addresses {
            Ok(addresses) => addresses,
            Err(error) => {
                platform::log(&format!(
                    "network-adapter-query-failed value={} error={error}",
                    value_name.to_string_lossy()
                ));
                // SAFETY: The arguments are unchanged from the selected Tower call.
                return unsafe { original(this, edx, output, capacity, value_name.as_ptr()) };
            }
        };
        let count = addresses.len().min(capacity as usize);
        let selected = addresses
            .iter()
            .take(count)
            .map(Ipv4Addr::to_string)
            .collect::<Vec<_>>()
            .join(",");
        for (index, address) in addresses.into_iter().take(count).enumerate() {
            // SAFETY: The Tower supplies capacity writable four-byte entries.
            unsafe {
                output
                    .add(index * 4)
                    .copy_from_nonoverlapping(address.octets().as_ptr(), 4)
            };
        }
        platform::log(&format!(
            "network-adapter-query value={} count={count} addresses={selected}",
            value_name.to_string_lossy(),
        ));
        count as i32
    }

    unsafe extern "system" fn hooked_async_host_lookup(
        window: HWND,
        message: u32,
        name: *const u8,
        buffer: *mut u8,
        buffer_length: i32,
    ) -> HANDLE {
        let Some(original) = ORIGINAL_ASYNC_HOST_LOOKUP.get() else {
            return std::ptr::null_mut();
        };
        let Some(config) = NETWORK_CONFIG.get() else {
            // SAFETY: The arguments are unchanged from the Winsock call.
            return unsafe { original(window, message, name, buffer, buffer_length) };
        };
        if name.is_null() {
            // SAFETY: The arguments are unchanged from the Winsock call.
            return unsafe { original(window, message, name, buffer, buffer_length) };
        }

        // SAFETY: WSAAsyncGetHostByName requires a null-terminated host name.
        let source = unsafe { CStr::from_ptr(name.cast()) };
        let Some(target) = config.replacement_for(source) else {
            // SAFETY: The arguments are unchanged from the Winsock call.
            return unsafe { original(window, message, name, buffer, buffer_length) };
        };
        platform::log(&format!(
            "dns-redirect source={} target={}",
            source.to_string_lossy(),
            target.to_string_lossy()
        ));
        // SAFETY: The replacement is a stable null-terminated string for this call.
        unsafe {
            original(
                window,
                message,
                target.as_ptr().cast(),
                buffer,
                buffer_length,
            )
        }
    }

    unsafe extern "system" fn hooked_legacy_interface_query(output: *mut u8, capacity: u32) -> u32 {
        let Some(original) = ORIGINAL_LEGACY_INTERFACE_QUERY.get() else {
            return 0;
        };
        let dynamic = NETWORK_CONFIG
            .get()
            .is_some_and(|config| config.adapter == AdapterLookup::Dynamic);
        if !dynamic || output.is_null() || capacity < INTERFACE_INFO_SIZE as u32 {
            // SAFETY: The arguments are unchanged from the selected Tower call.
            return unsafe { original(output, capacity) };
        }
        let adapter = match query_primary_adapter() {
            Ok(adapter) => adapter,
            Err(error) => {
                platform::log(&format!("network-interface-query-failed error={error}"));
                // SAFETY: The arguments are unchanged from the selected Tower call.
                return unsafe { original(output, capacity) };
            }
        };

        // INTERFACE_INFO is one flags field and three 24-byte SOCKADDR_GEN values.
        // SAFETY: The caller supplies at least one complete writable INTERFACE_INFO record.
        unsafe { output.write_bytes(0, INTERFACE_INFO_SIZE) };
        // IFF_UP makes the Tower classify this record as an active IPv4 interface.
        // SAFETY: All writes stay within the complete record checked above.
        unsafe { output.cast::<u32>().write_unaligned(1) };
        unsafe { write_ipv4_sockaddr(output.add(4), adapter.address) };
        unsafe { write_ipv4_sockaddr(output.add(28), broadcast(adapter.address, adapter.netmask)) };
        unsafe { write_ipv4_sockaddr(output.add(52), adapter.netmask) };
        platform::log(&format!(
            "network-interface-query address={} netmask={}",
            adapter.address, adapter.netmask
        ));
        INTERFACE_INFO_SIZE as u32
    }

    unsafe extern "C" fn hooked_plain_http_connect(client: *mut u8) -> i32 {
        let Some(original) = ORIGINAL_PLAIN_HTTP_CONNECT.get() else {
            return -1;
        };
        let Some(target_port) = NETWORK_CONFIG.get().and_then(|config| config.power_on_port) else {
            // SAFETY: The argument is unchanged from the selected Tower call.
            return unsafe { original(client) };
        };
        if !client.is_null() {
            // The sockaddr_in starts at +0x14. Its network-order port is at +0x16.
            // SAFETY: The selected function requires a client object with this complete field.
            let port_field = unsafe { client.add(0x16).cast::<u16>() };
            // SAFETY: The field can be unaligned and contains one initialized u16 value.
            let original_port = u16::from_be(unsafe { port_field.read_unaligned() });
            if original_port == 80 {
                // SAFETY: The field can be unaligned and remains valid for the original call.
                unsafe { port_field.write_unaligned(target_port.get().to_be()) };
                platform::log(&format!(
                    "power-on-port-redirect source=80 target={}",
                    target_port.get()
                ));
            }
        }
        // SAFETY: The client object remains valid for the original Tower function.
        unsafe { original(client) }
    }
}

#[cfg(windows)]
pub(crate) use windows_hook::{configure, queue_hooks};
