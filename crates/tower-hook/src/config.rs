use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;
use thiserror::Error;

const DISPLAY_MODE_OVERRIDE: &str = "DRUAGA_TOWER_DISPLAY_MODE";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayMode {
    #[default]
    Windowed,
    Original,
}

impl FromStr for DisplayMode {
    type Err = DisplayConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "windowed" => Ok(Self::Windowed),
            "original" => Ok(Self::Original),
            _ => Err(DisplayConfigError::InvalidDisplayMode {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct DisplayConfig {
    pub mode: DisplayMode,
    pub monitor: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub refresh_hz: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DisplayOverrides {
    pub mode: Option<DisplayMode>,
}

impl DisplayOverrides {
    pub fn from_environment() -> Result<Self, DisplayConfigError> {
        let Some(value) = std::env::var_os(DISPLAY_MODE_OVERRIDE) else {
            return Ok(Self::default());
        };
        let value =
            value
                .into_string()
                .map_err(|value| DisplayConfigError::InvalidDisplayMode {
                    value: value.to_string_lossy().into_owned(),
                })?;
        Ok(Self {
            mode: Some(DisplayMode::from_str(&value)?),
        })
    }
}

#[derive(Debug, Error)]
pub enum DisplayConfigError {
    #[error("display mode is invalid: {value}")]
    InvalidDisplayMode { value: String },
    #[error("display field is not supported in Phase 2: {field}")]
    UnsupportedPhase2Field { field: &'static str },
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct TowerHookConfig {
    display: DisplayConfig,
    startup: StartupConfig,
    network: NetworkConfigInput,
    cards: CardConfigInput,
    logging: SerialLoggingConfig,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct StartupConfig {
    pub(crate) skip_notice: bool,
    pub(crate) skip_logos: bool,
}

#[derive(Debug, Error)]
pub enum HookConfigError {
    #[error("configuration input failed for {path}: {source}")]
    Input {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("configuration is invalid: {source}")]
    Parse {
        #[source]
        source: toml::de::Error,
    },
    #[error(transparent)]
    Display(#[from] DisplayConfigError),
    #[error(transparent)]
    Network(#[from] NetworkConfigError),
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterLookup {
    #[default]
    Dynamic,
    Original,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum RouterCheckMode {
    #[default]
    Original,
    Emulated,
    Disconnected,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DnsTarget {
    Domain(String),
    Ipv4(Ipv4Addr),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DnsOverride {
    pub source: String,
    pub target: DnsTarget,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkConfig {
    pub adapter: AdapterLookup,
    pub router_checks: RouterCheckMode,
    pub disable_maintenance_window: bool,
    pub power_on_port: Option<NonZeroU16>,
    pub dns_overrides: Vec<DnsOverride>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct NetworkConfigInput {
    adapter: AdapterLookup,
    router_checks: RouterCheckMode,
    disable_maintenance_window: bool,
    power_on_port: Option<u16>,
    dns_overrides: Vec<DnsOverrideInput>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct DnsOverrideInput {
    source: String,
    domain: Option<String>,
    ip: Option<Ipv4Addr>,
}

#[derive(Debug, Error)]
pub enum NetworkConfigError {
    #[error("DNS override {host} is invalid: {reason}")]
    InvalidDnsOverride { host: String, reason: &'static str },
    #[error("PowerOn port must be in the range 1 through 65535")]
    InvalidPowerOnPort,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardConfig {
    pub directory: PathBuf,
    pub reset_usage_count: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SerialLoggingConfig {
    pub io_board: bool,
    pub left_reader: bool,
    pub right_reader: bool,
}

impl Default for SerialLoggingConfig {
    fn default() -> Self {
        Self {
            io_board: true,
            left_reader: true,
            right_reader: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct CardConfigInput {
    directory: PathBuf,
    reset_usage_count: bool,
}

impl Default for CardConfigInput {
    fn default() -> Self {
        Self {
            directory: PathBuf::from("cards"),
            reset_usage_count: false,
        }
    }
}

pub(crate) fn load_config(
    path: &Path,
    overrides: &DisplayOverrides,
) -> Result<
    (
        DisplayConfig,
        StartupConfig,
        NetworkConfig,
        CardConfig,
        SerialLoggingConfig,
    ),
    HookConfigError,
> {
    let input = match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str::<TowerHookConfig>(&text)
            .map_err(|source| HookConfigError::Parse { source })?,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => TowerHookConfig::default(),
        Err(source) => {
            return Err(HookConfigError::Input {
                path: path.display().to_string(),
                source,
            });
        }
    };

    let mut display = input.display;
    reject_future_fields(&display)?;
    if let Some(mode) = overrides.mode {
        display.mode = mode;
    }
    let network = convert_network_config(input.network)?;
    let reset_usage_count = input.cards.reset_usage_count;
    let directory = if input.cards.directory.is_absolute() {
        input.cards.directory
    } else {
        path.parent()
            .unwrap_or_else(|| Path::new("."))
            .join(input.cards.directory)
    };
    Ok((
        display,
        input.startup,
        network,
        CardConfig {
            directory,
            reset_usage_count,
        },
        input.logging,
    ))
}

fn convert_network_config(input: NetworkConfigInput) -> Result<NetworkConfig, NetworkConfigError> {
    let mut sources = HashSet::new();
    let mut dns_overrides = Vec::with_capacity(input.dns_overrides.len());
    for entry in input.dns_overrides {
        let source = entry.source.trim().to_ascii_lowercase();
        if source.is_empty() || !source.is_ascii() || source.as_bytes().contains(&0) {
            return Err(NetworkConfigError::InvalidDnsOverride {
                host: source,
                reason: "source must be a nonempty ASCII host name",
            });
        }
        if !sources.insert(source.clone()) {
            return Err(NetworkConfigError::InvalidDnsOverride {
                host: source,
                reason: "source is specified more than once",
            });
        }

        // A domain takes precedence when both forms are present.
        let target = if let Some(domain) = entry.domain {
            let domain = domain.trim().to_owned();
            if domain.is_empty() || !domain.is_ascii() || domain.as_bytes().contains(&0) {
                return Err(NetworkConfigError::InvalidDnsOverride {
                    host: source,
                    reason: "domain must be a nonempty ASCII host name",
                });
            }
            DnsTarget::Domain(domain)
        } else if let Some(ip) = entry.ip {
            DnsTarget::Ipv4(ip)
        } else {
            return Err(NetworkConfigError::InvalidDnsOverride {
                host: source,
                reason: "set domain or ip",
            });
        };
        dns_overrides.push(DnsOverride { source, target });
    }

    if input.power_on_port == Some(0) {
        return Err(NetworkConfigError::InvalidPowerOnPort);
    }

    Ok(NetworkConfig {
        adapter: input.adapter,
        router_checks: input.router_checks,
        disable_maintenance_window: input.disable_maintenance_window,
        power_on_port: input.power_on_port.and_then(NonZeroU16::new),
        dns_overrides,
    })
}

fn reject_future_fields(config: &DisplayConfig) -> Result<(), DisplayConfigError> {
    for (field, is_set) in [
        ("monitor", config.monitor.is_some()),
        ("width", config.width.is_some()),
        ("height", config.height.is_some()),
        ("refresh-hz", config.refresh_hz.is_some()),
    ] {
        if is_set {
            return Err(DisplayConfigError::UnsupportedPhase2Field { field });
        }
    }
    Ok(())
}
