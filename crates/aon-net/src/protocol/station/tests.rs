use std::io::Cursor;
use std::num::NonZeroU32;

use binrw::{BinRead, BinWrite};
use encoding_rs::SHIFT_JIS;

use crate::protocol::frame::Frame;
use crate::protocol::tower::{PartyQuestId, SpecialQuestId};

use super::event::MatchingActivationConfigurationWire;
use super::matching::{AssignmentReady, ConnectionRole, EndpointAssignmentWire};
use super::types::{FixedText, MAX_GAMEPLAY_BLOB_SIZE, PlayerIdentity};
use super::*;

#[test]
fn matching_activation_has_one_reserved_byte() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        deserialize_matching_request(&[0x00, 0x23, 0x00, 0x01, 0x00])?,
        MatchingRequest::Activation { reserved: 0 }
    );
    Ok(())
}

#[test]
fn lobby_lookup_has_elapsed_and_remaining_wait_seconds() -> Result<(), Box<dyn std::error::Error>> {
    let mut frame = vec![0x00, 0x0b, 0x00, 0x24, 0x00, 0x0c, 0x00, 0x17];
    frame.extend([0; 32]);

    let MatchingRequest::LobbyLookup(lookup) = deserialize_matching_request(&frame)? else {
        return Err("frame was not decoded as a lobby lookup".into());
    };
    assert_eq!(lookup.elapsed_wait_seconds, 12);
    assert_eq!(lookup.remaining_wait_seconds, 23);
    Ok(())
}

#[test]
fn ignored_acknowledgment_bytes_are_not_in_the_public_model()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        MatchingResponse::LobbyPrompt {}.serialize()?,
        [0x00, 0x0a, 0x00, 0x01, 0x00]
    );
    assert_eq!(
        GameplayResponse::ActionAccepted {}.serialize()?,
        [0x00, 0x18, 0x00, 0x01, 0x00]
    );
    Ok(())
}

#[test]
fn matching_activation_rejects_an_invalid_length() -> Result<(), Box<dyn std::error::Error>> {
    assert!(deserialize_matching_request(&[0x00, 0x23, 0x00, 0x00]).is_err());
    Ok(())
}

#[test]
fn matching_rejects_a_frame_length_mismatch() {
    assert!(deserialize_matching_request(&[0x00, 0x23, 0x00, 0x00, 0x00]).is_err());
    assert!(deserialize_matching_request(&[0x00, 0x23, 0x00, 0x02, 0x00]).is_err());
}

#[test]
fn gameplay_registration_has_flat_fields() -> Result<(), Box<dyn std::error::Error>> {
    let mut frame = vec![0; 4 + 0x134];
    frame[..4].copy_from_slice(&[0x00, 0x0f, 0x01, 0x34]);
    frame[4..8].copy_from_slice(&0x1234_5678_u32.to_be_bytes());
    frame[8] = 2;
    frame[10..12].copy_from_slice(&0x0102_u16.to_be_bytes());
    frame[12..16].copy_from_slice(&0x0304_0506_u32.to_be_bytes());

    let request = deserialize_gameplay_request(&frame)?;
    let GameplayRequest::EndpointRegistration {
        owner_key,
        party_slot,
        location,
        record_id,
        ..
    } = request
    else {
        return Err("request was not an endpoint registration".into());
    };
    assert_eq!(owner_key, OwnerKey::new(0x1234_5678)?);
    assert_eq!(party_slot, PartySlot::new(2)?);
    assert_eq!(location, 0x0102);
    assert_eq!(record_id, 0x0304_0506);
    Ok(())
}

