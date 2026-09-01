use std::path::Path;
use std::sync::OnceLock;

use native_db::{Builder, Database, Models, ToKey};
use native_model::{Model, native_model};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::protocol::tower::{AnnouncementRecord, CardDataUpload};
use aon_net_admin::contract::{BonusSettings, QuestMode, RewardSettings};

const SERVER_STATE_KEY: u8 = 1;
const ADMIN_SETTINGS_KEY: u8 = 1;
const ANNOUNCEMENT_SETTINGS_KEY: u8 = 1;
const DEFAULT_RANK_LIMIT: u8 = 31;
const DEFAULT_MONEY_LIMIT: u32 = 99_999_999;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[native_model(id = 1, version = 1)]
#[native_db::native_db]
pub(crate) struct CardRecord {
    #[primary_key]
    pub(crate) record_id: u32,
    pub(crate) location: u16,
    pub(crate) card_data: Vec<u8>,
    pub(crate) shop_name: Vec<u8>,
    pub(crate) region_names: Vec<Vec<u8>>,
}

impl From<&CardDataUpload> for CardRecord {
    fn from(upload: &CardDataUpload) -> Self {
        Self {
            record_id: upload.record_id,
            location: upload.location,
            card_data: upload.card_data.clone(),
            shop_name: upload.shop_name.to_vec(),
            region_names: upload
                .region_names
                .iter()
                .map(|name| name.to_vec())
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[native_model(id = 2, version = 1)]
#[native_db::native_db]
pub(crate) struct ServerStateRecord {
    #[primary_key]
    id: u8,
    pub news_total: u8,
    pub news_available: u8,
    pub backup_total: u16,
    pub backup_available: u16,
    pub rank_limit: u8,
    pub money_limit: u32,
    pub disabled_item_ids: Vec<u16>,
    // Retain these fields for serialized version-1 record compatibility.
    // Server responses use the runtime quest settings instead.
    pub party_quest_ids: [u16; 2],
    pub special_quest_id: u16,
    pub pe_primary: [i16; 8],
    pub pe_secondary: i16,
    pub party_quest_parameters: [i16; 2],
    pub rq: [i16; 2],
    pub special_quest_parameter: i16,
    pub relay_party_count: u16,
    pub relay_player_count: u16,
    pub normal_schedule_quest_ids: Vec<u16>,
    pub hard_schedule_quest_ids: Vec<u16>,
}

impl Default for ServerStateRecord {
    fn default() -> Self {
        Self {
            id: SERVER_STATE_KEY,
            news_total: 1,
            news_available: 1,
            backup_total: 1,
            backup_available: 1,
            rank_limit: DEFAULT_RANK_LIMIT,
            money_limit: DEFAULT_MONEY_LIMIT,
            disabled_item_ids: Vec::new(),
            party_quest_ids: [10, 11],
            special_quest_id: 25,
            pe_primary: [0; 8],
            pe_secondary: 0,
            party_quest_parameters: [0; 2],
            rq: [0; 2],
            special_quest_parameter: 0,
            relay_party_count: 0,
            relay_player_count: 0,
            normal_schedule_quest_ids: vec![12],
            hard_schedule_quest_ids: vec![13],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[native_model(id = 3, version = 1)]
#[native_db::native_db]
pub(crate) struct AdminSettingsRecord {
    #[primary_key]
    id: u8,
    pub(crate) revision: u64,
    pub(crate) shop_name: String,
    pub(crate) quest_mode: QuestMode,
    pub(crate) random_interval_minutes: u16,
    pub(crate) fixed_duration_minutes: u32,
    pub(crate) fixed_expires_at: Option<i64>,
    pub(crate) party_quests: [u16; 2],
    pub(crate) special_quest: u16,
    pub(crate) normal_quest: u16,
    pub(crate) hard_quest: u16,
    pub(crate) rewards: RewardSettings,
    pub(crate) bonuses: BonusSettings,
}

impl AdminSettingsRecord {
    pub(crate) fn defaults(shop_name: String) -> Self {
        Self {
            id: ADMIN_SETTINGS_KEY,
            revision: 1,
            shop_name,
            quest_mode: QuestMode::Random,
            random_interval_minutes: 60,
            fixed_duration_minutes: 180,
            fixed_expires_at: None,
            party_quests: [10, 11],
            special_quest: 25,
            normal_quest: 12,
            hard_quest: 13,
            rewards: RewardSettings {
                always: Some(0x401e),
                half: None,
                quarter: None,
                ten_percent: None,
                two_percent: None,
            },
            bonuses: BonusSettings {
                experience_percent: 100,
                money_percent: 100,
                item_drop_percent: 100,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[native_model(id = 4, version = 1)]
#[native_db::native_db]
pub(crate) struct AnnouncementSettingsRecord {
    #[primary_key]
    id: u8,
    pub(crate) announcements: Vec<AnnouncementRecord>,
}

impl Default for AnnouncementSettingsRecord {
    fn default() -> Self {
        Self {
            id: ANNOUNCEMENT_SETTINGS_KEY,
            announcements: Vec::new(),
        }
    }
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("cannot define the AON.Net database models: {0}")]
    ModelDefinition(String),
    #[error("AON.Net database operation failed: {0}")]
    Database(Box<native_db::db_type::Error>),
}

impl From<native_db::db_type::Error> for StorageError {
    fn from(error: native_db::db_type::Error) -> Self {
        Self::Database(Box::new(error))
    }
}

pub(crate) struct Storage {
    database: Database<'static>,
}

impl Storage {
    pub(crate) fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(native_db::db_type::Error::Io)
                .map_err(StorageError::from)?;
        }

        let database = if path.exists() {
            Builder::new().open(models()?, path)?
        } else {
            Builder::new().create(models()?, path)?
        };
        let storage = Self { database };
        storage.initialize_server_state()?;
        storage.initialize_announcement_settings()?;
        Ok(storage)
    }

    pub(crate) fn store_card(&self, upload: &CardDataUpload) -> Result<(), StorageError> {
        let transaction = self.database.rw_transaction()?;
        transaction.upsert(CardRecord::from(upload))?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn server_state(&self) -> Result<ServerStateRecord, StorageError> {
        let transaction = self.database.r_transaction()?;
        transaction
            .get()
            .primary(SERVER_STATE_KEY)?
            .ok_or_else(|| StorageError::ModelDefinition("server state is missing".to_owned()))
    }

    pub(crate) fn initialize_admin_settings(
        &self,
        shop_name: String,
    ) -> Result<AdminSettingsRecord, StorageError> {
        let transaction = self.database.r_transaction()?;
        let settings = transaction
            .get()
            .primary::<AdminSettingsRecord>(ADMIN_SETTINGS_KEY)?;
        drop(transaction);
        if let Some(settings) = settings {
            return Ok(settings);
        }

        let settings = AdminSettingsRecord::defaults(shop_name);
        let transaction = self.database.rw_transaction()?;
        transaction.insert(settings.clone())?;
        transaction.commit()?;
        Ok(settings)
    }

    pub(crate) fn admin_settings(&self) -> Result<AdminSettingsRecord, StorageError> {
        let transaction = self.database.r_transaction()?;
        transaction
            .get()
            .primary(ADMIN_SETTINGS_KEY)?
            .ok_or_else(|| StorageError::ModelDefinition("admin settings are missing".to_owned()))
    }

    pub(crate) fn store_admin_settings(
        &self,
        settings: AdminSettingsRecord,
    ) -> Result<(), StorageError> {
        let transaction = self.database.rw_transaction()?;
        transaction.upsert(settings)?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn announcements(&self) -> Result<Vec<AnnouncementRecord>, StorageError> {
        let transaction = self.database.r_transaction()?;
        transaction
            .get()
            .primary::<AnnouncementSettingsRecord>(ANNOUNCEMENT_SETTINGS_KEY)?
            .map(|record| record.announcements)
            .ok_or_else(|| {
                StorageError::ModelDefinition("announcement settings are missing".to_owned())
            })
    }

    pub(crate) fn store_announcements(
        &self,
        announcements: Vec<AnnouncementRecord>,
    ) -> Result<(), StorageError> {
        let transaction = self.database.rw_transaction()?;
        transaction.upsert(AnnouncementSettingsRecord {
            id: ANNOUNCEMENT_SETTINGS_KEY,
            announcements,
        })?;
        transaction.commit()?;
        Ok(())
    }

    #[cfg(test)]
    fn card(&self, record_id: u32) -> Result<Option<CardRecord>, StorageError> {
        let transaction = self.database.r_transaction()?;
        Ok(transaction.get().primary(record_id)?)
    }

    fn initialize_server_state(&self) -> Result<(), StorageError> {
        let transaction = self.database.r_transaction()?;
        let state = transaction
            .get()
            .primary::<ServerStateRecord>(SERVER_STATE_KEY)?;
        drop(transaction);

        let transaction = self.database.rw_transaction()?;
        if let Some(mut state) = state {
            let mut changed = false;
            if state.rank_limit == 0 {
                state.rank_limit = DEFAULT_RANK_LIMIT;
                changed = true;
            }
            if state.money_limit == 0 {
                state.money_limit = DEFAULT_MONEY_LIMIT;
                changed = true;
            }
            if !changed {
                return Ok(());
            }
            transaction.upsert(state)?;
        } else {
            transaction.insert(ServerStateRecord::default())?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn initialize_announcement_settings(&self) -> Result<(), StorageError> {
        let transaction = self.database.r_transaction()?;
        let settings = transaction
            .get()
            .primary::<AnnouncementSettingsRecord>(ANNOUNCEMENT_SETTINGS_KEY)?;
        drop(transaction);
        if settings.is_some() {
            return Ok(());
        }

        let transaction = self.database.rw_transaction()?;
        transaction.insert(AnnouncementSettingsRecord::default())?;
        transaction.commit()?;
        Ok(())
    }
}

fn models() -> Result<&'static Models, StorageError> {
    static MODELS: OnceLock<Result<Models, String>> = OnceLock::new();
    let result = MODELS.get_or_init(|| {
        let mut models = Models::new();
        models
            .define::<CardRecord>()
            .map_err(|error| error.to_string())?;
        models
            .define::<ServerStateRecord>()
            .map_err(|error| error.to_string())?;
        models
            .define::<AdminSettingsRecord>()
            .map_err(|error| error.to_string())?;
        models
            .define::<AnnouncementSettingsRecord>()
            .map_err(|error| error.to_string())?;
        Ok(models)
    });
    result
        .as_ref()
        .map_err(|error| StorageError::ModelDefinition(error.clone()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{DEFAULT_MONEY_LIMIT, DEFAULT_RANK_LIMIT, Storage};
    use crate::protocol::tower::{
        AnnouncementCursor, AnnouncementRecord, AnnouncementTime, CardDataUpload, TowerRequest,
        deserialize_tower_request,
    };

    #[test]
    fn card_upload_survives_database_reopen() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let database_path = directory.path().join("aon-net.db");
        let upload = deserialize_upload()?;

        {
            let storage = Storage::open(&database_path)?;
            storage.store_card(&upload)?;
        }

        let storage = Storage::open(&database_path)?;
        let record = storage
            .card(upload.record_id)?
            .ok_or("stored card record is missing")?;
        assert_eq!(record.record_id, 0x0102_0304);
        assert_eq!(record.location, 0x1234);
        assert_eq!(record.card_data, [0xaa, 0xbb, 0xcc]);
        assert_eq!(&record.shop_name[..4], b"SHOP");
        assert_eq!(&record.region_names[0][..3], b"R0\0");
        Ok(())
    }

    #[test]
    fn announcements_survive_database_reopen() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let database_path = directory.path().join("aon-net.db");
        let announcement = AnnouncementRecord::new(
            AnnouncementCursor::new(AnnouncementTime::new(2009, 8, 3, 12, 30)?, 0)?,
            AnnouncementTime::new(2009, 8, 4, 12, 30)?,
            "Test".to_owned(),
        )?;

        {
            let storage = Storage::open(&database_path)?;
            storage.store_announcements(vec![announcement.clone()])?;
        }

        let storage = Storage::open(&database_path)?;
        assert_eq!(storage.announcements()?, [announcement]);
        Ok(())
    }

    #[test]
    fn legacy_quest_fields_have_distinct_valid_ids() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("aon-net.db"))?;
        let state = storage.server_state()?;
        let ids = [
            state.party_quest_ids[0],
            state.party_quest_ids[1],
            state.special_quest_id,
            state.normal_schedule_quest_ids[0],
            state.hard_schedule_quest_ids[0],
        ];

        assert_eq!(ids.iter().copied().collect::<HashSet<_>>().len(), ids.len());
        assert!(
            state
                .party_quest_ids
                .iter()
                .all(|id| (10..=22).contains(id))
        );
        assert!((25..=74).contains(&state.special_quest_id));
        assert!(
            state
                .normal_schedule_quest_ids
                .iter()
                .chain(&state.hard_schedule_quest_ids)
                .all(|id| (10..=22).contains(id))
        );
        Ok(())
    }

    #[test]
    fn legacy_zero_character_limits_are_repaired_on_reopen()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let database_path = directory.path().join("aon-net.db");

        {
            let storage = Storage::open(&database_path)?;
            let mut state = storage.server_state()?;
            state.rank_limit = 0;
            state.money_limit = 0;
            let transaction = storage.database.rw_transaction()?;
            transaction.upsert(state)?;
            transaction.commit()?;
        }

        let storage = Storage::open(&database_path)?;
        let state = storage.server_state()?;
        assert_eq!(state.rank_limit, DEFAULT_RANK_LIMIT);
        assert_eq!(state.money_limit, DEFAULT_MONEY_LIMIT);
        Ok(())
    }

    fn deserialize_upload() -> Result<CardDataUpload, Box<dyn std::error::Error>> {
        let mut frame = vec![0; 4 + 0x450];
        frame[0..4].copy_from_slice(&[0x00, 0x15, 0x04, 0x50]);
        frame[4..8].copy_from_slice(&0x0102_0304_u32.to_be_bytes());
        frame[8..10].copy_from_slice(&3_u16.to_be_bytes());
        frame[10..12].copy_from_slice(&0x1234_u16.to_be_bytes());
        frame[12..15].copy_from_slice(&[0xaa, 0xbb, 0xcc]);
        frame[4 + 0x328..4 + 0x32c].copy_from_slice(b"SHOP");
        frame[4 + 0x350..4 + 0x353].copy_from_slice(b"R0\0");

        match deserialize_tower_request(&frame)? {
            TowerRequest::CardDataUpload { upload } => Ok(*upload),
            _ => Err("request was not a card-data upload".into()),
        }
    }
}
