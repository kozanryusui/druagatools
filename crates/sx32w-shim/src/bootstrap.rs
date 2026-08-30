//! Windows bootstrap that loads `tower-hook.dll` outside the loader lock.

use crate::logging::log;
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering};
use windows_sys::Win32::Foundation::{HINSTANCE, HWND};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleW, GetProcAddress, LoadLibraryW,
};
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_sys::Win32::UI::WindowsAndMessaging::{HMENU, MB_ICONERROR, MB_OK, MessageBoxW};

type CreateWindowExAFn = unsafe extern "system" fn(
    u32,
    *const c_char,
    *const c_char,
    u32,
    i32,
    i32,
    i32,
    i32,
    HWND,
    HMENU,
    HINSTANCE,
    *const c_void,
) -> HWND;
type InitializeHookFn = unsafe extern "system" fn() -> i16;

const TOWER_TIMESTAMP: u32 = 0x48bb_48f1;
const TOWER_IMAGE_SIZE: u32 = 0x0012_6000;
const TOWER_ENTRY_POINT_RVA: u32 = 0x000b_1fe0;
const TOWER_PE_HEADER_OFFSET: usize = 0x130;
const CREATE_WINDOW_IAT_RVA: usize = 0x000d_3234;
const STATE_UNARMED: u8 = 0;
const STATE_ARMED: u8 = 1;
const STATE_BUSY: u8 = 2;
const STATE_COMPLETE: u8 = 3;
const STATE_FAILED: u8 = 4;
const MAX_MODULE_PATH: usize = 1024;

static BOOTSTRAP_FAILED: AtomicBool = AtomicBool::new(false);
static BOOTSTRAP_STATE: AtomicU8 = AtomicU8::new(STATE_UNARMED);
static ORIGINAL_CREATE_WINDOW: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static CREATE_WINDOW_SLOT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

pub(crate) fn succeeded() -> bool {
    !BOOTSTRAP_FAILED.load(Ordering::Acquire)
}

/// Installs the selected Tower bootstrap during process attachment.
///
/// # Safety
///
/// The Windows loader must call this function with the documented `DllMain` arguments.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    _instance: HINSTANCE,
    reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    if reason != DLL_PROCESS_ATTACH {
        return 1;
    }

    // SAFETY: The loader has mapped the process image and holds loader lock.
    let _installed = unsafe { install_redirect() };
    1
}

unsafe fn install_redirect() -> bool {
    // SAFETY: A null module name requests the mapped process image.
    let image = unsafe { GetModuleHandleW(std::ptr::null()) };
    if image.is_null() {
        return false;
    }
    let base = image.cast::<u8>();
    // SAFETY: The process image starts with a mapped DOS header.
    if unsafe { read_u16(base, 0) } != Some(0x5a4d) {
        return false;
    }
    let Some(pe_offset) = (unsafe { read_u32(base, 0x3c) }).map(|value| value as usize) else {
        return false;
    };
    if pe_offset != TOWER_PE_HEADER_OFFSET {
        return false;
    }
    // SAFETY: The DOS header supplies the mapped PE header offset.
    if unsafe { read_u32(base, pe_offset) } != Some(0x0000_4550)
        || unsafe { read_u16(base, pe_offset + 4) } != Some(0x014c)
        || unsafe { read_u32(base, pe_offset + 8) } != Some(TOWER_TIMESTAMP)
        || unsafe { read_u16(base, pe_offset + 24) } != Some(0x010b)
        || unsafe { read_u32(base, pe_offset + 40) } != Some(TOWER_ENTRY_POINT_RVA)
        || unsafe { read_u32(base, pe_offset + 80) } != Some(TOWER_IMAGE_SIZE)
    {
        return false;
    }

    // SAFETY: The exact image markers establish this fixed IAT slot location.
    let slot = unsafe { base.add(CREATE_WINDOW_IAT_RVA) }.cast::<*mut c_void>();
    // SAFETY: The selected image has one writable pointer-sized IAT entry at this RVA.
    let original = unsafe { slot.read() };
    if original.is_null() {
        return false;
    }

    ORIGINAL_CREATE_WINDOW.store(original, Ordering::Release);
    CREATE_WINDOW_SLOT.store(slot.cast::<c_void>(), Ordering::Release);
    // SAFETY: The selected image markers establish the IAT slot contract.
    if !unsafe { write_slot(slot, hooked_create_window_ex_a as *mut c_void) } {
        ORIGINAL_CREATE_WINDOW.store(std::ptr::null_mut(), Ordering::Release);
        CREATE_WINDOW_SLOT.store(std::ptr::null_mut(), Ordering::Release);
        return false;
    }
    BOOTSTRAP_STATE.store(STATE_ARMED, Ordering::Release);
    true
}

unsafe fn read_u16(base: *const u8, offset: usize) -> Option<u16> {
    // SAFETY: The caller supplies a mapped image address and a header offset.
    Some(unsafe { base.add(offset).cast::<u16>().read_unaligned() })
}

unsafe fn read_u32(base: *const u8, offset: usize) -> Option<u32> {
    // SAFETY: The caller supplies a mapped image address and a header offset.
    Some(unsafe { base.add(offset).cast::<u32>().read_unaligned() })
}

