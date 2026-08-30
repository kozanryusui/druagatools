//! Typed COM3 board protocol.

use thiserror::Error;

pub const BOARD_REQUEST_BYTES: usize = 12;
pub const BOARD_RESPONSE_BYTES: usize = 8;

const STARTUP_WRITES: usize = 3;
const SELECT_UP_INPUT_BIT: u8 = 0x01;
const SELECT_DOWN_INPUT_BIT: u8 = 0x02;
const TEST_INPUT_BIT: u8 = 0x04;
const ENTER_INPUT_BIT: u8 = 0x08;
const SERVICE_INPUT_BIT: u8 = 0x01;
const COIN_COUNTER_MASK: u16 = 0x3fff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolState {
    Startup,
    AwaitingFirstResponse,
    AwaitingMatchingResponse,
    Ready,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoardCommandId {
    Exchange,
    Startup,
    Unknown(u8),
}

impl BoardCommandId {
    pub const fn deserialize(raw: u8) -> Self {
        match raw {
            0x80 => Self::Exchange,
            0x81 => Self::Startup,
            value => Self::Unknown(value),
        }
    }

    pub const fn serialize(self) -> u8 {
        match self {
            Self::Exchange => 0x80,
            Self::Startup => 0x81,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoardExchangeMode {
    Initialization,
    Value30,
    Value38,
    Unknown(u8),
}

impl BoardExchangeMode {
    pub const fn deserialize(raw: u8) -> Self {
        match raw {
            0x00 => Self::Initialization,
            0x30 => Self::Value30,
            0x38 => Self::Value38,
            value => Self::Unknown(value),
        }
    }

    pub const fn serialize(self) -> u8 {
        match self {
            Self::Initialization => 0x00,
            Self::Value30 => 0x30,
            Self::Value38 => 0x38,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoardClientRequest {
    Startup {
        unknown: [u8; 10],
    },
    Exchange {
        mode: BoardExchangeMode,
        reserved: u8,
        unknown_output: [u8; 8],
    },
    Unsupported {
        command: BoardCommandId,
        raw_payload: [u8; 10],
    },
}

impl BoardClientRequest {
    pub fn deserialize(raw: &[u8]) -> Result<Self, BoardError> {
        let frame: [u8; BOARD_REQUEST_BYTES] =
            raw.try_into().map_err(|_| BoardError::RequestLength)?;
        let checksum = frame[..BOARD_REQUEST_BYTES - 1]
            .iter()
            .copied()
            .fold(0_u8, u8::wrapping_add)
            & 0x7f;
        if frame[BOARD_REQUEST_BYTES - 1] != checksum {
            return Err(BoardError::RequestChecksum);
        }

        let command = BoardCommandId::deserialize(frame[0]);
        match command {
            BoardCommandId::Startup => {
                let mut unknown = [0_u8; 10];
                unknown.copy_from_slice(&frame[1..11]);
                Ok(Self::Startup { unknown })
            }
            BoardCommandId::Exchange => {
                let mut unknown_output = [0_u8; 8];
                unknown_output.copy_from_slice(&frame[3..11]);
                Ok(Self::Exchange {
                    mode: BoardExchangeMode::deserialize(frame[1]),
                    reserved: frame[2],
                    unknown_output,
                })
            }
            BoardCommandId::Unknown(_) => {
                let mut raw_payload = [0_u8; 10];
                raw_payload.copy_from_slice(&frame[1..11]);
                Ok(Self::Unsupported {
                    command,
                    raw_payload,
                })
            }
        }
    }

    pub const fn command_id(self) -> BoardCommandId {
        match self {
            Self::Startup { .. } => BoardCommandId::Startup,
            Self::Exchange { .. } => BoardCommandId::Exchange,
            Self::Unsupported { command, .. } => command,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoardResponseHeader {
    Accepted,
}

impl BoardResponseHeader {
    const fn serialize(self) -> u8 {
        match self {
            Self::Accepted => 0x80,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorAction {
    SelectUp,
    SelectDown,
    Test,
    Enter,
    Service,
    Coin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorInputState {
    Released,
    Pressed,
}

impl OperatorInputState {
    const fn is_pressed(self) -> bool {
        matches!(self, Self::Pressed)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperatorInputEvent {
    pub action: OperatorAction,
    pub state: OperatorInputState,
}

impl OperatorInputEvent {
    pub const fn new(action: OperatorAction, state: OperatorInputState) -> Self {
        Self { action, state }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BoardOperatorState {
    select_up: OperatorInputState,
    select_down: OperatorInputState,
    test: OperatorInputState,
    enter: OperatorInputState,
    service: OperatorInputState,
    coin_counter: u16,
}

impl BoardOperatorState {
    const fn new() -> Self {
        Self {
            select_up: OperatorInputState::Released,
            select_down: OperatorInputState::Released,
            test: OperatorInputState::Released,
            enter: OperatorInputState::Released,
            service: OperatorInputState::Released,
            coin_counter: 0,
        }
    }

    fn apply(&mut self, event: OperatorInputEvent) {
        match event.action {
            OperatorAction::SelectUp => self.select_up = event.state,
            OperatorAction::SelectDown => self.select_down = event.state,
            OperatorAction::Test => self.test = event.state,
            OperatorAction::Enter => self.enter = event.state,
            OperatorAction::Service => self.service = event.state,
            OperatorAction::Coin if event.state == OperatorInputState::Pressed => {
                self.coin_counter = self.coin_counter.wrapping_add(1) & COIN_COUNTER_MASK;
            }
            OperatorAction::Coin => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoardResponse {
    Status {
        header: BoardResponseHeader,
        select_up: OperatorInputState,
        select_down: OperatorInputState,
        test: OperatorInputState,
        enter: OperatorInputState,
        service: OperatorInputState,
        coin_counter: u16,
        unknown_bytes_5_to_6: [u8; 2],
    },
}

impl BoardResponse {
    pub const fn compatibility() -> Self {
        Self::from_operator_state(BoardOperatorState::new())
    }

    const fn from_operator_state(operator: BoardOperatorState) -> Self {
        Self::Status {
            header: BoardResponseHeader::Accepted,
            select_up: operator.select_up,
            select_down: operator.select_down,
            test: operator.test,
            enter: operator.enter,
            service: operator.service,
            coin_counter: operator.coin_counter,
            unknown_bytes_5_to_6: [0; 2],
        }
    }

    pub fn serialize(self) -> [u8; BOARD_RESPONSE_BYTES] {
        let Self::Status {
            header,
            select_up,
            select_down,
            test,
            enter,
            service,
            coin_counter,
            unknown_bytes_5_to_6,
        } = self;
        let mut raw = [0_u8; BOARD_RESPONSE_BYTES];
        raw[0] = header.serialize();
        raw[1] = SELECT_UP_INPUT_BIT | SELECT_DOWN_INPUT_BIT | TEST_INPUT_BIT | ENTER_INPUT_BIT;
        if select_up.is_pressed() {
            raw[1] &= !SELECT_UP_INPUT_BIT;
        }
        if select_down.is_pressed() {
            raw[1] &= !SELECT_DOWN_INPUT_BIT;
        }
        if test.is_pressed() {
            raw[1] &= !TEST_INPUT_BIT;
        }
        if enter.is_pressed() {
            raw[1] &= !ENTER_INPUT_BIT;
        }
        raw[2] = SERVICE_INPUT_BIT;
        if service.is_pressed() {
            raw[2] &= !SERVICE_INPUT_BIT;
        }
        raw[3] = (coin_counter & 0x7f) as u8;
        raw[4] = ((coin_counter >> 7) & 0x7f) as u8;
        raw[5..7].copy_from_slice(&unknown_bytes_5_to_6);
        raw[7] = raw[..7].iter().copied().fold(0_u8, u8::wrapping_add) & 0x7f;
        raw
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum BoardError {
    #[error("the request must contain exactly 12 bytes")]
    RequestLength,
    #[error("the request checksum does not satisfy the verified predicate")]
    RequestChecksum,
    #[error("the request does not match the verified startup sequence")]
    Sequence,
    #[error("the delivered response differs from the first delivered response")]
    DuplicateMismatch,
}

#[derive(Debug)]
pub struct BoardProtocol {
    state: ProtocolState,
    startup_writes: usize,
    first_response: Option<BoardResponse>,
    operator: BoardOperatorState,
}

impl BoardProtocol {
    pub const fn new() -> Self {
        Self {
            state: ProtocolState::Startup,
            startup_writes: 0,
            first_response: None,
            operator: BoardOperatorState::new(),
        }
    }

    pub const fn state(&self) -> ProtocolState {
        self.state
    }

    pub const fn reset(&mut self) {
        self.state = ProtocolState::Startup;
        self.startup_writes = 0;
        self.first_response = None;
        self.operator = BoardOperatorState::new();
    }

    pub fn apply_operator_event(&mut self, event: OperatorInputEvent) {
        self.operator.apply(event);
    }

    pub fn handle(
        &mut self,
        request: BoardClientRequest,
    ) -> Result<Option<BoardResponse>, BoardError> {
        self.accept_startup_request(request)?;

        let response = match self.state {
            ProtocolState::Startup => return Ok(None),
            ProtocolState::AwaitingFirstResponse => {
                let response = BoardResponse::from_operator_state(self.operator);
                self.first_response = Some(response);
                response
            }
            ProtocolState::AwaitingMatchingResponse => {
                self.first_response.ok_or(BoardError::Sequence)?
            }
            ProtocolState::Ready => BoardResponse::from_operator_state(self.operator),
        };
        Ok(Some(response))
    }

    pub fn accept_delivered_response(&mut self, response: BoardResponse) -> Result<(), BoardError> {
        match self.state {
            ProtocolState::AwaitingFirstResponse => {
                if self.first_response != Some(response) {
                    return Err(BoardError::DuplicateMismatch);
                }
                self.state = ProtocolState::AwaitingMatchingResponse;
                Ok(())
            }
            ProtocolState::AwaitingMatchingResponse => {
                if self.first_response != Some(response) {
                    return Err(BoardError::DuplicateMismatch);
                }
                self.state = ProtocolState::Ready;
                Ok(())
            }
            ProtocolState::Ready => Ok(()),
            ProtocolState::Startup => Err(BoardError::Sequence),
        }
    }

    fn accept_startup_request(&mut self, request: BoardClientRequest) -> Result<(), BoardError> {
        if !matches!(
            self.state,
            ProtocolState::Startup | ProtocolState::AwaitingFirstResponse
        ) {
            return Ok(());
        }

        let valid = if self.startup_writes < STARTUP_WRITES {
            matches!(request, BoardClientRequest::Startup { unknown } if unknown == [0; 10])
        } else if self.startup_writes == STARTUP_WRITES {
            matches!(
                request,
                BoardClientRequest::Exchange {
                    mode: BoardExchangeMode::Initialization,
                    reserved: 0,
                    unknown_output,
                }
                if unknown_output == [0; 8]
            )
        } else {
            false
        };
        if !valid {
            return Err(BoardError::Sequence);
        }

        self.startup_writes = self.startup_writes.saturating_add(1);
        if self.startup_writes > STARTUP_WRITES {
            self.state = ProtocolState::AwaitingFirstResponse;
        }
        Ok(())
    }
}

impl Default for BoardProtocol {
    fn default() -> Self {
        Self::new()
    }
}
