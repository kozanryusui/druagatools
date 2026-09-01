use std::io::Cursor;

use binrw::BinRead;

use super::*;
use crate::protocol::station::FixedText;

type CancellationReceivers = [mpsc::Receiver<PartyAbortReason>; 2];

trait TestOnlineState {
    fn queue_match(
        &self,
        connection_id: u64,
        registration: LobbyRegistration,
        lookup: LobbyLookup,
        assignment_tx: mpsc::UnboundedSender<EndpointAssignment>,
    ) -> Result<MatchOutcome, OnlineError>;
}

impl TestOnlineState for OnlineState {
    fn queue_match(
        &self,
        connection_id: u64,
        registration: LobbyRegistration,
        lookup: LobbyLookup,
        assignment_tx: mpsc::UnboundedSender<EndpointAssignment>,
    ) -> Result<MatchOutcome, OnlineError> {
        let (cancellation_tx, _cancellation_rx) = matching_cancellation_channel();
        self.queue_match_with_cancellation(
            connection_id,
            registration,
            lookup,
            assignment_tx,
            cancellation_tx,
        )
    }
}

fn registration(record_id: u32, identity: u8) -> Result<LobbyRegistration, StationProtocolError> {
    Ok(LobbyRegistration {
        mode: 1,
        location: 2,
        matching_quest_index: 10,
        alternate_quest_index: 3,
        lobby_values: [4, 5],
        player_identity: crate::protocol::station::PlayerIdentity::from_bytes([identity; 32]),
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
        player_or_lobby_key: crate::protocol::station::PlayerIdentity::from_bytes([0; 32]),
    }
}

fn participant_identity(marker: u8) -> PlayerIdentity {
    let mut bytes = [0; 32];
    bytes[1] = marker;
    PlayerIdentity::from_bytes(bytes)
}

fn finalized_two_player_party(
    state: &OnlineState,
) -> Result<(OwnerKey, CancellationReceivers), Box<dyn std::error::Error>> {
    let (assignment_tx_a, _assignment_rx_a) = mpsc::unbounded_channel();
    let (assignment_tx_b, _assignment_rx_b) = mpsc::unbounded_channel();
    let (cancellation_tx_a, cancellation_rx_a) = matching_cancellation_channel();
    let (cancellation_tx_b, cancellation_rx_b) = matching_cancellation_channel();
    state.queue_match_with_cancellation(
        1,
        registration(100, 0x11)?,
        lookup(),
        assignment_tx_a,
        cancellation_tx_a,
    )?;
    let mut expired = lookup();
    expired.remaining_wait_seconds = 0;
    let outcome = state.queue_match_with_cancellation(
        2,
        registration(200, 0x22)?,
        expired,
        assignment_tx_b,
        cancellation_tx_b,
    )?;
    let MatchOutcome::PartyCreated { owner_key, .. } = outcome else {
        return Err("party was not created".into());
    };
    Ok((owner_key, [cancellation_rx_a, cancellation_rx_b]))
}

#[test]
fn status_reports_matching_queues_and_relay_connections() -> Result<(), Box<dyn std::error::Error>>
{
    let state = OnlineState::new(EndpointHost::new("gameservers.aonnet".into())?, 33442);
    assert_eq!(
        state.status()?,
        OnlineStatus {
            matching_queues: Vec::new(),
            relays: Vec::new(),
        }
    );

    let (assignment_tx_a, _assignment_rx_a) = mpsc::unbounded_channel();
    let (assignment_tx_b, _assignment_rx_b) = mpsc::unbounded_channel();
    state.queue_match(1, registration(100, 0x11)?, lookup(), assignment_tx_a)?;
    let waiting = state.status()?;
    assert_eq!(waiting.matching_queues.len(), 1);
    assert_eq!(waiting.matching_queues[0].map_id, 10);
    assert_eq!(waiting.matching_queues[0].queued_players, 1);
    assert_eq!(waiting.matching_queues[0].party_capacity, 4);
    assert!(waiting.relays.is_empty());

    let mut expired = lookup();
    expired.remaining_wait_seconds = 0;
    let outcome = state.queue_match(2, registration(200, 0x22)?, expired, assignment_tx_b)?;
    let MatchOutcome::PartyCreated { owner_key, .. } = outcome else {
        return Err("party was not created".into());
    };
    let connecting = state.status()?;
    assert!(connecting.matching_queues.is_empty());
    assert_eq!(connecting.relays.len(), 1);
    assert_eq!(connecting.relays[0].map_id, 10);
    assert_eq!(connecting.relays[0].party_players, 2);
    assert_eq!(connecting.relays[0].connected_players, 0);
    assert_eq!(connecting.relays[0].status, RelayPartyStatus::Connecting);

    let (relay_a, _receivers_a) = relay_channels(NonZeroUsize::MIN);
    let (relay_b, _receivers_b) = relay_channels(NonZeroUsize::MIN);
    state.join_relay(owner_key, PartySlot::new(1)?, 100, 11, relay_a)?;
    assert_eq!(state.status()?.relays[0].connected_players, 1);
    state.join_relay(owner_key, PartySlot::new(2)?, 200, 22, relay_b)?;
    let playing = state.status()?;
    assert_eq!(playing.relays[0].connected_players, 2);
    assert_eq!(playing.relays[0].status, RelayPartyStatus::Playing);
    Ok(())
}

#[test]
fn online_status_changes_are_pushed_once() -> Result<(), Box<dyn std::error::Error>> {
    let hub = Arc::new(AdminHub::new(8));
    let mut events = hub.subscribe();
    let state =
        OnlineState::with_admin_hub(EndpointHost::new("gameservers.aonnet".into())?, 33442, hub);
    let (assignment_tx, _assignment_rx) = mpsc::unbounded_channel();
    state.queue_match(1, registration(100, 0x11)?, lookup(), assignment_tx.clone())?;

    let event = events.try_recv()?;
    let AdminEvent::OnlineStatusChanged(status) = event.event else {
        return Err("online state did not publish a status event".into());
    };
    assert_eq!(status.matching_queues[0].queued_players, 1);

    state.queue_match(1, registration(100, 0x11)?, lookup(), assignment_tx)?;
    assert!(events.try_recv().is_err());
    Ok(())
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
fn matching_relays_each_participant_record_in_order() -> Result<(), Box<dyn std::error::Error>> {
    let state = OnlineState::new(EndpointHost::new("gameservers.aonnet".into())?, 33442);
    let (tx_a, mut rx_a) = mpsc::unbounded_channel();
    let (tx_b, mut rx_b) = mpsc::unbounded_channel();
    state.queue_match(1, registration(100, 0x11)?, lookup(), tx_a.clone())?;
    state.queue_match(2, registration(200, 0x22)?, lookup(), tx_b)?;
    let mut expired = lookup();
    expired.elapsed_wait_seconds = 35;
    expired.remaining_wait_seconds = 0;
    state.queue_match(1, registration(100, 0x11)?, expired, tx_a)?;
    rx_a.try_recv()?;
    rx_b.try_recv()?;

    state.exchange_participant_record(1, participant_identity(0x10))?;
    let first = state.exchange_participant_record(2, participant_identity(0x13))?;
    assert_eq!(
        first.assignment.participants.as_slice()[0]
            .participant_marker()
            .get(),
        0x10
    );

    state.exchange_participant_record(1, participant_identity(0x11))?;
    state.exchange_participant_record(1, participant_identity(0x12))?;
    let second = state.exchange_participant_record(2, participant_identity(0x1d))?;
    let third = state.exchange_participant_record(2, participant_identity(0x16))?;
    assert_eq!(
        second.assignment.participants.as_slice()[0]
            .participant_marker()
            .get(),
        0x11
    );
    assert_eq!(
        third.assignment.participants.as_slice()[0]
            .participant_marker()
            .get(),
        0x12
    );
    assert_eq!(third.assignment.local_slot, PartySlot::new(2)?);
    assert_eq!(third.assignment.matching_quest_index, 10);
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
fn post_assignment_matching_close_preserves_the_gameplay_handoff()
-> Result<(), Box<dyn std::error::Error>> {
    let state = OnlineState::new(EndpointHost::new("gameservers.aonnet".into())?, 33442);
    let (owner_key, [mut cancellation_rx_a, mut cancellation_rx_b]) =
        finalized_two_player_party(&state)?;

    state.leave_matching(1)?;
    state.leave_matching(2)?;
    let (relay_a, _relay_rx_a) = relay_channels(NonZeroUsize::MIN);
    let (relay_b, _relay_rx_b) = relay_channels(NonZeroUsize::MIN);
    state.join_relay(owner_key, PartySlot::new(1)?, 100, 11, relay_a)?;
    let join_b = state.join_relay(owner_key, PartySlot::new(2)?, 200, 12, relay_b)?;

    assert!(join_b.flags.roster_ready());
    assert!(matches!(
        cancellation_rx_a.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        cancellation_rx_b.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    Ok(())
}

#[test]
fn completed_matching_close_waits_for_gameplay_handoff() -> Result<(), Box<dyn std::error::Error>> {
    let state = OnlineState::new(EndpointHost::new("gameservers.aonnet".into())?, 33442);
    let (_, [mut cancellation_rx_a, mut cancellation_rx_b]) = finalized_two_player_party(&state)?;

    state.exchange_participant_record(2, participant_identity(0x16))?;
    let exchange = state.exchange_participant_record(1, participant_identity(0x16))?;
    let Some(completion) = exchange.completion else {
        return Err("participant exchange did not complete".into());
    };
    assert!(state.confirm_matching_complete(completion)?);
    state.leave_matching(1)?;
    assert_eq!(state.service_counts()?.party_count, 1);

    state.expire_gameplay_handoff(1)?;
    assert_eq!(
        cancellation_rx_a.try_recv()?,
        PartyAbortReason::GameplayHandoffTimeout
    );
    assert_eq!(
        cancellation_rx_b.try_recv()?,
        PartyAbortReason::GameplayHandoffTimeout
    );
    assert_eq!(state.service_counts()?.party_count, 0);
    Ok(())
}

#[test]
fn gameplay_disconnect_before_roster_ready_aborts_the_party()
-> Result<(), Box<dyn std::error::Error>> {
    let state = OnlineState::new(EndpointHost::new("gameservers.aonnet".into())?, 33442);
    let (owner_key, [mut cancellation_rx_a, mut cancellation_rx_b]) =
        finalized_two_player_party(&state)?;
    let (relay, mut relay_rx) = relay_channels(NonZeroUsize::MIN);
    let join = state.join_relay(owner_key, PartySlot::new(1)?, 100, 11, relay)?;

    state.leave_relay(join.binding)?;

    assert_eq!(
        relay_rx.disconnect.try_recv()?,
        RelayDisconnectReason::PartyAborted(PartyAbortReason::GameplayDisconnectedBeforeReady)
    );
    assert_eq!(
        cancellation_rx_a.try_recv()?,
        PartyAbortReason::GameplayDisconnectedBeforeReady
    );
    assert_eq!(
        cancellation_rx_b.try_recv()?,
        PartyAbortReason::GameplayDisconnectedBeforeReady
    );
    assert_eq!(state.service_counts()?.party_count, 0);
    Ok(())
}

#[test]
fn alternate_quest_mode_keeps_different_quests_apart() -> Result<(), Box<dyn std::error::Error>> {
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
    let status = state.status()?;
    assert_eq!(
        status
            .matching_queues
            .iter()
            .map(|queue| queue.map_id)
            .collect::<Vec<_>>(),
        [10, 11]
    );
    Ok(())
}

#[test]
fn relay_pushes_to_peers_and_disconnects_a_full_destination()
-> Result<(), Box<dyn std::error::Error>> {
    let state = OnlineState::new(EndpointHost::new("gameservers.aonnet".into())?, 33442);
    let (owner_key, _) = finalized_two_player_party(&state)?;
    let (relay_a, mut relay_rx_a) = relay_channels(NonZeroUsize::MIN);
    let (relay_b, mut relay_rx_b) = relay_channels(NonZeroUsize::MIN);
    let join_a = state.join_relay(owner_key, PartySlot::new(1)?, 100, 11, relay_a)?;
    let join_b = state.join_relay(owner_key, PartySlot::new(2)?, 200, 22, relay_b)?;
    let source = state.relay_blob(join_a.binding, GameplayBlob::new(vec![0x13, 1])?)?;
    assert!(source.response.records.is_empty());
    assert!(source.response.flags.roster_ready());
    assert_eq!(source.response.flags.bits(), 0x07);
    assert!(relay_rx_a.envelope.try_recv().is_err());
    let overflow = state.relay_blob(join_a.binding, GameplayBlob::new(vec![0x13, 3])?)?;
    assert_eq!(overflow.disconnected_players, 1);
    assert_eq!(
        relay_rx_b.disconnect.try_recv()?,
        RelayDisconnectReason::QueueFull
    );
    let destination = relay_rx_b.envelope.try_recv()?;
    assert_eq!(destination.records[0].party_slot, PartySlot::new(1)?);
    assert_eq!(destination.records[0].blob.as_bytes(), &[0x13, 1]);
    let response = state.relay_blob(join_b.binding, GameplayBlob::new(vec![0x13, 2])?)?;
    assert!(response.response.records.is_empty());
    let destination = relay_rx_a.envelope.try_recv()?;
    assert_eq!(destination.records[0].party_slot, PartySlot::new(2)?);
    assert_eq!(destination.records[0].blob.as_bytes(), &[0x13, 2]);
    state.leave_relay(join_a.binding)?;
    state.leave_relay(join_b.binding)?;
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
fn relay_keeps_the_ready_flag_after_a_started_party_loses_a_player()
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
    let (relay_a, _relay_rx_a) = relay_channels(NonZeroUsize::MIN);
    let (relay_b, _relay_rx_b) = relay_channels(NonZeroUsize::MIN);
    let (relay_c, _relay_rx_c) = relay_channels(NonZeroUsize::MIN);
    let join_a = state.join_relay(owner_key, PartySlot::new(1)?, 100, 11, relay_a)?;
    let join_b = state.join_relay(owner_key, PartySlot::new(2)?, 200, 22, relay_b)?;
    let join_c = state.join_relay(owner_key, PartySlot::new(3)?, 300, 33, relay_c)?;
    state.leave_relay(join_c.binding)?;

    let outcome = state.relay_blob(join_a.binding, GameplayBlob::new(vec![0x13, 1])?)?;
    assert!(outcome.response.flags.roster_ready());
    assert_eq!(outcome.response.flags.active_player_count(), 2);
    assert!(!outcome.response.flags.has_sole_survivor());

    state.leave_relay(join_b.binding)?;
    let outcome = state.relay_blob(join_a.binding, GameplayBlob::new(vec![0x13, 1])?)?;
    assert!(outcome.response.flags.has_sole_survivor());
    Ok(())
}

#[test]
fn envelope_flags_report_a_sole_survivor_only_after_roster_is_ready()
-> Result<(), StationProtocolError> {
    let slot_1 = PartySlot::new(1)?;
    let slot_2 = PartySlot::new(2)?;

    assert!(
        !GameplayEnvelopeFlags::from_active_slots([slot_1], RosterReadiness::Waiting)
            .has_sole_survivor()
    );
    assert!(
        GameplayEnvelopeFlags::from_active_slots([slot_1], RosterReadiness::Ready)
            .has_sole_survivor()
    );
    assert!(
        !GameplayEnvelopeFlags::from_active_slots([slot_1, slot_2], RosterReadiness::Ready)
            .has_sole_survivor()
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
        player_or_lobby_key: crate::protocol::station::PlayerIdentity::from_bytes([0; 32]),
    };

    let MatchOutcome::PartyCreated {
        owner_key,
        player_count: 1,
    } = state.queue_match(1, registration, lookup, assignment_tx)?
    else {
        return Err("network check did not create a one-player party".into());
    };
    let assignment = assignment_rx.try_recv()?;
    assert!(assignment.ready);
    assert_eq!(assignment.local_slot, PartySlot::new(1)?);
    assert_eq!(assignment.participants.as_slice().len(), 1);

    state.leave_matching(1)?;
    let (relay, _receivers) = relay_channels(NonZeroUsize::MIN);
    let join = state.join_relay(owner_key, PartySlot::new(1)?, 0, 2, relay)?;
    assert!(join.flags.roster_ready());
    Ok(())
}
