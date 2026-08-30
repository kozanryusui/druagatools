use std::error::Error;
use std::time::{SystemTime, UNIX_EPOCH};

use tower_hook::reader_protocol::{
    ReaderClientRequest, ReaderConfigurationSlot, ReaderIdentity, ReaderOperationStatus,
    ReaderProtocol, ReaderResponse, ReaderSide, ReaderStatusBits,
};

const IDENTIFY_REQUEST: [u8; 7] = [0x02, 0x83, 0, 0x03, 0x80, 0x0d, 0x0a];
const IDENTIFY_RESPONSE: [u8; 11] = [
    0x02, 0x83, 0x04, b'3', b'5', b'0', b'0', 0x03, 0x82, 0x0d, 0x0a,
];
const INITIALIZE_REQUEST: [u8; 7] = [0x02, 0x56, 0, 0x03, 0x55, 0x0d, 0x0a];
const INITIALIZE_RESPONSE: [u8; 9] = [0x02, 0x56, 0x02, 0, 0, 0x03, 0x57, 0x0d, 0x0a];
const CONFIGURE_REQUEST: [u8; 20] = [
    0x02, 0x50, 0x0d, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0x03, 0x5e, 0x0d, 0x0a,
];
const CONFIGURE_RESPONSE: [u8; 9] = [0x02, 0x50, 0x02, 0, 0, 0x03, 0x51, 0x0d, 0x0a];
const STATUS_REQUEST: [u8; 7] = [0x02, 0x80, 0, 0x03, 0x83, 0x0d, 0x0a];
const STATUS_RESPONSE: [u8; 8] = [0x02, 0x80, 0x01, 0, 0x03, 0x82, 0x0d, 0x0a];
const CONTROL_OPERATION_7_REQUEST: [u8; 12] = [
    0x02, 0x44, 0x05, 0x02, 0x01, 0x00, 0x00, 0x00, 0x03, 0x41, 0x0d, 0x0a,
];
const CONTROL_OPERATION_7_RESPONSE: [u8; 9] =
    [0x02, 0x44, 0x02, 0x00, 0x00, 0x03, 0x45, 0x0d, 0x0a];

#[test]
fn captured_reader_frames_deserialize_handle_and_serialize() -> Result<(), Box<dyn Error>> {
    let mut protocol = ReaderProtocol::new(ReaderSide::Left);

    let identify = ReaderClientRequest::deserialize(&IDENTIFY_REQUEST)?;
    assert!(matches!(&identify, ReaderClientRequest::Identify));
    let Some(identity_response) = protocol.handle(identify)? else {
        return Err("the identify request did not produce a typed response".into());
    };
    assert!(matches!(
        identity_response,
        ReaderResponse::Identity {
            identity: ReaderIdentity::Value35,
            count,
        } if count.value() == 0
    ));
    assert_eq!(identity_response.serialize(), IDENTIFY_RESPONSE);

    let initialize = ReaderClientRequest::deserialize(&INITIALIZE_REQUEST)?;
    assert!(matches!(&initialize, ReaderClientRequest::Initialize));
    let Some(initialize_response) = protocol.handle(initialize)? else {
        return Err("the initialize request did not produce a typed response".into());
    };
    assert!(matches!(
        initialize_response,
        ReaderResponse::Initialize {
            status: ReaderOperationStatus::Success,
            unknown_byte_4: 0,
        }
    ));
    assert_eq!(initialize_response.serialize(), INITIALIZE_RESPONSE);

    let configure = ReaderClientRequest::deserialize(&CONFIGURE_REQUEST)?;
    assert!(matches!(
        &configure,
        ReaderClientRequest::Configure {
            slot: ReaderConfigurationSlot::Slot00,
            unknown,
        }
        if *unknown == [0xff; 12]
    ));
    let Some(configure_response) = protocol.handle(configure)? else {
        return Err("the configure request did not produce a typed response".into());
    };
    assert!(matches!(
        configure_response,
        ReaderResponse::Configure {
            status: ReaderOperationStatus::Success,
            unknown_byte_4: 0,
        }
    ));
    assert_eq!(configure_response.serialize(), CONFIGURE_RESPONSE);

    let status = ReaderClientRequest::deserialize(&STATUS_REQUEST)?;
    assert!(matches!(&status, ReaderClientRequest::PollStatus));
    let Some(status_response) = protocol.handle(status)? else {
        return Err("the status request did not produce a typed response".into());
    };
    assert!(matches!(
        status_response,
        ReaderResponse::Status { bits } if bits == ReaderStatusBits::empty()
    ));
    assert_eq!(status_response.serialize(), STATUS_RESPONSE);
    Ok(())
}

