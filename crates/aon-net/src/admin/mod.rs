pub mod contract;
pub mod routes;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod backend;

#[cfg(target_arch = "wasm32")]
pub mod ui;
