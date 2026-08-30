//! Sentinel SuperPro entry points used by the Tower executable.

use crate::bootstrap_succeeded;
use crate::logging::log;
use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

const SUCCESS: i16 = 0;
const INVALID_ARGUMENT: i16 = 1;
const EXPECTED_PACKET_SIZE: u16 = 0x0404;
const EXPECTED_DEVELOPER_ID: u16 = 0x557f;
const PRODUCT_WORD_ADDRESS: u16 = 8;
const PRODUCT_WORD_VALUE: u16 = 0x0324;
const CABINET_WORD_ADDRESS: u16 = 0;
const DEFAULT_CABINET_ID: u16 = 1;

static CABINET_ID: OnceLock<u16> = OnceLock::new();

fn cabinet_id() -> u16 {
    *CABINET_ID.get_or_init(|| {
        std::env::var("DRUAGA_CABINET_ID")
            .ok()
            .and_then(|value| parse_u16(&value))
            .unwrap_or(DEFAULT_CABINET_ID)
    })
}

fn parse_u16(value: &str) -> Option<u16> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u16::from_str_radix(hex, 16).ok()
    } else {
        trimmed.parse::<u16>().ok()
    }
}

fn packet_is_valid(packet: *mut c_void) -> bool {
    !packet.is_null()
}

/// Initializes the caller-owned Sentinel packet.
#[unsafe(no_mangle)]
pub extern "system" fn RNBOsproFormatPacket(packet: *mut c_void, packet_size: u16) -> i16 {
    log(&format!(
        "RNBOsproFormatPacket packet_size=0x{packet_size:04X}"
    ));
    if !bootstrap_succeeded() || !packet_is_valid(packet) || packet_size != EXPECTED_PACKET_SIZE {
        return INVALID_ARGUMENT;
    }
    SUCCESS
}

/// Starts the emulated standalone Sentinel session.
#[unsafe(no_mangle)]
pub extern "system" fn RNBOsproInitialize(packet: *mut c_void) -> i16 {
    log("RNBOsproInitialize");
    if !bootstrap_succeeded() || !packet_is_valid(packet) {
        return INVALID_ARGUMENT;
    }
    SUCCESS
}

/// Accepts the standalone contact-server mode used by the Tower.
///
/// # Safety
///
/// If `server` is not null, it must point to a valid null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn RNBOsproSetContactServer(
    packet: *mut c_void,
    server: *const c_char,
) -> i16 {
    if !bootstrap_succeeded() || !packet_is_valid(packet) || server.is_null() {
        log("RNBOsproSetContactServer invalid_argument");
        return INVALID_ARGUMENT;
    }

    // SAFETY: The caller contract requires a valid null-terminated string.
    let server = unsafe { CStr::from_ptr(server) };
    log(&format!(
        "RNBOsproSetContactServer server={}",
        server.to_string_lossy()
    ));
    SUCCESS
}

/// Selects the Sentinel unit that the Tower requests.
#[unsafe(no_mangle)]
pub extern "system" fn RNBOsproFindFirstUnit(packet: *mut c_void, developer_id: u16) -> i16 {
    log(&format!(
        "RNBOsproFindFirstUnit developer_id=0x{developer_id:04X}"
    ));
    if !bootstrap_succeeded() || !packet_is_valid(packet) || developer_id != EXPECTED_DEVELOPER_ID {
        return INVALID_ARGUMENT;
    }
    SUCCESS
}

/// Reads one emulated 16-bit Sentinel word.
///
/// # Safety
///
/// `output` must point to writable memory for one `u16` value.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn RNBOsproRead(
    packet: *mut c_void,
    address: u16,
    output: *mut u16,
) -> i16 {
    if !bootstrap_succeeded() || !packet_is_valid(packet) || output.is_null() {
        log("RNBOsproRead invalid_argument");
        return INVALID_ARGUMENT;
    }

    let value = match address {
        CABINET_WORD_ADDRESS => cabinet_id(),
        PRODUCT_WORD_ADDRESS => PRODUCT_WORD_VALUE,
        _ => 0,
    };

    // SAFETY: The caller contract requires writable memory for one u16 value.
    unsafe { output.write(value) };
    log(&format!(
        "RNBOsproRead address={address} value=0x{value:04X}"
    ));
    SUCCESS
}

/// Ends the emulated Sentinel session.
#[unsafe(no_mangle)]
pub extern "system" fn RNBOsproReleaseLicense(
    packet: *mut c_void,
    _unit: u16,
    _reserved: u16,
) -> i16 {
    log("RNBOsproReleaseLicense");
    if !bootstrap_succeeded() || !packet_is_valid(packet) {
        return INVALID_ARGUMENT;
    }
    SUCCESS
}
