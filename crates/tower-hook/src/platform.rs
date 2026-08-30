//! Shared Windows process services.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW, GetModuleHandleW,
};

use crate::HookFailure;

// A full card-creation comparison can contain two long serial transactions.
// Keep the log bounded, but retain enough data for both reader sequences.
const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;

struct Logger {
    file: File,
}

static LOGGER: OnceLock<Option<Mutex<Logger>>> = OnceLock::new();

pub(crate) fn log(message: &str) {
    let logger = LOGGER.get_or_init(|| {
        let path = std::env::var_os("DRUAGA_SX32W_LOG").unwrap_or_else(|| "sx32w-shim.log".into());
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(|file| Mutex::new(Logger { file }))
    });
    let Some(logger) = logger else {
        return;
    };
    let mut logger = logger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let current_length = logger
        .file
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let remaining = MAX_LOG_BYTES.saturating_sub(current_length) as usize;
    if remaining == 0 {
        return;
    }
    let mut record = message.as_bytes().to_vec();
    record.push(b'\n');
    record.truncate(remaining);
    let _ = logger.file.write_all(&record);
}

pub(crate) fn tower_image() -> Result<HMODULE, HookFailure> {
    // SAFETY: A null module name requests the current executable image.
    let image = unsafe { GetModuleHandleW(std::ptr::null()) };
    if image.is_null() {
        return Err(HookFailure::Image);
    }
    Ok(image)
}

pub(crate) fn tower_hook_config_path() -> Result<PathBuf, HookFailure> {
    let mut module = std::ptr::null_mut();
    let flags =
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    // SAFETY: FROM_ADDRESS interprets this pointer as an address in this loaded DLL.
    let found = unsafe {
        GetModuleHandleExW(
            flags,
            crate::druaga_tower_hook_initialize as *const () as *const u16,
            &mut module,
        )
    };
    if found == 0 || module.is_null() {
        return Err(HookFailure::ModulePath);
    }

    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: The module handle is valid and the vector supplies writable storage.
    let length = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), 32_768) };
    let length = usize::try_from(length).map_err(|_| HookFailure::ModulePath)?;
    if length == 0 || length >= buffer.len() {
        return Err(HookFailure::ModulePath);
    }
    buffer.truncate(length);
    let mut path = PathBuf::from(std::ffi::OsString::from_wide(&buffer));
    path.set_file_name("tower-hook.toml");
    Ok(path)
}

#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;
