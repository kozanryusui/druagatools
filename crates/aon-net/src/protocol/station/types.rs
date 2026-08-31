use std::fmt;
use std::num::NonZeroU32;

use binrw::{BinRead, BinWrite};
use encoding_rs::SHIFT_JIS;
use thiserror::Error;

use crate::protocol::tower::TowerProtocolError;

pub const MAX_GAMEPLAY_BLOB_SIZE: usize = 0x50;
pub const MAX_ENVELOPE_RECORDS: usize = 6;

#[derive(BinWrite, Clone, Copy, Debug, Eq, PartialEq)]
pub struct GameplayEnvelopeFlags(u8);

impl GameplayEnvelopeFlags {
    const ROSTER_CHANGED: u8 = 1;
    const ACTIVE_SLOTS: u8 = 0b0001_1110;

    pub fn from_active_slots(
        active_slots: impl IntoIterator<Item = PartySlot>,
        roster_changed: bool,
    ) -> Self {
        let mut bits = if roster_changed {
            Self::ROSTER_CHANGED
        } else {
            0
        };
        for slot in active_slots {
            bits |= 1 << slot.get();
        }
        Self(bits)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn roster_changed(self) -> bool {
        self.0 & Self::ROSTER_CHANGED != 0
    }

    pub const fn active_player_count(self) -> u32 {
        (self.0 & Self::ACTIVE_SLOTS).count_ones()
    }

    pub const fn has_sole_survivor(self) -> bool {
        self.roster_changed() && self.active_player_count() == 1
    }
}

#[derive(BinRead, Clone, Debug, Eq, PartialEq)]
#[br(big)]
pub struct LobbyRegistration {
    pub mode: u16,
    pub location: u16,
    pub matching_quest_index: u16,
    pub alternate_quest_index: u16,
    pub lobby_values: [u16; 2],
    pub participant_record: ParticipantRecord,
    pub player_controls: [u8; 4],
    pub record_id: u32,
    pub shop_name: FixedText<40>,
    pub region_names: [FixedText<64>; 4],
}

#[derive(BinRead, Clone, Debug, Eq, PartialEq)]
#[br(big)]
pub struct LobbyLookup {
    pub elapsed_wait_seconds: u16,
    pub remaining_wait_seconds: u16,
    pub participant_or_lobby_key: ParticipantRecord,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointAssignment {
    pub host: EndpointHost,
    pub port: u16,
    pub owner_key: OwnerKey,
    pub ready: bool,
    pub local_slot: PartySlot,
    pub matching_quest_index: u16,
    pub participants: PartyRoster,
}

#[binrw::binwrite]
#[derive(Clone, Debug, Eq, PartialEq)]
#[bw(big)]
pub struct PlayerRecord {
    pub party_slot: PartySlot,
    #[bw(try_calc = u8::try_from(blob.as_bytes().len()))]
    blob_length: u8,
    pub blob: GameplayBlob,
}

#[derive(BinRead, BinWrite, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParticipantRecord([u8; 32]);

impl ParticipantRecord {
    pub fn with_party_slot(mut self, party_slot: PartySlot) -> Self {
        self.0[0] = party_slot.get();
        self
    }

    #[cfg(test)]
    pub(crate) const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

#[derive(BinRead, BinWrite, Clone, Debug, Eq, PartialEq)]
#[br(try_map = Self::from_wire)]
#[bw(try_map = Self::to_wire)]
pub struct FixedText<const SIZE: usize>(String);

impl<const SIZE: usize> fmt::Display for FixedText<SIZE> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<const SIZE: usize> FixedText<SIZE> {
    fn from_wire(bytes: [u8; SIZE]) -> Result<Self, String> {
        let length = bytes.iter().position(|byte| *byte == 0).unwrap_or(SIZE);
        let (text, _, had_errors) = SHIFT_JIS.decode(&bytes[..length]);
        if had_errors {
            Err(format!("fixed {SIZE}-byte text is not valid Shift_JIS"))
        } else {
            Ok(Self(text.into_owned()))
        }
    }

    fn to_wire(&self) -> Result<[u8; SIZE], String> {
        let (encoded, _, had_errors) = SHIFT_JIS.encode(&self.0);
        if had_errors || encoded.len() >= SIZE {
            return Err(format!(
                "text does not fit a fixed {SIZE}-byte Shift_JIS field"
            ));
        }
        let mut bytes = [0; SIZE];
        bytes[..encoded.len()].copy_from_slice(&encoded);
        Ok(bytes)
    }
}

#[derive(BinRead, BinWrite, Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OwnerKey(
    #[br(try_map = |value: u32| NonZeroU32::new(value).ok_or(StationProtocolError::OwnerKey))]
    #[bw(map = |value: &NonZeroU32| value.get())]
    NonZeroU32,
);

impl OwnerKey {
    pub fn new(value: u32) -> Result<Self, StationProtocolError> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or(StationProtocolError::OwnerKey)
    }

    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl fmt::Display for OwnerKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}

#[derive(BinRead, BinWrite, Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartySlot(
    #[br(assert(
        (1..=4).contains(&self_0),
        StationProtocolError::PartySlot { value: self_0 }
    ))]
    u8,
);

