use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::sync::mpsc;

use crate::protocol::station::{
    EndpointAssignment, EndpointHost, GameplayBlob, LobbyLookup, LobbyRegistration,
    MAX_ENVELOPE_RECORDS, OwnerKey, PartyRoster, PartySlot, PlayerRecord, StationProtocolError,
};

const MAX_PLAYER_QUEUE: usize = 128;
const UNCLAIMED_PARTY_LIFETIME: Duration = Duration::from_secs(300);
const NETWORK_CHECK_QUEST_ID: u16 = 75;
const NETWORK_CHECK_WAIT_WINDOW: u16 = 5;
const ALTERNATE_QUEST_INDEX_MODE: u16 = 9;

pub(crate) struct OnlineState {
    matching_player_count: usize,
    gameplay_host: EndpointHost,
    gameplay_port: u16,
    inner: Mutex<OnlineInner>,
}

struct OnlineInner {
    next_owner_key: u32,
    waiting: Vec<WaitingPlayer>,
    parties: HashMap<OwnerKey, RelayParty>,
}

impl Default for OnlineInner {
    fn default() -> Self {
        Self {
            next_owner_key: 1,
            waiting: Vec::new(),
            parties: HashMap::new(),
        }
    }
}

struct WaitingPlayer {
    connection_id: u64,
    registration: LobbyRegistration,
    assignment_tx: mpsc::UnboundedSender<EndpointAssignment>,
}

struct RelayParty {
    members: Vec<PartyMember>,
    live_connections: [Option<u64>; 4],
    queues: [VecDeque<PlayerRecord>; 4],
    started: bool,
    last_activity: Instant,
}

