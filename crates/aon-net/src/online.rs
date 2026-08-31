use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::sync::mpsc;

use crate::protocol::station::{
    EndpointAssignment, EndpointHost, GameplayBlob, GameplayEnvelopeFlags, LobbyLookup,
    LobbyRegistration, MAX_ENVELOPE_RECORDS, OwnerKey, PartyRoster, PartySlot, PlayerRecord,
    StationProtocolError,
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
pub(crate) struct RelayBatch {
    pub flags: GameplayEnvelopeFlags,
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
    pub(crate) fn new(gameplay_host: EndpointHost, gameplay_port: u16) -> Self {
        Self {
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
        purge_closed_assembling_members(&mut inner);

        let (party_index, local_index) = if let Some((party_index, member_index)) = inner
            .assembling
            .iter()
            .enumerate()
            .find_map(|(party_index, party)| {
                party
                    .members
                    .iter()
                    .position(|member| member.connection_id == connection_id)
                    .map(|member_index| (party_index, member_index))
            }) {
            let member = &mut inner.assembling[party_index].members[member_index];
            member.registration = registration;
            member.assignment_tx = assignment_tx;
            (party_index, member_index)
        } else if let Some(index) = inner.assembling.iter().position(|party| {
            party.members.len() < MAX_PARTY_PLAYERS
                && compatible(&party.members[0].registration, &registration)
        }) {
            let member_index = inner.assembling[index].members.len();
            inner.assembling[index].members.push(AssemblingMember {
                connection_id,
                registration,
                assignment_tx,
            });
            (index, member_index)
        } else {
            let owner_key = allocate_owner_key(&mut inner)?;
            inner.assembling.push(AssemblingParty {
                owner_key,
                members: vec![AssemblingMember {
                    connection_id,
                    registration,
                    assignment_tx,
                }],
            });
            (inner.assembling.len() - 1, 0)
        };

        let should_finalize = {
            let party = &inner.assembling[party_index];
            party.members.len() == MAX_PARTY_PLAYERS
                || lookup.remaining_wait_seconds == 0
                || is_network_check(&party.members[0].registration, &lookup)
        };
        if should_finalize {
            return self.finalize_party(inner, party_index);
        }

        let assignment = self.assignment_for(&inner.assembling[party_index], local_index, false)?;
        let waiting_count = inner
            .assembling
            .iter()
            .map(|party| party.members.len())
            .sum();
        Ok(MatchOutcome::Assembling {
            assignment,
            waiting_count,
        })
    }

    fn assignment_for(
        &self,
        party: &AssemblingParty,
        local_index: usize,
        ready: bool,
    ) -> Result<EndpointAssignment, StationProtocolError> {
        let participants = PartyRoster::new(
            party
                .members
                .iter()
                .map(|member| member.registration.player_identity)
                .collect(),
        )?;
        Ok(EndpointAssignment {
            host: self.gameplay_host.clone(),
            port: self.gameplay_port,
            owner_key: party.owner_key,
            ready,
            local_slot: PartySlot::new((local_index + 1) as u8)?,
            matching_quest_index: party.members[0].registration.matching_quest_index,
            participants,
        })
    }

    fn finalize_party(
        &self,
        mut inner: MutexGuard<'_, OnlineInner>,
        party_index: usize,
    ) -> Result<MatchOutcome, OnlineError> {
        let party = inner.assembling.remove(party_index);
        let owner_key = party.owner_key;
        let assignments = party
            .members
            .iter()
            .enumerate()
            .map(|(local_index, member)| {
                Ok((
                    member.connection_id,
                    member.assignment_tx.clone(),
                    self.assignment_for(&party, local_index, true)?,
                ))
            })
            .collect::<Result<Vec<_>, StationProtocolError>>()?;
        let members = party
            .members
            .iter()
            .map(|member| PartyMember {
                record_id: member.registration.record_id,
            })
            .collect();
        let player_count = party.members.len();
        inner.parties.insert(
            owner_key,
            RelayParty {
                members,
                live_connections: [None; MAX_PARTY_PLAYERS],
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
        let mut inner = self.lock()?;
        for party in &mut inner.assembling {
            party
                .members
                .retain(|member| member.connection_id != connection_id);
        }
        inner.assembling.retain(|party| !party.members.is_empty());
        Ok(())
    }

    pub(crate) fn join_relay(
        &self,
        owner_key: OwnerKey,
        party_slot: PartySlot,
        record_id: u32,
        connection_id: u64,
    ) -> Result<GameplayEnvelopeFlags, OnlineError> {
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
        if !inner.parties.contains_key(&owner_key)
            && inner
                .assembling
                .iter()
                .all(|party| party.owner_key != owner_key)
        {
            return Ok(owner_key);
        }
    }
}

fn purge_closed_assembling_members(inner: &mut OnlineInner) {
    for party in &mut inner.assembling {
        party
            .members
            .retain(|member| !member.assignment_tx.is_closed());
    }
    inner.assembling.retain(|party| !party.members.is_empty());
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
        && lookup.elapsed_wait_seconds == NETWORK_CHECK_ELAPSED_WAIT_SECONDS
        && lookup.remaining_wait_seconds == 0
}

fn active_flags(party: &RelayParty) -> GameplayEnvelopeFlags {
    let active_slots = party
        .live_connections
        .iter()
        .zip(PartySlot::ALL)
        .filter_map(|(connection, slot)| connection.map(|_| slot));
    let roster_changed =
        party.started && party.live_connections.iter().flatten().count() < party.members.len();
    GameplayEnvelopeFlags::from_active_slots(active_slots, roster_changed)
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
            elapsed_wait_seconds: 16,
            remaining_wait_seconds: 19,
            player_or_lobby_key: crate::protocol::station::PlayerIdentity([0; 32]),
        }
    }

    #[test]
    fn matching_updates_partial_roster_until_the_party_is_full()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = OnlineState::new(EndpointHost::new("gameservers.aonnet".into())?, 33442);
        let (tx_a, mut rx_a) = mpsc::unbounded_channel();
        let (tx_b, mut rx_b) = mpsc::unbounded_channel();
        let (tx_c, mut rx_c) = mpsc::unbounded_channel();
        let (tx_d, mut rx_d) = mpsc::unbounded_channel();
        let MatchOutcome::Assembling {
            assignment: first,
            waiting_count: 1,
        } = state.queue_match(1, registration(100, 0x11)?, lookup(), tx_a.clone())?
        else {
            return Err("first player did not start an assembling party".into());
        };
        assert!(!first.ready);
        assert_eq!(first.local_slot, PartySlot::new(1)?);
        assert_eq!(first.participants.as_slice().len(), 1);

        let mut second_registration = registration(200, 0x22)?;
        second_registration.lobby_values = [40, 50];
        let MatchOutcome::Assembling {
            assignment: second,
            waiting_count: 2,
        } = state.queue_match(2, second_registration, lookup(), tx_b)?
        else {
            return Err("second player did not join the assembling party".into());
        };
        assert_eq!(first.owner_key, second.owner_key);
        assert_eq!(second.local_slot, PartySlot::new(2)?);
        assert_eq!(second.participants.as_slice().len(), 2);

        let MatchOutcome::Assembling {
            assignment: first_update,
            ..
        } = state.queue_match(1, registration(100, 0x11)?, lookup(), tx_a)?
        else {
            return Err("first player did not receive an updated partial roster".into());
        };
        assert_eq!(first_update.local_slot, PartySlot::new(1)?);
        assert_eq!(first_update.participants.as_slice().len(), 2);

        state.queue_match(3, registration(300, 0x33)?, lookup(), tx_c)?;
        let outcome = state.queue_match(4, registration(400, 0x44)?, lookup(), tx_d)?;
        assert!(matches!(
            outcome,
            MatchOutcome::PartyCreated {
                player_count: 4,
                ..
            }
        ));
        let a = rx_a.try_recv()?;
        let b = rx_b.try_recv()?;
        let c = rx_c.try_recv()?;
        let d = rx_d.try_recv()?;
        assert!(a.ready && b.ready && c.ready && d.ready);
        assert_eq!(a.local_slot, PartySlot::new(1)?);
        assert_eq!(b.local_slot, PartySlot::new(2)?);
        assert_eq!(c.local_slot, PartySlot::new(3)?);
        assert_eq!(d.local_slot, PartySlot::new(4)?);
        assert_eq!(a.participants.as_slice().len(), 4);
        Ok(())
    }

    #[test]
    fn one_expired_member_finalizes_the_partial_party_and_late_players_start_a_new_one()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = OnlineState::new(EndpointHost::new("gameservers.aonnet".into())?, 33442);
        let (tx_a, mut rx_a) = mpsc::unbounded_channel();
        let (tx_b, mut rx_b) = mpsc::unbounded_channel();
        let (tx_c, _rx_c) = mpsc::unbounded_channel();
        state.queue_match(1, registration(100, 0x11)?, lookup(), tx_a.clone())?;
        state.queue_match(2, registration(200, 0x22)?, lookup(), tx_b)?;

        let mut expired = lookup();
        expired.elapsed_wait_seconds = 35;
        expired.remaining_wait_seconds = 0;
        let outcome = state.queue_match(1, registration(100, 0x11)?, expired, tx_a)?;
        let MatchOutcome::PartyCreated {
            owner_key,
            player_count: 2,
        } = outcome
        else {
            return Err("expired wait did not finalize the two-player party".into());
        };
        assert!(rx_a.try_recv()?.ready);
        assert!(rx_b.try_recv()?.ready);

        let MatchOutcome::Assembling { assignment, .. } =
            state.queue_match(3, registration(300, 0x33)?, lookup(), tx_c)?
        else {
            return Err("late player did not start a new assembling party".into());
        };
        assert_ne!(assignment.owner_key, owner_key);
        assert_eq!(assignment.participants.as_slice().len(), 1);
        Ok(())
    }

    #[test]
    fn matching_does_not_create_a_party_with_a_closed_assignment_receiver()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = OnlineState::new(EndpointHost::new("gameservers.aonnet".into())?, 33442);
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
        let state = OnlineState::new(EndpointHost::new("gameservers.aonnet".into())?, 33442);
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        let mut first = registration(100, 0x11)?;
        first.matching_quest_index = ALTERNATE_QUEST_INDEX_MODE;
        first.alternate_quest_index = 10;
        let mut second = registration(200, 0x22)?;
        second.matching_quest_index = ALTERNATE_QUEST_INDEX_MODE;
        second.alternate_quest_index = 11;

        let MatchOutcome::Assembling {
            assignment: first_assignment,
            waiting_count: 1,
        } = state.queue_match(1, first, lookup(), tx_a)?
        else {
            return Err("first quest did not start an assembling party".into());
        };
        let MatchOutcome::Assembling {
            assignment: second_assignment,
            waiting_count: 2,
        } = state.queue_match(2, second, lookup(), tx_b)?
        else {
            return Err("second quest did not start a separate assembling party".into());
        };
        assert_ne!(first_assignment.owner_key, second_assignment.owner_key);
        Ok(())
    }

    #[test]
    fn relay_isolated_party_queues_and_does_not_echo() -> Result<(), Box<dyn std::error::Error>> {
        let state = OnlineState::new(EndpointHost::new("gameservers.aonnet".into())?, 33442);
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        state.queue_match(1, registration(100, 0x11)?, lookup(), tx_a)?;
        let mut expired = lookup();
        expired.elapsed_wait_seconds = 35;
        expired.remaining_wait_seconds = 0;
        let outcome = state.queue_match(2, registration(200, 0x22)?, expired, tx_b)?;
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
        let state = OnlineState::new(EndpointHost::new("gameservers.aonnet".into())?, 33442);
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        let (tx_c, _rx_c) = mpsc::unbounded_channel();
        state.queue_match(1, registration(100, 0x11)?, lookup(), tx_a)?;
        state.queue_match(2, registration(200, 0x22)?, lookup(), tx_b)?;
        let mut expired = lookup();
        expired.elapsed_wait_seconds = 35;
        expired.remaining_wait_seconds = 0;
        let outcome = state.queue_match(3, registration(300, 0x33)?, expired, tx_c)?;
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
        assert!(batch.flags.roster_changed());
        assert_eq!(batch.flags.active_player_count(), 2);
        assert!(!batch.flags.has_sole_survivor());

        state.leave_relay(owner_key, PartySlot::new(2)?, 22)?;
        let batch = state.relay_blob(
            owner_key,
            PartySlot::new(1)?,
            11,
            GameplayBlob::new(vec![0x13, 1])?,
        )?;
        assert!(batch.flags.has_sole_survivor());
        Ok(())
    }

    #[test]
    fn envelope_flags_report_a_sole_survivor_only_after_a_roster_change()
    -> Result<(), StationProtocolError> {
        let slot_1 = PartySlot::new(1)?;
        let slot_2 = PartySlot::new(2)?;

        assert!(!GameplayEnvelopeFlags::from_active_slots([slot_1], false).has_sole_survivor());
        assert!(GameplayEnvelopeFlags::from_active_slots([slot_1], true).has_sole_survivor());
        assert!(
            !GameplayEnvelopeFlags::from_active_slots([slot_1, slot_2], true).has_sole_survivor()
        );
        Ok(())
    }

    #[test]
    fn network_check_gets_an_immediate_single_station_assignment()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = OnlineState::new(EndpointHost::new("gameservers.aonnet".into())?, 33442);
        let (assignment_tx, mut assignment_rx) = mpsc::unbounded_channel();
        let mut registration = registration(0, 0)?;
        registration.matching_quest_index = NETWORK_CHECK_QUEST_ID;
        let lookup = LobbyLookup {
            elapsed_wait_seconds: NETWORK_CHECK_ELAPSED_WAIT_SECONDS,
            remaining_wait_seconds: 0,
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
        assert!(assignment.ready);
        assert_eq!(assignment.local_slot, PartySlot::new(1)?);
        assert_eq!(assignment.participants.as_slice().len(), 1);
        Ok(())
    }
}
