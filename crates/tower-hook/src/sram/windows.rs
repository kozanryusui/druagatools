//! Tower x86 SRAM hook boundary.

#[cfg(target_arch = "x86")]
use std::ffi::c_void;

use crate::HookFailure;
use crate::hook::HookInstaller;
#[cfg(target_arch = "x86")]
use crate::platform;

#[cfg(target_arch = "x86")]
use super::storage::{SramRange, read_range, sram_path, write_range};

#[cfg(target_arch = "x86")]
type SramReadFn = unsafe extern "fastcall" fn(*mut c_void, *mut c_void, *mut u8, u32, u32) -> u8;
#[cfg(target_arch = "x86")]
type SramWriteFn = unsafe extern "fastcall" fn(*mut c_void, *mut c_void, *const u8, u32, u32);

#[cfg(target_arch = "x86")]
const SRAM_READ_RVA: usize = 0x0004_bda0;
#[cfg(target_arch = "x86")]
const SRAM_WRITE_RVA: usize = 0x0004_bd10;

#[cfg(target_arch = "x86")]
pub(crate) fn queue_hooks(installer: &mut HookInstaller) -> Result<(), HookFailure> {
    let image_base = installer.image_base();
    let read_target =
        image_base
            .checked_add(SRAM_READ_RVA)
            .ok_or_else(|| HookFailure::HookPrepare {
                hook: "sram-read",
                detail: "target address overflow".to_owned(),
            })?;
    let write_target =
        image_base
            .checked_add(SRAM_WRITE_RVA)
            .ok_or_else(|| HookFailure::HookPrepare {
                hook: "sram-write",
                detail: "target address overflow".to_owned(),
            })?;
    // SAFETY: The fastcall shims preserve the two confirmed x86 thiscall stack layouts.
    let read = unsafe { std::mem::transmute::<usize, SramReadFn>(read_target) };
    // SAFETY: The fastcall shims preserve the two confirmed x86 thiscall stack layouts.
    let write = unsafe { std::mem::transmute::<usize, SramWriteFn>(write_target) };
    // SAFETY: Each target and hook use the matching confirmed x86 ABI.
    let _original_read = unsafe { installer.inline("sram-read", read, hooked_sram_read)? };
    // SAFETY: Each target and hook use the matching confirmed x86 ABI.
    let _original_write = unsafe { installer.inline("sram-write", write, hooked_sram_write)? };
    Ok(())
}

#[cfg(target_arch = "x86")]
unsafe extern "fastcall" fn hooked_sram_read(
    _this: *mut c_void,
    _edx: *mut c_void,
    destination: *mut u8,
    offset: u32,
    length: u32,
) -> u8 {
    if destination.is_null() {
        return 0;
    }
    let Ok(range) = SramRange::from_raw(offset, length) else {
        return 0;
    };
    let Ok(buffer_length) = usize::try_from(length) else {
        return 0;
    };
    // SAFETY: Tower supplies one writable buffer of length bytes.
    let destination = unsafe { std::slice::from_raw_parts_mut(destination, buffer_length) };
    let result = sram_path().and_then(|path| read_range(&path, range, destination));
    match &result {
        Ok(()) => platform::log(&format!("sram-read-ok offset={offset:04x} length={length}")),
        Err(error) => platform::log(&format!(
            "sram-read-error offset={offset:04x} length={length} error={error:?}"
        )),
    }
    u8::from(result.is_ok())
}

#[cfg(target_arch = "x86")]
unsafe extern "fastcall" fn hooked_sram_write(
    _this: *mut c_void,
    _edx: *mut c_void,
    source: *const u8,
    offset: u32,
    length: u32,
) {
    if source.is_null() {
        return;
    }
    let Ok(range) = SramRange::from_raw(offset, length) else {
        return;
    };
    let Ok(buffer_length) = usize::try_from(length) else {
        return;
    };
    // SAFETY: Tower supplies one readable buffer of length bytes.
    let source = unsafe { std::slice::from_raw_parts(source, buffer_length) };
    let result = sram_path().and_then(|path| write_range(&path, range, source));
    match result {
        Ok(()) => platform::log(&format!(
            "sram-write-ok offset={offset:04x} length={length}"
        )),
        Err(error) => platform::log(&format!(
            "sram-write-error offset={offset:04x} length={length} error={error:?}"
        )),
    }
}

#[cfg(not(target_arch = "x86"))]
pub(crate) fn queue_hooks(_installer: &mut HookInstaller) -> Result<(), HookFailure> {
    Err(HookFailure::HookPrepare {
        hook: "sram",
        detail: "Tower hooks require the 32-bit x86 target".to_owned(),
    })
}