impl PartySlot {
    pub(crate) const ALL: [Self; 4] = [Self(1), Self(2), Self(3), Self(4)];

    pub fn new(value: u8) -> Result<Self, StationProtocolError> {
        if (1..=4).contains(&value) {
            Ok(Self(value))
        } else {
            Err(StationProtocolError::PartySlot { value })
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    pub const fn index(self) -> usize {
        (self.0 - 1) as usize
    }
}

impl fmt::Display for PartySlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}

#[derive(BinRead, BinWrite, Clone, Debug, Eq, PartialEq)]
#[br(
    import { length: usize },
    assert(
        !self_0.is_empty() && self_0.len() <= MAX_GAMEPLAY_BLOB_SIZE,
        StationProtocolError::GameplayBlobLength { actual: self_0.len() }
    )
)]
pub struct GameplayBlob(#[br(count = length)] Vec<u8>);

impl GameplayBlob {
    #[cfg(test)]
    pub fn new(value: Vec<u8>) -> Result<Self, StationProtocolError> {
        validate_gameplay_blob(&value)?;
        Ok(Self(value))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(BinRead, BinWrite, Clone, Debug, Eq, PartialEq)]
#[br(try_map = Self::from_wire)]
#[bw(map = Self::to_wire)]
pub struct EndpointHost(String);

impl EndpointHost {
    pub fn new(value: String) -> Result<Self, StationProtocolError> {
        if value.is_empty() || value.len() > 31 || !value.is_ascii() || value.contains('\0') {
            Err(StationProtocolError::EndpointHost)
        } else {
            Ok(Self(value))
        }
    }

    fn from_wire(bytes: [u8; 32]) -> Result<Self, String> {
        let length = bytes.iter().position(|byte| *byte == 0).unwrap_or(32);
        let value = std::str::from_utf8(&bytes[..length]).map_err(|error| error.to_string())?;
        Self::new(value.to_owned()).map_err(|error| error.to_string())
    }

    fn to_wire(&self) -> [u8; 32] {
        let mut bytes = [0; 32];
        bytes[..self.0.len()].copy_from_slice(self.0.as_bytes());
        bytes
    }
}

impl fmt::Display for EndpointHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartyRoster(Vec<ParticipantRecord>);

impl PartyRoster {
    pub fn new(value: Vec<ParticipantRecord>) -> Result<Self, StationProtocolError> {
        if value.is_empty() || value.len() > 4 {
            Err(StationProtocolError::PartyRoster {
                actual: value.len(),
            })
        } else {
            Ok(Self(value))
        }
    }

    pub fn as_slice(&self) -> &[ParticipantRecord] {
        &self.0
    }
}

#[derive(Debug, Error)]
pub enum StationProtocolError {
    #[error("gameplay blob has {actual} bytes; the limit is 80")]
    GameplayBlobLength { actual: usize },
    #[error("owner key must not be zero")]
    OwnerKey,
    #[error("item ID must not be zero")]
    #[allow(dead_code, reason = "the server does not configure present items yet")]
    ItemId,
    #[error("party slot {value} is outside the range 1 through 4")]
    PartySlot { value: u8 },
    #[error("endpoint host must contain 1 through 31 ASCII bytes")]
    EndpointHost,
    #[error("party roster has {actual} entries; the range is 1 through 4")]
    PartyRoster { actual: usize },
    #[error("{role} does not support message type 0x{message_type:04X}")]
    UnsupportedType {
        role: &'static str,
        message_type: u16,
    },
    #[error("binary packet cannot be parsed: {0}")]
    Binrw(String),
    #[error("binary packet cannot be serialized: {0}")]
    Serialize(String),
    #[error("central Tower packet cannot be parsed: {0}")]
    Central(#[from] TowerProtocolError),
}

#[cfg(test)]
fn validate_gameplay_blob(payload: &[u8]) -> Result<(), StationProtocolError> {
    if payload.is_empty() || payload.len() > MAX_GAMEPLAY_BLOB_SIZE {
        Err(StationProtocolError::GameplayBlobLength {
            actual: payload.len(),
        })
    } else {
        Ok(())
    }
}
