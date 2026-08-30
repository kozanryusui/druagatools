//! Typed COM1 and COM2 reader protocol.

use std::path::Path;

use thiserror::Error;

use crate::card::{CardError, MountedCard};

const FRAME_START: u8 = 0x02;
const PAYLOAD_END: u8 = 0x03;
const FRAME_END: [u8; 2] = [0x0d, 0x0a];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderCommandId {
    Command40,
    Command41,
    Command42,
    Command43,
    Command44,
    Command45,
    Command46,
    Command47,
    Command48,
    Command49,
    Command50,
    Command56,
    Command80,
    Command81,
    Command82,
    Command83,
    Command9f,
    Unknown(u8),
}

impl ReaderCommandId {
    pub const fn deserialize(raw: u8) -> Self {
        match raw {
            0x40 => Self::Command40,
            0x41 => Self::Command41,
            0x42 => Self::Command42,
            0x43 => Self::Command43,
            0x44 => Self::Command44,
            0x45 => Self::Command45,
            0x46 => Self::Command46,
            0x47 => Self::Command47,
            0x48 => Self::Command48,
            0x49 => Self::Command49,
            0x50 => Self::Command50,
            0x56 => Self::Command56,
            0x80 => Self::Command80,
            0x81 => Self::Command81,
            0x82 => Self::Command82,
            0x83 => Self::Command83,
            0x9f => Self::Command9f,
            value => Self::Unknown(value),
        }
    }

