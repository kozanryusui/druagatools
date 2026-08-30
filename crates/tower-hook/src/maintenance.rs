//! Tower daily maintenance-window hook.

#[cfg(target_arch = "x86")]
use std::sync::OnceLock;

use crate::HookFailure;
use crate::hook::HookInstaller;

#[cfg(target_arch = "x86")]
type UpdateMaintenanceWindowFn = unsafe extern "C" fn(u32);

#[cfg(target_arch = "x86")]
const UPDATE_MAINTENANCE_WINDOW_RVA: usize = 0x0000_80d0;
#[cfg(target_arch = "x86")]
const APPLICATION_FLAGS_RVA: usize = 0x0010_39a4;
#[cfg(target_arch = "x86")]
const MAINTENANCE_WINDOW_FLAG: u32 = 0x2000;

#[cfg(target_arch = "x86")]
static ORIGINAL_UPDATE_MAINTENANCE_WINDOW: OnceLock<UpdateMaintenanceWindowFn> = OnceLock::new();
#[cfg(target_arch = "x86")]
static APPLICATION_FLAGS: OnceLock<usize> = OnceLock::new();

#[cfg(target_arch = "x86")]
pub(crate) fn queue_hook(
    installer: &mut HookInstaller,
    disable_maintenance_window: bool,
) -> Result<(), HookFailure> {
    if !disable_maintenance_window {
        return Ok(());
    }

    let image_base = installer.image_base();
    let target_address = image_base
        .checked_add(UPDATE_MAINTENANCE_WINDOW_RVA)
        .ok_or_else(|| HookFailure::HookPrepare {
            hook: "maintenance-window",
            detail: "target address overflow".to_owned(),
        })?;
    let flags_address = image_base
        .checked_add(APPLICATION_FLAGS_RVA)
        .ok_or_else(|| HookFailure::HookPrepare {
            hook: "maintenance-window",
            detail: "application flag address overflow".to_owned(),
        })?;
    // SAFETY: The selected Tower image defines this function with the confirmed cdecl ABI.
    let target = unsafe { std::mem::transmute::<usize, UpdateMaintenanceWindowFn>(target_address) };
    APPLICATION_FLAGS
        .set(flags_address)
        .map_err(|_| HookFailure::RuntimeState("Tower application flags"))?;
    // SAFETY: The target and detour use the same confirmed cdecl ABI.
    let original = unsafe { installer.inline("maintenance-window", target, hooked_update)? };
    ORIGINAL_UPDATE_MAINTENANCE_WINDOW
        .set(original)
        .map_err(|_| HookFailure::RuntimeState("maintenance-window function"))?;
    Ok(())
}

#[cfg(target_arch = "x86")]
unsafe extern "C" fn hooked_update(seconds_since_2000: u32) {
    if let Some(original) = ORIGINAL_UPDATE_MAINTENANCE_WINDOW.get() {
        // SAFETY: Hook installation records the original function's confirmed address and ABI.
        unsafe { original(seconds_since_2000) };
    }
    if let Some(address) = APPLICATION_FLAGS.get() {
        let flags = *address as *mut u32;
        // SAFETY: Hook installation records the writable application-flags address in the
        // loaded Tower image. Tower executes this function on its main update thread.
        unsafe { flags.write(flags.read() & !MAINTENANCE_WINDOW_FLAG) };
    }
}

#[cfg(not(target_arch = "x86"))]
pub(crate) fn queue_hook(
    _installer: &mut HookInstaller,
    disable_maintenance_window: bool,
) -> Result<(), HookFailure> {
    if disable_maintenance_window {
        return Err(HookFailure::HookPrepare {
            hook: "maintenance-window",
            detail: "Tower hooks require the 32-bit x86 target".to_owned(),
        });
    }
    Ok(())
}
