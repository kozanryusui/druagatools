use std::net::Ipv4Addr;

use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PowerOnRequest {
    pub game_id: String,
    pub game_version: String,
    pub serial: String,
    pub address: Ipv4Addr,
    pub firmware_version: FirmwareVersion,
    pub boot_version: FirmwareVersion,
    pub encoding: TextEncoding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FirmwareVersion {
    pub major: u8,
    pub minor: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextEncoding {
    ShiftJis,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PowerOnResponse {
    pub status: i32,
    pub uri: String,
    pub host: String,
    pub shop_name: String,
    pub shop_nickname: String,
    pub region_code: String,
    pub region_name_0: String,
    pub region_name_1: String,
    pub region_name_2: String,
    pub region_name_3: String,
    pub place_id: String,
    pub setting: String,
    pub time: PowerOnTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerOnTime {
    pub year: i16,
    pub month: i8,
    pub day: i8,
    pub hour: i8,
    pub minute: i8,
    pub second: i8,
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("PowerOn body is not valid Base64: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("PowerOn zlib data is invalid: {0}")]
    Zlib(std::io::Error),
    #[error("PowerOn request is larger than the supported limit")]
    RequestTooLarge,
    #[error("PowerOn request text is not ASCII")]
    RequestText,
    #[error("PowerOn request field {0} is missing")]
    MissingField(&'static str),
    #[error("PowerOn request field {0} occurs more than once")]
    DuplicateField(String),
    #[error("PowerOn request field {0} is not supported")]
    UnknownField(String),
    #[error("PowerOn request field {field} has an invalid value: {value}")]
    InvalidField { field: &'static str, value: String },
    #[error("PowerOn response contains text that Shift_JIS cannot encode")]
    ResponseEncoding,
    #[error("PowerOn response field {0} contains a reserved character")]
    InvalidResponseField(&'static str),
}
