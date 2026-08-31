use std::io::Cursor;

use binrw::{BinRead, BinWrite, binread, binwrite};

use crate::protocol::frame;
use crate::protocol::tower::{TowerRequest, TowerResponse, serialize_tower_response};

use super::event::{MatchingActivationConfiguration, MatchingActivationConfigurationWire};
use super::types::{
    EndpointAssignment, EndpointHost, LobbyLookup, LobbyRegistration, OwnerKey, PartySlot,
    PlayerIdentity, StationProtocolError,
};

#[derive(BinRead, BinWrite, Clone, Copy, Debug, Eq, PartialEq)]
#[brw(big, repr = u32)]
pub(super) enum ConnectionRole {
    GameRelay = 4,
}

#[derive(BinRead, BinWrite, Clone, Copy, Debug, Eq, PartialEq)]
#[brw(repr = u8)]
pub(super) enum AssignmentReady {
    Waiting = 0,
    Ready = 1,
}

#[derive(BinRead, BinWrite)]
#[br(big)]
#[bw(big)]
pub(super) struct EndpointAssignmentWire {
    pub(super) connection_role: ConnectionRole,
    control_04: [u8; 2],
    pub(super) port: u16,
    pub(super) host: EndpointHost,
    pub(super) owner_key: OwnerKey,
    pub(super) ready: AssignmentReady,
    pub(super) active_slot_mask: u8,
    pub(super) local_slot: PartySlot,
    participant_count: u8,
    pub(super) matching_quest_index: u16,
    reserved_32: [u8; 2],
    #[br(count = participant_count, pad_size_to = 0x80)]
    #[bw(pad_size_to = 0x80)]
    pub(super) participants: Vec<PlayerIdentity>,
}

impl From<&EndpointAssignment> for EndpointAssignmentWire {
    fn from(assignment: &EndpointAssignment) -> Self {
        let participants: Vec<_> = assignment
            .participants
            .as_slice()
            .iter()
            .enumerate()
            .map(|(index, identity)| {
                let mut identity = *identity;
                identity.0[0] = (index + 1) as u8;
                identity
            })
            .collect();

        Self {
            connection_role: ConnectionRole::GameRelay,
            control_04: [0; 2],
            port: assignment.port,
            host: assignment.host.clone(),
            owner_key: assignment.owner_key,
            ready: if assignment.ready {
                AssignmentReady::Ready
            } else {
                AssignmentReady::Waiting
            },
            active_slot_mask: ((1_u16 << participants.len()) - 1) as u8,
            local_slot: assignment.local_slot,
            participant_count: participants.len() as u8,
            matching_quest_index: assignment.matching_quest_index,
            reserved_32: [0; 2],
            participants,
        }
    }
}

#[binread]
#[derive(Clone, Debug, Eq, PartialEq)]
#[br(big)]
enum StationMatchingRequest {
    #[br(magic = 0x0009_u16)]
    LobbyRegistration {
        #[br(temp)]
        payload_length: u16,
        lobby: LobbyRegistration,
    },
    #[br(magic = 0x000b_u16)]
    LobbyLookup {
        #[br(temp)]
        payload_length: u16,
        lookup: LobbyLookup,
    },
    #[br(magic = 0x0023_u16)]
    Activation {
        #[br(temp)]
        payload_length: u16,
        reserved: u8,
    },
}

#[binwrite]
#[bw(big)]
enum StationMatchingResponseWire {
    #[bw(magic = 0x0006_u16)]
    ActivationConfiguration {
        #[bw(calc = 0x50_u16)]
        payload_length: u16,
        configuration: MatchingActivationConfigurationWire,
    },
    #[bw(magic = 0x000a_u16)]
    LobbyPrompt {
        #[bw(calc = 1_u16)]
        payload_length: u16,
        #[bw(calc = 0_u8)]
        unused: u8,
    },
    #[bw(magic = 0x000c_u16)]
    EndpointAssignment {
        #[bw(calc = 0xb4_u16)]
        payload_length: u16,
        assignment: EndpointAssignmentWire,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MatchingRequest {
    Central(TowerRequest),
    Activation { reserved: u8 },
    LobbyRegistration(LobbyRegistration),
    LobbyLookup(LobbyLookup),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MatchingResponse {
    Central(TowerResponse),
    ActivationConfiguration(MatchingActivationConfiguration),
    LobbyPrompt {},
    EndpointAssignment(EndpointAssignment),
}

impl MatchingResponse {
    pub fn serialize(&self) -> Result<Vec<u8>, StationProtocolError> {
        let wire = match self {
            Self::Central(response) => {
                return serialize_tower_response(response).map_err(Into::into);
            }
            Self::ActivationConfiguration(configuration) => {
                StationMatchingResponseWire::ActivationConfiguration {
                    configuration: configuration.into(),
                }
            }
            Self::LobbyPrompt {} => StationMatchingResponseWire::LobbyPrompt {},
            Self::EndpointAssignment(assignment) => {
                StationMatchingResponseWire::EndpointAssignment {
                    assignment: assignment.into(),
                }
            }
        };
        let mut output = Cursor::new(Vec::new());
        wire.write(&mut output)
            .map_err(|error| StationProtocolError::Serialize(error.to_string()))?;
        Ok(output.into_inner())
    }
}

pub fn deserialize_matching_request(bytes: &[u8]) -> Result<MatchingRequest, StationProtocolError> {
    if bytes.len() < 2 {
        return Err(StationProtocolError::Binrw(
            "matching frame header is incomplete".to_owned(),
        ));
    }
    let message_type = u16::from_be_bytes([bytes[0], bytes[1]]);
    match message_type {
        0x01 | 0x03 | 0x1d => frame::read(bytes)
            .map(MatchingRequest::Central)
            .map_err(|error| StationProtocolError::Binrw(error.to_string())),
        0x09 | 0x0b | 0x23 => {
            let request: StationMatchingRequest = frame::read(bytes)
                .map_err(|error| StationProtocolError::Binrw(error.to_string()))?;
            match request {
                StationMatchingRequest::LobbyRegistration { lobby } => {
                    Ok(MatchingRequest::LobbyRegistration(lobby))
                }
                StationMatchingRequest::LobbyLookup { lookup } => {
                    Ok(MatchingRequest::LobbyLookup(lookup))
                }
                StationMatchingRequest::Activation { reserved } => {
                    Ok(MatchingRequest::Activation { reserved })
                }
            }
        }
        message_type => Err(StationProtocolError::UnsupportedType {
            role: "matching service",
            message_type,
        }),
    }
}
