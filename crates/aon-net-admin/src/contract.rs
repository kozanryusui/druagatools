use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestOption {
    pub quest_id: u16,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestMode {
    Random,
    Fixed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestSettings {
    pub mode: QuestMode,
    pub random_interval_minutes: u16,
    pub fixed_duration_minutes: u32,
    pub party_quests: [u16; 2],
    pub special_quest: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RewardSettings {
    pub always: Option<u16>,
    pub half: Option<u16>,
    pub quarter: Option<u16>,
    pub ten_percent: Option<u16>,
    pub two_percent: Option<u16>,
}

impl RewardSettings {
    pub fn enabled_count(&self) -> usize {
        [
            self.always,
            self.half,
            self.quarter,
            self.ten_percent,
            self.two_percent,
        ]
        .into_iter()
        .flatten()
        .count()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BonusSettings {
    pub experience_percent: u32,
    pub money_percent: u32,
    pub item_drop_percent: u32,
}

impl BonusSettings {
    pub fn non_default_count(&self) -> usize {
        [
            self.experience_percent,
            self.money_percent,
            self.item_drop_percent,
        ]
        .into_iter()
        .filter(|percent| *percent != 100)
        .count()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SettingsSnapshot {
    pub shop_name: String,
    pub quests: QuestSettings,
    pub rewards: RewardSettings,
    pub bonuses: BonusSettings,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestTimetableEntry {
    pub starts_at: String,
    pub current: bool,
    pub party_quests: [String; 2],
    pub special_quest: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogRecord {
    pub sequence: u64,
    pub timestamp: String,
    pub level: LogLevel,
    pub target: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AdminSnapshot {
    pub sequence: u64,
    pub settings: SettingsSnapshot,
    pub party_quests: Vec<QuestOption>,
    pub special_quests: Vec<QuestOption>,
    pub timetable: Vec<QuestTimetableEntry>,
    pub logs: Vec<LogRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum AdminEvent {
    SettingsChanged(SettingsSnapshot),
    TimetableChanged(Vec<QuestTimetableEntry>),
    Log(LogRecord),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AdminEventEnvelope {
    pub sequence: u64,
    pub event: AdminEvent,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShopUpdate {
    pub shop_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AdminError {
    pub message: String,
    pub fields: Vec<FieldError>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AdminLogin {
    pub admin_token: String,
}