#[test]
fn matching_activation_configuration_has_confirmed_wire_layout()
-> Result<(), Box<dyn std::error::Error>> {
    let configuration = MatchingActivationConfiguration::new(
        [
            QuestEventConfiguration::new(
                PartyQuestId::new(10)?,
                [
                    QuestModifier::MoneyRewardMultiplier(300),
                    QuestModifier::ItemDropRateMultiplier(400),
                    QuestModifier::PlayerWeaponAttackMultiplier(200),
                    QuestModifier::CharacterAttackSpeedMultiplier {
                        characters: CharacterSelection::GILGAMESH,
                        percent: NonZeroU32::new(200).ok_or("percentage must not be zero")?,
                    },
                ],
            ),
            QuestEventConfiguration::new(
                PartyQuestId::new(11)?,
                [
                    QuestModifier::ExperienceRewardMultiplier(400),
                    QuestModifier::EnemyHealthMultiplier(150),
                    QuestModifier::EnemyAttackMultiplier(125),
                    QuestModifier::RevealHiddenMonsters,
                ],
            ),
        ],
        QuestEventConfiguration::new(
            SpecialQuestId::new(25)?,
            [
                QuestModifier::PresentItem {
                    chance: PresentChance::TwoPercent,
                    item_id: ItemId::new(0x9f39)?,
                },
                QuestModifier::CharacterPercentBonus {
                    characters: CharacterSelection::YOUNG_KI,
                    attributes: CharacterPercentAttributes::CASTING_SPEED,
                    percentage_points: 30,
                },
                QuestModifier::CharacterPointBonus {
                    characters: CharacterSelection::WALKURE,
                    attributes: CharacterPointAttributes::CRITICAL_RATE
                        | CharacterPointAttributes::EVASION_RATE
                        | CharacterPointAttributes::RESISTANCE_RATE,
                    points: 25,
                },
                QuestModifier::CharacterPercentBonus {
                    characters: CharacterSelection::XEOVALGA,
                    attributes: CharacterPercentAttributes::MOVEMENT_SPEED,
                    percentage_points: 30,
                },
            ],
        ),
    );

    let bytes = MatchingResponse::ActivationConfiguration(configuration.clone()).serialize()?;
    let frame = Frame::from_bytes(&bytes)?;
    assert_eq!(frame.message_type, 0x06);
    assert_eq!(frame.payload.len(), 0x50);
    let decoded = MatchingActivationConfigurationWire::read_be(&mut Cursor::new(&frame.payload))?;
    assert_eq!(decoded.party_quest_ids, [10, 11]);
    assert_eq!(decoded.special_quest_id, 25);
    assert_eq!(
        decoded.party_entry_attributes,
        [[0x11, 0x12, 0x32, 0x8c00], [0x10, 0x20, 0x21, 0x1f]]
    );
    assert_eq!(
        decoded.special_entry_attributes,
        [0x17, 0xa004, 0x9260, 0xc008]
    );
    assert_eq!(decoded.reserved, 0);
    assert_eq!(
        decoded.party_entry_values,
        [[300, 400, 200, 200], [400, 150, 125, 0]]
    );
    assert_eq!(decoded.special_entry_values, [0x9f39, 30, 25, 30]);
    Ok(())
}

#[test]
fn event_modifier_types_reject_zero_only_where_the_client_requires_it() {
    assert!(ItemId::new(0).is_err());
    assert!(NonZeroU32::new(0).is_none());
    assert_eq!(
        QuestModifier::ExperienceRewardMultiplier(0).wire_pair(),
        (0x10, 0)
    );
}

