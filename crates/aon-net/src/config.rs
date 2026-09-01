use std::net::IpAddr;
use std::num::{NonZeroU16, NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::time::Duration;

use encoding_rs::SHIFT_JIS;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;

use crate::protocol::station::EndpointHost;

#[derive(Debug)]
pub struct AonNetConfig {
    pub(crate) server: ServerConfig,
    pub(crate) admin_security: AdminSecurity,
    pub(crate) power_on: PowerOnConfig,
}

#[derive(Debug)]
pub(crate) enum AdminSecurity {
    Disabled,
    Enabled(SecureAdminConfig),
}

#[derive(Debug)]
pub(crate) struct SecureAdminConfig {
    pub(crate) tls_public_cert: PathBuf,
    pub(crate) tls_private_key: PathBuf,
    pub(crate) admin_token: AdminToken,
}

#[derive(Clone)]
pub(crate) struct AdminToken([u8; 32]);

impl std::fmt::Debug for AdminToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdminToken([redacted])")
    }
}

impl AdminToken {
    pub(crate) fn new(token: &str) -> Self {
        Self(Sha256::digest(token.as_bytes()).into())
    }

    pub(crate) fn matches(&self, candidate: &str) -> bool {
        let candidate: [u8; 32] = Sha256::digest(candidate.as_bytes()).into();
        bool::from(self.0.ct_eq(&candidate))
    }
}

