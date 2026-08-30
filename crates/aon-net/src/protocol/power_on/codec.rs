use std::collections::HashMap;
use std::io::{Read, Write};

use super::types::{FirmwareVersion, PowerOnRequest, PowerOnResponse, ProtocolError, TextEncoding};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use encoding_rs::SHIFT_JIS;
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

const MAX_DECOMPRESSED_REQUEST_SIZE: u64 = 4096;

pub fn deserialize_power_on_request(encoded_body: &[u8]) -> Result<PowerOnRequest, ProtocolError> {
    let compressed = STANDARD.decode(encoded_body)?;
    let mut decoder = ZlibDecoder::new(compressed.as_slice());
    let mut plain = Vec::new();
    decoder
        .by_ref()
        .take(MAX_DECOMPRESSED_REQUEST_SIZE + 1)
        .read_to_end(&mut plain)
        .map_err(ProtocolError::Zlib)?;
    if plain.len() as u64 > MAX_DECOMPRESSED_REQUEST_SIZE {
        return Err(ProtocolError::RequestTooLarge);
    }
    let text = std::str::from_utf8(&plain).map_err(|_| ProtocolError::RequestText)?;
    parse_power_on_form(text.strip_suffix("\r\n").unwrap_or(text))
}

pub fn serialize_power_on_response(response: &PowerOnResponse) -> Result<Vec<u8>, ProtocolError> {
    let plain = serialize_power_on_fields(response)?;
    let (encoded_text, _, had_errors) = SHIFT_JIS.encode(&plain);
    if had_errors {
        return Err(ProtocolError::ResponseEncoding);
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(encoded_text.as_ref())
        .map_err(ProtocolError::Zlib)?;
    let compressed = encoder.finish().map_err(ProtocolError::Zlib)?;
    let mut body = STANDARD.encode(compressed).into_bytes();
    body.extend_from_slice(b"\r\n");
    Ok(body)
}

fn parse_power_on_form(text: &str) -> Result<PowerOnRequest, ProtocolError> {
    let mut fields = HashMap::new();
    for pair in text.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(ProtocolError::InvalidField {
                field: "form",
                value: pair.to_owned(),
            });
        };
        if !matches!(
            key,
            "game_id" | "ver" | "serial" | "ip" | "firm_ver" | "boot_ver" | "encode"
        ) {
            return Err(ProtocolError::UnknownField(key.to_owned()));
        }
        if fields.insert(key, value).is_some() {
            return Err(ProtocolError::DuplicateField(key.to_owned()));
        }
    }

    let game_id = required(&fields, "game_id")?.to_owned();
    let game_version = required(&fields, "ver")?.to_owned();
    let serial = required(&fields, "serial")?.to_owned();
    let address_text = required(&fields, "ip")?;
    let address = address_text
        .parse()
        .map_err(|_| ProtocolError::InvalidField {
            field: "ip",
            value: address_text.to_owned(),
        })?;
    let firmware_version = parse_firmware_version(required(&fields, "firm_ver")?, "firm_ver")?;
    let boot_version = parse_firmware_version(required(&fields, "boot_ver")?, "boot_ver")?;
    let encoding = match required(&fields, "encode")? {
        "Shift_JIS" => TextEncoding::ShiftJis,
        value => {
            return Err(ProtocolError::InvalidField {
                field: "encode",
                value: (*value).to_owned(),
            });
        }
    };

    Ok(PowerOnRequest {
        game_id,
        game_version,
        serial,
        address,
        firmware_version,
        boot_version,
        encoding,
    })
}

fn required<'a>(
    fields: &'a HashMap<&str, &str>,
    name: &'static str,
) -> Result<&'a str, ProtocolError> {
    fields
        .get(name)
        .copied()
        .ok_or(ProtocolError::MissingField(name))
}

fn parse_firmware_version(
    value: &str,
    field: &'static str,
) -> Result<FirmwareVersion, ProtocolError> {
    if value.len() != 4 || !value.is_ascii() {
        return Err(ProtocolError::InvalidField {
            field,
            value: value.to_owned(),
        });
    }
    let major = u8::from_str_radix(&value[0..2], 16).map_err(|_| ProtocolError::InvalidField {
        field,
        value: value.to_owned(),
    })?;
    let minor = u8::from_str_radix(&value[2..4], 16).map_err(|_| ProtocolError::InvalidField {
        field,
        value: value.to_owned(),
    })?;
    Ok(FirmwareVersion { major, minor })
}

fn serialize_power_on_fields(response: &PowerOnResponse) -> Result<String, ProtocolError> {
    let fields = [
        ("uri", response.uri.as_str()),
        ("host", response.host.as_str()),
        ("name", response.shop_name.as_str()),
        ("nickname", response.shop_nickname.as_str()),
        ("region0", response.region_code.as_str()),
        ("region_name0", response.region_name_0.as_str()),
        ("region_name1", response.region_name_1.as_str()),
        ("region_name2", response.region_name_2.as_str()),
        ("region_name3", response.region_name_3.as_str()),
        ("place_id", response.place_id.as_str()),
        ("setting", response.setting.as_str()),
    ];
    for (name, value) in fields {
        if value.contains(['&', '\0', '\r', '\n']) {
            return Err(ProtocolError::InvalidResponseField(name));
        }
    }

    Ok([
        format!("stat={}", response.status),
        format!("uri={}", response.uri),
        format!("host={}", response.host),
        format!("name={}", response.shop_name),
        format!("nickname={}", response.shop_nickname),
        format!("region0={}", response.region_code),
        format!("region_name0={}", response.region_name_0),
        format!("region_name1={}", response.region_name_1),
        format!("region_name2={}", response.region_name_2),
        format!("region_name3={}", response.region_name_3),
        format!("place_id={}", response.place_id),
        format!("setting={}", response.setting),
        format!("year={}", response.time.year),
        format!("month={}", response.time.month),
        format!("day={}", response.time.day),
        format!("hour={}", response.time.hour),
        format!("minute={}", response.time.minute),
        format!("second={}", response.time.second),
    ]
    .join("&"))
}
