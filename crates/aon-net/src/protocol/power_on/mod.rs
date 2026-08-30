mod codec;
mod types;

pub use codec::{deserialize_power_on_request, serialize_power_on_response};
pub use types::{PowerOnRequest, PowerOnResponse, PowerOnTime};

#[cfg(test)]
use types::{FirmwareVersion, TextEncoding};

#[cfg(test)]
mod tests;
