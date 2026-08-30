#[cfg(windows)]
mod windows_proxy {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Direct3D9::{
        D3DADAPTER_IDENTIFIER9, D3DCAPS9, D3DDEVTYPE, D3DDISPLAYMODE, D3DFORMAT,
        D3DMULTISAMPLE_TYPE, D3DPRESENT_PARAMETERS, D3DRESOURCETYPE, IDirect3D9, IDirect3D9_Impl,
        IDirect3DDevice9,
    };
    use windows::Win32::Graphics::Gdi::HMONITOR;
    use windows::core::{ComObject, Error, HRESULT, Interface, OutRef, Result, implement};
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    use crate::config::DisplayMode;
    use crate::hook::HookInstaller;
    use crate::{HookFailure, platform};

    const D3DERR_INVALIDCALL: HRESULT = HRESULT(0x8876_086c_u32 as i32);
    const D3D9_DLL: [u16; 9] = [
        b'd' as u16,
        b'3' as u16,
        b'd' as u16,
        b'9' as u16,
        b'.' as u16,
        b'd' as u16,
        b'l' as u16,
        b'l' as u16,
        0,
    ];
    const DIRECT3D_CREATE9_NAME: &[u8] = b"Direct3DCreate9\0";

    type Direct3DCreate9Fn = unsafe extern "system" fn(u32) -> *mut c_void;

    static ORIGINAL_DIRECT3D_CREATE9: OnceLock<Direct3DCreate9Fn> = OnceLock::new();
    static DISPLAY_MODE: OnceLock<DisplayMode> = OnceLock::new();

    #[implement(IDirect3D9)]
    pub struct D3d9Proxy {
        inner: IDirect3D9,
        mode: DisplayMode,
    }

    impl D3d9Proxy {
        pub fn new(inner: IDirect3D9, mode: DisplayMode) -> Self {
            Self { inner, mode }
        }

        pub fn into_interface(self) -> IDirect3D9 {
            ComObject::new(self).into_interface()
        }
    }

    #[allow(non_snake_case)]
    impl IDirect3D9_Impl for D3d9Proxy_Impl {
        fn RegisterSoftwareDevice(&self, pinitializefunction: *mut c_void) -> Result<()> {
            // SAFETY: The proxy forwards the caller-owned function pointer unchanged.
            unsafe { self.inner.RegisterSoftwareDevice(pinitializefunction) }
        }

        fn GetAdapterCount(&self) -> u32 {
            // SAFETY: The owned inner interface is valid for this COM call.
            unsafe { self.inner.GetAdapterCount() }
        }

        fn GetAdapterIdentifier(
            &self,
            adapter: u32,
            flags: u32,
            identifier: *mut D3DADAPTER_IDENTIFIER9,
        ) -> Result<()> {
            // SAFETY: The proxy forwards all arguments and the caller-owned output pointer.
            unsafe { self.inner.GetAdapterIdentifier(adapter, flags, identifier) }
        }

        fn GetAdapterModeCount(&self, adapter: u32, format: D3DFORMAT) -> u32 {
            // SAFETY: The owned inner interface is valid for this COM call.
            let original = unsafe { self.inner.GetAdapterModeCount(adapter, format) };
            if self.mode == DisplayMode::Windowed {
                1
            } else {
                original
            }
        }

        fn EnumAdapterModes(
            &self,
            adapter: u32,
            format: D3DFORMAT,
            mode: u32,
            output: *mut D3DDISPLAYMODE,
        ) -> Result<()> {
            if self.mode == DisplayMode::Original {
                // SAFETY: Original mode forwards all arguments and the output pointer unchanged.
                return unsafe { self.inner.EnumAdapterModes(adapter, format, mode, output) };
            }
            if output.is_null() || mode != 0 {
                return Err(Error::from_hresult(D3DERR_INVALIDCALL));
            }
            // SAFETY: The null check above establishes one writable D3DDISPLAYMODE output.
            unsafe {
                output.write(D3DDISPLAYMODE {
                    Width: 1280,
                    Height: 1024,
                    RefreshRate: 0,
                    Format: format,
                });
            }
            Ok(())
        }

