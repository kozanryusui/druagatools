use std::io::Cursor;

use binrw::{BinWrite, binread, binwrite};

use crate::protocol::frame;

use super::types::{
    FixedText, GameplayBlob, GameplayEnvelopeFlags, MAX_ENVELOPE_RECORDS, OwnerKey, PartySlot,
    PlayerRecord, StationProtocolError,
};

#[binread]
#[derive(Clone, Debug, Eq, PartialEq)]
#[br(big)]
pub enum GameplayRequest {
    #[br(magic = 0x0001_u16)]
    InitialIdentity {
        #[br(temp)]
        payload_length: u16,
        identity: [u8; 4],
        reserved: u16,
    },
    #[br(magic = 0x0003_u16)]
    SessionConfirm {
        #[br(temp)]
        payload_length: u16,
        session_id: u32,
    },
    #[br(magic = 0x000f_u16)]
    EndpointRegistration {
        #[br(temp)]
        payload_length: u16,
        owner_key: OwnerKey,
        party_slot: PartySlot,
        reserved: u8,
        location: u16,
        record_id: u32,
        shop_name: FixedText<40>,
        region_names: [FixedText<64>; 4],
    },
    #[br(magic = 0x0011_u16)]
    GameplayBlob {
        #[br(temp)]
        payload_length: u16,
        #[br(args { length: payload_length.into() })]
        blob: GameplayBlob,
    },
    #[br(magic = 0x0017_u16)]
    ActionRecord {
        #[br(temp)]
        payload_length: u16,
        opaque: [u8; 0x18],
        value_18: u32,
        value_1c: u32,
        value_20: u32,
    },
}

#[binwrite]
#[derive(Clone, Debug, Eq, PartialEq)]
#[bw(big)]
pub enum GameplayResponse {
    #[bw(magic = 0x0002_u16)]
    InitialAccepted {
        #[bw(calc = 4_u16)]
        payload_length: u16,
        session_id: u32,
    },
    #[bw(magic = 0x0004_u16)]
    SessionConfirmed {
        #[bw(calc = 1_u16)]
        payload_length: u16,
        status: u8,
    },
    #[bw(magic = 0x0010_u16)]
    RegistrationResult {
        #[bw(calc = 1_u16)]
        payload_length: u16,
        status: u8,
    },
    #[bw(magic = 0x0012_u16)]
    Envelope {
        #[bw(try_calc = envelope_payload_length(records))]
        payload_length: u16,
        flags: GameplayEnvelopeFlags,
        #[bw(try_calc = u8::try_from(records.len().min(MAX_ENVELOPE_RECORDS)))]
        record_count: u8,
        #[bw(map = |records| &records[..records.len().min(MAX_ENVELOPE_RECORDS)])]
        records: Vec<PlayerRecord>,
    },
    #[bw(magic = 0x0018_u16)]
    ActionAccepted {
        #[bw(calc = 1_u16)]
        payload_length: u16,
        #[bw(calc = 0_u8)]
        unused: u8,
    },
}

impl GameplayResponse {
    pub fn serialize(&self) -> Result<Vec<u8>, StationProtocolError> {
        let mut output = Cursor::new(Vec::new());
        self.write(&mut output)
            .map_err(|error| StationProtocolError::Serialize(error.to_string()))?;
        Ok(output.into_inner())
    }
}

pub fn deserialize_gameplay_request(bytes: &[u8]) -> Result<GameplayRequest, StationProtocolError> {
    frame::read(bytes).map_err(|error| StationProtocolError::Binrw(error.to_string()))
}

fn envelope_payload_length(records: &[PlayerRecord]) -> Result<u16, std::num::TryFromIntError> {
    let records_length = records
        .iter()
        .take(MAX_ENVELOPE_RECORDS)
        .map(|record| 2 + record.blob.as_bytes().len())
        .sum::<usize>();
    u16::try_from(2 + records_length)
}
