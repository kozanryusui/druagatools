use super::{
    AnnouncementCursor, AnnouncementRecord, AnnouncementTime, DatabaseStatus, DiskCapacity,
    MatchingConfiguration, PartyQuestId, PartyQuestSchedule, PartyQuestScheduleEntry, QuestId,
    RelayStatus, ServiceTime, SpecialQuestId, TowerRequest, TowerResponse,
    deserialize_tower_request, serialize_tower_response,
};

#[test]
fn captured_initial_identity_has_typed_fields() -> Result<(), Box<dyn std::error::Error>> {
    let captured = [0x00, 0x01, 0x00, 0x06, 0x01, 0x06, 0x00, 0x00, 0x00, 0x00];

    let request = deserialize_tower_request(&captured)?;

    assert_eq!(
        request,
        TowerRequest::InitialIdentity {
            identity: [0x01, 0x06, 0x00, 0x00],
            reserved: 0,
        }
    );
    Ok(())
}

#[test]
fn session_confirmation_has_typed_session_id() -> Result<(), Box<dyn std::error::Error>> {
    let request = deserialize_tower_request(&[0x00, 0x03, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01])?;

    assert_eq!(request, TowerRequest::SessionConfirm { session_id: 1 });
    Ok(())
}

#[test]
fn handshake_responses_have_tower_frame_format() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        serialize_tower_response(&TowerResponse::InitialAccepted { session_id: 1 })?,
        [0x00, 0x02, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01]
    );
    assert_eq!(
        serialize_tower_response(&TowerResponse::SessionConfirmed { reserved: 0 })?,
        [0x00, 0x04, 0x00, 0x01, 0x00]
    );
    Ok(())
}

#[test]
fn captured_background_requests_have_typed_fields() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        deserialize_tower_request(&[0x00, 0x19, 0x00, 0x01, 0x00])?,
        TowerRequest::ServiceRecordRequest {}
    );
    assert_eq!(
        deserialize_tower_request(&[
            0x00, 0x13, 0x00, 0x08, 0x07, 0xd5, 0x01, 0x01, 0x00, 0x00, 0x00, 0x68,
        ])?,
        TowerRequest::AnnouncementRequest {
            cursor_year: 0x07d5,
            cursor_month: 1,
            cursor_day: 1,
            cursor_hour: 0,
            cursor_minute: 0,
            cursor_sub_minute: 0,
        }
    );
    Ok(())
}

#[test]
fn service_record_and_minimum_background_responses_have_exact_layouts()
-> Result<(), Box<dyn std::error::Error>> {
    let service_record = serialize_tower_response(&TowerResponse::ServiceRecord {
        rank_limit: 31,
        reserved: [0; 2],
        disabled_item_ids: vec![0x1234],
        money_limit: 99_999_999,
    })?;
    assert_eq!(
        &service_record[0..14],
        &[
            0x00, 0x1a, 0x00, 0x48, 31, 0, 0, 1, 0x05, 0xf5, 0xe0, 0xff, 0x12, 0x34,
        ]
    );
    assert_eq!(service_record.len(), 4 + 0x48);
    assert!(service_record[14..].iter().all(|byte| *byte == 0));
    assert_eq!(
        serialize_tower_response(&TowerResponse::AnnouncementComplete)?,
        [0x00, 0x14, 0x00, 0x02, 0xff, 0xff]
    );
    Ok(())
}

#[test]
fn announcement_response_has_cursor_dates_and_cp932_text() -> Result<(), Box<dyn std::error::Error>>
{
    let start = AnnouncementCursor::new(AnnouncementTime::new(2009, 8, 3, 12, 30)?, 7)?;
    let end = AnnouncementTime::new(2009, 8, 4, 23, 59)?;
    let response = serialize_tower_response(&TowerResponse::Announcement(
        AnnouncementRecord::new(start, end, "更新".to_owned())?,
    ))?;

    assert_eq!(
        response,
        [
            0x00, 0x14, 0x00, 0x14, 0x07, 0xd9, 8, 3, 12, 30, 0x07, 0xd9, 8, 4, 23, 59, 0, 7, 0, 4,
            0x8d, 0x58, 0x90, 0x56,
        ]
    );
    Ok(())
}