        fn GetAdapterDisplayMode(&self, adapter: u32, output: *mut D3DDISPLAYMODE) -> Result<()> {
            // SAFETY: The proxy forwards all arguments and the caller-owned output pointer.
            unsafe { self.inner.GetAdapterDisplayMode(adapter, output) }
        }

        fn CheckDeviceType(
            &self,
            adapter: u32,
            device_type: D3DDEVTYPE,
            adapter_format: D3DFORMAT,
            back_buffer_format: D3DFORMAT,
            windowed: windows::core::BOOL,
        ) -> Result<()> {
            // SAFETY: The proxy forwards all arguments without a policy change.
            unsafe {
                self.inner.CheckDeviceType(
                    adapter,
                    device_type,
                    adapter_format,
                    back_buffer_format,
                    windowed.as_bool(),
                )
            }
        }

        fn CheckDeviceFormat(
            &self,
            adapter: u32,
            device_type: D3DDEVTYPE,
            adapter_format: D3DFORMAT,
            usage: u32,
            resource_type: D3DRESOURCETYPE,
            check_format: D3DFORMAT,
        ) -> Result<()> {
            // SAFETY: The proxy forwards all arguments without a policy change.
            unsafe {
                self.inner.CheckDeviceFormat(
                    adapter,
                    device_type,
                    adapter_format,
                    usage,
                    resource_type,
                    check_format,
                )
            }
        }

        fn CheckDeviceMultiSampleType(
            &self,
            adapter: u32,
            device_type: D3DDEVTYPE,
            surface_format: D3DFORMAT,
            windowed: windows::core::BOOL,
            multisample_type: D3DMULTISAMPLE_TYPE,
            quality_levels: *mut u32,
        ) -> Result<()> {
            // SAFETY: The proxy forwards all arguments and the caller-owned output pointer.
            unsafe {
                self.inner.CheckDeviceMultiSampleType(
                    adapter,
                    device_type,
                    surface_format,
                    windowed.as_bool(),
                    multisample_type,
                    quality_levels,
                )
            }
        }

        fn CheckDepthStencilMatch(
            &self,
            adapter: u32,
            device_type: D3DDEVTYPE,
            adapter_format: D3DFORMAT,
            render_target_format: D3DFORMAT,
            depth_stencil_format: D3DFORMAT,
        ) -> Result<()> {
            // SAFETY: The proxy forwards all arguments without a policy change.
            unsafe {
                self.inner.CheckDepthStencilMatch(
                    adapter,
                    device_type,
                    adapter_format,
                    render_target_format,
                    depth_stencil_format,
                )
            }
        }

        fn CheckDeviceFormatConversion(
            &self,
            adapter: u32,
            device_type: D3DDEVTYPE,
            source_format: D3DFORMAT,
            target_format: D3DFORMAT,
        ) -> Result<()> {
            // SAFETY: The proxy forwards all arguments without a policy change.
            unsafe {
                self.inner.CheckDeviceFormatConversion(
                    adapter,
                    device_type,
                    source_format,
                    target_format,
                )
            }
        }

        fn GetDeviceCaps(
            &self,
            adapter: u32,
            device_type: D3DDEVTYPE,
            caps: *mut D3DCAPS9,
        ) -> Result<()> {
            // SAFETY: The proxy forwards all arguments and the caller-owned output pointer.
            unsafe { self.inner.GetDeviceCaps(adapter, device_type, caps) }
        }

        fn GetAdapterMonitor(&self, adapter: u32) -> HMONITOR {
            // SAFETY: The owned inner interface is valid for this COM call.
            unsafe { self.inner.GetAdapterMonitor(adapter) }
        }

