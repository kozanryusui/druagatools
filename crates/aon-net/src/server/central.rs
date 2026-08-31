use std::sync::Arc;

use thiserror::Error;
use tracing::{debug, info};

use crate::online::{OnlineError, OnlineState, ServiceCounts};
use crate::protocol::station::{MatchingActivationConfiguration, QuestEventConfiguration};
use crate::protocol::tower::{
    AnnouncementCursor, AnnouncementRecord, AnnouncementTime, DatabaseStatus, DiskCapacity,
    MatchingConfiguration, PartyQuestSchedule, PartyQuestScheduleEntry, RelayStatus, ServiceTime,
    TowerProtocolError, TowerRequest, TowerResponse,
};
use crate::runtime_settings::{EffectiveQuestRotation, RuntimeSettings, SettingsError};
use crate::storage::{ServerStateRecord, Storage, StorageError};

pub(super) struct CentralServices {
    session_id: u32,
    storage: Arc<Storage>,
    settings: Arc<RuntimeSettings>,
    online: Arc<OnlineState>,
    announcements: Vec<AnnouncementRecord>,
}

#[derive(Debug, Error)]
pub(super) enum CentralServiceError {
    #[error("central Tower protocol failed: {0}")]
    Protocol(#[from] TowerProtocolError),
    #[error("Station protocol failed: {0}")]
    Settings(#[from] SettingsError),
    #[error("central storage operation failed: {0}")]
    Storage(#[from] StorageError),
    #[error("online party operation failed: {0}")]
    Online(#[from] OnlineError),
    #[error("client returned session ID {actual}, expected {expected}")]
    SessionId { expected: u32, actual: u32 },
}

impl CentralServices {
    pub(super) const fn new(
        session_id: u32,
        storage: Arc<Storage>,
        settings: Arc<RuntimeSettings>,
        online: Arc<OnlineState>,
        announcements: Vec<AnnouncementRecord>,
    ) -> Self {
        Self {
            session_id,
            storage,
            settings,
            online,
            announcements,
        }
    }

    pub(super) fn respond(
        &self,
        request: TowerRequest,
    ) -> Result<TowerResponse, CentralServiceError> {
        let response = match request {
            TowerRequest::InitialIdentity { identity, reserved } => {
                info!(?identity, reserved, "accepted Tower service identity");
                TowerResponse::InitialAccepted {
                    session_id: self.session_id,
                }
            }
            TowerRequest::SessionConfirm { session_id } => {
                if session_id != self.session_id {
                    return Err(CentralServiceError::SessionId {
                        expected: self.session_id,
                        actual: session_id,
                    });
                }
                info!(session_id, "confirmed Tower service session");
                TowerResponse::SessionConfirmed { reserved: 0 }
            }
            TowerRequest::ServiceRecordRequest {} => {
                info!("accepted Tower service-record request");
                let state = self.storage.server_state()?;
                TowerResponse::ServiceRecord {
                    rank_limit: state.rank_limit,
                    reserved: [0; 2],
                    disabled_item_ids: state.disabled_item_ids,
                    money_limit: state.money_limit,
                }
            }
            TowerRequest::AnnouncementRequest {
                cursor_year,
                cursor_month,
                cursor_day,
                cursor_hour,
                cursor_minute,
                cursor_sub_minute,
            } => {
                info!(
                    cursor_year,
                    cursor_month,
                    cursor_day,
                    cursor_hour,
                    cursor_minute,
                    cursor_sub_minute,
                    "accepted Tower announcement request"
                );
                let cursor = AnnouncementCursor {
                    time: AnnouncementTime {
                        year: cursor_year,
                        month: cursor_month,
                        day: cursor_day,
                        hour: cursor_hour,
                        minute: cursor_minute,
                    },
                    sub_minute: cursor_sub_minute,
                };
                self.announcements
                    .iter()
                    .find(|announcement| announcement.start > cursor)
                    .cloned()
                    .map(TowerResponse::Announcement)
                    .unwrap_or(TowerResponse::AnnouncementComplete)
            }
            TowerRequest::CardDataUpload { upload } => {
                info!(
                    record_id = upload.record_id,
                    location = upload.location,
                    data_length = upload.card_data.len(),
                    "accepted Tower card-data upload"
                );
                self.storage.store_card(&upload)?;
                TowerResponse::CardDataStored
            }
            TowerRequest::DatabaseStatusRequest {} => {
                debug!("accepted Tower database-status request");
                database_status_response(&self.storage.server_state()?)?
            }
            TowerRequest::MatchingConfigurationRequest {} => {
                debug!("accepted Tower matching-configuration request");
                matching_configuration_response(
                    &self.storage.server_state()?,
                    self.settings.rotation()?,
                )?
            }
            TowerRequest::RelayStatusRequest {} => {
                debug!("accepted Tower relay-status request");
                relay_status_response(&self.storage.server_state()?, self.online.service_counts()?)?
            }
            TowerRequest::PartyQuestScheduleRequest {} => {
                debug!("accepted Tower party-quest schedule request");
                party_quest_schedule_response(self.settings.rotation()?)?
            }
        };
        Ok(response)
    }

    pub(super) fn matching_activation_configuration(
        &self,
    ) -> Result<MatchingActivationConfiguration, CentralServiceError> {
        let rotation = self.settings.rotation()?;
        let modifiers = self.settings.modifiers()?;
        Ok(MatchingActivationConfiguration::new(
            rotation
                .party
                .map(|quest_id| QuestEventConfiguration::new(quest_id, modifiers)),
            QuestEventConfiguration::new(rotation.special, modifiers),
        ))
    }
}

fn database_status_response(
    state: &ServerStateRecord,
) -> Result<TowerResponse, TowerProtocolError> {
    Ok(TowerResponse::DatabaseStatus(DatabaseStatus::new(
        current_service_time()?,
        healthy_disk_capacity()?,
        state.news_total,
        state.news_available,
        state.backup_total,
        state.backup_available,
        state.rank_limit,
        state.money_limit,
        state.disabled_item_ids.clone(),
    )?))
}

fn matching_configuration_response(
    state: &ServerStateRecord,
    rotation: EffectiveQuestRotation,
) -> Result<TowerResponse, TowerProtocolError> {
    let time = current_service_time()?;
    Ok(TowerResponse::MatchingConfiguration(
        MatchingConfiguration::new(
            time,
            healthy_disk_capacity()?,
            rotation.party.map(Some),
            Some(rotation.special),
            state.pe_primary,
            state.pe_secondary,
            state.party_quest_parameters,
            state.rq,
            state.special_quest_parameter,
        ),
    ))
}

fn relay_status_response(
    state: &ServerStateRecord,
    counts: ServiceCounts,
) -> Result<TowerResponse, TowerProtocolError> {
    Ok(TowerResponse::RelayStatus(RelayStatus::new(
        current_service_time()?,
        healthy_disk_capacity()?,
        counts.party_count.max(state.relay_party_count),
        counts.player_count.max(state.relay_player_count),
    )?))
}

fn party_quest_schedule_response(
    rotation: EffectiveQuestRotation,
) -> Result<TowerResponse, TowerProtocolError> {
    let time = current_service_time()?;
    Ok(TowerResponse::PartyQuestSchedule(PartyQuestSchedule::new(
        vec![PartyQuestScheduleEntry::new(time, rotation.normal)],
        vec![PartyQuestScheduleEntry::new(time, rotation.hard)],
    )?))
}

fn current_service_time() -> Result<ServiceTime, TowerProtocolError> {
    service_time_at(&jiff::Zoned::now())
}

fn service_time_at(now: &jiff::Zoned) -> Result<ServiceTime, TowerProtocolError> {
    let year = u16::try_from(now.year()).map_err(|_| TowerProtocolError::ServiceTime)?;
    let month = u8::try_from(now.month()).map_err(|_| TowerProtocolError::ServiceTime)?;
    let day = u8::try_from(now.day()).map_err(|_| TowerProtocolError::ServiceTime)?;
    let hour = u8::try_from(now.hour()).map_err(|_| TowerProtocolError::ServiceTime)?;
    let minute = u8::try_from(now.minute()).map_err(|_| TowerProtocolError::ServiceTime)?;
    let second = u8::try_from(now.second()).map_err(|_| TowerProtocolError::ServiceTime)?;
    ServiceTime::new(year, month, day, hour, minute, second)
}

fn healthy_disk_capacity() -> Result<DiskCapacity, TowerProtocolError> {
    DiskCapacity::new(100, 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::contract::{BonusSettings, RewardSettings};
    use crate::protocol::frame::Frame;
    use crate::protocol::station::{EndpointHost, MatchingResponse};
    use crate::protocol::tower::serialize_tower_response;

    #[test]
    fn service_record_uses_the_station_character_limits() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let storage = Arc::new(Storage::open(&directory.path().join("aon-net.db"))?);
        let online = Arc::new(OnlineState::new(
            EndpointHost::new("gameservers.aonnet".into())?,
            33442,
        ));
        let settings = RuntimeSettings::for_tests(Arc::clone(&storage))?;
        let central = CentralServices::new(1, storage, settings, online, Vec::new());

        let response = central.respond(TowerRequest::ServiceRecordRequest {})?;
        let frame = serialize_tower_response(&response)?;

        assert_eq!(frame[4], 31);
        assert_eq!(&frame[8..12], &99_999_999_u32.to_be_bytes());
        Ok(())
    }

    #[test]
    fn active_quests_use_configured_rewards_and_bonuses() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let storage = Arc::new(Storage::open(&directory.path().join("aon-net.db"))?);
        let online = Arc::new(OnlineState::new(
            EndpointHost::new("gameservers.aonnet".into())?,
            33442,
        ));
        let settings = RuntimeSettings::for_tests(Arc::clone(&storage))?;
        settings.update_rewards(RewardSettings {
            always: Some(0x401e),
            half: Some(0x401d),
            quarter: None,
            ten_percent: None,
            two_percent: None,
        })?;
        settings.update_bonuses(BonusSettings {
            experience_percent: 150,
            money_percent: 100,
            item_drop_percent: 125,
        })?;
        let central = CentralServices::new(1, storage, settings, online, Vec::new());

        let configuration = central.matching_activation_configuration()?;
        let bytes = MatchingResponse::ActivationConfiguration(configuration).serialize()?;
        let frame = Frame::from_bytes(&bytes)?;

        let expected_modifier = [0x00, 0x13, 0x00, 0x14, 0x00, 0x10, 0x00, 0x12];
        for offset in [0x06, 0x0e, 0x16] {
            assert_eq!(&frame.payload[offset..offset + 8], &expected_modifier);
        }

        let expected_value = [
            0x00, 0x00, 0x40, 0x1e, 0x00, 0x00, 0x40, 0x1d, 0x00, 0x00, 0x00, 0x96, 0x00, 0x00,
            0x00, 0x7d,
        ];
        for offset in [0x20, 0x30, 0x40] {
            assert_eq!(&frame.payload[offset..offset + 16], &expected_value);
        }
        Ok(())
    }

    #[test]
    fn announcement_request_returns_the_next_cursor_then_the_terminal_marker()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let storage = Arc::new(Storage::open(&directory.path().join("aon-net.db"))?);
        let online = Arc::new(OnlineState::new(
            EndpointHost::new("gameservers.aonnet".into())?,
            33442,
        ));
        let start_time = AnnouncementTime::new(2009, 8, 3, 12, 30)?;
        let start = AnnouncementCursor::new(start_time, 7)?;
        let announcement = AnnouncementRecord::new(
            start,
            AnnouncementTime::new(2009, 8, 4, 23, 59)?,
            "Test".to_owned(),
        )?;
        let settings = RuntimeSettings::for_tests(Arc::clone(&storage))?;
        let central =
            CentralServices::new(1, storage, settings, online, vec![announcement.clone()]);

        let first = central.respond(TowerRequest::AnnouncementRequest {
            cursor_year: 2005,
            cursor_month: 1,
            cursor_day: 1,
            cursor_hour: 0,
            cursor_minute: 0,
            cursor_sub_minute: 0,
        })?;
        assert_eq!(first, TowerResponse::Announcement(announcement));

        let last = central.respond(TowerRequest::AnnouncementRequest {
            cursor_year: start_time.year,
            cursor_month: start_time.month,
            cursor_day: start_time.day,
            cursor_hour: start_time.hour,
            cursor_minute: start_time.minute,
            cursor_sub_minute: start.sub_minute,
        })?;
        assert_eq!(last, TowerResponse::AnnouncementComplete);
        Ok(())
    }
}
