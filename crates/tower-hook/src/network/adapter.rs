//! Windows IPv4 adapter discovery and Tower record conversion.

use std::mem::size_of;
use std::net::Ipv4Addr;

use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FIXED_INFO_W2KSP1, GetAdaptersInfo, GetNetworkParams, IP_ADAPTER_INFO, IP_ADDR_STRING,
    IP_ADDRESS_STRING,
};

use crate::platform;

const MAX_LINKED_ITEMS: usize = 256;

#[derive(Clone, Copy, Debug)]
pub(super) struct PrimaryAdapter {
    pub(super) address: Ipv4Addr,
    pub(super) netmask: Ipv4Addr,
    pub(super) gateway: Option<Ipv4Addr>,
}

#[derive(Debug)]
pub(super) enum AdapterQueryError {
    Api(u32),
    Empty,
    BufferSize,
}

impl std::fmt::Display for AdapterQueryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api(status) => write!(formatter, "Windows status {status}"),
            Self::Empty => formatter.write_str("no usable IPv4 address"),
            Self::BufferSize => formatter.write_str("invalid API buffer size"),
        }
    }
}

pub(super) fn query_default_gateways() -> Result<Vec<Ipv4Addr>, AdapterQueryError> {
    let adapter = query_primary_adapter()?;
    adapter
        .gateway
        .map(|value| vec![value])
        .ok_or(AdapterQueryError::Empty)
}

pub(super) fn query_primary_adapter() -> Result<PrimaryAdapter, AdapterQueryError> {
    let mut byte_count = 0_u32;
    // SAFETY: A null buffer requests the required size.
    let first = unsafe { GetAdaptersInfo(std::ptr::null_mut(), &mut byte_count) };
    if first != ERROR_BUFFER_OVERFLOW && first != ERROR_SUCCESS {
        return Err(AdapterQueryError::Api(first));
    }
    let mut storage = aligned_storage(byte_count)?;
    let adapters = storage.as_mut_ptr().cast::<IP_ADAPTER_INFO>();
    // SAFETY: Storage is aligned and has at least byte_count writable bytes.
    let status = unsafe { GetAdaptersInfo(adapters, &mut byte_count) };
    if status != ERROR_SUCCESS {
        return Err(AdapterQueryError::Api(status));
    }

    let mut fallback = None;
    let mut adapter = adapters;
    for _ in 0..MAX_LINKED_ITEMS {
        if adapter.is_null() {
            break;
        }
        // SAFETY: GetAdaptersInfo created this linked adapter record.
        let record = unsafe { &*adapter };
        let gateway = first_address(&record.GatewayList);
        let mut address_record = &record.IpAddressList as *const IP_ADDR_STRING;
        for _ in 0..MAX_LINKED_ITEMS {
            if address_record.is_null() {
                break;
            }
            // SAFETY: GetAdaptersInfo created this linked address record.
            let address_entry = unsafe { &*address_record };
            if let (Some(address), Some(netmask)) = (
                parse_ipv4(&address_entry.IpAddress),
                parse_ipv4(&address_entry.IpMask),
            ) && !address.is_unspecified()
                && !address.is_loopback()
                && !netmask.is_unspecified()
            {
                let candidate = PrimaryAdapter {
                    address,
                    netmask,
                    gateway,
                };
                if gateway.is_some() {
                    return Ok(candidate);
                }
                fallback.get_or_insert(candidate);
            }
            address_record = address_entry.Next;
        }
        adapter = record.Next;
    }
    fallback.ok_or(AdapterQueryError::Empty)
}

