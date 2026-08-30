//! Shared hook installation and process-lifetime ownership.

use std::ffi::c_void;
use std::sync::OnceLock;

use neohook::{DetourError, DetourTransaction};
use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};

use crate::HookFailure;

struct PendingOrdinalIat {
    name: &'static str,
    slot_rva: usize,
    detour: *const u8,
}

struct OrdinalIatHook {
    slot: usize,
    original: usize,
}

// SAFETY: Both values are process-global addresses. The hook is installed once.
unsafe impl Send for OrdinalIatHook {}
// SAFETY: The stored addresses do not change after installation.
unsafe impl Sync for OrdinalIatHook {}

impl Drop for OrdinalIatHook {
    fn drop(&mut self) {
        // SAFETY: The loaded executable and its IAT remain valid for the DLL lifetime.
        let _ = unsafe { write_iat_slot(self.slot as *mut usize, self.original) };
    }
}

static ORDINAL_IAT_HOOK: OnceLock<OrdinalIatHook> = OnceLock::new();

pub(crate) struct HookInstaller {
    image: HMODULE,
    transaction: DetourTransaction,
    names: Vec<&'static str>,
    ordinal_iat: Option<PendingOrdinalIat>,
}

impl HookInstaller {
    pub(crate) fn new(image: HMODULE) -> Self {
        let mut transaction = DetourTransaction::begin();
        transaction.update_all_threads();
        Self {
            image,
            transaction,
            names: Vec::new(),
            ordinal_iat: None,
        }
    }

    pub(crate) fn image_base(&self) -> usize {
        self.image as usize
    }

    pub(crate) unsafe fn inline<T: Copy>(
        &mut self,
        name: &'static str,
        target: T,
        detour: T,
    ) -> Result<T, HookFailure> {
        let target_address = unsafe { function_address(target) }?;
        let detour_address = unsafe { function_address(detour) }?;
        let gateway = self
            .transaction
            .attach(target_address as *mut u8, detour_address as *const u8)
            .map_err(|error| prepare_error(name, error))?;
        self.names.push(name);
        // SAFETY: NeoHook returns a gateway with the target function's exact ABI.
        unsafe { function_from_address(gateway as usize) }
    }

    pub(crate) fn iat(
        &mut self,
        name: &'static str,
        dll: &'static str,
        function: &'static str,
        detour: *const u8,
    ) -> Result<(), HookFailure> {
        self.transaction
            .attach_iat(self.image, dll, function, detour)
            .map_err(|error| prepare_error(name, error))?;
        self.names.push(name);
        Ok(())
    }

    pub(crate) fn ordinal_iat(
        &mut self,
        name: &'static str,
        slot_rva: usize,
        detour: *const u8,
    ) -> Result<usize, HookFailure> {
        if self.ordinal_iat.is_some() {
            return Err(HookFailure::HookPrepare {
                hook: name,
                detail: "more than one ordinal IAT hook was queued".to_owned(),
            });
        }
        let slot_address = (self.image as usize).checked_add(slot_rva).ok_or_else(|| {
            HookFailure::HookPrepare {
                hook: name,
                detail: "IAT slot address overflow".to_owned(),
            }
        })?;
        // SAFETY: The selected Tower image defines this pointer-sized IAT slot.
        let original = unsafe { (slot_address as *const usize).read() };
        if original == 0 {
            return Err(HookFailure::HookPrepare {
                hook: name,
                detail: "IAT slot is null".to_owned(),
            });
        }
        self.ordinal_iat = Some(PendingOrdinalIat {
            name,
            slot_rva,
            detour,
        });
        Ok(original)
    }