#[test]
fn captured_service_requests_ignore_the_opaque_byte() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (0x1b, TowerRequest::DatabaseStatusRequest {}),
        (0x1d, TowerRequest::MatchingConfigurationRequest {}),
        (0x1f, TowerRequest::RelayStatusRequest {}),
        (0x21, TowerRequest::PartyQuestScheduleRequest {}),
    ];

    for (message_type, expected) in cases {
        let request = deserialize_tower_request(&[0, message_type, 0, 1, 0xa5])?;
        assert_eq!(request, expected);
    }
    Ok(())
}

#[test]
fn card_upload_has_typed_fields() -> Result<(), Box<dyn std::error::Error>> {
    let mut frame = vec![0; 4 + 0x450];
    frame[0..4].copy_from_slice(&[0x00, 0x15, 0x04, 0x50]);
    frame[4..8].copy_from_slice(&0x0102_0304_u32.to_be_bytes());
    frame[8..10].copy_from_slice(&3_u16.to_be_bytes());
    frame[10..12].copy_from_slice(&0x1234_u16.to_be_bytes());
    frame[12..15].copy_from_slice(&[0xaa, 0xbb, 0xcc]);
    frame[4 + 0x328..4 + 0x32c].copy_from_slice(b"SHOP");
    frame[4 + 0x350..4 + 0x353].copy_from_slice(b"R0\0");

    let request = deserialize_tower_request(&frame)?;
    let TowerRequest::CardDataUpload { upload } = request else {
        return Err("request was not a card-data upload".into());
    };
    assert_eq!(upload.record_id, 0x0102_0304);
    assert_eq!(upload.location, 0x1234);
    assert_eq!(upload.card_data, [0xaa, 0xbb, 0xcc]);
    assert_eq!(&upload.shop_name[..4], b"SHOP");
    assert_eq!(&upload.region_names[0][..3], b"R0\0");
    Ok(())
}

#[test]
fn protocol_types_reject_values_that_the_tower_rejects() -> Result<(), Box<dyn std::error::Error>> {
    assert!(ServiceTime::new(2025, 2, 29, 0, 0, 0).is_err());
    assert!(ServiceTime::new(2024, 2, 29, 23, 59, 59).is_ok());
    assert!(DiskCapacity::new(100, 9).is_err());
    assert!(DiskCapacity::new(100, 10).is_ok());
    assert!(QuestId::new(0).is_err());
    assert!(QuestId::new(93).is_err());
    assert!(PartyQuestId::new(9).is_err());
    assert!(PartyQuestId::new(10).is_ok());
    assert!(SpecialQuestId::new(22).is_err());
    assert!(SpecialQuestId::new(25).is_ok());

    assert!(AnnouncementTime::new(1999, 1, 1, 0, 0).is_err());
    assert!(AnnouncementTime::new(i16::MAX as u16 + 1, 1, 1, 0, 0).is_err());
    let announcement_time = AnnouncementTime::new(2009, 8, 3, 12, 30)?;
    assert!(AnnouncementCursor::new(announcement_time, 127).is_ok());
    assert!(AnnouncementCursor::new(announcement_time, 128).is_err());
    let cursor = AnnouncementCursor::new(announcement_time, 0)?;
    assert!(AnnouncementRecord::new(cursor, announcement_time, "A".repeat(0x1ad)).is_err());

    let time = ServiceTime::new(2026, 8, 12, 10, 20, 30)?;
    let disk = DiskCapacity::new(100, 100)?;
    assert!(DatabaseStatus::new(time, disk, 0, 0, 0, 0, 0, 0, vec![0; 33]).is_err());

    let entry = PartyQuestScheduleEntry::new(time, PartyQuestId::new(10)?);
    assert!(PartyQuestSchedule::new(vec![entry; 20], Vec::new()).is_err());
    assert!(RelayStatus::new(time, disk, 32767, 0).is_ok());
    assert!(RelayStatus::new(time, disk, 32768, 0).is_err());
    Ok(())
}