pub(super) fn query_dns_servers() -> Result<Vec<Ipv4Addr>, AdapterQueryError> {
    let mut byte_count = 0_u32;
    // SAFETY: A null buffer requests the required size.
    let first = unsafe { GetNetworkParams(std::ptr::null_mut(), &mut byte_count) };
    if first != ERROR_BUFFER_OVERFLOW && first != ERROR_SUCCESS {
        return Err(AdapterQueryError::Api(first));
    }
    let mut storage = aligned_storage(byte_count)?;
    let fixed_info = storage.as_mut_ptr().cast::<FIXED_INFO_W2KSP1>();
    // SAFETY: Storage is aligned and has at least byte_count writable bytes.
    let status = unsafe { GetNetworkParams(fixed_info, &mut byte_count) };
    if status != ERROR_SUCCESS {
        return Err(AdapterQueryError::Api(status));
    }
    let mut result = Vec::new();
    // SAFETY: GetNetworkParams initialized the fixed record and its list.
    append_address_list(unsafe { &(*fixed_info).DnsServerList }, &mut result);
    result.sort_unstable();
    result.dedup();
    if result.is_empty() {
        Err(AdapterQueryError::Empty)
    } else {
        Ok(result)
    }
}

pub(super) fn log_adapter_snapshot(
    value_name: &str,
    result: Result<Vec<Ipv4Addr>, AdapterQueryError>,
) {
    match result {
        Ok(addresses) => {
            let addresses = addresses
                .iter()
                .map(Ipv4Addr::to_string)
                .collect::<Vec<_>>()
                .join(",");
            platform::log(&format!(
                "network-adapter-snapshot value={value_name} addresses={addresses}"
            ));
        }
        Err(error) => platform::log(&format!(
            "network-adapter-snapshot-failed value={value_name} error={error}"
        )),
    }
}

pub(super) unsafe fn write_ipv4_sockaddr(output: *mut u8, address: Ipv4Addr) {
    // SAFETY: The caller provides one writable 24-byte SOCKADDR_GEN field.
    unsafe { output.cast::<u16>().write_unaligned(2) };
    // SAFETY: IPv4 bytes start at offset four in the SOCKADDR_IN view.
    unsafe {
        output
            .add(4)
            .copy_from_nonoverlapping(address.octets().as_ptr(), 4)
    };
}

pub(super) fn broadcast(address: Ipv4Addr, netmask: Ipv4Addr) -> Ipv4Addr {
    Ipv4Addr::from(u32::from(address) | !u32::from(netmask))
}

fn aligned_storage(byte_count: u32) -> Result<Vec<u64>, AdapterQueryError> {
    let bytes = usize::try_from(byte_count).map_err(|_| AdapterQueryError::BufferSize)?;
    if bytes == 0 {
        return Err(AdapterQueryError::BufferSize);
    }
    let words = bytes
        .checked_add(size_of::<u64>() - 1)
        .ok_or(AdapterQueryError::BufferSize)?
        / size_of::<u64>();
    Ok(vec![0_u64; words])
}

fn append_address_list(first: &IP_ADDR_STRING, output: &mut Vec<Ipv4Addr>) {
    let mut current = first as *const IP_ADDR_STRING;
    for _ in 0..MAX_LINKED_ITEMS {
        if current.is_null() {
            break;
        }
        // SAFETY: The API initialized the linked address record.
        let record = unsafe { &*current };
        if let Some(address) = parse_ipv4(&record.IpAddress)
            && !address.is_unspecified()
        {
            output.push(address);
        }
        current = record.Next;
    }
}

fn first_address(first: &IP_ADDR_STRING) -> Option<Ipv4Addr> {
    let mut current = first as *const IP_ADDR_STRING;
    for _ in 0..MAX_LINKED_ITEMS {
        if current.is_null() {
            break;
        }
        // SAFETY: The API initialized the linked address record.
        let record = unsafe { &*current };
        if let Some(address) = parse_ipv4(&record.IpAddress)
            && !address.is_unspecified()
        {
            return Some(address);
        }
        current = record.Next;
    }
    None
}

fn parse_ipv4(value: &IP_ADDRESS_STRING) -> Option<Ipv4Addr> {
    let bytes: Vec<u8> = value
        .String
        .iter()
        .take_while(|unit| **unit != 0)
        .map(|unit| *unit as u8)
        .collect();
    std::str::from_utf8(&bytes).ok()?.parse().ok()
}