    pub const fn serialize(self) -> u8 {
        match self {
            Self::Command40 => 0x40,
            Self::Command41 => 0x41,
            Self::Command42 => 0x42,
            Self::Command43 => 0x43,
            Self::Command44 => 0x44,
            Self::Command45 => 0x45,
            Self::Command46 => 0x46,
            Self::Command47 => 0x47,
            Self::Command48 => 0x48,
            Self::Command49 => 0x49,
            Self::Command50 => 0x50,
            Self::Command56 => 0x56,
            Self::Command80 => 0x80,
            Self::Command81 => 0x81,
            Self::Command82 => 0x82,
            Self::Command83 => 0x83,
            Self::Command9f => 0x9f,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderConfigurationSlot {
    Slot00,
    Slot01,
    Slot02,
    Slot03,
    Slot04,
    Slot05,
    Slot06,
    Slot07,
    Slot08,
    Slot09,
    Slot0a,
    Slot0b,
    Slot0c,
    Slot0d,
    Slot0e,
    Slot0f,
    Unknown(u8),
}

impl ReaderConfigurationSlot {
    pub const fn deserialize(raw: u8) -> Self {
        match raw {
            0x00 => Self::Slot00,
            0x01 => Self::Slot01,
            0x02 => Self::Slot02,
            0x03 => Self::Slot03,
            0x04 => Self::Slot04,
            0x05 => Self::Slot05,
            0x06 => Self::Slot06,
            0x07 => Self::Slot07,
            0x08 => Self::Slot08,
            0x09 => Self::Slot09,
            0x0a => Self::Slot0a,
            0x0b => Self::Slot0b,
            0x0c => Self::Slot0c,
            0x0d => Self::Slot0d,
            0x0e => Self::Slot0e,
            0x0f => Self::Slot0f,
            value => Self::Unknown(value),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderIdentity {
    Value35,
    Value36,
    Value40,
    Value50,
    Value60,
}

impl ReaderIdentity {
    const fn decimal(self) -> u8 {
        match self {
            Self::Value35 => 35,
            Self::Value36 => 36,
            Self::Value40 => 40,
            Self::Value50 => 50,
            Self::Value60 => 60,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReaderCount(u8);

impl ReaderCount {
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 99 { Some(Self(value)) } else { None }
    }

    pub const fn zero() -> Self {
        Self(0)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderOperationStatus {
    Success,
    Unknown(u8),
}

impl ReaderOperationStatus {
    const fn serialize(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReaderStatusBits(u8);

impl ReaderStatusBits {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn raw(self) -> u8 {
        self.0
    }

    const fn transport_requested() -> Self {
        Self(0x01)
    }

    const fn ready() -> Self {
        Self(0x02)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReaderClientRequest {
    PrepareAccess {
        magic: [u8; 4],
    },
    Read16 {
        block_index: u8,
    },
    Write16 {
        block_index: u8,
        data: [u8; 16],
    },
    Command43 {
        unknown: [u8; 5],
    },
    ControlOperation7 {
        selector: u8,
        value: u32,
    },
    Command45 {
        unknown: u8,
    },
    Command46 {
        unknown: [u8; 5],
    },
    FinishAccess,
    Read48 {
        block_index: u8,
    },
    Write48 {
        block_index: u8,
        data: [u8; 48],
    },
    Configure {
        slot: ReaderConfigurationSlot,
        unknown: [u8; 12],
    },
    Initialize,
    PollStatus,
    TransportStart {
        action: u8,
    },
    TransportContinue {
        action: u8,
    },
    Identify,
    Command9f {
        unknown: u8,
    },
    Unsupported {
        command: ReaderCommandId,
        raw_payload: Vec<u8>,
    },
}

impl ReaderClientRequest {
    pub fn deserialize(raw: &[u8]) -> Result<Self, ReaderProtocolError> {
        if raw.len() < 7 || raw.first().copied() != Some(FRAME_START) {
            return Err(ReaderProtocolError::FrameStartOrLength);
        }
        let payload_length = usize::from(raw[2]);
        let total_length = payload_length
            .checked_add(7)
            .ok_or(ReaderProtocolError::FrameStartOrLength)?;
        if raw.len() != total_length {
            return Err(ReaderProtocolError::FrameStartOrLength);
        }
        let delimiter_index = payload_length
            .checked_add(3)
            .ok_or(ReaderProtocolError::FrameStartOrLength)?;
        let checksum_index = delimiter_index
            .checked_add(1)
            .ok_or(ReaderProtocolError::FrameStartOrLength)?;
        if raw.get(delimiter_index).copied() != Some(PAYLOAD_END) {
            return Err(ReaderProtocolError::PayloadEnd);
        }
        if raw.get(checksum_index + 1..checksum_index + 3) != Some(FRAME_END.as_slice()) {
            return Err(ReaderProtocolError::FrameEnd);
        }
        let calculated = raw[1..checksum_index]
            .iter()
            .copied()
            .fold(0_u8, |value, byte| value ^ byte);
        if raw.get(checksum_index).copied() != Some(calculated) {
            return Err(ReaderProtocolError::Checksum);
        }

        let command = ReaderCommandId::deserialize(raw[1]);
        let payload = &raw[3..delimiter_index];
        match command {
            ReaderCommandId::Command40 => Ok(Self::PrepareAccess {
                magic: fixed_payload(payload, command)?,
            }),
            ReaderCommandId::Command41 => Ok(Self::Read16 {
                block_index: one_byte_payload(payload, command)?,
            }),
            ReaderCommandId::Command42 => {
                let payload: [u8; 17] = fixed_payload(payload, command)?;
                let mut data = [0; 16];
                data.copy_from_slice(&payload[1..]);
                Ok(Self::Write16 {
                    block_index: payload[0],
                    data,
                })
            }
            ReaderCommandId::Command43 => Ok(Self::Command43 {
                unknown: fixed_payload(payload, command)?,
            }),
            ReaderCommandId::Command44 => {
                let (selector, value) = control_payload(payload, command)?;
                Ok(Self::ControlOperation7 { selector, value })
            }
            ReaderCommandId::Command45 => Ok(Self::Command45 {
                unknown: one_byte_payload(payload, command)?,
            }),
            ReaderCommandId::Command46 => Ok(Self::Command46 {
                unknown: fixed_payload(payload, command)?,
            }),
            ReaderCommandId::Command47 => {
                require_empty_payload(payload, command)?;
                Ok(Self::FinishAccess)
            }
            ReaderCommandId::Command48 => Ok(Self::Read48 {
                block_index: one_byte_payload(payload, command)?,
            }),
            ReaderCommandId::Command49 => {
                let payload: [u8; 49] = fixed_payload(payload, command)?;
                let mut data = [0; 48];
                data.copy_from_slice(&payload[1..]);
                Ok(Self::Write48 {
                    block_index: payload[0],
                    data,
                })
            }
            ReaderCommandId::Command50 => {
                let frame: [u8; 13] = fixed_payload(payload, command)?;
                let mut unknown = [0_u8; 12];
                unknown.copy_from_slice(&frame[1..]);
                Ok(Self::Configure {
                    slot: ReaderConfigurationSlot::deserialize(frame[0]),
                    unknown,
                })
            }
            ReaderCommandId::Command56 => {
                require_empty_payload(payload, command)?;
                Ok(Self::Initialize)
            }
            ReaderCommandId::Command80 => {
                require_empty_payload(payload, command)?;
                Ok(Self::PollStatus)
            }
            ReaderCommandId::Command81 => Ok(Self::TransportStart {
                action: one_byte_payload(payload, command)?,
            }),
            ReaderCommandId::Command82 => Ok(Self::TransportContinue {
                action: one_byte_payload(payload, command)?,
            }),
            ReaderCommandId::Command83 => {
                require_empty_payload(payload, command)?;
                Ok(Self::Identify)
            }
            ReaderCommandId::Command9f => Ok(Self::Command9f {
                unknown: one_byte_payload(payload, command)?,
            }),
            ReaderCommandId::Unknown(_) => Ok(Self::Unsupported {
                command,
                raw_payload: payload.to_vec(),
            }),
        }
    }

    pub const fn command_id(&self) -> ReaderCommandId {
        match self {
            Self::PrepareAccess { .. } => ReaderCommandId::Command40,
            Self::Read16 { .. } => ReaderCommandId::Command41,
            Self::Write16 { .. } => ReaderCommandId::Command42,
            Self::Command43 { .. } => ReaderCommandId::Command43,
            Self::ControlOperation7 { .. } => ReaderCommandId::Command44,
            Self::Command45 { .. } => ReaderCommandId::Command45,
            Self::Command46 { .. } => ReaderCommandId::Command46,
            Self::FinishAccess => ReaderCommandId::Command47,
            Self::Read48 { .. } => ReaderCommandId::Command48,
            Self::Write48 { .. } => ReaderCommandId::Command49,
            Self::Configure { .. } => ReaderCommandId::Command50,
            Self::Initialize => ReaderCommandId::Command56,
            Self::PollStatus => ReaderCommandId::Command80,
            Self::TransportStart { .. } => ReaderCommandId::Command81,
            Self::TransportContinue { .. } => ReaderCommandId::Command82,
            Self::Identify => ReaderCommandId::Command83,
            Self::Command9f { .. } => ReaderCommandId::Command9f,
            Self::Unsupported { command, .. } => *command,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderResponse {
    Identity {
        identity: ReaderIdentity,
        count: ReaderCount,
    },
    Initialize {
        status: ReaderOperationStatus,
        unknown_byte_4: u8,
    },
    Configure {
        status: ReaderOperationStatus,
        unknown_byte_4: u8,
    },
    Status {
        bits: ReaderStatusBits,
    },
    Transport {
        command: ReaderCommandId,
        status: u8,
    },
    Operation {
        command: ReaderCommandId,
        status: ReaderOperationStatus,
        reserved: u8,
    },
    Read16 {
        status: ReaderOperationStatus,
        reserved: u8,
        data: [u8; 16],
    },
    Read48 {
        status: ReaderOperationStatus,
        reserved: u8,
        data: [u8; 48],
    },
}

impl ReaderResponse {
    pub const fn command_id(self) -> ReaderCommandId {
        match self {
            Self::Identity { .. } => ReaderCommandId::Command83,
            Self::Initialize { .. } => ReaderCommandId::Command56,
            Self::Configure { .. } => ReaderCommandId::Command50,
            Self::Status { .. } => ReaderCommandId::Command80,
            Self::Transport { command, .. } | Self::Operation { command, .. } => command,
            Self::Read16 { .. } => ReaderCommandId::Command41,
            Self::Read48 { .. } => ReaderCommandId::Command48,
        }
    }

    pub fn serialize(self) -> Vec<u8> {
        let (command, payload) = match self {
            Self::Identity { identity, count } => {
                let identity = decimal_ascii(identity.decimal());
                let count = decimal_ascii(count.value());
                (
                    ReaderCommandId::Command83,
                    vec![identity[0], identity[1], count[0], count[1]],
                )
            }
            Self::Initialize {
                status,
                unknown_byte_4,
            } => (
                ReaderCommandId::Command56,
                vec![status.serialize(), unknown_byte_4],
            ),
            Self::Configure {
                status,
                unknown_byte_4,
            } => (
                ReaderCommandId::Command50,
                vec![status.serialize(), unknown_byte_4],
            ),
            Self::Status { bits } => (ReaderCommandId::Command80, vec![bits.raw()]),
            Self::Transport { command, status } => (command, vec![status]),
            Self::Operation {
                command,
                status,
                reserved,
            } => (command, vec![status.serialize(), reserved]),
            Self::Read16 {
                status,
                reserved,
                data,
            } => {
                let mut payload = Vec::with_capacity(18);
                payload.extend_from_slice(&[status.serialize(), reserved]);
                payload.extend_from_slice(&data);
                (ReaderCommandId::Command41, payload)
            }
            Self::Read48 {
                status,
                reserved,
                data,
            } => {
                let mut payload = Vec::with_capacity(50);
                payload.extend_from_slice(&[status.serialize(), reserved]);
                payload.extend_from_slice(&data);
                (ReaderCommandId::Command48, payload)
            }
        };
        serialize_frame(command, &payload)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CardPhase {
    Absent,
    TransportRequested,
    TransportContinuing,
    Ready,
    Releasing,
}

#[derive(Debug)]
pub struct ReaderProtocol {
    side: ReaderSide,
    phase: CardPhase,
    card: Option<MountedCard>,
}

impl ReaderProtocol {
    pub const fn new(side: ReaderSide) -> Self {
        Self {
            side,
            phase: CardPhase::Absent,
            card: None,
        }
    }

    pub const fn side(&self) -> ReaderSide {
        self.side
    }

    pub const fn is_absent(&self) -> bool {
        matches!(self.phase, CardPhase::Absent)
    }

    pub fn mount(&mut self, directory: &Path, number: u8) -> Result<(), CardError> {
        self.card = Some(MountedCard::load_or_create(directory, number)?);
        self.phase = CardPhase::TransportRequested;
        Ok(())
    }

    pub fn mounted_card_number(&self) -> Option<u8> {
        self.card.as_ref().map(MountedCard::number)
    }

    pub fn handle(
        &mut self,
        request: ReaderClientRequest,
    ) -> Result<Option<ReaderResponse>, ReaderProtocolError> {
        let response = match request {
            ReaderClientRequest::Identify => Some(ReaderResponse::Identity {
                identity: ReaderIdentity::Value35,
                count: ReaderCount::zero(),
            }),
            ReaderClientRequest::Initialize => Some(ReaderResponse::Initialize {
                status: ReaderOperationStatus::Success,
                unknown_byte_4: 0,
            }),
            ReaderClientRequest::Configure { slot, unknown } => {
                let _preserved_unknown = (slot, unknown);
                Some(ReaderResponse::Configure {
                    status: ReaderOperationStatus::Success,
                    unknown_byte_4: 0,
                })
            }
            ReaderClientRequest::PollStatus => Some(ReaderResponse::Status {
                bits: match self.phase {
                    CardPhase::Absent => ReaderStatusBits::empty(),
                    CardPhase::TransportRequested | CardPhase::TransportContinuing => {
                        ReaderStatusBits::transport_requested()
                    }
                    CardPhase::Ready | CardPhase::Releasing => ReaderStatusBits::ready(),
                },
            }),
            ReaderClientRequest::TransportStart { action } => {
                if action == b'6' && self.card.is_some() {
                    self.phase = CardPhase::Releasing;
                } else if action == b'1' && self.phase == CardPhase::TransportRequested {
                    self.phase = CardPhase::TransportContinuing;
                }
                Some(ReaderResponse::Transport {
                    command: ReaderCommandId::Command81,
                    status: b'1',
                })
            }
            ReaderClientRequest::TransportContinue { action } => {
                if action == b'1' {
                    match self.phase {
                        CardPhase::TransportContinuing => self.phase = CardPhase::Ready,
                        CardPhase::Releasing => {
                            self.card = None;
                            self.phase = CardPhase::Absent;
                        }
                        _ => {}
                    }
                }
                Some(ReaderResponse::Transport {
                    command: ReaderCommandId::Command82,
                    status: b'1',
                })
            }
            ReaderClientRequest::PrepareAccess { magic } => {
                let _verified_request_magic = magic;
                Some(operation_response(
                    ReaderCommandId::Command40,
                    self.card.is_some(),
                ))
            }
            ReaderClientRequest::FinishAccess => Some(operation_response(
                ReaderCommandId::Command47,
                self.card.is_some(),
            )),
            ReaderClientRequest::Read16 { block_index } => {
                let data = self
                    .card
                    .as_ref()
                    .and_then(|card| card.read_block::<16>(block_index));
                Some(ReaderResponse::Read16 {
                    status: operation_status(data.is_some()),
                    reserved: 0,
                    data: data.unwrap_or([0; 16]),
                })
            }
            ReaderClientRequest::Read48 { block_index } => {
                let data = self
                    .card
                    .as_ref()
                    .and_then(|card| card.read_block::<48>(block_index));
                Some(ReaderResponse::Read48 {
                    status: operation_status(data.is_some()),
                    reserved: 0,
                    data: data.unwrap_or([0; 48]),
                })
            }
            ReaderClientRequest::Write16 { block_index, data } => {
                let written = match self.card.as_mut() {
                    Some(card) => card.write_block(block_index, data)?,
                    None => false,
                };
                Some(operation_response(ReaderCommandId::Command42, written))
            }
            ReaderClientRequest::Write48 { block_index, data } => {
                let written = match self.card.as_mut() {
                    Some(card) => card.write_block(block_index, data)?,
                    None => false,
                };
                Some(operation_response(ReaderCommandId::Command49, written))
            }
            ReaderClientRequest::ControlOperation7 { selector, value } => {
                // The first-use flow sends selector 2 and value 1 after it
                // writes payload blocks 1 through 15. Tower requires the
                // standard two-byte success reply before it updates the card
                // header. The physical device operation name is not known.
                let committed = match self.card.as_mut() {
                    Some(card) if selector == 2 => card.decrement_generation(value)?,
                    _ => false,
                };
                Some(operation_response(ReaderCommandId::Command44, committed))
            }
            ReaderClientRequest::Command43 { .. }
            | ReaderClientRequest::Command45 { .. }
            | ReaderClientRequest::Command46 { .. }
            | ReaderClientRequest::Command9f { .. }
            | ReaderClientRequest::Unsupported { .. } => None,
        };
        Ok(response)
    }
}

fn operation_response(command: ReaderCommandId, success: bool) -> ReaderResponse {
    ReaderResponse::Operation {
        command,
        status: operation_status(success),
        reserved: 0,
    }
}

const fn operation_status(success: bool) -> ReaderOperationStatus {
    if success {
        ReaderOperationStatus::Success
    } else {
        ReaderOperationStatus::Unknown(0x02)
    }
}

#[derive(Debug, Error)]
pub enum ReaderProtocolError {
    #[error("the reader frame start or length is invalid")]
    FrameStartOrLength,
    #[error("the reader payload delimiter is invalid")]
    PayloadEnd,
    #[error("the reader frame terminator is invalid")]
    FrameEnd,
    #[error("the reader frame checksum is invalid")]
    Checksum,
    #[error("command {command:?} has payload length {actual}; expected {expected}")]
    PayloadLength {
        command: ReaderCommandId,
        actual: usize,
        expected: usize,
    },
    #[error(transparent)]
    Card(#[from] CardError),
}

fn require_empty_payload(
    payload: &[u8],
    command: ReaderCommandId,
) -> Result<(), ReaderProtocolError> {
    if payload.is_empty() {
        Ok(())
    } else {
        Err(ReaderProtocolError::PayloadLength {
            command,
            actual: payload.len(),
            expected: 0,
        })
    }
}

fn one_byte_payload(payload: &[u8], command: ReaderCommandId) -> Result<u8, ReaderProtocolError> {
    if let [value] = payload {
        Ok(*value)
    } else {
        Err(ReaderProtocolError::PayloadLength {
            command,
            actual: payload.len(),
            expected: 1,
        })
    }
}

fn fixed_payload<const N: usize>(
    payload: &[u8],
    command: ReaderCommandId,
) -> Result<[u8; N], ReaderProtocolError> {
    payload
        .try_into()
        .map_err(|_| ReaderProtocolError::PayloadLength {
            command,
            actual: payload.len(),
            expected: N,
        })
}

fn control_payload(
    payload: &[u8],
    command: ReaderCommandId,
) -> Result<(u8, u32), ReaderProtocolError> {
    let payload: [u8; 5] = fixed_payload(payload, command)?;
    Ok((
        payload[0],
        u32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]),
    ))
}

fn decimal_ascii(value: u8) -> [u8; 2] {
    [
        b'0'.saturating_add(value / 10),
        b'0'.saturating_add(value % 10),
    ]
}

fn serialize_frame(command: ReaderCommandId, payload: &[u8]) -> Vec<u8> {
    let Ok(payload_length) = u8::try_from(payload.len()) else {
        return Vec::new();
    };
    let mut raw = Vec::with_capacity(payload.len().saturating_add(7));
    raw.extend_from_slice(&[FRAME_START, command.serialize(), payload_length]);
    raw.extend_from_slice(payload);
    raw.push(PAYLOAD_END);
    let checksum = raw[1..]
        .iter()
        .copied()
        .fold(0_u8, |value, byte| value ^ byte);
    raw.extend_from_slice(&[checksum, FRAME_END[0], FRAME_END[1]]);
    raw
}