        fn CreateDevice(
            &self,
            adapter: u32,
            device_type: D3DDEVTYPE,
            focus_window: HWND,
            behavior_flags: u32,
            presentation: *mut D3DPRESENT_PARAMETERS,
            returned_device: OutRef<IDirect3DDevice9>,
        ) -> Result<()> {
            if self.mode == DisplayMode::Windowed && !presentation.is_null() {
                // SAFETY: The caller supplies one writable presentation structure.
                let parameters = unsafe { &mut *presentation };
                parameters.Windowed = true.into();
                parameters.FullScreen_RefreshRateInHz = 0;
            }

            // SAFETY: OutRef is a transparent wrapper for the same COM output pointer type.
            let device_output: *mut Option<IDirect3DDevice9> =
                unsafe { std::mem::transmute_copy(&returned_device) };
            // SAFETY: The proxy forwards all arguments. Windowed policy changes only the two
            // documented presentation fields before this call.
            unsafe {
                self.inner.CreateDevice(
                    adapter,
                    device_type,
                    focus_window,
                    behavior_flags,
                    presentation,
                    device_output,
                )
            }
        }
    }

    pub fn wrap_direct3d9(inner: IDirect3D9, mode: DisplayMode) -> IDirect3D9 {
        D3d9Proxy::new(inner, mode).into_interface()
    }

    pub unsafe fn wrap_direct3d9_raw(inner: *mut c_void, mode: DisplayMode) -> *mut c_void {
        // SAFETY: The caller transfers one owned IDirect3D9 reference to the proxy.
        let inner = unsafe { IDirect3D9::from_raw(inner) };
        wrap_direct3d9(inner, mode).into_raw()
    }

    pub(crate) fn queue_hook(
        installer: &mut HookInstaller,
        mode: DisplayMode,
    ) -> std::result::Result<(), HookFailure> {
        DISPLAY_MODE
            .set(mode)
            .map_err(|_| HookFailure::RuntimeState("display mode"))?;
        // SAFETY: The name is a valid null-terminated UTF-16 string.
        let module = unsafe { GetModuleHandleW(D3D9_DLL.as_ptr()) };
        if module.is_null() {
            return Err(HookFailure::HookPrepare {
                hook: "direct3d-create9",
                detail: "d3d9.dll is not loaded".to_owned(),
            });
        }
        // SAFETY: The module is loaded and the function name is null-terminated.
        let procedure = unsafe { GetProcAddress(module, DIRECT3D_CREATE9_NAME.as_ptr()) }
            .ok_or_else(|| HookFailure::HookPrepare {
                hook: "direct3d-create9",
                detail: "Direct3DCreate9 export was not found".to_owned(),
            })?;
        // SAFETY: GetProcAddress returned the documented Direct3DCreate9 export.
        let original = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Direct3DCreate9Fn>(
                procedure,
            )
        };
        ORIGINAL_DIRECT3D_CREATE9
            .set(original)
            .map_err(|_| HookFailure::RuntimeState("Direct3DCreate9 original"))?;
        installer.iat(
            "direct3d-create9",
            "d3d9.dll",
            "Direct3DCreate9",
            hooked_direct3d_create9 as *const u8,
        )
    }

    unsafe extern "system" fn hooked_direct3d_create9(sdk_version: u32) -> *mut c_void {
        platform::log(&format!("direct3d-create9 sdk_version={sdk_version}"));
        let Some(original) = ORIGINAL_DIRECT3D_CREATE9.get() else {
            return std::ptr::null_mut();
        };
        // SAFETY: The call uses the original Direct3DCreate9 signature.
        let inner = unsafe { original(sdk_version) };
        if inner.is_null() {
            return inner;
        }
        let mode = DISPLAY_MODE.get().copied().unwrap_or(DisplayMode::Windowed);
        // SAFETY: Direct3DCreate9 returned one owned IDirect3D9 reference.
        unsafe { wrap_direct3d9_raw(inner, mode) }
    }
}

#[cfg(windows)]
pub(crate) use windows_proxy::queue_hook;
