//! Tower startup screen hooks.

#[cfg(target_arch = "x86")]
use std::ffi::c_void;
#[cfg(target_arch = "x86")]
use std::sync::OnceLock;

use crate::HookFailure;
use crate::hook::HookInstaller;

#[cfg(target_arch = "x86")]
type StartupNoticeTimerFn = unsafe extern "fastcall" fn(*mut c_void, usize, f32) -> u8;
#[cfg(target_arch = "x86")]
type StartupNoticeRenderFn = unsafe extern "fastcall" fn(*mut c_void, usize);
#[cfg(target_arch = "x86")]
type StartupLogoUpdateFn = unsafe extern "fastcall" fn(*mut c_void, usize) -> i32;
#[cfg(target_arch = "x86")]
type StartupLogoRenderFn = unsafe extern "fastcall" fn(*mut c_void, usize);
#[cfg(target_arch = "x86")]
type SelectNextSceneFn = unsafe extern "fastcall" fn(*mut c_void, usize, i32);

#[cfg(target_arch = "x86")]
const STARTUP_NOTICE_TIMER_RVA: usize = 0x0000_85e0;
#[cfg(target_arch = "x86")]
const STARTUP_NOTICE_RENDER_RVA: usize = 0x0000_8610;
#[cfg(target_arch = "x86")]
const STARTUP_LOGO_UPDATE_RVA: usize = 0x0002_7c60;
#[cfg(target_arch = "x86")]
const STARTUP_LOGO_RENDER_RVA: usize = 0x0002_7e30;
#[cfg(target_arch = "x86")]
const SELECT_NEXT_SCENE_RVA: usize = 0x0001_6510;

#[cfg(target_arch = "x86")]
static SELECT_NEXT_SCENE: OnceLock<SelectNextSceneFn> = OnceLock::new();

#[cfg(target_arch = "x86")]
pub(crate) fn queue_hooks(
    installer: &mut HookInstaller,
    skip_notice: bool,
    skip_logos: bool,
) -> Result<(), HookFailure> {
    if skip_notice {
        queue_notice_hooks(installer)?;
    }
    if skip_logos {
        queue_logo_hooks(installer)?;
    }
    Ok(())
}

#[cfg(target_arch = "x86")]
fn queue_notice_hooks(installer: &mut HookInstaller) -> Result<(), HookFailure> {
    let image_base = installer.image_base();
    let timer_target = image_base
        .checked_add(STARTUP_NOTICE_TIMER_RVA)
        .ok_or_else(|| HookFailure::HookPrepare {
            hook: "startup-notice-timer",
            detail: "target address overflow".to_owned(),
        })?;
    let render_target = image_base
        .checked_add(STARTUP_NOTICE_RENDER_RVA)
        .ok_or_else(|| HookFailure::HookPrepare {
            hook: "startup-notice-render",
            detail: "target address overflow".to_owned(),
        })?;

    // SAFETY: The fastcall shim preserves the confirmed x86 thiscall stack layout.
    let timer = unsafe { std::mem::transmute::<usize, StartupNoticeTimerFn>(timer_target) };
    // SAFETY: The target and hook use the same confirmed x86 fastcall layout.
    let render = unsafe { std::mem::transmute::<usize, StartupNoticeRenderFn>(render_target) };
    // SAFETY: The target and hook use the matching confirmed x86 ABI.
    let _original_timer =
        unsafe { installer.inline("startup-notice-timer", timer, hooked_startup_notice_timer)? };
    // SAFETY: The target and hook use the matching confirmed x86 ABI.
    let _original_render = unsafe {
        installer.inline(
            "startup-notice-render",
            render,
            hooked_startup_notice_render,
        )?
    };
    Ok(())
}

#[cfg(target_arch = "x86")]
fn queue_logo_hooks(installer: &mut HookInstaller) -> Result<(), HookFailure> {
    let image_base = installer.image_base();
    let update_target = image_base
        .checked_add(STARTUP_LOGO_UPDATE_RVA)
        .ok_or_else(|| HookFailure::HookPrepare {
            hook: "startup-logo-update",
            detail: "target address overflow".to_owned(),
        })?;
    let render_target = image_base
        .checked_add(STARTUP_LOGO_RENDER_RVA)
        .ok_or_else(|| HookFailure::HookPrepare {
            hook: "startup-logo-render",
            detail: "target address overflow".to_owned(),
        })?;
    let select_target = image_base
        .checked_add(SELECT_NEXT_SCENE_RVA)
        .ok_or_else(|| HookFailure::HookPrepare {
            hook: "startup-logo-select-next-scene",
            detail: "target address overflow".to_owned(),
        })?;

    // SAFETY: The target and hook use the same confirmed x86 fastcall layout.
    let update = unsafe { std::mem::transmute::<usize, StartupLogoUpdateFn>(update_target) };
    // SAFETY: The target and hook use the same confirmed x86 fastcall layout.
    let render = unsafe { std::mem::transmute::<usize, StartupLogoRenderFn>(render_target) };
    // SAFETY: The fastcall shim preserves the confirmed x86 thiscall stack layout.
    let select = unsafe { std::mem::transmute::<usize, SelectNextSceneFn>(select_target) };
    SELECT_NEXT_SCENE
        .set(select)
        .map_err(|_| HookFailure::RuntimeState("startup logo scene selector"))?;

    // SAFETY: The target and hook use the matching confirmed x86 ABI.
    let _original_update =
        unsafe { installer.inline("startup-logo-update", update, hooked_startup_logo_update)? };
    // SAFETY: The target and hook use the matching confirmed x86 ABI.
    let _original_render =
        unsafe { installer.inline("startup-logo-render", render, hooked_startup_logo_render)? };
    Ok(())
}

#[cfg(target_arch = "x86")]
unsafe extern "fastcall" fn hooked_startup_notice_timer(
    _this: *mut c_void,
    _edx: usize,
    _frame_delta: f32,
) -> u8 {
    1
}

#[cfg(target_arch = "x86")]
unsafe extern "fastcall" fn hooked_startup_notice_render(_this: *mut c_void, _edx: usize) {}

#[cfg(target_arch = "x86")]
unsafe extern "fastcall" fn hooked_startup_logo_update(this: *mut c_void, _edx: usize) -> i32 {
    if let Some(select_next_scene) = SELECT_NEXT_SCENE.get() {
        // SAFETY: Hook installation records the confirmed scene-selector address and ABI.
        unsafe { select_next_scene(this, 0, 1) };
    }
    0
}

#[cfg(target_arch = "x86")]
unsafe extern "fastcall" fn hooked_startup_logo_render(_this: *mut c_void, _edx: usize) {}

#[cfg(not(target_arch = "x86"))]
pub(crate) fn queue_hooks(
    _installer: &mut HookInstaller,
    _skip_notice: bool,
    _skip_logos: bool,
) -> Result<(), HookFailure> {
    Err(HookFailure::HookPrepare {
        hook: "startup-notice",
        detail: "Tower hooks require the 32-bit x86 target".to_owned(),
    })
}
