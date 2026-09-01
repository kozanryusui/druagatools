use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::sync::mpsc;

use crate::protocol::station::{
    EndpointAssignment, EndpointHost, GameplayBlob, GameplayEnvelopeFlags, LobbyLookup,
    LobbyRegistration, MAX_ENVELOPE_RECORDS, OwnerKey, ParticipantRecord, PartyRoster, PartySlot,
    PlayerIdentity, PlayerRecord, RosterReadiness, StationProtocolError,
};

const MAX_PLAYER_QUEUE: usize = 128;
const MAX_PARTY_PLAYERS: usize = 4;
const UNCLAIMED_PARTY_LIFETIME: Duration = Duration::from_secs(300);
const NETWORK_CHECK_QUEST_ID: u16 = 75;
const NETWORK_CHECK_ELAPSED_WAIT_SECONDS: u16 = 5;
const ALTERNATE_QUEST_INDEX_MODE: u16 = 9;

pub(crate) struct OnlineState {
    gameplay_host: EndpointHost,
    gameplay_port: u16,
    inner: Mutex<OnlineInner>,
}

struct OnlineInner {
    next_owner_key: u32,
    assembling: Vec<AssemblingParty>,
    parties: HashMap<OwnerKey, RelayParty>,
}

impl Default for OnlineInner {
    fn default() -> Self {
        Self {
            next_owner_key: 1,
            assembling: Vec::new(),
            parties: HashMap::new(),
        }
    }
}

struct AssemblingMember {
    connection_id: u64,
    registration: LobbyRegistration,
    assignment_tx: mpsc::UnboundedSender<EndpointAssignment>,
}

struct AssemblingParty {
    owner_key: OwnerKey,
    members: Vec<AssemblingMember>,
}

struct RelayParty {
    members: Vec<RelayMember>,
    matching_quest_index: u16,
    roster_readiness: RosterReadiness,
    last_activity: Instant,
}

struct RelayMember {
    record_id: u32,
    matching_connection_id: u64,
    matching_record: ParticipantRecord,
    matching_queues: [VecDeque<ParticipantRecord>; MAX_PARTY_PLAYERS],
    gameplay_connection_id: Option<u64>,
    gameplay_queue: VecDeque<PlayerRecord>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ServiceCounts {
    pub party_count: u16,
    pub player_count: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MatchOutcome {
    Assembling {
        assignment: EndpointAssignment,
        waiting_count: usize,
    },
    PartyCreated {
        owner_key: OwnerKey,
        player_count: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RelayEnvelope {
    pub flags: GameplayEnvelopeFlags,
    pub records: Vec<PlayerRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RelayOutcome {
    pub response: RelayEnvelope,
    pub dropped_records: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RelayBinding {
    owner_key: OwnerKey,
    party_slot: PartySlot,
    connection_id: u64,
}

impl RelayBinding {
    pub(crate) const fn owner_key(self) -> OwnerKey {
        self.owner_key
    }

    pub(crate) const fn party_slot(self) -> PartySlot {
        self.party_slot
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RelayJoin {
    pub binding: RelayBinding,
    pub flags: GameplayEnvelopeFlags,
}

#[derive(Debug, Error)]
pub(crate) enum OnlineError {
    #[error("online state lock is poisoned")]
    Lock,
    #[error("lobby assignment cannot be encoded: {0}")]
    Protocol(#[from] StationProtocolError),
    #[error("owner key {owner_key} does not identify an active party")]
    UnknownParty { owner_key: OwnerKey },
    #[error("party slot {party_slot} is not authorized for owner key {owner_key}")]
    UnauthorizedSlot {
        owner_key: OwnerKey,
        party_slot: PartySlot,
    },
    #[error("record ID {record_id} does not match party slot {party_slot}")]
    RecordId {
        record_id: u32,
        party_slot: PartySlot,
    },
    #[error("party slot {party_slot} already has a live connection")]
    SlotOccupied { party_slot: PartySlot },
    #[error("connection is not bound to this party slot")]
    Connection,
    #[error("matching connection {connection_id} closed before endpoint assignment")]
    AssignmentClosed { connection_id: u64 },
    #[error("matching connection {connection_id} is not bound to an active party")]
    UnknownMatchingConnection { connection_id: u64 },
}

mod gameplay;
mod matching;
#[cfg(test)]
mod tests;

impl OnlineState {
    pub(crate) fn new(gameplay_host: EndpointHost, gameplay_port: u16) -> Self {
        Self {
            gameplay_host,
            gameplay_port,
            inner: Mutex::new(OnlineInner::default()),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, OnlineInner>, OnlineError> {
        self.inner.lock().map_err(|_| OnlineError::Lock)
    }
}
