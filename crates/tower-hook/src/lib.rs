//! Tower 1.60 compatibility hook.

#[path = "serial/card.rs"]
pub mod card;
#[cfg(windows)]
mod card_usage;
#[cfg(windows)]
mod config;
#[cfg(windows)]
mod d3d9;
#[cfg(windows)]
mod hook;
#[cfg(windows)]
mod maintenance;
#[cfg(windows)]
mod network;
#[cfg(windows)]
mod platform;
#[path = "serial/reader_protocol.rs"]
pub mod reader_protocol;
#[cfg(windows)]
mod serial;
#[cfg(windows)]
mod sram;
#[cfg(windows)]
mod startup;
#[cfg(windows)]
mod window;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HookFailure {
    #[error("the Tower executable image is not available")]
    Image,
    #[error("the tower-hook module path is not available")]
    ModulePath,
    #[error("configuration failed: {0}")]
    Config(String),
    #[error("hook preparation failed for {hook}: {detail}")]
    HookPrepare { hook: &'static str, detail: String },
    #[error("hook installation failed for {hook}: {detail}")]
    HookCommit { hook: &'static str, detail: String },
    #[error("runtime state was already set: {0}")]
    RuntimeState(&'static str),
}

#[cfg(windows)]
fn initialize() -> Result<(), HookFailure> {
    let config_path = platform::tower_hook_config_path()?;
    let overrides = config::DisplayOverrides::from_environment()
        .map_err(|error| HookFailure::Config(error.to_string()))?;
    let (display, startup, network_config, card_config, serial_logging) =
        config::load_config(&config_path, &overrides)
            .map_err(|error| HookFailure::Config(error.to_string()))?;
    let image = platform::tower_image()?;

    let disable_maintenance_window = network_config.disable_maintenance_window;
    network::configure(network_config)?;
    let reset_usage_count = card_config.reset_usage_count;
    serial::configure(card_config.directory, serial_logging)?;
    let mut installer = hook::HookInstaller::new(image);
    card_usage::queue_hook(&mut installer, reset_usage_count)?;
    window::queue_hooks(&mut installer)?;
    d3d9::queue_hook(&mut installer, display.mode)?;
    maintenance::queue_hook(&mut installer, disable_maintenance_window)?;
    network::queue_hooks(&mut installer)?;
    serial::queue_hooks(&mut installer)?;
    sram::queue_hooks(&mut installer)?;
    startup::queue_hooks(&mut installer, startup.skip_notice, startup.skip_logos)?;
    installer.commit()
}

#[cfg(windows)]
#[unsafe(no_mangle)]
pub extern "system" fn druaga_tower_hook_initialize() -> i16 {
    platform::log("hook-initializer-entered");
    if std::env::var_os("DRUAGA_TOWER_HOOK_REJECT_INITIALIZER")
        .is_some_and(|value| value.eq_ignore_ascii_case("on"))
    {
        platform::log("hook-initializer-rejected");
        return 1;
    }
    match initialize() {
        Ok(()) => {
            platform::log("hook-initializer-complete");
            0
        }
        Err(error) => {
            platform::log(&format!("hook-initializer-failed error={error}"));
            1
        }
    }
}
