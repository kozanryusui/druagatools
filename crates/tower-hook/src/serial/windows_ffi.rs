use std::ffi::CStr;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Devices::Communication::{
    COMMPROP, COMMTIMEOUTS, COMSTAT, ClearCommError, DCB, EVENPARITY, GetCommProperties,
    GetCommState, NOPARITY, ONESTOPBIT, SetCommState, SetCommTimeouts, TWOSTOPBITS,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_FUNCTION, ERROR_OPEN_FAILED, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, SetLastError, TRUE, WIN32_ERROR,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{CreateFileA, ReadFile, WriteFile};
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::CreateEventA;
use windows_sys::core::BOOL;

use super::capture::{SerialCapture, log_board_frame, log_reader_frame, log_reader_read};
use super::operator_input::KeyboardInputCapture;
use super::{
    CardMountDisposition, CloseDisposition, CreateFileRequest, SerialCall, SerialDispatch,
    SerialHandle, SerialParity, SerialPort, SerialPortState, SerialStopBits,
};
use crate::HookFailure;
use crate::config::SerialLoggingConfig;
use crate::hook::HookInstaller;

mod comm;
mod io;

type CreateFileAFn = unsafe extern "system" fn(
    *const u8,
    u32,
    u32,
    *const SECURITY_ATTRIBUTES,
    u32,
    u32,
    HANDLE,
) -> HANDLE;
type GetCommPropertiesFn = unsafe extern "system" fn(HANDLE, *mut COMMPROP) -> BOOL;
type GetCommStateFn = unsafe extern "system" fn(HANDLE, *mut DCB) -> BOOL;
type SetCommStateFn = unsafe extern "system" fn(HANDLE, *const DCB) -> BOOL;
type SetCommTimeoutsFn = unsafe extern "system" fn(HANDLE, *const COMMTIMEOUTS) -> BOOL;
type ReadFileFn =
    unsafe extern "system" fn(HANDLE, *mut u8, u32, *mut u32, *mut OVERLAPPED) -> BOOL;
type WriteFileFn =
    unsafe extern "system" fn(HANDLE, *const u8, u32, *mut u32, *mut OVERLAPPED) -> BOOL;
type ClearCommErrorFn = unsafe extern "system" fn(HANDLE, *mut u32, *mut COMSTAT) -> BOOL;
type GetOverlappedResultFn =
    unsafe extern "system" fn(HANDLE, *const OVERLAPPED, *mut u32, BOOL) -> BOOL;
type CloseHandleFn = unsafe extern "system" fn(HANDLE) -> BOOL;

static ORIGINAL_CREATE_FILE_A: OnceLock<CreateFileAFn> = OnceLock::new();
static ORIGINAL_GET_COMM_PROPERTIES: OnceLock<GetCommPropertiesFn> = OnceLock::new();
static ORIGINAL_GET_COMM_STATE: OnceLock<GetCommStateFn> = OnceLock::new();
static ORIGINAL_SET_COMM_STATE: OnceLock<SetCommStateFn> = OnceLock::new();
static ORIGINAL_SET_COMM_TIMEOUTS: OnceLock<SetCommTimeoutsFn> = OnceLock::new();
static ORIGINAL_READ_FILE: OnceLock<ReadFileFn> = OnceLock::new();
static ORIGINAL_WRITE_FILE: OnceLock<WriteFileFn> = OnceLock::new();
static ORIGINAL_CLEAR_COMM_ERROR: OnceLock<ClearCommErrorFn> = OnceLock::new();
static ORIGINAL_GET_OVERLAPPED_RESULT: OnceLock<GetOverlappedResultFn> = OnceLock::new();
static ORIGINAL_CLOSE_HANDLE: OnceLock<CloseHandleFn> = OnceLock::new();
static SERIAL_RUNTIME: OnceLock<Mutex<WindowsSerialRuntime>> = OnceLock::new();

struct WindowsSerialRuntime {
    dispatch: SerialDispatch,
    capture: SerialCapture,
    operator_input: KeyboardInputCapture,
    logging: SerialLoggingConfig,
}

impl WindowsSerialRuntime {
    fn new(card_directory: PathBuf, logging: SerialLoggingConfig) -> Self {
        Self {
            dispatch: SerialDispatch::new(card_directory),
            capture: SerialCapture::new(),
            operator_input: KeyboardInputCapture::new(),
            logging,
        }
    }

    fn serial_logging_enabled(&self, port: SerialPort) -> bool {
        match port {
            SerialPort::Com1 => self.logging.left_reader,
            SerialPort::Com2 => self.logging.right_reader,
            SerialPort::Com3 => self.logging.io_board,
        }
    }
}

fn runtime() -> &'static Mutex<WindowsSerialRuntime> {
    SERIAL_RUNTIME.get_or_init(|| {
        Mutex::new(WindowsSerialRuntime::new(
            PathBuf::from("cards"),
            SerialLoggingConfig::default(),
        ))
    })
}

