//! Minimal Sentinel SuperPro compatibility library for the Druaga Tower.
//!
//! This library implements only the six functions that `v324ct.exe` imports.
//! It supplies the values that the observed Tower license worker requires.

mod logging;
mod sentinel;

#[cfg(windows)]
mod bootstrap;

#[cfg(windows)]
fn bootstrap_succeeded() -> bool {
    bootstrap::succeeded()
}

#[cfg(not(windows))]
fn bootstrap_succeeded() -> bool {
    true
}
