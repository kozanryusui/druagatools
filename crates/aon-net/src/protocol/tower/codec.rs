use std::io::Cursor;

use binrw::{BinWrite, binwrite};
use encoding_rs::SHIFT_JIS;

use crate::protocol::frame;

use super::types::{
    AnnouncementRecord, DatabaseStatus, DiskCapacity, MatchingConfiguration, PartyQuestSchedule,
    RelayStatus, ServiceTime, TowerProtocolError, TowerRequest, TowerResponse,
};

const MAX_DISABLED_ITEMS: usize = 32;
const MAX_SCHEDULE_ENTRIES: usize = 19;

#[derive(BinWrite)]
#[bw(big)]
struct ServiceTimeWire {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    reserved: u8,
}

impl From<ServiceTime> for ServiceTimeWire {
    fn from(value: ServiceTime) -> Self {
        Self {
            year: value.year,
            month: value.month,
            day: value.day,
            hour: value.hour,
            minute: value.minute,
            second: value.second,
            reserved: 0,
        }
    }
}

#[derive(BinWrite)]
struct DiskCapacityWire {
    total: u8,
    available: u8,
}

impl From<DiskCapacity> for DiskCapacityWire {
    fn from(value: DiskCapacity) -> Self {
        Self {
            total: value.total,
            available: value.available,
        }
    }
}

#[derive(BinWrite)]
struct CommonStatusWire {
    time: ServiceTimeWire,
    disk: DiskCapacityWire,
}

impl CommonStatusWire {
    fn new(time: ServiceTime, disk: DiskCapacity) -> Self {
        Self {
            time: time.into(),
            disk: disk.into(),
        }
    }
}

#[derive(BinWrite)]
#[bw(big)]
struct ServiceRecordWire {
    rank_limit: u8,
    reserved: [u8; 2],
    item_count: u8,
    money_limit: u32,
    disabled_item_ids: Vec<u16>,
    padding: Vec<u8>,
}

#[derive(BinWrite)]
#[bw(big)]
struct DatabaseStatusWire {
    common: CommonStatusWire,
    news_total: u8,
    news_available: u8,
    backup_total: u16,
    backup_available: u16,
    rank_limit: u8,
    reserved: [u8; 2],
    item_count: u8,
    money_limit: u32,
    disabled_item_ids: [u16; MAX_DISABLED_ITEMS],
}

#[derive(BinWrite)]
#[bw(big)]
struct MatchingConfigurationWire {
    common: CommonStatusWire,
    party_quests: [u16; 2],
    special_quest: u16,
    pe_status_primary: [i16; 8],
    pe_status_secondary: i16,
    pq_status: [i16; 2],
    rq_status: [i16; 2],
    sq_status: i16,
}

#[derive(BinWrite)]
#[bw(big)]
struct RelayStatusWire {
    common: CommonStatusWire,
    party_count: u16,
    player_count: u16,
}

#[derive(BinWrite, Clone, Copy, Default)]
#[bw(big)]
struct PartyQuestScheduleEntryWire {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    reserved: u8,
    quest_id: u16,
}

#[derive(BinWrite)]
#[bw(big)]
struct PartyQuestScheduleWire {
    entries: [PartyQuestScheduleEntryWire; MAX_SCHEDULE_ENTRIES * 2],
}

#[binwrite]
#[bw(big)]
enum TowerResponseWire {
    #[bw(magic = 0x0002_u16)]
    InitialAccepted {
        #[bw(calc = 4_u16)]
        payload_length: u16,
        session_id: u32,
    },
    #[bw(magic = 0x0004_u16)]
    SessionConfirmed {
        #[bw(calc = 1_u16)]
        payload_length: u16,
        reserved: u8,
    },
    #[bw(magic = 0x001a_u16)]
    ServiceRecord {
        #[bw(calc = 0x48_u16)]
        payload_length: u16,
        record: ServiceRecordWire,
    },
    #[bw(magic = 0x0014_u16)]
    Announcement {
        #[bw(calc = 0x10_u16 + text.len() as u16)]
        payload_length: u16,
        start_year: u16,
        start_month: u8,
        start_day: u8,
        start_hour: u8,
        start_minute: u8,
        end_year: u16,
        end_month: u8,
        end_day: u8,
        end_hour: u8,
        end_minute: u8,
        control: u8,
        sub_minute: u8,
        #[bw(calc = text.len() as u16)]
        text_length: u16,
        text: Vec<u8>,
    },
    #[bw(magic = 0x0014_u16)]
    AnnouncementComplete {
        #[bw(calc = 2_u16)]
        payload_length: u16,
        #[bw(calc = -1_i16)]
        terminal_year: i16,
    },
    #[bw(magic = 0x0016_u16)]
    CardDataStored {
        #[bw(calc = 1_u16)]
        payload_length: u16,
        status: u8,
    },
    #[bw(magic = 0x001c_u16)]
    DatabaseStatus {
        #[bw(calc = 0x58_u16)]
        payload_length: u16,
        status: DatabaseStatusWire,
    },
    #[bw(magic = 0x001e_u16)]
    MatchingConfiguration {
        #[bw(calc = 0x2c_u16)]
        payload_length: u16,
        configuration: MatchingConfigurationWire,
    },
    #[bw(magic = 0x0020_u16)]
    RelayStatus {
        #[bw(calc = 0x0e_u16)]
        payload_length: u16,
        status: RelayStatusWire,
    },
    #[bw(magic = 0x0022_u16)]
    PartyQuestSchedule {
        #[bw(calc = 0x17c_u16)]
        payload_length: u16,
        schedule: Box<PartyQuestScheduleWire>,
    },
}