pub(super) fn configure(
    card_directory: PathBuf,
    logging: SerialLoggingConfig,
) -> Result<(), HookFailure> {
    SERIAL_RUNTIME
        .set(Mutex::new(WindowsSerialRuntime::new(
            card_directory,
            logging,
        )))
        .map_err(|_| HookFailure::RuntimeState("serial-runtime"))
}

fn as_serial_handle(handle: HANDLE) -> SerialHandle {
    SerialHandle::new(handle as isize)
}

fn create_file_failure(error: WIN32_ERROR) -> HANDLE {
    // SAFETY: The hook supplies a documented Windows error code for its own failure.
    unsafe { SetLastError(error) };
    INVALID_HANDLE_VALUE
}

pub(crate) fn queue_hooks(installer: &mut HookInstaller) -> Result<(), HookFailure> {
    queue_iat(
        installer,
        "create-file-a",
        "CreateFileA",
        &ORIGINAL_CREATE_FILE_A,
        CreateFileA as CreateFileAFn,
        io::hooked_create_file_a as *const u8,
    )?;
    queue_iat(
        installer,
        "get-comm-properties",
        "GetCommProperties",
        &ORIGINAL_GET_COMM_PROPERTIES,
        GetCommProperties as GetCommPropertiesFn,
        comm::hooked_get_comm_properties as *const u8,
    )?;
    queue_iat(
        installer,
        "get-comm-state",
        "GetCommState",
        &ORIGINAL_GET_COMM_STATE,
        GetCommState as GetCommStateFn,
        comm::hooked_get_comm_state as *const u8,
    )?;
    queue_iat(
        installer,
        "set-comm-state",
        "SetCommState",
        &ORIGINAL_SET_COMM_STATE,
        SetCommState as SetCommStateFn,
        comm::hooked_set_comm_state as *const u8,
    )?;
    queue_iat(
        installer,
        "set-comm-timeouts",
        "SetCommTimeouts",
        &ORIGINAL_SET_COMM_TIMEOUTS,
        SetCommTimeouts as SetCommTimeoutsFn,
        comm::hooked_set_comm_timeouts as *const u8,
    )?;
    queue_iat(
        installer,
        "read-file",
        "ReadFile",
        &ORIGINAL_READ_FILE,
        ReadFile as ReadFileFn,
        io::hooked_read_file as *const u8,
    )?;
    queue_iat(
        installer,
        "write-file",
        "WriteFile",
        &ORIGINAL_WRITE_FILE,
        WriteFile as WriteFileFn,
        io::hooked_write_file as *const u8,
    )?;
    queue_iat(
        installer,
        "clear-comm-error",
        "ClearCommError",
        &ORIGINAL_CLEAR_COMM_ERROR,
        ClearCommError as ClearCommErrorFn,
        comm::hooked_clear_comm_error as *const u8,
    )?;
    queue_iat(
        installer,
        "get-overlapped-result",
        "GetOverlappedResult",
        &ORIGINAL_GET_OVERLAPPED_RESULT,
        GetOverlappedResult as GetOverlappedResultFn,
        comm::hooked_get_overlapped_result as *const u8,
    )?;
    queue_iat(
        installer,
        "close-handle",
        "CloseHandle",
        &ORIGINAL_CLOSE_HANDLE,
        CloseHandle as CloseHandleFn,
        io::hooked_close_handle as *const u8,
    )
}

fn queue_iat<T: Copy>(
    installer: &mut HookInstaller,
    hook_name: &'static str,
    function_name: &'static str,
    original_slot: &'static OnceLock<T>,
    original: T,
    hook: *const u8,
) -> Result<(), HookFailure> {
    original_slot
        .set(original)
        .map_err(|_| HookFailure::RuntimeState(hook_name))?;
    installer.iat(hook_name, "KERNEL32.dll", function_name, hook)
}

fn write_count(output: *mut u32, count: usize) -> BOOL {
    if output.is_null() {
        return 0;
    }
    let Ok(count) = u32::try_from(count) else {
        return 0;
    };
    // SAFETY: The caller supplied a nonnull byte-count output pointer.
    unsafe { output.write(count) };
    TRUE
}
