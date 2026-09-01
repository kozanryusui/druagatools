use std::io::Cursor;

use binrw::BinRead;

use super::*;
use crate::protocol::station::FixedText;

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
        first.participants.as_slice()[0].participant_marker().get(),
        0x10
    );

    state.exchange_participant_record(1, participant_identity(0x11))?;
    state.exchange_participant_record(1, participant_identity(0x12))?;
    let second = state.exchange_participant_record(2, participant_identity(0x1d))?;
    let third = state.exchange_participant_record(2, participant_identity(0x16))?;
    assert_eq!(
        second.participants.as_slice()[0].participant_marker().get(),
        0x11
    );
    assert_eq!(
        third.participants.as_slice()[0].participant_marker().get(),
        0x12
    );
    assert_eq!(third.local_slot, PartySlot::new(2)?);
    assert_eq!(third.matching_quest_index, 10);
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
    let join_a = state.join_relay(owner_key, PartySlot::new(1)?, 100, 11)?;
    let join_b = state.join_relay(owner_key, PartySlot::new(2)?, 200, 22)?;
    let source = state.relay_blob(join_a.binding, GameplayBlob::new(vec![0x13, 1])?)?;
    assert!(source.response.records.is_empty());
    assert!(source.response.flags.roster_ready());
    assert_eq!(source.response.flags.bits(), 0x07);
    let destination = state.relay_blob(join_b.binding, GameplayBlob::new(vec![0x13, 2])?)?;
    assert_eq!(
        destination.response.records[0].party_slot,
        PartySlot::new(1)?
    );
    assert_eq!(destination.response.records[0].blob.as_bytes(), &[0x13, 1]);
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
    let join_a = state.join_relay(owner_key, PartySlot::new(1)?, 100, 11)?;
    let join_b = state.join_relay(owner_key, PartySlot::new(2)?, 200, 22)?;
    let join_c = state.join_relay(owner_key, PartySlot::new(3)?, 300, 33)?;
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