pub fn deserialize_tower_request(bytes: &[u8]) -> Result<TowerRequest, TowerProtocolError> {
    frame::read(bytes).map_err(|error| TowerProtocolError::Binrw(error.to_string()))
}

pub fn serialize_tower_response(response: &TowerResponse) -> Result<Vec<u8>, TowerProtocolError> {
    let wire = match response {
        TowerResponse::InitialAccepted { session_id } => TowerResponseWire::InitialAccepted {
            session_id: *session_id,
        },
        TowerResponse::SessionConfirmed { reserved } => TowerResponseWire::SessionConfirmed {
            reserved: *reserved,
        },
        TowerResponse::ServiceRecord {
            rank_limit,
            reserved,
            disabled_item_ids,
            money_limit,
        } => TowerResponseWire::ServiceRecord {
            record: service_record_wire(*rank_limit, *reserved, disabled_item_ids, *money_limit),
        },
        TowerResponse::Announcement(record) => announcement_wire(record),
        TowerResponse::AnnouncementComplete => TowerResponseWire::AnnouncementComplete {},
        TowerResponse::CardDataStored => TowerResponseWire::CardDataStored { status: 0 },
        TowerResponse::DatabaseStatus(status) => TowerResponseWire::DatabaseStatus {
            status: database_status_wire(status),
        },
        TowerResponse::MatchingConfiguration(config) => TowerResponseWire::MatchingConfiguration {
            configuration: matching_configuration_wire(config),
        },
        TowerResponse::RelayStatus(status) => TowerResponseWire::RelayStatus {
            status: relay_status_wire(status),
        },
        TowerResponse::PartyQuestSchedule(schedule) => TowerResponseWire::PartyQuestSchedule {
            schedule: Box::new(party_quest_schedule_wire(schedule)),
        },
    };
    let mut output = Cursor::new(Vec::new());
    wire.write(&mut output)
        .map_err(|error| TowerProtocolError::Serialize(error.to_string()))?;
    Ok(output.into_inner())
}

fn announcement_wire(record: &AnnouncementRecord) -> TowerResponseWire {
    let (text, _, had_errors) = SHIFT_JIS.encode(&record.text);
    debug_assert!(!had_errors);
    TowerResponseWire::Announcement {
        start_year: record.start.time.year,
        start_month: record.start.time.month,
        start_day: record.start.time.day,
        start_hour: record.start.time.hour,
        start_minute: record.start.time.minute,
        end_year: record.end.year,
        end_month: record.end.month,
        end_day: record.end.day,
        end_hour: record.end.hour,
        end_minute: record.end.minute,
        control: 0,
        sub_minute: record.start.sub_minute,
        text: text.into_owned(),
    }
}

fn service_record_wire(
    rank_limit: u8,
    reserved: [u8; 2],
    item_ids: &[u16],
    money_limit: u32,
) -> ServiceRecordWire {
    let item_count = u8::try_from(item_ids.len()).unwrap_or(u8::MAX);
    let item_ids = &item_ids[..usize::from(item_count)];
    let encoded_size = 8 + size_of_val(item_ids);
    ServiceRecordWire {
        rank_limit,
        reserved,
        item_count,
        money_limit,
        disabled_item_ids: item_ids.to_vec(),
        padding: vec![0; 0x48_usize.saturating_sub(encoded_size)],
    }
}

fn database_status_wire(status: &DatabaseStatus) -> DatabaseStatusWire {
    DatabaseStatusWire {
        common: CommonStatusWire::new(status.time, status.disk),
        news_total: status.news_total,
        news_available: status.news_available,
        backup_total: status.backup_total,
        backup_available: status.backup_available,
        rank_limit: status.rank_limit,
        reserved: [0; 2],
        item_count: status.disabled_item_ids.len() as u8,
        money_limit: status.money_limit,
        disabled_item_ids: padded_item_ids(&status.disabled_item_ids),
    }
}

fn matching_configuration_wire(config: &MatchingConfiguration) -> MatchingConfigurationWire {
    MatchingConfigurationWire {
        common: CommonStatusWire::new(config.time, config.disk),
        party_quests: config
            .party_quests
            .map(|quest| quest.map_or(0, |id| id.get())),
        special_quest: config.special_quest.map_or(0, |id| id.get()),
        pe_status_primary: config.pe_status_primary,
        pe_status_secondary: config.pe_status_secondary,
        pq_status: config.pq_status,
        rq_status: config.rq_status,
        sq_status: config.sq_status,
    }
}

fn relay_status_wire(status: &RelayStatus) -> RelayStatusWire {
    RelayStatusWire {
        common: CommonStatusWire::new(status.time, status.disk),
        party_count: status.party_count.get(),
        player_count: status.player_count.get(),
    }
}

fn party_quest_schedule_wire(schedule: &PartyQuestSchedule) -> PartyQuestScheduleWire {
    let mut entries = [PartyQuestScheduleEntryWire::default(); MAX_SCHEDULE_ENTRIES * 2];
    for (bank, source) in [&schedule.normal, &schedule.hard].into_iter().enumerate() {
        for (index, entry) in source.iter().enumerate() {
            let time = entry.time;
            entries[bank * MAX_SCHEDULE_ENTRIES + index] = PartyQuestScheduleEntryWire {
                year: time.year,
                month: time.month,
                day: time.day,
                hour: time.hour,
                minute: time.minute,
                second: time.second,
                reserved: 0,
                quest_id: entry.quest_id.get(),
            };
        }
    }
    PartyQuestScheduleWire { entries }
}

fn padded_item_ids(item_ids: &[u16]) -> [u16; MAX_DISABLED_ITEMS] {
    let mut output = [0; MAX_DISABLED_ITEMS];
    output[..item_ids.len()].copy_from_slice(item_ids);
    output
}
