//! Optional reset for the character-card usage counter.

use crate::HookFailure;
use crate::hook::HookInstaller;

#[cfg(target_arch = "x86")]
mod x86_hook {
    use std::sync::OnceLock;

    use super::{HookFailure, HookInstaller};

    const PREPARE_CARD_DATA_FOR_WRITE_RVA: usize = 0x0003_f420;
    const STAGED_RECORD_OFFSET: usize = 0x630;
    const CHARACTER_DATA_OFFSET: usize = 0x10;
    const USAGE_COUNT_OFFSET: usize = 0x42;
    const INITIAL_USAGE_COUNT: u8 = 100;
    const CHARACTER_RECORD_ID: [u8; 4] = *b"DOCH";

    type PrepareCardDataForWriteFn =
        unsafe extern "fastcall" fn(*mut u8, usize, *mut u8, u32) -> u32;

    static ORIGINAL_PREPARE_CARD_DATA_FOR_WRITE: OnceLock<PrepareCardDataForWriteFn> =
        OnceLock::new();

    pub(super) fn queue_hook(installer: &mut HookInstaller) -> Result<(), HookFailure> {
        let target_address = installer
            .image_base()
            .checked_add(PREPARE_CARD_DATA_FOR_WRITE_RVA)
            .ok_or_else(|| HookFailure::HookPrepare {
                hook: "card-usage-count",
                detail: "target address overflow".to_owned(),
            })?;
        // SAFETY: Tower 1.60 has the confirmed thiscall method at this RVA.
        let target =
            unsafe { std::mem::transmute::<usize, PrepareCardDataForWriteFn>(target_address) };
        // SAFETY: The fastcall shim preserves ECX, EDX, and the two stack arguments.
        let original = unsafe {
            installer.inline(
                "card-usage-count",
                target,
                hooked_prepare_card_data_for_write,
            )?
        };
        ORIGINAL_PREPARE_CARD_DATA_FOR_WRITE
            .set(original)
            .map_err(|_| HookFailure::RuntimeState("card usage-count original"))
    }

    unsafe extern "fastcall" fn hooked_prepare_card_data_for_write(
        this: *mut u8,
        edx: usize,
        data: *mut u8,
        card_type: u32,
    ) -> u32 {
        let Some(original) = ORIGINAL_PREPARE_CARD_DATA_FOR_WRITE.get() else {
            return 0;
        };
        // SAFETY: Forward the call with the exact original ABI and arguments.
        let result = unsafe { original(this, edx, data, card_type) };
        if this.is_null() {
            return result;
        }

        // SAFETY: The original method receives the complete Tower card-slot object.
        // Ghidra confirms the staged record and character-data offsets in this object.
        let record = unsafe { this.add(STAGED_RECORD_OFFSET) };
        // SAFETY: The four-byte record identifier is inside the staged record.
        let record_id = unsafe { record.cast::<[u8; 4]>().read_unaligned() };
        if record_id == CHARACTER_RECORD_ID {
            // SAFETY: DOCH has a 0x250-byte character record at staged offset 0x10.
            unsafe {
                record
                    .add(CHARACTER_DATA_OFFSET + USAGE_COUNT_OFFSET)
                    .write(INITIAL_USAGE_COUNT)
            };
        }
        result
    }
}

pub(crate) fn queue_hook(
    installer: &mut HookInstaller,
    reset_usage_count: bool,
) -> Result<(), HookFailure> {
    if !reset_usage_count {
        return Ok(());
    }

    #[cfg(target_arch = "x86")]
    return x86_hook::queue_hook(installer);

    #[cfg(not(target_arch = "x86"))]
    Err(HookFailure::HookPrepare {
        hook: "card-usage-count",
        detail: "Tower hooks require the 32-bit x86 target".to_owned(),
    })
}
