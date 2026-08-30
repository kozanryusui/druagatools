//! Local replacement for the retired Druaga Online network services.

pub mod admin;

#[cfg(not(target_arch = "wasm32"))]
mod config;
#[cfg(not(target_arch = "wasm32"))]
mod logging;
#[cfg(not(target_arch = "wasm32"))]
mod online;
#[cfg(not(target_arch = "wasm32"))]
mod protocol;
#[cfg(not(target_arch = "wasm32"))]
mod runtime_settings;
#[cfg(not(target_arch = "wasm32"))]
mod server;
#[cfg(not(target_arch = "wasm32"))]
mod storage;

#[cfg(not(target_arch = "wasm32"))]
pub use config::{AonNetConfig, ConfigError, load_config};
#[cfg(not(target_arch = "wasm32"))]
pub use logging::AdminHub;
#[cfg(not(target_arch = "wasm32"))]
pub use server::{ServerError, serve};
#[cfg(not(target_arch = "wasm32"))]
pub use storage::StorageError;
