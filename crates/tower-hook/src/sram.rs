//! Fixed file-backed storage for the Tower SRAM address space.

mod storage;
#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub(crate) use windows::queue_hooks;
