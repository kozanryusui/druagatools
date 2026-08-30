//! Exact-handle Tower serial dispatch.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use druaga_tower_board::{
    BoardClientRequest, BoardError, BoardProtocol, BoardResponse, OperatorInputEvent,
};

use crate::card::CardError;
use crate::reader_protocol::{
    ReaderClientRequest, ReaderProtocol, ReaderProtocolError, ReaderResponse, ReaderSide,
};

#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod operator_input;
#[cfg(windows)]
mod windows_ffi;

#[cfg(windows)]
pub(crate) use windows_ffi::queue_hooks;

#[cfg(windows)]
pub(crate) fn configure(
    card_directory: PathBuf,
    logging: crate::config::SerialLoggingConfig,
) -> Result<(), crate::HookFailure> {
    windows_ffi::configure(card_directory, logging)
}

pub const OBSERVED_COM3_ACCESS: u32 = 0xc000_0000;
pub const OBSERVED_COM3_SHARE: u32 = 0;
pub const OBSERVED_COM3_DISPOSITION: u32 = 3;
pub const OBSERVED_COM3_FLAGS: u32 = 0x4000_0000;
pub const PST_RS232_VALUE: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialDevice {
    Reader(ReaderSide),
    Board,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialPort {
    Com1,
    Com2,
    Com3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CardMountDisposition {
    Mounted(ReaderSide),
    AlreadyMounted(ReaderSide),
    NoAbsentReader,
}

impl SerialPort {
    const fn index(self) -> usize {
        match self {
            Self::Com1 => 0,
            Self::Com2 => 1,
            Self::Com3 => 2,
        }
    }

    pub const fn device(self) -> SerialDevice {
        match self {
            Self::Com1 => SerialDevice::Reader(ReaderSide::Left),
            Self::Com2 => SerialDevice::Reader(ReaderSide::Right),
            Self::Com3 => SerialDevice::Board,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Com1 => "COM1",
            Self::Com2 => "COM2",
            Self::Com3 => "COM3",
        }
    }

    const fn default_state(self) -> SerialPortState {
        match self.device() {
            SerialDevice::Reader(_) => SerialPortState::reader_default(),
            SerialDevice::Board => SerialPortState::tower_default(),
        }
    }

    const fn reader_index(self) -> Option<usize> {
        match self {
            Self::Com1 => Some(0),
            Self::Com2 => Some(1),
            Self::Com3 => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SerialHandle(isize);

impl SerialHandle {
    pub const fn new(raw: isize) -> Self {
        Self(raw)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreateFileRequest<'a> {
    pub name: &'a [u8],
    pub desired_access: u32,
    pub share_mode: u32,
    pub creation_disposition: u32,
    pub flags_and_attributes: u32,
}

impl CreateFileRequest<'_> {
    pub fn observed_port(self) -> Option<SerialPort> {
        let port = match self.name {
            b"COM1" => SerialPort::Com1,
            b"COM2" => SerialPort::Com2,
            b"COM3" => SerialPort::Com3,
            _ => return None,
        };
        (self.desired_access == OBSERVED_COM3_ACCESS
            && self.share_mode == OBSERVED_COM3_SHARE
            && self.creation_disposition == OBSERVED_COM3_DISPOSITION
            && self.flags_and_attributes == OBSERVED_COM3_FLAGS)
            .then_some(port)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialCall<T> {
    Emulated(T),
    Forward,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseDisposition {
    CloseEvent,
    AlreadyClosed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialParity {
    None,
    Even,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialStopBits {
    One,
    Two,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SerialPortState {
    pub baud_rate: u32,
    pub byte_size: u8,
    pub parity: SerialParity,
    pub stop_bits: SerialStopBits,
}

impl SerialPortState {
    pub const fn tower_default() -> Self {
        Self {
            baud_rate: 9_600,
            byte_size: 8,
            parity: SerialParity::None,
            stop_bits: SerialStopBits::One,
        }
    }

    pub const fn reader_default() -> Self {
        Self {
            baud_rate: 19_200,
            byte_size: 8,
            parity: SerialParity::Even,
            stop_bits: SerialStopBits::Two,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OpenTransport {
    port: SerialPort,
    handle: SerialHandle,
}

#[derive(Debug)]
pub struct SerialDispatch {
    transports: [Option<OpenTransport>; 3],
    last_closed_handles: [Option<SerialHandle>; 3],
    readers: [ReaderProtocol; 2],
    reader_responses: [VecDeque<ReaderResponse>; 2],
    board: BoardProtocol,
    board_responses: VecDeque<BoardResponse>,
    card_directory: PathBuf,
}

impl SerialDispatch {
    pub fn new(card_directory: PathBuf) -> Self {
        Self {
            transports: [None; 3],
            last_closed_handles: [None; 3],
            readers: [
                ReaderProtocol::new(ReaderSide::Left),
                ReaderProtocol::new(ReaderSide::Right),
            ],
            reader_responses: [VecDeque::new(), VecDeque::new()],
            board: BoardProtocol::new(),
            board_responses: VecDeque::new(),
            card_directory,
        }
    }

    pub fn apply_operator_event(&mut self, event: OperatorInputEvent) {
        self.board.apply_operator_event(event);
    }

    /// Mount a numbered card in the selected reader.
    ///
    /// Each numbered key has one fixed reader. A mounted identifier still has
    /// one owner, which prevents the same card from being in both slots.
    pub fn mount_card(
        &mut self,
        side: ReaderSide,
        number: u8,
    ) -> Result<CardMountDisposition, CardError> {
        if let Some(reader) = self
            .readers
            .iter()
            .find(|reader| reader.mounted_card_number() == Some(number))
        {
            return Ok(CardMountDisposition::AlreadyMounted(reader.side()));
        }
        let reader_index = match side {
            ReaderSide::Left => 0,
            ReaderSide::Right => 1,
        };
        let reader = &mut self.readers[reader_index];
        if !reader.is_absent() {
            return Ok(CardMountDisposition::NoAbsentReader);
        }
        reader.mount(Path::new(&self.card_directory), number)?;
        Ok(CardMountDisposition::Mounted(side))
    }

    pub fn create_file(
        &mut self,
        request: CreateFileRequest<'_>,
        event_handle: SerialHandle,
    ) -> SerialCall<SerialHandle> {
        let Some(port) = request.observed_port() else {
            return SerialCall::Forward;
        };
        let index = port.index();
        if self.transports[index].is_some() {
            return SerialCall::Forward;
        }
        self.reset_protocol(port);
        self.transports[index] = Some(OpenTransport {
            port,
            handle: event_handle,
        });
        self.last_closed_handles[index] = None;
        SerialCall::Emulated(event_handle)
    }

    pub fn close_handle(&mut self, handle: SerialHandle) -> SerialCall<CloseDisposition> {
        if self.last_closed_handles.contains(&Some(handle)) {
            return SerialCall::Emulated(CloseDisposition::AlreadyClosed);
        }
        let Some(port) = self.port(handle) else {
            return SerialCall::Forward;
        };
        self.reset_protocol(port);
        let index = port.index();
        self.transports[index] = None;
        self.last_closed_handles[index] = Some(handle);
        SerialCall::Emulated(CloseDisposition::CloseEvent)
    }

    pub fn get_comm_properties(&self, handle: SerialHandle) -> SerialCall<u32> {
        self.for_open_handle(handle, PST_RS232_VALUE)
    }

    pub fn get_comm_state(&self, handle: SerialHandle) -> SerialCall<SerialPortState> {
        match self.port(handle) {
            Some(port) => SerialCall::Emulated(port.default_state()),
            None => SerialCall::Forward,
        }
    }

    pub fn set_comm_state(&self, handle: SerialHandle, state: SerialPortState) -> SerialCall<bool> {
        match self.port(handle) {
            Some(port) => SerialCall::Emulated(state == port.default_state()),
            None => SerialCall::Forward,
        }
    }

    pub fn set_comm_timeouts(&self, handle: SerialHandle) -> SerialCall<bool> {
        self.for_open_handle(handle, true)
    }

    pub fn clear_comm_error(&self, handle: SerialHandle) -> SerialCall<usize> {
        self.for_open_handle(handle, 0)
    }

    pub fn write_reader_frame(
        &mut self,
        handle: SerialHandle,
        raw: &[u8],
    ) -> Result<SerialCall<usize>, ReaderProtocolError> {
        let Some(port) = self.port(handle) else {
            return Ok(SerialCall::Forward);
        };
        let Some(reader_index) = port.reader_index() else {
            return Ok(SerialCall::Forward);
        };
        if port.device() != SerialDevice::Reader(self.readers[reader_index].side()) {
            return Ok(SerialCall::Forward);
        }

        let request = ReaderClientRequest::deserialize(raw)?;
        if let Some(response) = self.readers[reader_index].handle(request)? {
            self.reader_responses[reader_index].push_back(response);
        }
        Ok(SerialCall::Emulated(raw.len()))
    }

    pub fn read_reader_response(
        &mut self,
        handle: SerialHandle,
    ) -> SerialCall<Option<ReaderResponse>> {
        let Some(port) = self.port(handle) else {
            return SerialCall::Forward;
        };
        let Some(reader_index) = port.reader_index() else {
            return SerialCall::Forward;
        };
        SerialCall::Emulated(self.reader_responses[reader_index].pop_front())
    }

    pub fn write_board_frame(
        &mut self,
        handle: SerialHandle,
        raw: &[u8],
    ) -> Result<SerialCall<usize>, BoardError> {
        if self.port(handle) != Some(SerialPort::Com3) {
            return Ok(SerialCall::Forward);
        }

        let request = BoardClientRequest::deserialize(raw)?;
        if let Some(response) = self.board.handle(request)?
            && self.board_responses.is_empty()
        {
            self.board_responses.push_back(response);
        }
        Ok(SerialCall::Emulated(raw.len()))
    }

    pub fn read_board_response(
        &mut self,
        handle: SerialHandle,
    ) -> Result<SerialCall<Option<BoardResponse>>, BoardError> {
        if self.port(handle) != Some(SerialPort::Com3) {
            return Ok(SerialCall::Forward);
        }
        let Some(response) = self.board_responses.pop_front() else {
            return Ok(SerialCall::Emulated(None));
        };

        self.board.accept_delivered_response(response)?;
        Ok(SerialCall::Emulated(Some(response)))
    }

    pub fn get_overlapped_result(&self, handle: SerialHandle) -> SerialCall<usize> {
        self.for_open_handle(handle, 0)
    }

    pub fn port(&self, handle: SerialHandle) -> Option<SerialPort> {
        self.transports
            .iter()
            .flatten()
            .find(|transport| transport.handle == handle)
            .map(|transport| transport.port)
    }

    fn reset_protocol(&mut self, port: SerialPort) {
        match port.reader_index() {
            Some(reader_index) => self.reader_responses[reader_index].clear(),
            None => {
                self.board.reset();
                self.board_responses.clear();
            }
        }
    }

    fn for_open_handle<T>(&self, handle: SerialHandle, value: T) -> SerialCall<T> {
        if self.port(handle).is_some() {
            SerialCall::Emulated(value)
        } else {
            SerialCall::Forward
        }
    }
}

impl Default for SerialDispatch {
    fn default() -> Self {
        Self::new(PathBuf::from("cards"))
    }
}
