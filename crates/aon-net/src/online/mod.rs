use std::collections::{HashMap, VecDeque};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::sync::mpsc;

use aon_net_admin::contract::{
    AdminEvent, MatchingQueueStatus, OnlineStatus, RelayPartyStatus, RelayStatus,
};

use crate::logging::AdminHub;

use crate::protocol::station::{
    EndpointAssignment, EndpointHost, GameplayBlob, GameplayEnvelopeFlags, LobbyLookup,
    LobbyRegistration, OwnerKey, ParticipantRecord, PartyRoster, PartySlot, PlayerIdentity,
    PlayerRecord, RosterReadiness, StationProtocolError,
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
    admin_hub: Option<Arc<AdminHub>>,
    last_published_status: Mutex<OnlineStatus>,
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
    cancellation_tx: mpsc::Sender<PartyAbortReason>,
}

struct AssemblingParty {
    owner_key: OwnerKey,
    members: Vec<AssemblingMember>,
}

struct RelayParty {
    members: Vec<RelayMember>,
    matching_quest_index: u16,
    status_map_id: u16,
    roster_readiness: RosterReadiness,
    last_activity: Instant,
}

struct RelayMember {
    record_id: u32,
    matching_connection_id: u64,
    matching_complete: bool,
    matching_cancellation_tx: mpsc::Sender<PartyAbortReason>,
    matching_record: ParticipantRecord,
    matching_queues: [VecDeque<ParticipantRecord>; MAX_PARTY_PLAYERS],
    gameplay_connection: Option<GameplayConnection>,
}

struct GameplayConnection {
    id: u64,
    relay: RelaySenders,
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
pub(crate) struct RelayOutcome {
    pub response: RelayEnvelope,
    pub disconnected_players: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParticipantExchange {
    pub assignment: EndpointAssignment,
    pub completion: Option<MatchingCompletion>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MatchingCompletion {
    connection_id: u64,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RelayEnvelope {
    pub flags: GameplayEnvelopeFlags,
    pub records: Vec<PlayerRecord>,
}

#[derive(Clone)]
pub(crate) struct RelaySenders {
    record: mpsc::Sender<PlayerRecord>,
    disconnect: mpsc::Sender<RelayDisconnectReason>,
}

pub(crate) struct RelayReceivers {
    pub record: mpsc::Receiver<PlayerRecord>,
    pub disconnect: mpsc::Receiver<RelayDisconnectReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RelayDisconnectReason {
    QueueFull,
    PartyAborted(PartyAbortReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PartyAbortReason {
    MatchingDisconnected,
    GameplayHandoffTimeout,
    GameplayDisconnectedBeforeReady,
}

pub(crate) fn relay_channels(capacity: NonZeroUsize) -> (RelaySenders, RelayReceivers) {
    let (record_tx, record_rx) = mpsc::channel(capacity.get());
    let (disconnect_tx, disconnect_rx) = mpsc::channel(1);
    (
        RelaySenders {
            record: record_tx,
            disconnect: disconnect_tx,
        },
        RelayReceivers {
            record: record_rx,
            disconnect: disconnect_rx,
        },
    )
}

pub(crate) fn matching_cancellation_channel() -> (
    mpsc::Sender<PartyAbortReason>,
    mpsc::Receiver<PartyAbortReason>,
) {
    mpsc::channel(1)
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
mod lifecycle;
mod matching;
#[cfg(test)]
mod tests;

impl OnlineState {
    #[cfg(test)]
    pub(crate) fn new(gameplay_host: EndpointHost, gameplay_port: u16) -> Self {
        Self {
            gameplay_host,
            gameplay_port,
            admin_hub: None,
            last_published_status: Mutex::new(OnlineStatus::default()),
            inner: Mutex::new(OnlineInner::default()),
        }
    }

    pub(crate) fn with_admin_hub(
        gameplay_host: EndpointHost,
        gameplay_port: u16,
        admin_hub: Arc<AdminHub>,
    ) -> Self {
        Self {
            gameplay_host,
            gameplay_port,
            admin_hub: Some(admin_hub),
            last_published_status: Mutex::new(OnlineStatus::default()),
            inner: Mutex::new(OnlineInner::default()),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, OnlineInner>, OnlineError> {
        self.inner.lock().map_err(|_| OnlineError::Lock)
    }

    pub(crate) fn status(&self) -> Result<OnlineStatus, OnlineError> {
        let mut inner = self.lock()?;
        lifecycle::purge_expired_parties(&mut inner);
        matching::purge_closed_assembling_members(&mut inner);

        let matching_queues = inner
            .assembling
            .iter()
            .map(|party| MatchingQueueStatus {
                party_id: party.owner_key.get(),
                map_id: status_map_id(&party.members[0].registration),
                queued_players: bounded_player_count(party.members.len()),
                party_capacity: MAX_PARTY_PLAYERS as u8,
            })
            .collect();
        let mut relays = inner
            .parties
            .iter()
            .map(|(owner_key, party)| RelayStatus {
                party_id: owner_key.get(),
                map_id: party.status_map_id,
                party_players: bounded_player_count(party.members.len()),
                connected_players: bounded_player_count(
                    party
                        .members
                        .iter()
                        .filter(|member| member.gameplay_connection.is_some())
                        .count(),
                ),
                status: if party.roster_readiness == RosterReadiness::Ready {
                    RelayPartyStatus::Playing
                } else {
                    RelayPartyStatus::Connecting
                },
            })
            .collect::<Vec<_>>();
        relays.sort_unstable_by_key(|relay| relay.party_id);

        Ok(OnlineStatus {
            matching_queues,
            relays,
        })
    }

    fn publish_status(&self) -> Result<(), OnlineError> {
        if let Some(hub) = &self.admin_hub {
            let status = self.status()?;
            let mut previous = self
                .last_published_status
                .lock()
                .map_err(|_| OnlineError::Lock)?;
            if *previous != status {
                *previous = status.clone();
                hub.publish(AdminEvent::OnlineStatusChanged(status));
            }
        }
        Ok(())
    }
}

fn bounded_player_count(count: usize) -> u8 {
    count.min(u8::MAX as usize) as u8
}

fn status_map_id(registration: &LobbyRegistration) -> u16 {
    if registration.matching_quest_index == ALTERNATE_QUEST_INDEX_MODE {
        registration.alternate_quest_index
    } else {
        registration.matching_quest_index
    }
}