#[test]
fn every_event_modifier_family_has_an_exact_wire_pair() -> Result<(), Box<dyn std::error::Error>> {
    let item = ItemId::new(0x207b)?;
    let percent = 200;
    let cases = [
        (QuestModifier::None, (0, 0)),
        (
            QuestModifier::ExperienceRewardMultiplier(percent),
            (0x10, 200),
        ),
        (QuestModifier::MoneyRewardMultiplier(percent), (0x11, 200)),
        (QuestModifier::ItemDropRateMultiplier(percent), (0x12, 200)),
        (
            QuestModifier::PresentItem {
                chance: PresentChance::Always,
                item_id: item,
            },
            (0x13, 0x207b),
        ),
        (
            QuestModifier::PresentItem {
                chance: PresentChance::Half,
                item_id: item,
            },
            (0x14, 0x207b),
        ),
        (
            QuestModifier::PresentItem {
                chance: PresentChance::Quarter,
                item_id: item,
            },
            (0x15, 0x207b),
        ),
        (
            QuestModifier::PresentItem {
                chance: PresentChance::TenPercent,
                item_id: item,
            },
            (0x16, 0x207b),
        ),
        (
            QuestModifier::PresentItem {
                chance: PresentChance::TwoPercent,
                item_id: item,
            },
            (0x17, 0x207b),
        ),
        (QuestModifier::RevealHiddenMonsters, (0x1f, 0)),
        (QuestModifier::EnemyHealthMultiplier(percent), (0x20, 200)),
        (QuestModifier::EnemyAttackMultiplier(percent), (0x21, 200)),
        (QuestModifier::PlayerMaxHpMultiplier(percent), (0x30, 200)),
        (QuestModifier::PlayerMaxApMultiplier(percent), (0x31, 200)),
        (
            QuestModifier::PlayerWeaponAttackMultiplier(percent),
            (0x32, 200),
        ),
        (
            QuestModifier::CharacterDefenseMultiplier {
                characters: CharacterSelection::ALL,
                attributes: DefenseAttributes::BOTH,
                percent,
            },
            (0xf980, 200),
        ),
        (
            QuestModifier::CharacterPointBonus {
                characters: CharacterSelection::WALKURE,
                attributes: CharacterPointAttributes::RETALIATION_DAMAGE,
                points: -20,
            },
            (0x9010, (-20_i32) as u32),
        ),
        (
            QuestModifier::CharacterPercentBonus {
                characters: CharacterSelection::GILGAMESH | CharacterSelection::XEOVALGA,
                attributes: CharacterPercentAttributes::PHYSICAL_DAMAGE_RECEIVED
                    | CharacterPercentAttributes::MAGIC_DAMAGE_RECEIVED,
                percentage_points: -25,
            },
            (0xc803, (-25_i32) as u32),
        ),
    ];

    for (modifier, expected) in cases {
        assert_eq!(modifier.wire_pair(), expected);
    }
    Ok(())
}

#[test]
fn endpoint_assignment_has_confirmed_wire_layout() -> Result<(), Box<dyn std::error::Error>> {
    let assignment = EndpointAssignment {
        host: EndpointHost::new("gameservers.aonnet".into())?,
        port: 33442,
        owner_key: OwnerKey::new(0x1234_5678)?,
        ready: true,
        local_slot: PartySlot::new(2)?,
        matching_quest_index: 25,
        participants: PartyRoster::new(vec![
            PlayerIdentity([0x11; 32]),
            PlayerIdentity([0x22; 32]),
        ])?,
    };
    let frame = MatchingResponse::EndpointAssignment(assignment.clone()).serialize()?;
    let wire = Frame::from_bytes(&frame)?;
    assert_eq!(wire.message_type, 0x0c);
    assert_eq!(wire.payload.len(), 0xb4);
    assert_eq!(&wire.payload[0x2c..0x30], &[1, 0b0011, 2, 2]);
    assert_eq!(&wire.payload[0x30..0x34], &[0, 25, 0, 0]);
    let decoded = EndpointAssignmentWire::read_be(&mut Cursor::new(&wire.payload))
        .map_err(|error| StationProtocolError::Binrw(error.to_string()))?;
    assert_eq!(decoded.connection_role, ConnectionRole::GameRelay);
    assert_eq!(decoded.port, 33442);
    assert_eq!(
        decoded.host,
        EndpointHost::new("gameservers.aonnet".into())?
    );
    assert_eq!(decoded.owner_key, OwnerKey::new(0x1234_5678)?);
    assert_eq!(decoded.ready, AssignmentReady::Ready);
    assert_eq!(decoded.active_slot_mask, 0b0011);
    assert_eq!(decoded.local_slot, PartySlot::new(2)?);
    assert_eq!(decoded.matching_quest_index, 25);
    let mut first_participant = [0x11; 32];
    first_participant[0] = 1;
    let mut second_participant = [0x22; 32];
    second_participant[0] = 2;
    assert_eq!(
        decoded.participants,
        vec![
            PlayerIdentity(first_participant),
            PlayerIdentity(second_participant)
        ]
    );
    Ok(())
}