#[derive(Debug)]
pub(crate) struct ServerConfig {
    pub(crate) bind_ip: IpAddr,
    pub(crate) http_port: u16,
    pub(crate) http_connection_limit: NonZeroUsize,
    pub(crate) http_request_timeout: Duration,
    pub(crate) http_body_limit: NonZeroUsize,
    pub(crate) game_connection_limit: NonZeroUsize,
    pub(crate) tower_connection_timeout: Duration,
    pub(crate) database_path: PathBuf,
    pub(crate) game_port: u16,
    pub(crate) matching_port: u16,
    pub(crate) relay_ports: [u16; 3],
    pub(crate) gameplay_port: u16,
    pub(crate) gameplay_advertise_host: EndpointHost,
    pub(crate) gameplay_advertise_port: NonZeroU16,
    pub(crate) gameplay_relay_queue_capacity: NonZeroUsize,
    pub(crate) gameplay_player_timeout: Duration,
    pub(crate) matching_player_timeout: Duration,
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
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawAonNetConfig {
    server: RawServerConfig,
    #[serde(default)]
    admin_security: RawAdminSecurity,
    power_on: RawPowerOnConfig,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawAdminSecurity {
    #[serde(default)]
    enabled: bool,
    tls_public_cert: Option<PathBuf>,
    tls_private_key: Option<PathBuf>,
    admin_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawServerConfig {
    bind_ip: IpAddr,
    http_port: u16,
    #[serde(default = "default_http_connection_limit")]
    http_connection_limit: NonZeroUsize,
    #[serde(default = "default_http_request_timeout_seconds")]
    http_request_timeout_seconds: NonZeroU64,
    #[serde(default = "default_http_body_limit_bytes")]
    http_body_limit_bytes: NonZeroUsize,
    #[serde(default = "default_game_connection_limit")]
    game_connection_limit: NonZeroUsize,
    #[serde(default = "default_tower_connection_timeout_seconds")]
    tower_connection_timeout_seconds: NonZeroU64,
    database_path: PathBuf,
    game_port: u16,
    matching_port: u16,
    relay_ports: [u16; 3],
    gameplay_port: u16,
    gameplay_advertise_host: String,
    gameplay_advertise_port: u16,
    #[serde(default = "default_gameplay_relay_queue_capacity")]
    gameplay_relay_queue_capacity: NonZeroUsize,
    #[serde(default = "default_gameplay_player_timeout_seconds")]
    gameplay_player_timeout_seconds: NonZeroU64,
    #[serde(default = "default_matching_player_timeout_seconds")]
    matching_player_timeout_seconds: NonZeroU64,
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
    #[error("admin-security field {field} is required when admin security is enabled")]
    AdminSecurityRequired { field: &'static str },
    #[error("admin-security field admin-token must contain at least 32 bytes")]
    AdminTokenTooShort,
    #[error("admin security cannot use TCP port 443 because another service uses it")]
    AdminPortConflict,
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
        let admin_security = self.admin_security.validate(&self.server)?;
        let gameplay_advertise_host = EndpointHost::new(self.server.gameplay_advertise_host)
            .map_err(|_| ConfigError::GameplayHost)?;
        let gameplay_advertise_port = NonZeroU16::new(self.server.gameplay_advertise_port)
            .ok_or(ConfigError::GameplayPort)?;
        Ok(AonNetConfig {
            admin_security,
            server: ServerConfig {
                bind_ip: self.server.bind_ip,
                http_port: self.server.http_port,
                http_connection_limit: self.server.http_connection_limit,
                http_request_timeout: Duration::from_secs(
                    self.server.http_request_timeout_seconds.get(),
                ),
                http_body_limit: self.server.http_body_limit_bytes,
                game_connection_limit: self.server.game_connection_limit,
                tower_connection_timeout: Duration::from_secs(
                    self.server.tower_connection_timeout_seconds.get(),
                ),
                database_path: self.server.database_path,
                game_port: self.server.game_port,
                matching_port: self.server.matching_port,
                relay_ports: self.server.relay_ports,
                gameplay_port: self.server.gameplay_port,
                gameplay_advertise_host,
                gameplay_advertise_port,
                gameplay_relay_queue_capacity: self.server.gameplay_relay_queue_capacity,
                gameplay_player_timeout: Duration::from_secs(
                    self.server.gameplay_player_timeout_seconds.get(),
                ),
                matching_player_timeout: Duration::from_secs(
                    self.server.matching_player_timeout_seconds.get(),
                ),
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
            },
        })
    }
}

impl RawAdminSecurity {
    fn validate(self, server: &RawServerConfig) -> Result<AdminSecurity, ConfigError> {
        if !self.enabled {
            return Ok(AdminSecurity::Disabled);
        }
        if [
            server.http_port,
            server.game_port,
            server.matching_port,
            server.relay_ports[0],
            server.relay_ports[1],
            server.relay_ports[2],
            server.gameplay_port,
        ]
        .contains(&443)
        {
            return Err(ConfigError::AdminPortConflict);
        }
        let tls_public_cert = required_path(self.tls_public_cert, "tls-public-cert")?;
        let tls_private_key = required_path(self.tls_private_key, "tls-private-key")?;
        let admin_token = self.admin_token.ok_or(ConfigError::AdminSecurityRequired {
            field: "admin-token",
        })?;
        if admin_token.len() < 32 {
            return Err(ConfigError::AdminTokenTooShort);
        }
        Ok(AdminSecurity::Enabled(SecureAdminConfig {
            tls_public_cert,
            tls_private_key,
            admin_token: AdminToken::new(&admin_token),
        }))
    }
}

fn required_path(path: Option<PathBuf>, field: &'static str) -> Result<PathBuf, ConfigError> {
    path.filter(|path| !path.as_os_str().is_empty())
        .ok_or(ConfigError::AdminSecurityRequired { field })
}

fn default_gameplay_relay_queue_capacity() -> NonZeroUsize {
    nonzero_usize(4096)
}

fn default_http_connection_limit() -> NonZeroUsize {
    nonzero_usize(64)
}

fn default_http_request_timeout_seconds() -> NonZeroU64 {
    nonzero_u64(10)
}

fn default_http_body_limit_bytes() -> NonZeroUsize {
    nonzero_usize(65_536)
}

fn default_game_connection_limit() -> NonZeroUsize {
    nonzero_usize(256)
}

fn default_tower_connection_timeout_seconds() -> NonZeroU64 {
    nonzero_u64(30)
}

fn default_gameplay_player_timeout_seconds() -> NonZeroU64 {
    nonzero_u64(10)
}

fn default_matching_player_timeout_seconds() -> NonZeroU64 {
    nonzero_u64(10)
}

fn nonzero_usize(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap_or(NonZeroUsize::MIN)
}

fn nonzero_u64(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap_or(NonZeroU64::MIN)
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
    ] {
        if value.contains(['&', '\0', '\r', '\n']) {
            return Err(ConfigError::ReservedCharacter { field });
        }
        if value.is_empty() {
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
        assert_eq!(config.server.http_connection_limit.get(), 64);
        assert_eq!(config.server.game_connection_limit.get(), 256);
        assert_eq!(config.server.http_request_timeout, Duration::from_secs(10));
        assert_eq!(config.server.http_body_limit.get(), 65_536);
        assert_eq!(
            config.server.tower_connection_timeout,
            Duration::from_secs(30)
        );
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
        raw.server.gameplay_advertise_host = "not an endpoint host because it is too long".into();
        assert!(matches!(raw.validate(), Err(ConfigError::GameplayHost)));
    }

    #[test]
    fn admin_security_is_optional_but_complete_when_enabled() {
        let config = supplied_config()
            .validate()
            .unwrap_or_else(|error| panic!("disabled admin security must validate: {error}"));
        assert!(matches!(config.admin_security, AdminSecurity::Disabled));

        let mut raw = supplied_config();
        raw.admin_security.enabled = true;
        assert!(matches!(
            raw.validate(),
            Err(ConfigError::AdminSecurityRequired {
                field: "tls-public-cert"
            })
        ));

        let mut raw = supplied_config();
        raw.admin_security.enabled = true;
        raw.admin_security.tls_public_cert = Some("fullchain.pem".into());
        raw.admin_security.tls_private_key = Some("private-key.pem".into());
        raw.admin_security.admin_token = Some("too-short".into());
        assert!(matches!(
            raw.validate(),
            Err(ConfigError::AdminTokenTooShort)
        ));

        let token = "a-random-admin-token-with-32-bytes";
        let mut raw = supplied_config();
        raw.admin_security.enabled = true;
        raw.admin_security.tls_public_cert = Some("fullchain.pem".into());
        raw.admin_security.tls_private_key = Some("private-key.pem".into());
        raw.admin_security.admin_token = Some(token.into());
        let config = raw
            .validate()
            .unwrap_or_else(|error| panic!("complete admin security must validate: {error}"));
        let AdminSecurity::Enabled(admin) = config.admin_security else {
            panic!("admin security must be enabled");
        };
        assert!(admin.admin_token.matches(token));
        assert!(!admin.admin_token.matches("a-different-random-token-value"));
    }

    #[test]
    fn secure_admin_port_must_be_available() {
        let mut raw = supplied_config();
        raw.server.game_port = 443;
        raw.admin_security.enabled = true;
        assert!(matches!(
            raw.validate(),
            Err(ConfigError::AdminPortConflict)
        ));
    }

    #[test]
    fn connection_limits_have_backward_compatible_defaults() {
        let text = include_str!("../aon-net.example.toml")
            .lines()
            .filter(|line| {
                ![
                    "http-connection-limit",
                    "http-request-timeout-seconds",
                    "http-body-limit-bytes",
                    "game-connection-limit",
                    "tower-connection-timeout-seconds",
                ]
                .iter()
                .any(|field| line.starts_with(field))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let raw: RawAonNetConfig = toml::from_str(&text).unwrap_or_else(|error| {
            panic!("configuration with omitted limits must parse: {error}")
        });
        let config = raw.validate().unwrap_or_else(|error| {
            panic!("configuration with omitted limits must validate: {error}")
        });

        assert_eq!(config.server.http_connection_limit.get(), 64);
        assert_eq!(config.server.game_connection_limit.get(), 256);
        assert_eq!(config.server.http_request_timeout, Duration::from_secs(10));
        assert_eq!(config.server.http_body_limit.get(), 65_536);
        assert_eq!(
            config.server.tower_connection_timeout,
            Duration::from_secs(30)
        );
    }
}
