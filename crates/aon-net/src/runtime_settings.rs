use std::array;
use std::sync::{Arc, Mutex, MutexGuard};

use encoding_rs::SHIFT_JIS;
use thiserror::Error;

use crate::logging::AdminHub;
use crate::protocol::station::{ItemId, PresentChance, QuestModifier, StationProtocolError};
use crate::protocol::tower::{PartyQuestId, SpecialQuestId, TowerProtocolError};
use crate::server::quest_rotation::RandomQuestRotation;
use crate::storage::{AdminSettingsRecord, Storage, StorageError};
use aon_net_admin::contract::{
    AdminEvent, AdminSnapshot, BonusSettings, FieldError, QuestMode, QuestOption, QuestSettings,
    QuestTimetableEntry, RewardSettings, SettingsSnapshot,
};

include!("quest_catalog.rs");

const TIMETABLE_ENTRY_COUNT: usize = 12;
const MAX_MODIFIERS: usize = 4;

#[derive(Clone, Copy, Debug)]
pub(crate) struct EffectiveQuestRotation {
    pub(crate) party: [PartyQuestId; 2],
    pub(crate) special: SpecialQuestId,
    pub(crate) normal: PartyQuestId,
    pub(crate) hard: PartyQuestId,
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Tower(#[from] TowerProtocolError),
    #[error(transparent)]
    Station(#[from] StationProtocolError),
    #[error("{message}")]
    Field {
        field: &'static str,
        message: String,
    },
    #[error("the settings revision is too large")]
    Revision,
    #[error("the system time is outside the supported range")]
    Time,
}

impl SettingsError {
    pub(crate) fn field_error(&self) -> Option<FieldError> {
        match self {
            Self::Field { field, message } => Some(FieldError {
                field: (*field).to_owned(),
                message: message.clone(),
            }),
            _ => None,
        }
    }
}

pub(crate) struct RuntimeSettings {
    storage: Arc<Storage>,
    hub: Arc<AdminHub>,
    update_lock: Mutex<()>,
}

impl RuntimeSettings {
    pub(crate) fn new(
        storage: Arc<Storage>,
        hub: Arc<AdminHub>,
        default_shop_name: String,
    ) -> Result<Self, SettingsError> {
        storage.initialize_admin_settings(default_shop_name)?;
        Ok(Self {
            storage,
            hub,
            update_lock: Mutex::new(()),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_tests(storage: Arc<Storage>) -> Result<Arc<Self>, SettingsError> {
        Ok(Arc::new(Self::new(
            storage,
            Arc::new(AdminHub::new(32)),
            "AON.Net".to_owned(),
        )?))
    }

    pub(crate) fn snapshot(&self) -> Result<AdminSnapshot, SettingsError> {
        let settings = self.current_record()?;
        Ok(AdminSnapshot {
            sequence: self.hub.sequence(),
            settings: snapshot_from(&settings),
            party_quests: quest_options(10..=22),
            special_quests: quest_options((25..=74).chain(76..=89)),
            timetable: timetable_for(&settings)?,
            logs: self.hub.logs(),
            online_status: Default::default(),
        })
    }

    pub(crate) fn shop_name(&self) -> Result<String, SettingsError> {
        Ok(self.current_record()?.shop_name)
    }

    pub(crate) fn rotation(&self) -> Result<EffectiveQuestRotation, SettingsError> {
        let settings = self.current_record()?;
        if settings.quest_mode == QuestMode::Fixed {
            return rotation_from_ids(&settings);
        }
        random_rotation_at(now_seconds()?, settings.random_interval_minutes)
    }

    pub(crate) fn modifiers(&self) -> Result<[QuestModifier; MAX_MODIFIERS], SettingsError> {
        modifiers_for(&self.current_record()?)
    }

    pub(crate) fn update_shop(&self, shop_name: String) -> Result<SettingsSnapshot, SettingsError> {
        validate_shop_name(&shop_name)?;
        self.update(|settings| settings.shop_name = shop_name, false)
    }

    pub(crate) fn update_quests(
        &self,
        quests: QuestSettings,
    ) -> Result<SettingsSnapshot, SettingsError> {
        validate_quests(&quests)?;
        let now = now_seconds()?;
        let fixed_expires_at = match quests.mode {
            QuestMode::Random => None,
            QuestMode::Fixed => Some(
                now.checked_add(i64::from(quests.fixed_duration_minutes) * 60)
                    .ok_or(SettingsError::Time)?,
            ),
        };
        self.update(
            |settings| {
                settings.quest_mode = quests.mode;
                settings.random_interval_minutes = quests.random_interval_minutes;
                settings.fixed_duration_minutes = quests.fixed_duration_minutes;
                settings.fixed_expires_at = fixed_expires_at;
                settings.party_quests = quests.party_quests;
                settings.special_quest = quests.special_quest;
            },
            true,
        )
    }

    pub(crate) fn update_rewards(
        &self,
        rewards: RewardSettings,
    ) -> Result<SettingsSnapshot, SettingsError> {
        validate_reward_items(&rewards)?;
        self.update(
            |settings| {
                validate_modifier_count(&rewards, &settings.bonuses)?;
                settings.rewards = rewards;
                Ok(())
            },
            false,
        )
    }

    pub(crate) fn update_bonuses(
        &self,
        bonuses: BonusSettings,
    ) -> Result<SettingsSnapshot, SettingsError> {
        self.update(
            |settings| {
                validate_modifier_count(&settings.rewards, &bonuses)?;
                settings.bonuses = bonuses;
                Ok(())
            },
            false,
        )
    }

    fn update<F, R>(
        &self,
        change: F,
        timetable_changed: bool,
    ) -> Result<SettingsSnapshot, SettingsError>
    where
        F: FnOnce(&mut AdminSettingsRecord) -> R,
        R: IntoUpdateResult,
    {
        let _guard = self.lock_updates();
        let mut settings = self.storage.admin_settings()?;
        change(&mut settings).into_result()?;
        settings.revision = settings
            .revision
            .checked_add(1)
            .ok_or(SettingsError::Revision)?;
        self.storage.store_admin_settings(settings.clone())?;
        let snapshot = snapshot_from(&settings);
        self.hub
            .publish(AdminEvent::SettingsChanged(snapshot.clone()));
        if timetable_changed {
            self.hub
                .publish(AdminEvent::TimetableChanged(timetable_for(&settings)?));
        }
        Ok(snapshot)
    }

    fn current_record(&self) -> Result<AdminSettingsRecord, SettingsError> {
        let settings = self.storage.admin_settings()?;
        let now = now_seconds()?;
        if settings.quest_mode != QuestMode::Fixed
            || settings.fixed_expires_at.is_some_and(|expiry| expiry > now)
        {
            return Ok(settings);
        }
        let _guard = self.lock_updates();
        let mut settings = self.storage.admin_settings()?;
        if settings.quest_mode == QuestMode::Fixed
            && settings.fixed_expires_at.is_none_or(|expiry| expiry <= now)
        {
            settings.quest_mode = QuestMode::Random;
            settings.fixed_expires_at = None;
            settings.revision = settings
                .revision
                .checked_add(1)
                .ok_or(SettingsError::Revision)?;
            self.storage.store_admin_settings(settings.clone())?;
            let snapshot = snapshot_from(&settings);
            self.hub.publish(AdminEvent::SettingsChanged(snapshot));
            self.hub
                .publish(AdminEvent::TimetableChanged(timetable_for(&settings)?));
        }
        Ok(settings)
    }

    fn lock_updates(&self) -> MutexGuard<'_, ()> {
        self.update_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

trait IntoUpdateResult {
    fn into_result(self) -> Result<(), SettingsError>;
}

impl IntoUpdateResult for () {
    fn into_result(self) -> Result<(), SettingsError> {
        Ok(())
    }
}

impl IntoUpdateResult for Result<(), SettingsError> {
    fn into_result(self) -> Result<(), SettingsError> {
        self
    }
}

fn snapshot_from(settings: &AdminSettingsRecord) -> SettingsSnapshot {
    SettingsSnapshot {
        shop_name: settings.shop_name.clone(),
        quests: QuestSettings {
            mode: settings.quest_mode,
            random_interval_minutes: settings.random_interval_minutes,
            fixed_duration_minutes: settings.fixed_duration_minutes,
            party_quests: settings.party_quests,
            special_quest: settings.special_quest,
        },
        rewards: settings.rewards.clone(),
        bonuses: settings.bonuses.clone(),
    }
}

fn validate_shop_name(shop_name: &str) -> Result<(), SettingsError> {
    if shop_name.is_empty() {
        return field("shop_name", "Enter a shop name.");
    }
    if shop_name.contains(['&', '\0', '\r', '\n']) {
        return field(
            "shop_name",
            "The shop name contains an unsupported character.",
        );
    }
    let (_, _, encoding_error) = SHIFT_JIS.encode(shop_name);
    if encoding_error {
        return field(
            "shop_name",
            "The shop name contains a character that Shift JIS cannot encode.",
        );
    }
    Ok(())
}

fn validate_quests(quests: &QuestSettings) -> Result<(), SettingsError> {
    if quests.random_interval_minutes == 0 {
        return field(
            "random_interval_minutes",
            "Enter a value greater than zero.",
        );
    }
    if quests.fixed_duration_minutes == 0 {
        return field("fixed_duration_minutes", "Enter a value greater than zero.");
    }
    for id in quests.party_quests {
        PartyQuestId::new(id).map_err(|error| SettingsError::Field {
            field: "party_quests",
            message: error.to_string(),
        })?;
    }
    SpecialQuestId::new(quests.special_quest).map_err(|error| SettingsError::Field {
        field: "special_quest",
        message: error.to_string(),
    })?;
    let party_ids = quests.party_quests;
    for (index, id) in party_ids.into_iter().enumerate() {
        if party_ids[..index].contains(&id) {
            return field("party_quests", "Select four different party quests.");
        }
    }
    Ok(())
}

fn validate_reward_items(rewards: &RewardSettings) -> Result<(), SettingsError> {
    for id in [
        rewards.always,
        rewards.half,
        rewards.quarter,
        rewards.ten_percent,
        rewards.two_percent,
    ]
    .into_iter()
    .flatten()
    {
        ItemId::new(id).map_err(|error| SettingsError::Field {
            field: "rewards",
            message: error.to_string(),
        })?;
    }
    Ok(())
}

fn validate_modifier_count(
    rewards: &RewardSettings,
    bonuses: &BonusSettings,
) -> Result<(), SettingsError> {
    if rewards.enabled_count() + bonuses.non_default_count() > MAX_MODIFIERS {
        return field(
            "modifiers",
            "Enable no more than four quest rewards and bonuses in total.",
        );
    }
    Ok(())
}

fn modifiers_for(settings: &AdminSettingsRecord) -> Result<[QuestModifier; 4], SettingsError> {
    validate_modifier_count(&settings.rewards, &settings.bonuses)?;
    let mut modifiers = Vec::with_capacity(4);
    let rewards = [
        (settings.rewards.always, PresentChance::Always),
        (settings.rewards.half, PresentChance::Half),
        (settings.rewards.quarter, PresentChance::Quarter),
        (settings.rewards.ten_percent, PresentChance::TenPercent),
        (settings.rewards.two_percent, PresentChance::TwoPercent),
    ];
    for (item, chance) in rewards {
        if let Some(item) = item {
            modifiers.push(QuestModifier::PresentItem {
                chance,
                item_id: ItemId::new(item)?,
            });
        }
    }
    let bonuses = [
        (settings.bonuses.experience_percent, 0_u8),
        (settings.bonuses.money_percent, 1),
        (settings.bonuses.item_drop_percent, 2),
    ];
    for (percent, kind) in bonuses {
        if percent != 100 {
            modifiers.push(match kind {
                0 => QuestModifier::ExperienceRewardMultiplier(percent),
                1 => QuestModifier::MoneyRewardMultiplier(percent),
                _ => QuestModifier::ItemDropRateMultiplier(percent),
            });
        }
    }
    Ok(array::from_fn(|index| {
        modifiers.get(index).copied().unwrap_or(QuestModifier::None)
    }))
}

fn rotation_from_ids(
    settings: &AdminSettingsRecord,
) -> Result<EffectiveQuestRotation, SettingsError> {
    Ok(EffectiveQuestRotation {
        party: [
            PartyQuestId::new(settings.party_quests[0])?,
            PartyQuestId::new(settings.party_quests[1])?,
        ],
        special: SpecialQuestId::new(settings.special_quest)?,
        normal: PartyQuestId::new(settings.normal_quest)?,
        hard: PartyQuestId::new(settings.hard_quest)?,
    })
}

fn random_rotation_at(
    timestamp: i64,
    interval_minutes: u16,
) -> Result<EffectiveQuestRotation, SettingsError> {
    let interval_seconds = i64::from(interval_minutes) * 60;
    let period_start = timestamp.div_euclid(interval_seconds) * interval_seconds;
    let zoned = jiff::Timestamp::from_second(period_start)
        .map_err(|_| SettingsError::Time)?
        .to_zoned(jiff::tz::TimeZone::system());
    let rotation = RandomQuestRotation::new(
        u16::try_from(zoned.year()).map_err(|_| SettingsError::Time)?,
        u8::try_from(zoned.month()).map_err(|_| SettingsError::Time)?,
        u8::try_from(zoned.day()).map_err(|_| SettingsError::Time)?,
        u8::try_from(zoned.hour()).map_err(|_| SettingsError::Time)?,
        u8::try_from(zoned.minute()).map_err(|_| SettingsError::Time)?,
    )?;
    Ok(EffectiveQuestRotation {
        party: rotation.active_party,
        special: rotation.active_special,
        normal: rotation.normal_quest,
        hard: rotation.hard_quest,
    })
}

fn timetable_for(
    settings: &AdminSettingsRecord,
) -> Result<Vec<QuestTimetableEntry>, SettingsError> {
    let interval_seconds = i64::from(settings.random_interval_minutes) * 60;
    let now = now_seconds()?;
    let current_start = now.div_euclid(interval_seconds) * interval_seconds;
    (0..TIMETABLE_ENTRY_COUNT)
        .map(|index| {
            let starts_at = current_start
                .checked_add(
                    i64::try_from(index).map_err(|_| SettingsError::Time)? * interval_seconds,
                )
                .ok_or(SettingsError::Time)?;
            let zoned = jiff::Timestamp::from_second(starts_at)
                .map_err(|_| SettingsError::Time)?
                .to_zoned(jiff::tz::TimeZone::system());
            let rotation = random_rotation_at(starts_at, settings.random_interval_minutes)?;
            Ok(QuestTimetableEntry {
                starts_at: format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}",
                    zoned.year(),
                    zoned.month(),
                    zoned.day(),
                    zoned.hour(),
                    zoned.minute()
                ),
                current: index == 0 && settings.quest_mode == QuestMode::Random,
                party_quests: [
                    quest_name(rotation.party[0].get()),
                    quest_name(rotation.party[1].get()),
                ],
                special_quest: quest_name(rotation.special.get()),
            })
        })
        .collect()
}

fn quest_options(ids: impl IntoIterator<Item = u16>) -> Vec<QuestOption> {
    ids.into_iter()
        .map(|quest_id| QuestOption {
            quest_id,
            name: quest_name(quest_id),
        })
        .collect()
}

fn quest_name(id: u16) -> String {
    QUEST_NAMES
        .iter()
        .find_map(|(quest_id, name)| (*quest_id == id).then_some(*name))
        .unwrap_or("Unknown quest")
        .to_owned()
}

fn now_seconds() -> Result<i64, SettingsError> {
    Ok(jiff::Timestamp::now().as_second())
}

fn field<T>(field: &'static str, message: impl Into<String>) -> Result<T, SettingsError> {
    Err(SettingsError::Field {
        field,
        message: message.into(),
    })
}
