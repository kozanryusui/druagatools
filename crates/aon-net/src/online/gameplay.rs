use super::lifecycle::{purge_expired_parties, remove_party_for_abort};
use super::*;

impl OnlineState {
    pub(crate) fn join_relay(
        &self,
        owner_key: OwnerKey,
        party_slot: PartySlot,
        record_id: u32,
        connection_id: u64,
        relay: RelaySenders,
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
        if member.gameplay_connection.is_some() {
            return Err(OnlineError::SlotOccupied { party_slot });
        }
        party.members[slot].gameplay_connection = Some(GameplayConnection {
            id: connection_id,
            relay,
        });
        party.last_activity = Instant::now();
        if party
            .members
            .iter()
            .all(|member| member.gameplay_connection.is_some())
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
        let (flags, destinations) = {
            let mut inner = self.lock()?;
            let party =
                inner
                    .parties
                    .get_mut(&binding.owner_key)
                    .ok_or(OnlineError::UnknownParty {
                        owner_key: binding.owner_key,
                    })?;
            if party.members[source]
                .gameplay_connection
                .as_ref()
                .map(|connection| connection.id)
                != Some(binding.connection_id)
            {
                return Err(OnlineError::Connection);
            }
            party.last_activity = Instant::now();
            let flags = active_flags(party);
            let destinations = party
                .members
                .iter()
                .enumerate()
                .filter(|(destination, _)| *destination != source)
                .filter_map(|(_, member)| {
                    member
                        .gameplay_connection
                        .as_ref()
                        .map(|connection| connection.relay.clone())
                })
                .collect::<Vec<_>>();
            (flags, destinations)
        };

        let record = PlayerRecord {
            party_slot: binding.party_slot,
            blob,
        };
        let disconnected_players = destinations
            .into_iter()
            .filter(|destination| {
                match destination.envelope.try_send(RelayEnvelope {
                    flags,
                    records: vec![record.clone()],
                }) {
                    Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => false,
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        let _ = destination
                            .disconnect
                            .try_send(RelayDisconnectReason::QueueFull);
                        true
                    }
                }
            })
            .count();
        Ok(RelayOutcome {
            response: RelayEnvelope {
                flags,
                records: Vec::new(),
            },
            disconnected_players,
        })
    }

    pub(crate) fn leave_relay(&self, binding: RelayBinding) -> Result<(), OnlineError> {
        let slot = binding.party_slot.index();
        let mut inner = self.lock()?;
        let (abort_party, remove_party) =
            if let Some(party) = inner.parties.get_mut(&binding.owner_key) {
                let connection_matches = party.members[slot]
                    .gameplay_connection
                    .as_ref()
                    .map(|connection| connection.id)
                    == Some(binding.connection_id);
                let abort_party =
                    connection_matches && party.roster_readiness == RosterReadiness::Waiting;
                if connection_matches && !abort_party {
                    party.members[slot].gameplay_connection = None;
                    party.last_activity = Instant::now();
                }
                (
                    abort_party,
                    !abort_party
                        && party.roster_readiness == RosterReadiness::Ready
                        && party
                            .members
                            .iter()
                            .all(|member| member.gameplay_connection.is_none()),
                )
            } else {
                (false, false)
            };
        let notifications = if abort_party {
            remove_party_for_abort(
                &mut inner,
                binding.owner_key,
                PartyAbortReason::GameplayDisconnectedBeforeReady,
            )
        } else if remove_party {
            inner.parties.remove(&binding.owner_key);
            None
        } else {
            None
        };
        drop(inner);
        if let Some(notifications) = notifications {
            notifications.send();
        }
        Ok(())
    }
}

fn active_flags(party: &RelayParty) -> GameplayEnvelopeFlags {
    let active_slots = party
        .members
        .iter()
        .zip(PartySlot::ALL)
        .filter_map(|(member, slot)| member.gameplay_connection.as_ref().map(|_| slot));
    GameplayEnvelopeFlags::from_active_slots(active_slots, party.roster_readiness)
}