    pub(crate) fn commit(self) -> Result<(), HookFailure> {
        let Self {
            image,
            transaction,
            names,
            ordinal_iat,
        } = self;
        let registered = neohook::registry::names();
        if let Some(name) = names
            .iter()
            .find(|name| registered.iter().any(|item| item == **name))
        {
            return Err(HookFailure::HookCommit {
                hook: name,
                detail: "hook name is already registered".to_owned(),
            });
        }
        if ordinal_iat.is_some() && ORDINAL_IAT_HOOK.get().is_some() {
            return Err(HookFailure::HookCommit {
                hook: "wsa-async-get-host-by-name",
                detail: "ordinal IAT hook is already installed".to_owned(),
            });
        }
        let mut hooks = transaction.commit().map_err(|error| {
            let hook = commit_hook_name(&names, &error);
            HookFailure::HookCommit {
                hook,
                detail: error.to_string(),
            }
        })?;

        let ordinal_guard = if let Some(pending) = ordinal_iat {
            let slot_address = (image as usize)
                .checked_add(pending.slot_rva)
                .ok_or_else(|| HookFailure::HookCommit {
                    hook: pending.name,
                    detail: "IAT slot address overflow".to_owned(),
                })?;
            // SAFETY: Preparation validated the selected Tower IAT slot.
            let original =
                unsafe { write_iat_slot(slot_address as *mut usize, pending.detour as usize) }
                    .map_err(|detail| HookFailure::HookCommit {
                        hook: pending.name,
                        detail,
                    })?;
            Some(OrdinalIatHook {
                slot: slot_address,
                original,
            })
        } else {
            None
        };

        if hooks.len() != names.len() {
            return Err(HookFailure::HookCommit {
                hook: "hook-registry",
                detail: "NeoHook returned an unexpected hook count".to_owned(),
            });
        }
        for (name, hook) in names.into_iter().zip(hooks.drain(..)) {
            if neohook::registry::register(name, hook).is_some() {
                return Err(HookFailure::HookCommit {
                    hook: name,
                    detail: "hook name is already registered".to_owned(),
                });
            }
        }
        if let Some(guard) = ordinal_guard {
            ORDINAL_IAT_HOOK
                .set(guard)
                .map_err(|_| HookFailure::HookCommit {
                    hook: "wsa-async-get-host-by-name",
                    detail: "ordinal IAT hook is already installed".to_owned(),
                })?;
        }
        Ok(())
    }
}

fn prepare_error(name: &'static str, error: DetourError) -> HookFailure {
    HookFailure::HookPrepare {
        hook: name,
        detail: error.to_string(),
    }
}

fn commit_hook_name(names: &[&'static str], error: &DetourError) -> &'static str {
    match error {
        DetourError::CommitFailed { index, .. } => {
            names.get(*index).copied().unwrap_or("unknown-hook")
        }
        _ => "hook-transaction",
    }
}

unsafe fn function_address<T: Copy>(function: T) -> Result<usize, HookFailure> {
    if std::mem::size_of::<T>() != std::mem::size_of::<usize>() {
        return Err(HookFailure::HookPrepare {
            hook: "function-pointer",
            detail: "function pointer has an unexpected size".to_owned(),
        });
    }
    // SAFETY: The size check establishes that T has one pointer-sized representation.
    Ok(unsafe { std::mem::transmute_copy::<T, usize>(&function) })
}

unsafe fn function_from_address<T: Copy>(address: usize) -> Result<T, HookFailure> {
    if std::mem::size_of::<T>() != std::mem::size_of::<usize>() {
        return Err(HookFailure::HookPrepare {
            hook: "function-pointer",
            detail: "function pointer has an unexpected size".to_owned(),
        });
    }
    // SAFETY: The size check and caller contract establish T as the matching function type.
    Ok(unsafe { std::mem::transmute_copy::<usize, T>(&address) })
}

unsafe fn write_iat_slot(slot: *mut usize, value: usize) -> Result<usize, String> {
    if slot.is_null() {
        return Err("IAT slot is null".to_owned());
    }
    let mut old_protection = 0_u32;
    // SAFETY: The caller supplies one pointer-sized IAT entry in a committed image.
    if unsafe {
        VirtualProtect(
            slot.cast::<c_void>(),
            std::mem::size_of::<usize>(),
            PAGE_READWRITE,
            &mut old_protection,
        )
    } == 0
    {
        return Err(format!(
            "VirtualProtect failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: VirtualProtect made the pointer-sized IAT entry writable.
    let original = unsafe { slot.replace(value) };
    let mut ignored = 0_u32;
    // SAFETY: Restore the protection returned by the first call.
    if unsafe {
        VirtualProtect(
            slot.cast::<c_void>(),
            std::mem::size_of::<usize>(),
            old_protection,
            &mut ignored,
        )
    } == 0
    {
        // SAFETY: The first protection change succeeded, so the slot is writable.
        unsafe { slot.write(original) };
        let _ = unsafe {
            VirtualProtect(
                slot.cast::<c_void>(),
                std::mem::size_of::<usize>(),
                old_protection,
                &mut ignored,
            )
        };
        return Err(format!(
            "protection restore failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(original)
}