#[test]
fn partial_endpoint_assignment_keeps_the_station_in_matching()
-> Result<(), Box<dyn std::error::Error>> {
    let assignment = EndpointAssignment {
        host: EndpointHost::new("gameservers.aonnet".into())?,
        port: 33442,
        owner_key: OwnerKey::new(1)?,
        ready: false,
        local_slot: PartySlot::new(1)?,
        matching_quest_index: 10,
        participants: PartyRoster::new(vec![PlayerIdentity([0x11; 32])])?,
    };

    let frame = MatchingResponse::EndpointAssignment(assignment).serialize()?;
    let wire = Frame::from_bytes(&frame)?;
    assert_eq!(&wire.payload[0x2c..0x30], &[0, 0b0001, 1, 1]);
    let decoded = EndpointAssignmentWire::read_be(&mut Cursor::new(&wire.payload))
        .map_err(|error| StationProtocolError::Binrw(error.to_string()))?;
    assert_eq!(decoded.ready, AssignmentReady::Waiting);
    Ok(())
}

#[test]
fn envelope_keeps_slot_and_blob_boundaries() -> Result<(), StationProtocolError> {
    let frame = GameplayResponse::Envelope {
        flags: GameplayEnvelopeFlags::from_active_slots(
            [PartySlot::new(1)?, PartySlot::new(3)?],
            false,
        ),
        records: vec![PlayerRecord {
            party_slot: PartySlot::new(3)?,
            blob: GameplayBlob::new(vec![0x10, 0x20, 0x30])?,
        }],
    }
    .serialize()?;
    assert_eq!(frame, vec![0, 0x12, 0, 7, 0x0a, 1, 3, 3, 0x10, 0x20, 0x30]);
    Ok(())
}

#[test]
fn invariant_types_reject_invalid_wire_values() {
    assert!(OwnerKey::read_be(&mut Cursor::new(0_u32.to_be_bytes())).is_err());
    assert!(PartySlot::read_be(&mut Cursor::new([0_u8])).is_err());
    assert!(PartySlot::read_be(&mut Cursor::new([5_u8])).is_err());
    assert!(GameplayBlob::new(Vec::new()).is_err());
    assert!(GameplayBlob::new(vec![0; MAX_GAMEPLAY_BLOB_SIZE + 1]).is_err());
    assert!(EndpointHost::new(String::new()).is_err());
    assert!(EndpointHost::new("a".repeat(32)).is_err());
}

#[test]
fn typed_strings_convert_only_at_binrw_edge() -> Result<(), StationProtocolError> {
    let host = EndpointHost::new("gameservers.aonnet".into())?;
    let mut host_wire = Cursor::new(Vec::new());
    host.write_be(&mut host_wire)
        .map_err(|error| StationProtocolError::Serialize(error.to_string()))?;
    host_wire.set_position(0);
    let decoded_host = EndpointHost::read_be(&mut host_wire)
        .map_err(|error| StationProtocolError::Binrw(error.to_string()))?;
    assert_eq!(decoded_host, host);

    let (encoded, _, had_errors) = SHIFT_JIS.encode("AON.Net 店舗");
    assert!(!had_errors);
    let mut encoded = encoded.into_owned();
    encoded.resize(40, 0);
    let text = FixedText::<40>::read_be(&mut Cursor::new(encoded))
        .map_err(|error| StationProtocolError::Binrw(error.to_string()))?;
    let mut text_wire = Cursor::new(Vec::new());
    text.write_be(&mut text_wire)
        .map_err(|error| StationProtocolError::Serialize(error.to_string()))?;
    text_wire.set_position(0);
    let decoded_text = FixedText::<40>::read_be(&mut text_wire)
        .map_err(|error| StationProtocolError::Binrw(error.to_string()))?;
    assert_eq!(decoded_text, text);
    Ok(())
}
