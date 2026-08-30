use super::*;

pub(super) unsafe extern "system" fn hooked_create_file_a(
    name: *const u8,
    desired_access: u32,
    share_mode: u32,
    security_attributes: *const SECURITY_ATTRIBUTES,
    creation_disposition: u32,
    flags_and_attributes: u32,
    template_file: HANDLE,
) -> HANDLE {
    let Some(original) = ORIGINAL_CREATE_FILE_A.get() else {
        return create_file_failure(ERROR_INVALID_FUNCTION);
    };
    let Some(name_bytes) = read_observed_serial_name(name) else {
        return unsafe {
            original(
                name,
                desired_access,
                share_mode,
                security_attributes,
                creation_disposition,
                flags_and_attributes,
                template_file,
            )
        };
    };
    let request = CreateFileRequest {
        name: &name_bytes,
        desired_access,
        share_mode,
        creation_disposition,
        flags_and_attributes,
    };
    let Some(port) = request.observed_port() else {
        return unsafe {
            original(
                name,
                desired_access,
                share_mode,
                security_attributes,
                creation_disposition,
                flags_and_attributes,
                template_file,
            )
        };
    };
    let mut state = runtime()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if port == SerialPort::Com3 {
        state.operator_input.reset();
        if state.capture.prepare().is_err() {
            return create_file_failure(ERROR_OPEN_FAILED);
        }
    }
    // SAFETY: Null security and name pointers request one unnamed manual-reset event.
    let event = unsafe { CreateEventA(std::ptr::null(), TRUE, TRUE, std::ptr::null()) };
    if event.is_null() {
        return create_file_failure(unsafe { GetLastError() });
    }
    match state.dispatch.create_file(request, as_serial_handle(event)) {
        SerialCall::Emulated(_) => event,
        SerialCall::Forward => {
            drop(state);
            if let Some(close_original) = ORIGINAL_CLOSE_HANDLE.get() {
                // SAFETY: The event was created here and is not published.
                let _ = unsafe { close_original(event) };
            }
            unsafe {
                original(
                    name,
                    desired_access,
                    share_mode,
                    security_attributes,
                    creation_disposition,
                    flags_and_attributes,
                    template_file,
                )
            }
        }
    }
}

pub(super) unsafe extern "system" fn hooked_read_file(
    handle: HANDLE,
    buffer: *mut u8,
    bytes_to_read: u32,
    bytes_read: *mut u32,
    overlapped: *mut OVERLAPPED,
) -> BOOL {
    let Some(original) = ORIGINAL_READ_FILE.get() else {
        return 0;
    };
    let mut state = runtime()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let serial_handle = as_serial_handle(handle);
    let Some(port) = state.dispatch.port(serial_handle) else {
        drop(state);
        return unsafe { original(handle, buffer, bytes_to_read, bytes_read, overlapped) };
    };
    match port {
        SerialPort::Com3 => {
            if bytes_to_read < 8 || buffer.is_null() {
                return 0;
            }
            match state.dispatch.read_board_response(serial_handle) {
                Ok(SerialCall::Emulated(Some(response))) => {
                    let reply = response.serialize();
                    // SAFETY: ReadFile supplies a writable buffer of bytes_to_read bytes.
                    unsafe { std::ptr::copy_nonoverlapping(reply.as_ptr(), buffer, reply.len()) };
                    if state.serial_logging_enabled(port) {
                        log_board_frame("board-to-tower", &reply);
                    }
                    let _ = state.capture.record_board_response(&reply);
                    write_count(bytes_read, reply.len())
                }
                Ok(SerialCall::Emulated(None)) => write_count(bytes_read, 0),
                Ok(SerialCall::Forward) | Err(_) => 0,
            }
        }
        SerialPort::Com1 | SerialPort::Com2 => {
            if bytes_to_read != 0 && buffer.is_null() {
                return 0;
            }
            match state.dispatch.read_reader_response(serial_handle) {
                SerialCall::Emulated(Some(response)) => {
                    let reply = response.serialize();
                    let Ok(capacity) = usize::try_from(bytes_to_read) else {
                        return 0;
                    };
                    if capacity < reply.len() || buffer.is_null() {
                        return 0;
                    }
                    // SAFETY: ReadFile supplies a writable buffer of bytes_to_read bytes.
                    unsafe { std::ptr::copy_nonoverlapping(reply.as_ptr(), buffer, reply.len()) };
                    if state.serial_logging_enabled(port) {
                        log_reader_frame(port, "reader-to-tower", &reply);
                        log_reader_read(port, bytes_to_read, reply.len());
                    }
                    write_count(bytes_read, reply.len())
                }
                SerialCall::Emulated(None) => {
                    if state.serial_logging_enabled(port) {
                        log_reader_read(port, bytes_to_read, 0);
                    }
                    write_count(bytes_read, 0)
                }
                SerialCall::Forward => 0,
            }
        }
    }
}

