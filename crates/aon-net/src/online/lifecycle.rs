use super::*;

impl OnlineState {
    pub(crate) fn leave_matching(&self, connection_id: u64) -> Result<(), OnlineError> {
        let mut inner = self.lock()?;
        for party in &mut inner.assembling {
            party
                .members
                .retain(|member| member.connection_id != connection_id);
        }
        inner.assembling.retain(|party| !party.members.is_empty());
        let abort_owner_key = inner.parties.iter().find_map(|(owner_key, party)| {
            party
                .members
                .iter()
                .find(|member| member.matching_connection_id == connection_id)
                .filter(|member| {
                    party.roster_readiness == RosterReadiness::Waiting && !member.matching_complete
                })
                .map(|_| *owner_key)
        });
        let notifications = abort_owner_key.and_then(|owner_key| {
            remove_party_for_abort(
                &mut inner,
                owner_key,
                PartyAbortReason::MatchingDisconnected,
            )
        });
        drop(inner);
        if let Some(notifications) = notifications {
            notifications.send();
        }
        self.publish_status()?;
        Ok(())
    }

    pub(crate) fn expire_gameplay_handoff(&self, connection_id: u64) -> Result<(), OnlineError> {
        let mut inner = self.lock()?;
        let owner_key = inner.parties.iter().find_map(|(owner_key, party)| {
            party
                .members
                .iter()
                .find(|member| member.matching_connection_id == connection_id)
                .filter(|member| {
                    member.matching_complete
                        && member.gameplay_connection.is_none()
                        && party.roster_readiness == RosterReadiness::Waiting
                })
                .map(|_| *owner_key)
        });
        let notifications = owner_key.and_then(|owner_key| {
            remove_party_for_abort(
                &mut inner,
                owner_key,
                PartyAbortReason::GameplayHandoffTimeout,
            )
        });
        drop(inner);
        if let Some(notifications) = notifications {
            notifications.send();
        }
        self.publish_status()?;
        Ok(())
    }

    pub(crate) fn service_counts(&self) -> Result<ServiceCounts, OnlineError> {
        let mut inner = self.lock()?;
        let status_changed = purge_expired_parties(&mut inner);
        let counts = ServiceCounts {
            party_count: inner.parties.len().min(u16::MAX as usize) as u16,
            player_count: inner
                .parties
                .values()
                .map(|party| {
                    party
                        .members
                        .iter()
                        .filter(|member| member.gameplay_connection.is_some())
                        .count()
                })
                .sum::<usize>()
                .min(u16::MAX as usize) as u16,
        };
        drop(inner);
        if status_changed {
            self.publish_status()?;
        }
        Ok(counts)
    }
}

pub(super) struct PartyAbortNotifications {
    reason: PartyAbortReason,
    matching: Vec<mpsc::Sender<PartyAbortReason>>,
    gameplay: Vec<mpsc::Sender<RelayDisconnectReason>>,
}

impl PartyAbortNotifications {
    pub(super) fn send(self) {
        for destination in self.matching {
            let _ = destination.try_send(self.reason);
        }
        for destination in self.gameplay {
            let _ = destination.try_send(RelayDisconnectReason::PartyAborted(self.reason));
        }
    }
}

pub(super) fn remove_party_for_abort(
    inner: &mut OnlineInner,
    owner_key: OwnerKey,
    reason: PartyAbortReason,
) -> Option<PartyAbortNotifications> {
    let party = inner.parties.remove(&owner_key)?;
    Some(PartyAbortNotifications {
        reason,
        matching: party
            .members
            .iter()
            .map(|member| member.matching_cancellation_tx.clone())
            .collect(),
        gameplay: party
            .members
            .iter()
            .filter_map(|member| {
                member
                    .gameplay_connection
                    .as_ref()
                    .map(|connection| connection.relay.disconnect.clone())
            })
            .collect(),
    })
}

pub(super) fn purge_expired_parties(inner: &mut OnlineInner) -> bool {
    let previous_len = inner.parties.len();
    inner.parties.retain(|_, party| {
        party
            .members
            .iter()
            .any(|member| member.gameplay_connection.is_some())
            || party.last_activity.elapsed() < UNCLAIMED_PARTY_LIFETIME
    });
    inner.parties.len() != previous_len
}
