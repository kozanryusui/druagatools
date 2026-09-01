use super::*;

impl OnlineState {
    pub(crate) fn join_relay(
        &self,
        owner_key: OwnerKey,
        party_slot: PartySlot,
        record_id: u32,
        connection_id: u64,
    ) -> Result<RelayJoin, OnlineError> {
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
        if member.gameplay_connection_id.is_some() {
            return Err(OnlineError::SlotOccupied { party_slot });
        }
        party.members[slot].gameplay_connection_id = Some(connection_id);
        party.last_activity = Instant::now();
        if party
            .members
            .iter()
            .all(|member| member.gameplay_connection_id.is_some())
        {
            party.roster_readiness = RosterReadiness::Ready;
        }
        Ok(RelayJoin {
            binding: RelayBinding {
                owner_key,
                party_slot,
                connection_id,
            },
            flags: active_flags(party),
        })
    }

    pub(crate) fn relay_blob(
        &self,
        binding: RelayBinding,
        blob: GameplayBlob,
    ) -> Result<RelayOutcome, OnlineError> {
        let source = binding.party_slot.index();
        let mut inner = self.lock()?;
        let party = inner
            .parties
            .get_mut(&binding.owner_key)
            .ok_or(OnlineError::UnknownParty {
                owner_key: binding.owner_key,
            })?;
        if party.members[source].gameplay_connection_id != Some(binding.connection_id) {
            return Err(OnlineError::Connection);
        }
        party.last_activity = Instant::now();

        let mut dropped_records = 0;
        for (destination_index, destination) in party.members.iter_mut().enumerate() {
            if destination_index == source || destination.gameplay_connection_id.is_none() {
                continue;
            }
            let queue = &mut destination.gameplay_queue;
            if queue.len() == MAX_PLAYER_QUEUE {
                queue.pop_front();
                dropped_records += 1;
            }
            queue.push_back(PlayerRecord {
                party_slot: binding.party_slot,
                blob: blob.clone(),
            });
        }
        let source_queue = &mut party.members[source].gameplay_queue;
        let records = source_queue
            .drain(..source_queue.len().min(MAX_ENVELOPE_RECORDS))
            .collect();
        Ok(RelayOutcome {
            response: RelayEnvelope {
                flags: active_flags(party),
                records,
            },
            dropped_records,
        })
    }

    pub(crate) fn leave_relay(&self, binding: RelayBinding) -> Result<(), OnlineError> {
        let slot = binding.party_slot.index();
        let mut inner = self.lock()?;
        let remove_party = if let Some(party) = inner.parties.get_mut(&binding.owner_key) {
            if party.members[slot].gameplay_connection_id == Some(binding.connection_id) {
                party.members[slot].gameplay_connection_id = None;
                party.members[slot].gameplay_queue.clear();
                party.last_activity = Instant::now();
            }
            party.roster_readiness == RosterReadiness::Ready
                && party
                    .members
                    .iter()
                    .all(|member| member.gameplay_connection_id.is_none())
        } else {
            false
        };
        if remove_party {
            inner.parties.remove(&binding.owner_key);
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
                .map(|party| {
                    party
                        .members
                        .iter()
                        .filter(|member| member.gameplay_connection_id.is_some())
                        .count()
                })
                .sum::<usize>()
                .min(u16::MAX as usize) as u16,
        })
    }
}

pub(super) fn purge_expired_parties(inner: &mut OnlineInner) {
    inner.parties.retain(|_, party| {
        party
            .members
            .iter()
            .any(|member| member.gameplay_connection_id.is_some())
            || party.last_activity.elapsed() < UNCLAIMED_PARTY_LIFETIME
    });
}

fn active_flags(party: &RelayParty) -> GameplayEnvelopeFlags {
    let active_slots = party
        .members
        .iter()
        .zip(PartySlot::ALL)
        .filter_map(|(member, slot)| member.gameplay_connection_id.map(|_| slot));
    GameplayEnvelopeFlags::from_active_slots(active_slots, party.roster_readiness)
}