pub(super) unsafe extern "system" fn hooked_write_file(
    handle: HANDLE,
    buffer: *const u8,
    bytes_to_write: u32,
    bytes_written: *mut u32,
    overlapped: *mut OVERLAPPED,
) -> BOOL {
    let Some(original) = ORIGINAL_WRITE_FILE.get() else {
        return 0;
    };
    let mut state = runtime()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let serial_handle = as_serial_handle(handle);
    let Some(port) = state.dispatch.port(serial_handle) else {
        drop(state);
        return unsafe { original(handle, buffer, bytes_to_write, bytes_written, overlapped) };
    };
    let Ok(length) = usize::try_from(bytes_to_write) else {
        return 0;
    };
    if buffer.is_null() && length != 0 {
        return 0;
    }
    let raw = if length == 0 {
        &[]
    } else {
        // SAFETY: WriteFile requires a readable buffer of bytes_to_write bytes.
        unsafe { std::slice::from_raw_parts(buffer, length) }
    };
    if let Some(request) = state.operator_input.poll_card_mount() {
        let number = request.number;
        let requested_side = request.side;
        match state.dispatch.mount_card(requested_side, number) {
            Ok(CardMountDisposition::Mounted(side)) => {
                crate::platform::log(&format!("card-mounted number={number} reader={side:?}"))
            }
            Ok(CardMountDisposition::AlreadyMounted(side)) => crate::platform::log(&format!(
                "card-mount-ignored number={number} reader={side:?} reason=already-mounted"
            )),
            Ok(CardMountDisposition::NoAbsentReader) => crate::platform::log(&format!(
                "card-mount-ignored number={number} reader={requested_side:?} reason=reader-occupied"
            )),
            Err(error) => {
                crate::platform::log(&format!("card-mount-failed number={number} error={error}"))
            }
        }
    }
    if matches!(port, SerialPort::Com1 | SerialPort::Com2) {
        if state.serial_logging_enabled(port) {
            log_reader_frame(port, "tower-to-reader", raw);
        }
        return match state.dispatch.write_reader_frame(serial_handle, raw) {
            Ok(SerialCall::Emulated(count)) => write_count(bytes_written, count),
            Ok(SerialCall::Forward) | Err(_) => 0,
        };
    }
    for event in state.operator_input.poll().into_iter().flatten() {
        state.dispatch.apply_operator_event(event);
    }
    let count = match state.dispatch.write_board_frame(serial_handle, raw) {
        Ok(SerialCall::Emulated(count)) => count,
        Ok(SerialCall::Forward) | Err(_) => return 0,
    };
    if state.serial_logging_enabled(port) {
        log_board_frame("tower-to-board", raw);
    }
    let _ = state.capture.record_board_request(raw);
    write_count(bytes_written, count)
}

pub(super) unsafe extern "system" fn hooked_close_handle(handle: HANDLE) -> BOOL {
    let Some(original) = ORIGINAL_CLOSE_HANDLE.get() else {
        return 0;
    };
    let call = {
        let mut state = runtime()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.dispatch.close_handle(as_serial_handle(handle))
    };
    match call {
        SerialCall::Forward | SerialCall::Emulated(CloseDisposition::CloseEvent) => unsafe {
            original(handle)
        },
        SerialCall::Emulated(CloseDisposition::AlreadyClosed) => TRUE,
    }
}

fn read_observed_serial_name(name: *const u8) -> Option<[u8; 4]> {
    if name.is_null() {
        return None;
    }
    // SAFETY: CreateFileA requires a valid null-terminated name.
    let bytes = unsafe { CStr::from_ptr(name.cast()) }.to_bytes();
    match bytes {
        b"COM1" => Some([b'C', b'O', b'M', b'1']),
        b"COM2" => Some([b'C', b'O', b'M', b'2']),
        b"COM3" => Some([b'C', b'O', b'M', b'3']),
        _ => None,
    }
}
