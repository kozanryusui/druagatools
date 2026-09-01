use super::gameplay::purge_expired_parties;
use super::*;

impl OnlineState {
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
                .enumerate()
                .map(|(index, member)| {
                    ParticipantRecord::from_player_identity(
                        PartySlot::ALL[index],
                        member.registration.player_identity,
                    )
                })
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
            .enumerate()
            .map(|(index, member)| RelayMember {
                record_id: member.registration.record_id,
                matching_connection_id: member.connection_id,
                matching_record: ParticipantRecord::from_player_identity(
                    PartySlot::ALL[index],
                    member.registration.player_identity,
                ),
                matching_queues: std::array::from_fn(|_| VecDeque::new()),
                gameplay_connection_id: None,
                gameplay_queue: VecDeque::new(),
            })
            .collect();
        let player_count = party.members.len();
        inner.parties.insert(
            owner_key,
            RelayParty {
                members,
                matching_quest_index: party.members[0].registration.matching_quest_index,
                roster_readiness: RosterReadiness::Waiting,
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

    pub(crate) fn exchange_participant_record(
        &self,
        connection_id: u64,
        player_record: PlayerIdentity,
    ) -> Result<EndpointAssignment, OnlineError> {
        let mut inner = self.lock()?;
        let (owner_key, party) = inner
            .parties
            .iter_mut()
            .find(|(_, party)| {
                party
                    .members
                    .iter()
                    .any(|member| member.matching_connection_id == connection_id)
            })
            .ok_or(OnlineError::UnknownMatchingConnection { connection_id })?;
        let local_index = party
            .members
            .iter()
            .position(|member| member.matching_connection_id == connection_id)
            .ok_or(OnlineError::UnknownMatchingConnection { connection_id })?;
        let record =
            ParticipantRecord::from_player_identity(PartySlot::ALL[local_index], player_record);
        party.members[local_index].matching_record = record;
        for (destination_index, destination) in party.members.iter_mut().enumerate() {
            if destination_index == local_index {
                continue;
            }
            let queue = &mut destination.matching_queues[local_index];
            if queue.len() == MAX_PLAYER_QUEUE {
                queue.pop_front();
            }
            queue.push_back(record);
        }

        let latest_records = party
            .members
            .iter()
            .map(|member| member.matching_record)
            .collect::<Vec<_>>();
        let local_member = &mut party.members[local_index];
        let participants = PartyRoster::new(
            latest_records
                .into_iter()
                .enumerate()
                .map(|(source, latest_record)| {
                    local_member.matching_queues[source]
                        .pop_front()
                        .unwrap_or(latest_record)
                })
                .collect(),
        )?;
        party.last_activity = Instant::now();
        Ok(EndpointAssignment {
            host: self.gameplay_host.clone(),
            port: self.gameplay_port,
            owner_key: *owner_key,
            ready: true,
            local_slot: PartySlot::ALL[local_index],
            matching_quest_index: party.matching_quest_index,
            participants,
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
