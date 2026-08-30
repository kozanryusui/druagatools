use super::*;

pub(super) unsafe extern "system" fn hooked_get_comm_properties(
    handle: HANDLE,
    properties: *mut COMMPROP,
) -> BOOL {
    let Some(original) = ORIGINAL_GET_COMM_PROPERTIES.get() else {
        return 0;
    };
    let call = {
        let state = runtime()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.dispatch.get_comm_properties(as_serial_handle(handle))
    };
    match call {
        SerialCall::Forward => unsafe { original(handle, properties) },
        SerialCall::Emulated(provider_subtype) => {
            if properties.is_null() {
                return 0;
            }
            let Ok(packet_length) = u16::try_from(std::mem::size_of::<COMMPROP>()) else {
                return 0;
            };
            let value = COMMPROP {
                wPacketLength: packet_length,
                dwProvSubType: provider_subtype,
                ..COMMPROP::default()
            };
            // SAFETY: The caller supplied a nonnull writable COMMPROP pointer.
            unsafe { properties.write(value) };
            TRUE
        }
    }
}

pub(super) unsafe extern "system" fn hooked_get_comm_state(handle: HANDLE, dcb: *mut DCB) -> BOOL {
    let Some(original) = ORIGINAL_GET_COMM_STATE.get() else {
        return 0;
    };
    let call = {
        let state = runtime()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.dispatch.get_comm_state(as_serial_handle(handle))
    };
    match call {
        SerialCall::Forward => unsafe { original(handle, dcb) },
        SerialCall::Emulated(serial) => {
            if dcb.is_null() {
                return 0;
            }
            let Ok(length) = u32::try_from(std::mem::size_of::<DCB>()) else {
                return 0;
            };
            let (bitfield, parity, stop_bits) = match (serial.parity, serial.stop_bits) {
                (SerialParity::None, SerialStopBits::One) => (1, NOPARITY, ONESTOPBIT),
                (SerialParity::Even, SerialStopBits::Two) => (3, EVENPARITY, TWOSTOPBITS),
                _ => return 0,
            };
            let value = DCB {
                DCBlength: length,
                BaudRate: serial.baud_rate,
                _bitfield: bitfield,
                ByteSize: serial.byte_size,
                Parity: parity,
                StopBits: stop_bits,
                ..DCB::default()
            };
            // SAFETY: The caller supplied a nonnull writable DCB pointer.
            unsafe { dcb.write(value) };
            TRUE
        }
    }
}

pub(super) unsafe extern "system" fn hooked_set_comm_state(
    handle: HANDLE,
    dcb: *const DCB,
) -> BOOL {
    let Some(original) = ORIGINAL_SET_COMM_STATE.get() else {
        return 0;
    };
    let state = runtime()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let serial_handle = as_serial_handle(handle);
    if matches!(
        state.dispatch.get_comm_state(serial_handle),
        SerialCall::Forward
    ) {
        drop(state);
        return unsafe { original(handle, dcb) };
    }
    if dcb.is_null() {
        return 0;
    }
    // SAFETY: The pointer was checked and SetCommState requires a readable DCB.
    let value = unsafe { dcb.read() };
    let parity = match value.Parity {
        NOPARITY => SerialParity::None,
        EVENPARITY => SerialParity::Even,
        _ => return 0,
    };
    let stop_bits = match value.StopBits {
        ONESTOPBIT => SerialStopBits::One,
        TWOSTOPBITS => SerialStopBits::Two,
        _ => return 0,
    };
    let serial = SerialPortState {
        baud_rate: value.BaudRate,
        byte_size: value.ByteSize,
        parity,
        stop_bits,
    };
    match state.dispatch.set_comm_state(serial_handle, serial) {
        SerialCall::Forward => 0,
        SerialCall::Emulated(true) => TRUE,
        SerialCall::Emulated(false) => 0,
    }
}

pub(super) unsafe extern "system" fn hooked_set_comm_timeouts(
    handle: HANDLE,
    timeouts: *const COMMTIMEOUTS,
) -> BOOL {
    let Some(original) = ORIGINAL_SET_COMM_TIMEOUTS.get() else {
        return 0;
    };
    let call = {
        let state = runtime()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.dispatch.set_comm_timeouts(as_serial_handle(handle))
    };
    match call {
        SerialCall::Forward => unsafe { original(handle, timeouts) },
        SerialCall::Emulated(_) if timeouts.is_null() => 0,
        SerialCall::Emulated(_) => TRUE,
    }
}

pub(super) unsafe extern "system" fn hooked_clear_comm_error(
    handle: HANDLE,
    errors: *mut u32,
    status: *mut COMSTAT,
) -> BOOL {
    let Some(original) = ORIGINAL_CLEAR_COMM_ERROR.get() else {
        return 0;
    };
    let call = {
        let state = runtime()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.dispatch.clear_comm_error(as_serial_handle(handle))
    };
    match call {
        SerialCall::Forward => unsafe { original(handle, errors, status) },
        SerialCall::Emulated(_) => {
            if errors.is_null() || status.is_null() {
                return 0;
            }
            // SAFETY: Both pointers were checked and the API requires writable outputs.
            unsafe {
                errors.write(0);
                status.write(COMSTAT::default());
            }
            TRUE
        }
    }
}

pub(super) unsafe extern "system" fn hooked_get_overlapped_result(
    handle: HANDLE,
    overlapped: *const OVERLAPPED,
    bytes_transferred: *mut u32,
    wait: BOOL,
) -> BOOL {
    let Some(original) = ORIGINAL_GET_OVERLAPPED_RESULT.get() else {
        return 0;
    };
    let call = {
        let state = runtime()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .dispatch
            .get_overlapped_result(as_serial_handle(handle))
    };
    match call {
        SerialCall::Forward => unsafe { original(handle, overlapped, bytes_transferred, wait) },
        SerialCall::Emulated(count) => write_count(bytes_transferred, count),
    }
}