#[test]
fn the_right_reader_uses_the_same_verified_response_contract() -> Result<(), Box<dyn Error>> {
    let request = ReaderClientRequest::deserialize(&IDENTIFY_REQUEST)?;
    let mut protocol = ReaderProtocol::new(ReaderSide::Right);
    assert_eq!(protocol.side(), ReaderSide::Right);
    let Some(response) = protocol.handle(request)? else {
        return Err("the right reader did not produce a typed response".into());
    };
    assert_eq!(response.serialize(), IDENTIFY_RESPONSE);
    Ok(())
}

#[test]
fn a_numbered_factory_card_completes_the_verified_mount_and_read_flow() -> Result<(), Box<dyn Error>>
{
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "druaga-reader-protocol-{}-{unique}",
        std::process::id()
    ));
    let mut protocol = ReaderProtocol::new(ReaderSide::Left);
    protocol.mount(&directory, 1)?;

    let status = protocol.handle(ReaderClientRequest::PollStatus)?;
    assert!(matches!(
        status,
        Some(ReaderResponse::Status { bits }) if bits.raw() == 0x01
    ));
    let start = protocol.handle(ReaderClientRequest::TransportStart { action: b'1' })?;
    assert!(matches!(
        start,
        Some(ReaderResponse::Transport {
            command: tower_hook::reader_protocol::ReaderCommandId::Command81,
            status: b'1'
        })
    ));
    let continuation = protocol.handle(ReaderClientRequest::TransportContinue { action: b'1' })?;
    assert!(matches!(
        continuation,
        Some(ReaderResponse::Transport {
            command: tower_hook::reader_protocol::ReaderCommandId::Command82,
            status: b'1'
        })
    ));
    let status = protocol.handle(ReaderClientRequest::PollStatus)?;
    assert!(matches!(
        status,
        Some(ReaderResponse::Status { bits }) if bits.raw() == 0x02
    ));

    let response = protocol.handle(ReaderClientRequest::Read48 { block_index: 0 })?;
    let Some(ReaderResponse::Read48 { status, data, .. }) = response else {
        return Err("the mounted card did not return its first 48-byte block".into());
    };
    assert_eq!(status, ReaderOperationStatus::Success);
    assert_eq!(
        data,
        [
            0x44, 0x52, 0x55, 0x41, 0x47, 0x41, 0x2d, 0x43, 0x41, 0x52, 0x44, 0x2d, 0x30, 0x30,
            0x30, 0x31, 0x10, 0x01, 0x56, 0x33, 0x32, 0x34, 0x30, 0x31, 0x38, 0x5e, 0x98, 0x78,
            0xa1, 0xfa, 0x9d, 0x82, 0xff, 0xff, 0xff, 0x7f, 0x00, 0x00, 0x00, 0x80, 0xff, 0xff,
            0xff, 0x7f, 0x00, 0x00, 0x00, 0x00,
        ]
    );

    let control = ReaderClientRequest::deserialize(&CONTROL_OPERATION_7_REQUEST)?;
    assert!(matches!(
        control,
        ReaderClientRequest::ControlOperation7 {
            selector: 2,
            value: 1
        }
    ));
    let Some(response) = protocol.handle(control)? else {
        return Err("control operation 7 did not produce a response".into());
    };
    assert!(matches!(
        response,
        ReaderResponse::Operation {
            command: tower_hook::reader_protocol::ReaderCommandId::Command44,
            status: ReaderOperationStatus::Success,
            reserved: 0
        }
    ));
    assert_eq!(response.serialize(), CONTROL_OPERATION_7_RESPONSE);
    let Some(ReaderResponse::Read48 { data, .. }) =
        protocol.handle(ReaderClientRequest::Read48 { block_index: 0 })?
    else {
        return Err("the committed card header could not be read".into());
    };
    assert_eq!(
        u32::from_le_bytes(data[0x20..0x24].try_into()?),
        0x7fff_fffe
    );
    assert_eq!(
        u32::from_le_bytes(data[0x24..0x28].try_into()?),
        0x8000_0001
    );
    assert_eq!(
        u32::from_le_bytes(data[0x28..0x2c].try_into()?),
        0x7fff_fffe
    );
    std::fs::remove_dir_all(directory)?;
    Ok(())
}
