//! Tower window-message quality-of-life hook.

/// `WM_ACTIVATEAPP` reports process-level activation changes.
pub const WM_ACTIVATEAPP: u32 = 0x001c;
/// `WM_KEYDOWN` carries the repeat state in bit 30 of `lParam`.
pub const WM_KEYDOWN: u32 = 0x0100;
/// The original Tower cursor toggle uses F4.
pub const VK_F4: usize = 0x73;
const PREVIOUS_KEY_STATE_BIT: isize = 1 << 30;
const WINDOW_MESSAGE_HOOK_RVA: usize = 0x0004_3f80;
const SET_CURSOR_VISIBLE_RVA: usize = 0x0004_3e80;
const GET_WINDOW_INTERFACE_RVA: usize = 0x0007_1720;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowMessageAction {
    Forward,
    SuppressFocusLossClose,
    SuppressRepeatedCursorToggle,
}

/// Classify only the messages whose original behavior must change.
pub fn classify_window_message(message: u32, wparam: usize, lparam: isize) -> WindowMessageAction {
    if message == WM_ACTIVATEAPP && wparam == 0 {
        return WindowMessageAction::SuppressFocusLossClose;
    }
    if message == WM_KEYDOWN && wparam == VK_F4 && lparam & PREVIOUS_KEY_STATE_BIT != 0 {
        return WindowMessageAction::SuppressRepeatedCursorToggle;
    }
    WindowMessageAction::Forward
}

#[cfg(windows)]
mod windows_hook {
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{
        GET_WINDOW_INTERFACE_RVA, SET_CURSOR_VISIBLE_RVA, WINDOW_MESSAGE_HOOK_RVA, WM_ACTIVATEAPP,
        WindowMessageAction, classify_window_message,
    };
    use crate::HookFailure;
    use crate::hook::HookInstaller;
    use crate::platform;
    use windows_sys::Win32::Foundation::HWND;

    #[repr(C)]
    struct TowerWindowInterface {
        vtable: *const c_void,
        hwnd: HWND,
        reserved_08: [u8; 12],
        flags: u32,
    }

    type WindowMessageHookFn =
        unsafe extern "C" fn(HWND, u32, usize, isize, *mut c_void, *mut isize) -> u8;
    type GetWindowInterfaceFn = unsafe extern "C" fn() -> *mut TowerWindowInterface;
    type SetCursorVisibleFn = unsafe extern "thiscall" fn(*mut TowerWindowInterface, u8);
    static ORIGINAL_WINDOW_MESSAGE: OnceLock<WindowMessageHookFn> = OnceLock::new();
    static STARTUP_CURSOR_RESTORED: AtomicBool = AtomicBool::new(false);

    pub(crate) fn queue_hooks(installer: &mut HookInstaller) -> Result<(), HookFailure> {
        let target_address = installer
            .image_base()
            .checked_add(WINDOW_MESSAGE_HOOK_RVA)
            .ok_or_else(|| HookFailure::HookPrepare {
                hook: "window-message",
                detail: "target address overflow".to_owned(),
            })?;
        // SAFETY: The selected Tower image has a six-argument cdecl callback at this RVA.
        let target = unsafe { std::mem::transmute::<usize, WindowMessageHookFn>(target_address) };
        // SAFETY: The target and replacement use the same confirmed cdecl ABI.
        let original =
            unsafe { installer.inline("window-message", target, hooked_window_message)? };
        ORIGINAL_WINDOW_MESSAGE
            .set(original)
            .map_err(|_| HookFailure::RuntimeState("window-message original"))
    }

    fn tower_image_base() -> Result<usize, HookFailure> {
        Ok(platform::tower_image()? as usize)
    }

    unsafe fn restore_startup_cursor(image_base: usize) {
        if STARTUP_CURSOR_RESTORED.load(Ordering::Acquire) {
            return;
        }
        let Some(getter_address) = image_base.checked_add(GET_WINDOW_INTERFACE_RVA) else {
            return;
        };
        let Some(setter_address) = image_base.checked_add(SET_CURSOR_VISIBLE_RVA) else {
            return;
        };
        // SAFETY: The selected Tower image contains this exact function at the confirmed RVA.
        let getter = unsafe { std::mem::transmute::<usize, GetWindowInterfaceFn>(getter_address) };
        // SAFETY: The selected Tower image contains this exact function at the confirmed RVA.
        let setter = unsafe { std::mem::transmute::<usize, SetCursorVisibleFn>(setter_address) };
        // SAFETY: The callback is registered after window initialization.
        let window = unsafe { getter() };
        if window.is_null() {
            return;
        }
        // SAFETY: Ghidra confirms the flags field at object offset 0x14.
        let flags_before = unsafe { (*window).flags };
        if flags_before & 0x1000 == 0 {
            return;
        }
        if STARTUP_CURSOR_RESTORED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        // SAFETY: The setter accepts the shared Tower window object and one byte boolean.
        unsafe { setter(window, 1) };
        // SAFETY: The setter leaves the shared object valid.
        let flags_after = unsafe { (*window).flags };
        platform::log(&format!(
            "window-qol-startup-cursor flags_before={flags_before:#010x} flags_after={flags_after:#010x}"
        ));
    }

    unsafe extern "C" fn hooked_window_message(
        hwnd: HWND,
        message: u32,
        wparam: usize,
        lparam: isize,
        context: *mut c_void,
        result: *mut isize,
    ) -> u8 {
        if matches!(message, 0x0006 | 0x0007 | 0x0008 | WM_ACTIVATEAPP | 0x0010) {
            platform::log(&format!(
                "window-message message={message:#06x} wparam={wparam:#010x} lparam={lparam:#010x}"
            ));
        }
        if let Ok(image_base) = tower_image_base() {
            // SAFETY: This runs only at the registered Tower callback boundary.
            unsafe { restore_startup_cursor(image_base) };
        }

        match classify_window_message(message, wparam, lparam) {
            WindowMessageAction::SuppressFocusLossClose => {
                platform::log("window-message-suppress focus-loss-close");
                0
            }
            WindowMessageAction::SuppressRepeatedCursorToggle => 1,
            WindowMessageAction::Forward => {
                let Some(original) = ORIGINAL_WINDOW_MESSAGE.get() else {
                    return 0;
                };
                // SAFETY: Forward the message with the exact original callback ABI.
                unsafe { original(hwnd, message, wparam, lparam, context, result) }
            }
        }
    }
}

#[cfg(windows)]
pub(crate) use windows_hook::queue_hooks;
