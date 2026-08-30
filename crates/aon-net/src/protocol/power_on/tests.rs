use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use flate2::read::ZlibDecoder;
use std::io::Read;
use std::net::Ipv4Addr;

use super::*;

#[test]
fn power_on_response_has_required_tower_fields() -> Result<(), Box<dyn std::error::Error>> {
    let response = PowerOnResponse {
        status: 0,
        uri: "/".to_owned(),
        host: "localhost".to_owned(),
        shop_name: "AON.Net Local Shop".to_owned(),
        shop_nickname: "AON".to_owned(),
        region_code: "0".to_owned(),
        region_name_0: "Local".to_owned(),
        region_name_1: "Local".to_owned(),
        region_name_2: "Local".to_owned(),
        region_name_3: "Local".to_owned(),
        place_id: "AON0001".to_owned(),
        setting: String::new(),
        time: PowerOnTime {
            year: 2026,
            month: 8,
            day: 12,
            hour: 1,
            minute: 2,
            second: 3,
        },
    };

    let encoded = serialize_power_on_response(&response)?;
    assert!(encoded.ends_with(b"\r\n"));
    let compressed = STANDARD.decode(&encoded[..encoded.len() - 2])?;
    let mut plain = String::new();
    ZlibDecoder::new(compressed.as_slice()).read_to_string(&mut plain)?;

    assert_eq!(
        plain,
        "stat=0&uri=/&host=localhost&name=AON.Net Local Shop&nickname=AON&region0=0&region_name0=Local&region_name1=Local&region_name2=Local&region_name3=Local&place_id=AON0001&setting=&year=2026&month=8&day=12&hour=1&minute=2&second=3"
    );
    Ok(())
}

#[test]
fn power_on_request_deserializes_to_typed_variant() -> Result<(), Box<dyn std::error::Error>> {
    // This body was captured from Tower 1.60 during a live PowerOn request.
    let encoded = b"eJxLT8xNjc9MsQ128glRK0stsjXUMzNQK04tykzMsXV2dPIzNDYAAkO1zAJbQyNzPQMgNFRLyyzKjQepNjA0MFBLys8vgfCAQC01Lzk/JdU2OCMzrSTeyzOYlwsA8n0eFw==";

    let request = deserialize_power_on_request(encoded)?;

    assert_eq!(
        request,
        PowerOnRequest {
            game_id: "SBLT".to_owned(),
            game_version: "1.60".to_owned(),
            serial: "CABN1300001".to_owned(),
            address: Ipv4Addr::LOCALHOST,
            firmware_version: FirmwareVersion { major: 1, minor: 0 },
            boot_version: FirmwareVersion { major: 0, minor: 0 },
            encoding: TextEncoding::ShiftJis,
        }
    );
    Ok(())
}
