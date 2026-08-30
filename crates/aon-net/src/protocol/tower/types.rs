use binrw::binread;
use encoding_rs::SHIFT_JIS;
use thiserror::Error;

const MAX_DISABLED_ITEMS: usize = 32;
const MAX_SCHEDULE_ENTRIES: usize = 19;
const CARD_DATA_CAPACITY: usize = 800;
const MAX_ANNOUNCEMENT_TEXT_BYTES: usize = 0x1ac;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuestFamily {
    PartyEpic,
    PastEpic,
    PartyQuest,
    RandomQuest,
    SpecialQuest,
    TravelAlone,
    FreeMission,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuestId(u16);

impl QuestId {
    pub fn new(value: u16) -> Result<Self, TowerProtocolError> {
        if (1..=92).contains(&value) {
            Ok(Self(value))
        } else {
            Err(TowerProtocolError::QuestId { value })
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }

    pub const fn family(self) -> Option<QuestFamily> {
        match self.0 {
            1..=8 => Some(QuestFamily::PartyEpic),
            9 => Some(QuestFamily::PastEpic),
            10..=22 => Some(QuestFamily::PartyQuest),
            23..=24 => Some(QuestFamily::RandomQuest),
            25..=74 | 76..=89 => Some(QuestFamily::SpecialQuest),
            75 => Some(QuestFamily::TravelAlone),
            90..=92 => Some(QuestFamily::FreeMission),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartyQuestId(QuestId);

impl PartyQuestId {
    pub fn new(value: u16) -> Result<Self, TowerProtocolError> {
        let id = QuestId::new(value)?;
        if id.family() != Some(QuestFamily::PartyQuest) {
            return Err(TowerProtocolError::QuestFamily {
                value,
                expected: "party quest",
            });
        }
        Ok(Self(id))
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpecialQuestId(QuestId);

impl SpecialQuestId {
    pub fn new(value: u16) -> Result<Self, TowerProtocolError> {
        let id = QuestId::new(value)?;
        if id.family() != Some(QuestFamily::SpecialQuest) {
            return Err(TowerProtocolError::QuestFamily {
                value,
                expected: "special quest",
            });
        }
        Ok(Self(id))
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiskCapacity {
    pub(super) total: u8,
    pub(super) available: u8,
}

impl DiskCapacity {
    pub fn new(total: u8, available: u8) -> Result<Self, TowerProtocolError> {
        if total == 0 || available == 0 || available > total {
            return Err(TowerProtocolError::DiskCapacity { total, available });
        }
        if u16::from(available) * 100 / u16::from(total) < 10 {
            return Err(TowerProtocolError::DiskCapacity { total, available });
        }
        Ok(Self { total, available })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceTime {
    pub(super) year: u16,
    pub(super) month: u8,
    pub(super) day: u8,
    pub(super) hour: u8,
    pub(super) minute: u8,
    pub(super) second: u8,
}

impl ServiceTime {
    pub fn new(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
    ) -> Result<Self, TowerProtocolError> {
        let maximum_day = days_in_month(year, month).ok_or(TowerProtocolError::ServiceTime)?;
        if year < 2005 || day == 0 || day > maximum_day || hour > 23 || minute > 59 || second > 59 {
            return Err(TowerProtocolError::ServiceTime);
        }
        Ok(Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AnnouncementTime {
    pub(crate) year: u16,
    pub(crate) month: u8,
    pub(crate) day: u8,
    pub(crate) hour: u8,
    pub(crate) minute: u8,
}

impl AnnouncementTime {
    pub fn new(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
    ) -> Result<Self, TowerProtocolError> {
        let maximum_day = days_in_month(year, month).ok_or(TowerProtocolError::AnnouncementTime)?;
        if !(2000..=i16::MAX as u16).contains(&year)
            || day == 0
            || day > maximum_day
            || hour > 23
            || minute > 59
        {
            return Err(TowerProtocolError::AnnouncementTime);
        }
        Ok(Self {
            year,
            month,
            day,
            hour,
            minute,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AnnouncementCursor {
    pub(crate) time: AnnouncementTime,
    pub(crate) sub_minute: u8,
}

impl AnnouncementCursor {
    pub fn new(time: AnnouncementTime, sub_minute: u8) -> Result<Self, TowerProtocolError> {
        if sub_minute > i8::MAX as u8 {
            return Err(TowerProtocolError::AnnouncementCursor { sub_minute });
        }
        Ok(Self { time, sub_minute })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnnouncementRecord {
    pub(crate) start: AnnouncementCursor,
    pub(crate) end: AnnouncementTime,
    pub(crate) text: String,
}

impl AnnouncementRecord {
    pub fn new(
        start: AnnouncementCursor,
        end: AnnouncementTime,
        text: String,
    ) -> Result<Self, TowerProtocolError> {
        let (encoded, _, had_errors) = SHIFT_JIS.encode(&text);
        if had_errors {
            return Err(TowerProtocolError::AnnouncementTextEncoding);
        }
        if encoded.len() > MAX_ANNOUNCEMENT_TEXT_BYTES {
            return Err(TowerProtocolError::AnnouncementTextLength {
                actual: encoded.len(),
                maximum: MAX_ANNOUNCEMENT_TEXT_BYTES,
            });
        }
        if encoded.contains(&0) {
            return Err(TowerProtocolError::AnnouncementTextNul);
        }
        Ok(Self { start, end, text })
    }
}

#[binread]
#[derive(Clone, Debug, Eq, PartialEq)]
#[br(big)]
pub struct CardDataUpload {
    pub(crate) record_id: u32,
    #[br(
        temp,
        assert(
            usize::from(data_length) <= CARD_DATA_CAPACITY,
            TowerProtocolError::CardDataLength {
                declared: usize::from(data_length),
                capacity: CARD_DATA_CAPACITY,
            }
        )
    )]
    data_length: u16,
    pub(crate) location: u16,
    #[br(
        count = CARD_DATA_CAPACITY,
        map = |data: Vec<u8>| data[..usize::from(data_length)].to_vec()
    )]
    pub(crate) card_data: Vec<u8>,
    pub(crate) shop_name: [u8; 40],
    pub(crate) region_names: [[u8; 64]; 4],
}

#[binread]
#[derive(Clone, Debug, Eq, PartialEq)]
#[br(big)]
pub enum TowerRequest {
    #[br(magic = 0x0001_u16)]
    InitialIdentity {
        #[br(temp)]
        payload_length: u16,
        identity: [u8; 4],
        reserved: u16,
    },
    #[br(magic = 0x0003_u16)]
    SessionConfirm {
        #[br(temp)]
        payload_length: u16,
        session_id: u32,
    },
    #[br(magic = 0x0013_u16)]
    AnnouncementRequest {
        #[br(temp)]
        payload_length: u16,
        cursor_year: u16,
        cursor_month: u8,
        cursor_day: u8,
        cursor_hour: u8,
        cursor_minute: u8,
        cursor_sub_minute: u8,
        #[br(temp)]
        unused: u8,
    },
    #[br(magic = 0x0015_u16)]
    CardDataUpload {
        #[br(temp)]
        payload_length: u16,
        upload: Box<CardDataUpload>,
    },
    #[br(magic = 0x0019_u16)]
    ServiceRecordRequest {
        #[br(temp)]
        payload_length: u16,
        #[br(temp)]
        unused: u8,
    },
    #[br(magic = 0x001b_u16)]
    DatabaseStatusRequest {
        #[br(temp)]
        payload_length: u16,
        #[br(temp)]
        unused: u8,
    },
    #[br(magic = 0x001d_u16)]
    MatchingConfigurationRequest {
        #[br(temp)]
        payload_length: u16,
        #[br(temp)]
        unused: u8,
    },
    #[br(magic = 0x001f_u16)]
    RelayStatusRequest {
        #[br(temp)]
        payload_length: u16,
        #[br(temp)]
        unused: u8,
    },
    #[br(magic = 0x0021_u16)]
    PartyQuestScheduleRequest {
        #[br(temp)]
        payload_length: u16,
        #[br(temp)]
        unused: u8,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseStatus {
    pub(super) time: ServiceTime,
    pub(super) disk: DiskCapacity,
    pub(super) news_total: u8,
    pub(super) news_available: u8,
    pub(super) backup_total: u16,
    pub(super) backup_available: u16,
    pub(super) rank_limit: u8,
    pub(super) money_limit: u32,
    pub(super) disabled_item_ids: Vec<u16>,
}

impl DatabaseStatus {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        time: ServiceTime,
        disk: DiskCapacity,
        news_total: u8,
        news_available: u8,
        backup_total: u16,
        backup_available: u16,
        rank_limit: u8,
        money_limit: u32,
        disabled_item_ids: Vec<u16>,
    ) -> Result<Self, TowerProtocolError> {
        if news_available > news_total || backup_available > backup_total {
            return Err(TowerProtocolError::CapacityOrder);
        }
        if disabled_item_ids.len() > MAX_DISABLED_ITEMS {
            return Err(TowerProtocolError::ItemCount {
                actual: disabled_item_ids.len(),
            });
        }
        Ok(Self {
            time,
            disk,
            news_total,
            news_available,
            backup_total,
            backup_available,
            rank_limit,
            money_limit,
            disabled_item_ids,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchingConfiguration {
    pub(super) time: ServiceTime,
    pub(super) disk: DiskCapacity,
    pub(super) party_quests: [Option<PartyQuestId>; 2],
    pub(super) special_quest: Option<SpecialQuestId>,
    pub(super) pe_status_primary: [i16; 8],
    pub(super) pe_status_secondary: i16,
    pub(super) pq_status: [i16; 2],
    pub(super) rq_status: [i16; 2],
    pub(super) sq_status: i16,
}

impl MatchingConfiguration {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        time: ServiceTime,
        disk: DiskCapacity,
        party_quests: [Option<PartyQuestId>; 2],
        special_quest: Option<SpecialQuestId>,
        pe_status_primary: [i16; 8],
        pe_status_secondary: i16,
        pq_status: [i16; 2],
        rq_status: [i16; 2],
        sq_status: i16,
    ) -> Self {
        Self {
            time,
            disk,
            party_quests,
            special_quest,
            pe_status_primary,
            pe_status_secondary,
            pq_status,
            rq_status,
            sq_status,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayStatus {
    pub(super) time: ServiceTime,
    pub(super) disk: DiskCapacity,
    pub(super) party_count: ServiceCount,
    pub(super) player_count: ServiceCount,
}

impl RelayStatus {
    pub fn new(
        time: ServiceTime,
        disk: DiskCapacity,
        party_count: u16,
        player_count: u16,
    ) -> Result<Self, TowerProtocolError> {
        Ok(Self {
            time,
            disk,
            party_count: ServiceCount::new(party_count)?,
            player_count: ServiceCount::new(player_count)?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ServiceCount(u16);

impl ServiceCount {
    fn new(value: u16) -> Result<Self, TowerProtocolError> {
        if value > i16::MAX as u16 {
            return Err(TowerProtocolError::ServiceCount { value });
        }
        Ok(Self(value))
    }

    pub(super) const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartyQuestScheduleEntry {
    pub(super) time: ServiceTime,
    pub(super) quest_id: PartyQuestId,
}

impl PartyQuestScheduleEntry {
    pub const fn new(time: ServiceTime, quest_id: PartyQuestId) -> Self {
        Self { time, quest_id }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartyQuestSchedule {
    pub(super) normal: Vec<PartyQuestScheduleEntry>,
    pub(super) hard: Vec<PartyQuestScheduleEntry>,
}

impl PartyQuestSchedule {
    pub fn new(
        normal: Vec<PartyQuestScheduleEntry>,
        hard: Vec<PartyQuestScheduleEntry>,
    ) -> Result<Self, TowerProtocolError> {
        if normal.len() > MAX_SCHEDULE_ENTRIES || hard.len() > MAX_SCHEDULE_ENTRIES {
            return Err(TowerProtocolError::ScheduleCount {
                normal: normal.len(),
                hard: hard.len(),
            });
        }
        Ok(Self { normal, hard })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TowerResponse {
    InitialAccepted {
        session_id: u32,
    },
    SessionConfirmed {
        reserved: u8,
    },
    ServiceRecord {
        rank_limit: u8,
        reserved: [u8; 2],
        disabled_item_ids: Vec<u16>,
        money_limit: u32,
    },
    Announcement(AnnouncementRecord),
    AnnouncementComplete,
    CardDataStored,
    DatabaseStatus(DatabaseStatus),
    MatchingConfiguration(MatchingConfiguration),
    RelayStatus(RelayStatus),
    PartyQuestSchedule(PartyQuestSchedule),
}

#[derive(Debug, Error)]
pub enum TowerProtocolError {
    #[error("card upload declares {declared} bytes but the capacity is {capacity}")]
    CardDataLength { declared: usize, capacity: usize },
    #[error("quest ID {value} does not exist in Tower 1.60")]
    QuestId { value: u16 },
    #[error("quest ID {value} is not a {expected}")]
    QuestFamily { value: u16, expected: &'static str },
    #[error("service timestamp is outside the supported calendar range")]
    ServiceTime,
    #[error("announcement timestamp is outside the supported calendar range")]
    AnnouncementTime,
    #[error("announcement sub-minute cursor {sub_minute} is outside the signed client range")]
    AnnouncementCursor { sub_minute: u8 },
    #[error("announcement text cannot be encoded as CP932")]
    AnnouncementTextEncoding,
    #[error("announcement text contains a null byte")]
    AnnouncementTextNul,
    #[error("announcement text has {actual} CP932 bytes; the limit is {maximum}")]
    AnnouncementTextLength { actual: usize, maximum: usize },
    #[error("disk capacity {available}/{total} is not accepted by the Tower")]
    DiskCapacity { total: u8, available: u8 },
    #[error("an available count is larger than its total count")]
    CapacityOrder,
    #[error("database response has {actual} disabled items; the Station permits at most 32")]
    ItemCount { actual: usize },
    #[error("party-quest schedule has {normal} normal and {hard} hard entries; each limit is 19")]
    ScheduleCount { normal: usize, hard: usize },
    #[error("service count {value} is larger than the Tower's signed display range")]
    ServiceCount { value: u16 },
    #[error("Tower packet cannot be parsed: {0}")]
    Binrw(String),
    #[error("Tower packet cannot be serialized: {0}")]
    Serialize(String),
}

fn days_in_month(year: u16, month: u8) -> Option<u8> {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            Some(29)
        }
        2 => Some(28),
        _ => None,
    }
}