unsafe fn write_slot(slot: *mut *mut c_void, value: *mut c_void) -> bool {
    let mut previous = 0_u32;
    // SAFETY: The selected image markers establish a pointer-sized IAT slot.
    if unsafe {
        VirtualProtect(
            slot.cast::<c_void>(),
            size_of::<*mut c_void>(),
            PAGE_READWRITE,
            &mut previous,
        )
    } == 0
    {
        return false;
    }
    // SAFETY: VirtualProtect made the pointer-sized IAT slot writable.
    unsafe { slot.write(value) };
    let mut ignored = 0_u32;
    // SAFETY: Restore the protection returned by the first VirtualProtect call.
    let _restored = unsafe {
        VirtualProtect(
            slot.cast::<c_void>(),
            size_of::<*mut c_void>(),
            previous,
            &mut ignored,
        )
    };
    true
}

/// Restores the original import slot, initializes the hook once, and forwards the call.
///
/// # Safety
///
/// The selected Tower executable must call this function through its `CreateWindowExA`
/// import with valid Windows API arguments.
unsafe extern "system" fn hooked_create_window_ex_a(
    extended_style: u32,
    class_name: *const c_char,
    window_name: *const c_char,
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    parent: HWND,
    menu: HMENU,
    instance: HINSTANCE,
    parameter: *const c_void,
) -> HWND {
    let original_pointer = ORIGINAL_CREATE_WINDOW.load(Ordering::Acquire);
    if original_pointer.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: DllMain stored the imported CreateWindowExA function pointer.
    let original =
        unsafe { std::mem::transmute::<*mut c_void, CreateWindowExAFn>(original_pointer) };

    if BOOTSTRAP_STATE
        .compare_exchange(STATE_ARMED, STATE_BUSY, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        log("bootstrap-entry");
        let slot = CREATE_WINDOW_SLOT
            .load(Ordering::Acquire)
            .cast::<*mut c_void>();
        // Restore the original slot before any library load or initialization call.
        let restored = !slot.is_null() && unsafe { write_slot(slot, original_pointer) };
        if !restored {
            record_failure("Tower hook bootstrap could not restore CreateWindowExA.");
        } else if hook_is_disabled() {
            log("iat-restored");
            log("comparison-forward");
            BOOTSTRAP_STATE.store(STATE_COMPLETE, Ordering::Release);
        } else {
            log("iat-restored");
            log("hook-load-requested");
            if initialize_hook().is_err() {
                record_failure("Tower hook bootstrap could not initialize tower-hook.dll.");
            } else {
                log("hook-initialized");
                BOOTSTRAP_STATE.store(STATE_COMPLETE, Ordering::Release);
            }
        }
    }

    // SAFETY: Forward all arguments with the imported CreateWindowExA ABI.
    unsafe {
        original(
            extended_style,
            class_name,
            window_name,
            style,
            x,
            y,
            width,
            height,
            parent,
            menu,
            instance,
            parameter,
        )
    }
}

fn hook_is_disabled() -> bool {
    std::env::var_os("DRUAGA_TOWER_HOOK").is_some_and(|value| value.eq_ignore_ascii_case("off"))
}

fn initialize_hook() -> Result<(), ()> {
    let mut path = [0_u16; MAX_MODULE_PATH];
    // SAFETY: The buffer is writable and its size matches the passed count.
    let length =
        unsafe { GetModuleFileNameW(std::ptr::null_mut(), path.as_mut_ptr(), path.len() as u32) }
            as usize;
    if length == 0 || length >= path.len() {
        return Err(());
    }
    let Some(separator) = path[..length]
        .iter()
        .rposition(|unit| *unit == b'\\' as u16 || *unit == b'/' as u16)
    else {
        return Err(());
    };
    const HOOK_NAME: &[u16] = &[
        b't' as u16,
        b'o' as u16,
        b'w' as u16,
        b'e' as u16,
        b'r' as u16,
        b'-' as u16,
        b'h' as u16,
        b'o' as u16,
        b'o' as u16,
        b'k' as u16,
        b'.' as u16,
        b'd' as u16,
        b'l' as u16,
        b'l' as u16,
        0,
    ];
    let start = separator + 1;
    let Some(end) = start.checked_add(HOOK_NAME.len()) else {
        return Err(());
    };
    if end > path.len() {
        return Err(());
    }
    path[start..end].copy_from_slice(HOOK_NAME);

    // SAFETY: The constructed path is full and null-terminated.
    let module = unsafe { LoadLibraryW(path.as_ptr()) };
    if module.is_null() {
        return Err(());
    }
    // SAFETY: The module is loaded and the export name is null-terminated.
    let procedure = unsafe {
        GetProcAddress(
            module,
            c"druaga_tower_hook_initialize".as_ptr().cast::<u8>(),
        )
    };
    let Some(procedure) = procedure else {
        return Err(());
    };
    // SAFETY: The resolved export has the declared initialization ABI.
    let initialize = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, InitializeHookFn>(procedure)
    };
    // SAFETY: The export takes no parameters and returns a Sentinel-style status.
    if unsafe { initialize() } == 0 {
        Ok(())
    } else {
        Err(())
    }
}

fn record_failure(message: &str) {
    BOOTSTRAP_FAILED.store(true, Ordering::Release);
    BOOTSTRAP_STATE.store(STATE_FAILED, Ordering::Release);
    log(message);
    let mut text: Vec<u16> = message.encode_utf16().collect();
    text.push(0);
    const TITLE: &[u16] = &[
        b'D' as u16,
        b'r' as u16,
        b'u' as u16,
        b'a' as u16,
        b'g' as u16,
        b'a' as u16,
        b' ' as u16,
        b'T' as u16,
        b'o' as u16,
        b'w' as u16,
        b'e' as u16,
        b'r' as u16,
        0,
    ];
    // SAFETY: Both strings are null-terminated and remain alive for the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            TITLE.as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}