#[derive(Clone)]
struct PartyMember {
    record_id: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ServiceCounts {
    pub party_count: u16,
    pub player_count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MatchOutcome {
    Waiting {
        waiting_count: usize,
    },
    PartyCreated {
        owner_key: OwnerKey,
        player_count: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RelayBatch {
    pub flags: u8,
    pub records: Vec<PlayerRecord>,
    pub dropped_records: usize,
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
}

impl OnlineState {
    pub(crate) fn new(
        matching_player_count: u8,
        gameplay_host: EndpointHost,
        gameplay_port: u16,
    ) -> Self {
        Self {
            matching_player_count: usize::from(matching_player_count),
            gameplay_host,
            gameplay_port,
            inner: Mutex::new(OnlineInner::default()),
        }
    }

    pub(crate) fn queue_match(
        &self,
        connection_id: u64,
        registration: LobbyRegistration,
        lookup: LobbyLookup,
        assignment_tx: mpsc::UnboundedSender<EndpointAssignment>,
    ) -> Result<MatchOutcome, OnlineError> {
        if assignment_tx.is_closed() {
            return Err(OnlineError::AssignmentClosed { connection_id });
        }
        let mut inner = self.lock()?;
        purge_expired_parties(&mut inner);
        inner
            .waiting
            .retain(|player| !player.assignment_tx.is_closed());
        inner
            .waiting
            .retain(|player| player.connection_id != connection_id);
        let required_player_count = if is_network_check(&registration, &lookup) {
            1
        } else {
            self.matching_player_count
        };
        let compatible_indices: Vec<usize> = inner
            .waiting
            .iter()
            .enumerate()
            .filter(|(_, player)| compatible(&player.registration, &registration))
            .map(|(index, _)| index)
            .take(required_player_count - 1)
            .collect();

        if compatible_indices.len() + 1 < required_player_count {
            inner.waiting.push(WaitingPlayer {
                connection_id,
                registration,
                assignment_tx,
            });
            return Ok(MatchOutcome::Waiting {
                waiting_count: inner.waiting.len(),
            });
        }

        let mut players = Vec::with_capacity(required_player_count);
        for index in compatible_indices.into_iter().rev() {
            players.push(inner.waiting.remove(index));
        }
        players.reverse();
        players.push(WaitingPlayer {
            connection_id,
            registration,
            assignment_tx,
        });

        let owner_key = allocate_owner_key(&mut inner)?;
        let participants = players
            .iter()
            .map(|player| player.registration.player_identity)
            .collect();
        let members = players
            .iter()
            .map(|player| PartyMember {
                record_id: player.registration.record_id,
            })
            .collect();
        let participants = PartyRoster::new(participants)?;
        let matching_quest_index = players[0].registration.matching_quest_index;

        let assignments = players
            .iter()
            .enumerate()
            .map(|(index, player)| {
                Ok((
                    player.connection_id,
                    player.assignment_tx.clone(),
                    EndpointAssignment {
                        host: self.gameplay_host.clone(),
                        port: self.gameplay_port,
                        owner_key,
                        local_slot: PartySlot::new((index + 1) as u8)?,
                        matching_quest_index,
                        participants: participants.clone(),
                    },
                ))
            })
            .collect::<Result<Vec<_>, StationProtocolError>>()?;
        let player_count = players.len();
        inner.parties.insert(
            owner_key,
            RelayParty {
                members,
                live_connections: [None; 4],
                queues: std::array::from_fn(|_| VecDeque::new()),
                started: false,
                last_activity: Instant::now(),
            },
        );
        drop(inner);
        for (assignment_connection_id, assignment_tx, assignment) in assignments {
            if assignment_tx.send(assignment).is_err() {
                self.lock()?.parties.remove(&owner_key);
                return Err(OnlineError::AssignmentClosed {
                    connection_id: assignment_connection_id,
                });
            }
        }
        Ok(MatchOutcome::PartyCreated {
            owner_key,
            player_count,
        })
    }

    pub(crate) fn remove_waiter(&self, connection_id: u64) -> Result<(), OnlineError> {
        self.lock()?
            .waiting
            .retain(|player| player.connection_id != connection_id);
        Ok(())
    }

    pub(crate) fn join_relay(
        &self,
        owner_key: OwnerKey,
        party_slot: PartySlot,
        record_id: u32,
        connection_id: u64,
    ) -> Result<u8, OnlineError> {
        let slot = party_slot.index();
        let mut inner = self.lock()?;
        purge_expired_parties(&mut inner);
        let party = inner
            .parties
            .get_mut(&owner_key)
            .ok_or(OnlineError::UnknownParty { owner_key })?;
        let member = party
            .members
            .get(slot)
            .ok_or(OnlineError::UnauthorizedSlot {
                owner_key,
                party_slot,
            })?;
        if member.record_id != record_id {
            return Err(OnlineError::RecordId {
                record_id,
                party_slot,
            });
        }
        if party.live_connections[slot].is_some() {
            return Err(OnlineError::SlotOccupied { party_slot });
        }
        party.live_connections[slot] = Some(connection_id);
        party.last_activity = Instant::now();
        if party.live_connections.iter().flatten().count() == party.members.len() {
            party.started = true;
        }
        Ok(active_flags(party))
    }

    pub(crate) fn relay_blob(
        &self,
        owner_key: OwnerKey,
        party_slot: PartySlot,
        connection_id: u64,
        blob: GameplayBlob,
    ) -> Result<RelayBatch, OnlineError> {
        let source = party_slot.index();
        let mut inner = self.lock()?;
        let party = inner
            .parties
            .get_mut(&owner_key)
            .ok_or(OnlineError::UnknownParty { owner_key })?;
        if party.live_connections[source] != Some(connection_id) {
            return Err(OnlineError::Connection);
        }
        party.last_activity = Instant::now();

        let mut dropped_records = 0;
        for destination in 0..party.members.len() {
            if destination == source || party.live_connections[destination].is_none() {
                continue;
            }
            let queue = &mut party.queues[destination];
            if queue.len() == MAX_PLAYER_QUEUE {
                queue.pop_front();
                dropped_records += 1;
            }
            queue.push_back(PlayerRecord {
                party_slot,
                blob: blob.clone(),
            });
        }
        let records = party.queues[source]
            .drain(..party.queues[source].len().min(MAX_ENVELOPE_RECORDS))
            .collect();
        Ok(RelayBatch {
            flags: active_flags(party),
            records,
            dropped_records,
        })
    }

    pub(crate) fn leave_relay(
        &self,
        owner_key: OwnerKey,
        party_slot: PartySlot,
        connection_id: u64,
    ) -> Result<(), OnlineError> {
        let slot = party_slot.index();
        let mut inner = self.lock()?;
        let remove_party = if let Some(party) = inner.parties.get_mut(&owner_key) {
            if party.live_connections[slot] == Some(connection_id) {
                party.live_connections[slot] = None;
                party.queues[slot].clear();
                party.last_activity = Instant::now();
            }
            party.started && party.live_connections.iter().all(Option::is_none)
        } else {
            false
        };
        if remove_party {
            inner.parties.remove(&owner_key);
        }
        Ok(())
    }

    pub(crate) fn service_counts(&self) -> Result<ServiceCounts, OnlineError> {
        let mut inner = self.lock()?;
        purge_expired_parties(&mut inner);
        Ok(ServiceCounts {
            party_count: inner.parties.len().min(u16::MAX as usize) as u16,
            player_count: inner
                .parties
                .values()
                .map(|party| party.live_connections.iter().flatten().count())
                .sum::<usize>()
                .min(u16::MAX as usize) as u16,
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, OnlineInner>, OnlineError> {
        self.inner.lock().map_err(|_| OnlineError::Lock)
    }
}

fn allocate_owner_key(inner: &mut OnlineInner) -> Result<OwnerKey, StationProtocolError> {
    loop {
        let candidate = inner.next_owner_key;
        inner.next_owner_key = inner.next_owner_key.wrapping_add(1).max(1);
        let owner_key = OwnerKey::new(candidate)?;
        if !inner.parties.contains_key(&owner_key) {
            return Ok(owner_key);
        }
    }
}

fn purge_expired_parties(inner: &mut OnlineInner) {
    inner.parties.retain(|_, party| {
        party.live_connections.iter().any(Option::is_some)
            || party.last_activity.elapsed() < UNCLAIMED_PARTY_LIFETIME
    });
}

fn compatible(left: &LobbyRegistration, right: &LobbyRegistration) -> bool {
    left.matching_quest_index == right.matching_quest_index
        && (left.matching_quest_index != ALTERNATE_QUEST_INDEX_MODE
            || left.alternate_quest_index == right.alternate_quest_index)
}

fn is_network_check(registration: &LobbyRegistration, lookup: &LobbyLookup) -> bool {
    registration.matching_quest_index == NETWORK_CHECK_QUEST_ID
        && registration.record_id == 0
        && lookup.wait_window == NETWORK_CHECK_WAIT_WINDOW
        && lookup.lobby_value == 0
}

fn active_flags(party: &RelayParty) -> u8 {
    let mut flags = 0;
    for (index, connection) in party.live_connections.iter().enumerate() {
        if connection.is_some() {
            flags |= 1 << (index + 1);
        }
    }
    if party.started && party.live_connections.iter().flatten().count() < party.members.len() {
        flags |= 1;
    }
    flags
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use binrw::BinRead;

    use super::*;
    use crate::protocol::station::FixedText;

    fn registration(
        record_id: u32,
        identity: u8,
    ) -> Result<LobbyRegistration, StationProtocolError> {
        Ok(LobbyRegistration {
            mode: 1,
            location: 2,
            matching_quest_index: 10,
            alternate_quest_index: 3,
            lobby_values: [4, 5],
            player_identity: crate::protocol::station::PlayerIdentity([identity; 32]),
            player_controls: [0; 4],
            record_id,
            shop_name: empty_text()?,
            region_names: [empty_text()?, empty_text()?, empty_text()?, empty_text()?],
        })
    }

    fn empty_text<const SIZE: usize>() -> Result<FixedText<SIZE>, StationProtocolError> {
        FixedText::read_be(&mut Cursor::new(vec![0; SIZE]))
            .map_err(|error| StationProtocolError::Binrw(error.to_string()))
    }

    fn lookup() -> LobbyLookup {
        LobbyLookup {
            wait_window: 0x10,
            lobby_value: 0,
            player_or_lobby_key: crate::protocol::station::PlayerIdentity([0; 32]),
        }
    }

    #[test]
    fn matching_assigns_one_owner_and_distinct_slots() -> Result<(), Box<dyn std::error::Error>> {
        let state = OnlineState::new(2, EndpointHost::new("gameservers.aonnet".into())?, 33442);
        let (tx_a, mut rx_a) = mpsc::unbounded_channel();
        let (tx_b, mut rx_b) = mpsc::unbounded_channel();
        assert_eq!(
            state.queue_match(1, registration(100, 0x11)?, lookup(), tx_a)?,
            MatchOutcome::Waiting { waiting_count: 1 }
        );
        let mut second_registration = registration(200, 0x22)?;
        second_registration.lobby_values = [40, 50];
        let mut second_lookup = lookup();
        second_lookup.wait_window = 12;
        assert!(matches!(
            state.queue_match(2, second_registration, second_lookup, tx_b)?,
            MatchOutcome::PartyCreated {
                player_count: 2,
                ..
            }
        ));
        let a = rx_a.try_recv()?;
        let b = rx_b.try_recv()?;
        assert_eq!(a.owner_key, b.owner_key);
        assert_eq!(a.local_slot, PartySlot::new(1)?);
        assert_eq!(b.local_slot, PartySlot::new(2)?);
        Ok(())
    }

    #[test]
    fn matching_does_not_create_a_party_with_a_closed_assignment_receiver()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = OnlineState::new(2, EndpointHost::new("gameservers.aonnet".into())?, 33442);
        let (tx_a, mut rx_a) = mpsc::unbounded_channel();
        let (tx_b, rx_b) = mpsc::unbounded_channel();
        state.queue_match(1, registration(100, 0x11)?, lookup(), tx_a)?;
        drop(rx_b);

        assert!(
            state
                .queue_match(2, registration(200, 0x22)?, lookup(), tx_b)
                .is_err()
        );
        assert!(matches!(
            rx_a.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(
            state.service_counts()?,
            ServiceCounts {
                party_count: 0,
                player_count: 0,
            }
        );
        Ok(())
    }

    #[test]
    fn alternate_quest_mode_keeps_different_quests_apart() -> Result<(), Box<dyn std::error::Error>>
    {
        let state = OnlineState::new(2, EndpointHost::new("gameservers.aonnet".into())?, 33442);
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        let mut first = registration(100, 0x11)?;
        first.matching_quest_index = ALTERNATE_QUEST_INDEX_MODE;
        first.alternate_quest_index = 10;
        let mut second = registration(200, 0x22)?;
        second.matching_quest_index = ALTERNATE_QUEST_INDEX_MODE;
        second.alternate_quest_index = 11;

        assert!(matches!(
            state.queue_match(1, first, lookup(), tx_a)?,
            MatchOutcome::Waiting { waiting_count: 1 }
        ));
        assert!(matches!(
            state.queue_match(2, second, lookup(), tx_b)?,
            MatchOutcome::Waiting { waiting_count: 2 }
        ));
        Ok(())
    }

    #[test]
    fn relay_isolated_party_queues_and_does_not_echo() -> Result<(), Box<dyn std::error::Error>> {
        let state = OnlineState::new(2, EndpointHost::new("gameservers.aonnet".into())?, 33442);
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        state.queue_match(1, registration(100, 0x11)?, lookup(), tx_a)?;
        let outcome = state.queue_match(2, registration(200, 0x22)?, lookup(), tx_b)?;
        let MatchOutcome::PartyCreated { owner_key, .. } = outcome else {
            return Err("party was not created".into());
        };
        state.join_relay(owner_key, PartySlot::new(1)?, 100, 11)?;
        state.join_relay(owner_key, PartySlot::new(2)?, 200, 22)?;
        let source = state.relay_blob(
            owner_key,
            PartySlot::new(1)?,
            11,
            GameplayBlob::new(vec![0x13, 1])?,
        )?;
        assert!(source.records.is_empty());
        let destination = state.relay_blob(
            owner_key,
            PartySlot::new(2)?,
            22,
            GameplayBlob::new(vec![0x13, 2])?,
        )?;
        assert_eq!(destination.records[0].party_slot, PartySlot::new(1)?);
        assert_eq!(destination.records[0].blob.as_bytes(), &[0x13, 1]);
        state.leave_relay(owner_key, PartySlot::new(1)?, 11)?;
        state.leave_relay(owner_key, PartySlot::new(2)?, 22)?;
        assert_eq!(
            state.service_counts()?,
            ServiceCounts {
                party_count: 0,
                player_count: 0,
            }
        );
        Ok(())
    }

    #[test]
    fn relay_announces_the_first_departure_from_a_started_party()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = OnlineState::new(3, EndpointHost::new("gameservers.aonnet".into())?, 33442);
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        let (tx_c, _rx_c) = mpsc::unbounded_channel();
        state.queue_match(1, registration(100, 0x11)?, lookup(), tx_a)?;
        state.queue_match(2, registration(200, 0x22)?, lookup(), tx_b)?;
        let outcome = state.queue_match(3, registration(300, 0x33)?, lookup(), tx_c)?;
        let MatchOutcome::PartyCreated { owner_key, .. } = outcome else {
            return Err("party was not created".into());
        };
        state.join_relay(owner_key, PartySlot::new(1)?, 100, 11)?;
        state.join_relay(owner_key, PartySlot::new(2)?, 200, 22)?;
        state.join_relay(owner_key, PartySlot::new(3)?, 300, 33)?;
        state.leave_relay(owner_key, PartySlot::new(3)?, 33)?;

        let batch = state.relay_blob(
            owner_key,
            PartySlot::new(1)?,
            11,
            GameplayBlob::new(vec![0x13, 1])?,
        )?;
        assert_eq!(batch.flags & 1, 1);
        assert_eq!(batch.flags & (1 << 3), 0);
        Ok(())
    }

    #[test]
    fn network_check_gets_an_immediate_single_station_assignment()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = OnlineState::new(2, EndpointHost::new("gameservers.aonnet".into())?, 33442);
        let (assignment_tx, mut assignment_rx) = mpsc::unbounded_channel();
        let mut registration = registration(0, 0)?;
        registration.matching_quest_index = NETWORK_CHECK_QUEST_ID;
        let lookup = LobbyLookup {
            wait_window: NETWORK_CHECK_WAIT_WINDOW,
            lobby_value: 0,
            player_or_lobby_key: crate::protocol::station::PlayerIdentity([0; 32]),
        };

        assert!(matches!(
            state.queue_match(1, registration, lookup, assignment_tx)?,
            MatchOutcome::PartyCreated {
                player_count: 1,
                ..
            }
        ));
        let assignment = assignment_rx.try_recv()?;
        assert_eq!(assignment.local_slot, PartySlot::new(1)?);
        assert_eq!(assignment.participants.as_slice().len(), 1);
        Ok(())
    }
}