#[test]
fn database_matching_and_relay_responses_have_exact_layouts()
-> Result<(), Box<dyn std::error::Error>> {
    let time = ServiceTime::new(2026, 8, 12, 10, 20, 30)?;
    let disk = DiskCapacity::new(100, 50)?;

    let database = serialize_tower_response(&TowerResponse::DatabaseStatus(DatabaseStatus::new(
        time,
        disk,
        5,
        4,
        0x1234,
        0x1020,
        7,
        0x1122_3344,
        vec![0x5566],
    )?))?;
    assert_eq!(&database[0..4], &[0x00, 0x1c, 0x00, 0x58]);
    assert_eq!(
        &database[4..14],
        &[0x07, 0xea, 8, 12, 10, 20, 30, 0, 100, 50]
    );
    assert_eq!(&database[14..20], &[5, 4, 0x12, 0x34, 0x10, 0x20]);
    assert_eq!(database[20], 7);
    assert_eq!(database[23], 1);
    assert_eq!(&database[24..30], &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
    assert_eq!(database.len(), 4 + 0x58);

    let matching = serialize_tower_response(&TowerResponse::MatchingConfiguration(
        MatchingConfiguration::new(
            time,
            disk,
            [Some(PartyQuestId::new(10)?), Some(PartyQuestId::new(22)?)],
            Some(SpecialQuestId::new(25)?),
            [-1, -2, -3, -4, -5, -6, -7, -8],
            -9,
            [-10, -11],
            [-12, -13],
            -14,
        ),
    ))?;
    assert_eq!(&matching[0..4], &[0x00, 0x1e, 0x00, 0x2c]);
    assert_eq!(&matching[14..20], &[0, 10, 0, 22, 0, 25]);
    assert_eq!(
        &matching[20..36],
        &[
            0xff, 0xff, 0xff, 0xfe, 0xff, 0xfd, 0xff, 0xfc, 0xff, 0xfb, 0xff, 0xfa, 0xff, 0xf9,
            0xff, 0xf8,
        ]
    );
    assert_eq!(
        &matching[36..48],
        &[
            0xff, 0xf7, 0xff, 0xf6, 0xff, 0xf5, 0xff, 0xf4, 0xff, 0xf3, 0xff, 0xf2
        ]
    );
    assert_eq!(matching.len(), 4 + 0x2c);

    let relay = serialize_tower_response(&TowerResponse::RelayStatus(RelayStatus::new(
        time, disk, 0x1234, 0x5678,
    )?))?;
    assert_eq!(&relay[0..4], &[0x00, 0x20, 0x00, 0x0e]);
    assert_eq!(&relay[14..18], &[0x12, 0x34, 0x56, 0x78]);
    assert_eq!(relay.len(), 18);
    Ok(())
}

#[test]
fn party_quest_schedule_has_two_fixed_banks() -> Result<(), Box<dyn std::error::Error>> {
    let normal_time = ServiceTime::new(2026, 8, 12, 10, 20, 30)?;
    let hard_time = ServiceTime::new(2026, 8, 13, 11, 21, 31)?;
    let normal = PartyQuestScheduleEntry::new(normal_time, PartyQuestId::new(10)?);
    let hard = PartyQuestScheduleEntry::new(hard_time, PartyQuestId::new(22)?);
    let response = serialize_tower_response(&TowerResponse::PartyQuestSchedule(
        PartyQuestSchedule::new(vec![normal], vec![hard])?,
    ))?;

    assert_eq!(&response[0..4], &[0x00, 0x22, 0x01, 0x7c]);
    assert_eq!(&response[4..14], &[0x07, 0xea, 8, 12, 10, 20, 30, 0, 0, 10]);
    let hard_start = 4 + 19 * 10;
    assert_eq!(
        &response[hard_start..hard_start + 10],
        &[0x07, 0xea, 8, 13, 11, 21, 31, 0, 0, 22]
    );
    assert_eq!(response.len(), 4 + 0x17c);
    assert!(response[14..hard_start].iter().all(|byte| *byte == 0));
    Ok(())
}
