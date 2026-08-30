use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};

use encoding_rs::SHIFT_JIS;
use serde::Deserialize;
use thiserror::Error;

use crate::protocol::station::EndpointHost;
use crate::protocol::tower::{
    AnnouncementCursor, AnnouncementRecord, AnnouncementTime, TowerProtocolError,
};

#[derive(Debug)]
pub struct AonNetConfig {
    pub(crate) server: ServerConfig,
    pub(crate) power_on: PowerOnConfig,
    pub(crate) announcements: Vec<AnnouncementRecord>,
}

#[derive(Debug)]
pub(crate) struct ServerConfig {
    pub(crate) listen: SocketAddr,
    pub(crate) database_path: PathBuf,
    pub(crate) game_listen: SocketAddr,
    pub(crate) matching_listen: SocketAddr,
    pub(crate) relay_listen: [SocketAddr; 3],
    pub(crate) gameplay_listen: SocketAddr,
    pub(crate) gameplay_advertise_host: EndpointHost,
    pub(crate) gameplay_advertise_port: NonZeroU16,
    pub(crate) matching_player_count: MatchingPlayerCount,
    pub(crate) game_session_id: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct PowerOnConfig {
    pub(crate) uri: String,
    pub(crate) host: String,
    pub(crate) shop_name: String,
    pub(crate) shop_nickname: String,
    pub(crate) region_code: String,
    pub(crate) region_name_0: String,
    pub(crate) region_name_1: String,
    pub(crate) region_name_2: String,
    pub(crate) region_name_3: String,
    pub(crate) place_id: String,
    pub(crate) setting: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MatchingPlayerCount(u8);

impl MatchingPlayerCount {
    pub(crate) const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawAonNetConfig {
    server: RawServerConfig,
    power_on: RawPowerOnConfig,
    #[serde(default)]
    announcements: Vec<RawAnnouncement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawServerConfig {
    listen: SocketAddr,
    database_path: PathBuf,
    game_listen: SocketAddr,
    matching_listen: SocketAddr,
    relay_listen: [SocketAddr; 3],
    gameplay_listen: SocketAddr,
    gameplay_advertise_host: String,
    gameplay_advertise_port: u16,
    matching_player_count: u8,
    game_session_id: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawPowerOnConfig {
    uri: String,
    host: String,
    shop_name: String,
    shop_nickname: String,
    region_code: String,
    region_name_0: String,
    region_name_1: String,
    region_name_2: String,
    region_name_3: String,
    place_id: String,
    setting: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawAnnouncement {
    start: String,
    end: String,
    #[serde(default)]
    sub_minute: u8,
    text: String,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read AON.Net configuration {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("AON.Net configuration is invalid: {source}")]
    Parse {
        #[source]
        source: toml::de::Error,
    },
    #[error("PowerOn field {field} contains a reserved character")]
    ReservedCharacter { field: &'static str },
    #[error("PowerOn field {field} must not be empty")]
    EmptyField { field: &'static str },
    #[error("PowerOn field {field} contains text that Shift_JIS cannot encode")]
    UnsupportedText { field: &'static str },
    #[error("gameplay-advertise-host must contain 1 through 31 ASCII bytes")]
    GameplayHost,
    #[error("gameplay-advertise-port must not be zero")]
    GameplayPort,
    #[error("matching-player-count must be in the range 2 through 4")]
    MatchingPlayerCount,
    #[error("announcement {index} has an invalid {field}; use YYYY-MM-DD HH:MM")]
    AnnouncementTime { index: usize, field: &'static str },
    #[error("announcement {index} is invalid: {source}")]
    Announcement {
        index: usize,
        #[source]
        source: TowerProtocolError,
    },
    #[error("announcement {index} ends before it starts")]
    AnnouncementOrder { index: usize },
    #[error("announcements must have different start cursors")]
    AnnouncementCursorOrder,
    #[error("at most 16 announcements can be configured")]
    AnnouncementCount,
}

pub fn load_config(path: &Path) -> Result<AonNetConfig, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let raw: RawAonNetConfig =
        toml::from_str(&text).map_err(|source| ConfigError::Parse { source })?;
    raw.validate()
}

impl RawAonNetConfig {
    fn validate(self) -> Result<AonNetConfig, ConfigError> {
        validate_power_on(&self.power_on)?;
        let gameplay_advertise_host = EndpointHost::new(self.server.gameplay_advertise_host)
            .map_err(|_| ConfigError::GameplayHost)?;
        let gameplay_advertise_port = NonZeroU16::new(self.server.gameplay_advertise_port)
            .ok_or(ConfigError::GameplayPort)?;
        if !(2..=4).contains(&self.server.matching_player_count) {
            return Err(ConfigError::MatchingPlayerCount);
        }

        let announcements = validate_announcements(self.announcements)?;
        Ok(AonNetConfig {
            server: ServerConfig {
                listen: self.server.listen,
                database_path: self.server.database_path,
                game_listen: self.server.game_listen,
                matching_listen: self.server.matching_listen,
                relay_listen: self.server.relay_listen,
                gameplay_listen: self.server.gameplay_listen,
                gameplay_advertise_host,
                gameplay_advertise_port,
                matching_player_count: MatchingPlayerCount(self.server.matching_player_count),
                game_session_id: self.server.game_session_id,
            },
            power_on: PowerOnConfig {
                uri: self.power_on.uri,
                host: self.power_on.host,
                shop_name: self.power_on.shop_name,
                shop_nickname: self.power_on.shop_nickname,
                region_code: self.power_on.region_code,
                region_name_0: self.power_on.region_name_0,
                region_name_1: self.power_on.region_name_1,
                region_name_2: self.power_on.region_name_2,
                region_name_3: self.power_on.region_name_3,
                place_id: self.power_on.place_id,
                setting: self.power_on.setting,
            },
            announcements,
        })
    }
}

fn validate_announcements(
    raw_announcements: Vec<RawAnnouncement>,
) -> Result<Vec<AnnouncementRecord>, ConfigError> {
    if raw_announcements.len() > 16 {
        return Err(ConfigError::AnnouncementCount);
    }
    let mut announcements = Vec::with_capacity(raw_announcements.len());
    for (index, raw) in raw_announcements.into_iter().enumerate() {
        let start_time =
            parse_announcement_time(&raw.start).ok_or(ConfigError::AnnouncementTime {
                index,
                field: "start",
            })?;
        let end = parse_announcement_time(&raw.end).ok_or(ConfigError::AnnouncementTime {
            index,
            field: "end",
        })?;
        if end < start_time || (end == start_time && raw.sub_minute != 0) {
            return Err(ConfigError::AnnouncementOrder { index });
        }
        let start = AnnouncementCursor::new(start_time, raw.sub_minute)
            .map_err(|source| ConfigError::Announcement { index, source })?;
        announcements.push(
            AnnouncementRecord::new(start, end, raw.text)
                .map_err(|source| ConfigError::Announcement { index, source })?,
        );
    }
    announcements.sort_by_key(|announcement| announcement.start);
    if announcements
        .windows(2)
        .any(|pair| pair[0].start == pair[1].start)
    {
        return Err(ConfigError::AnnouncementCursorOrder);
    }
    Ok(announcements)
}

fn parse_announcement_time(value: &str) -> Option<AnnouncementTime> {
    let (date, time) = value.split_once(' ')?;
    let mut date = date.split('-');
    let mut time = time.split(':');
    let year = date.next()?.parse().ok()?;
    let month = date.next()?.parse().ok()?;
    let day = date.next()?.parse().ok()?;
    let hour = time.next()?.parse().ok()?;
    let minute = time.next()?.parse().ok()?;
    if date.next().is_some() || time.next().is_some() {
        return None;
    }
    AnnouncementTime::new(year, month, day, hour, minute).ok()
}

fn validate_power_on(power_on: &RawPowerOnConfig) -> Result<(), ConfigError> {
    for (field, value) in [
        ("uri", power_on.uri.as_str()),
        ("host", power_on.host.as_str()),
        ("shop-name", power_on.shop_name.as_str()),
        ("shop-nickname", power_on.shop_nickname.as_str()),
        ("region-code", power_on.region_code.as_str()),
        ("region-name-0", power_on.region_name_0.as_str()),
        ("region-name-1", power_on.region_name_1.as_str()),
        ("region-name-2", power_on.region_name_2.as_str()),
        ("region-name-3", power_on.region_name_3.as_str()),
        ("place-id", power_on.place_id.as_str()),
        ("setting", power_on.setting.as_str()),
    ] {
        if value.contains(['&', '\0', '\r', '\n']) {
            return Err(ConfigError::ReservedCharacter { field });
        }
        if field != "setting" && value.is_empty() {
            return Err(ConfigError::EmptyField { field });
        }
        if SHIFT_JIS.encode(value).2 {
            return Err(ConfigError::UnsupportedText { field });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supplied_config() -> RawAonNetConfig {
        toml::from_str(include_str!("../aon-net.example.toml"))
            .unwrap_or_else(|error| panic!("supplied AON.Net configuration must parse: {error}"))
    }

    #[test]
    fn supplied_config_satisfies_runtime_invariants() {
        let config = supplied_config().validate().unwrap_or_else(|error| {
            panic!("supplied AON.Net configuration must validate: {error}")
        });

        assert_eq!(config.server.gameplay_advertise_port.get(), 33442);
        assert_eq!(config.server.matching_player_count.get(), 2);
        assert_eq!(
            config.server.gameplay_advertise_host.to_string(),
            "gameservers.aonnet"
        );
    }

    #[test]
    fn invalid_server_values_fail_at_the_configuration_edge() {
        let mut raw = supplied_config();
        raw.server.gameplay_advertise_port = 0;
        assert!(matches!(raw.validate(), Err(ConfigError::GameplayPort)));

        let mut raw = supplied_config();
        raw.server.matching_player_count = 1;
        assert!(matches!(
            raw.validate(),
            Err(ConfigError::MatchingPlayerCount)
        ));

        let mut raw = supplied_config();
        raw.server.gameplay_advertise_host = "not an endpoint host because it is too long".into();
        assert!(matches!(raw.validate(), Err(ConfigError::GameplayHost)));
    }
}
