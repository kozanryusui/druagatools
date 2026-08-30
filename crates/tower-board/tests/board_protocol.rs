use std::error::Error;

use druaga_tower_board::{
    BoardClientRequest, BoardExchangeMode, BoardProtocol, BoardResponse, BoardResponseHeader,
    OperatorAction, OperatorInputEvent, OperatorInputState, ProtocolState,
};

const STARTUP: [u8; 12] = [0x81, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01];
const FIRST_EXCHANGE: [u8; 12] = [0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
const STEADY_EXCHANGE: [u8; 12] = [
    0x80, 0x38, 0, 0x04, 0x06, 0x04, 0x06, 0x04, 0x05, 0x04, 0x05, 0x5e,
];
const ACCEPTED_RESPONSE: [u8; 8] = [0x80, 0x0f, 0x01, 0, 0, 0, 0, 0x10];

#[test]
fn compatibility_response_releases_operator_lines() {
    assert_eq!(
        BoardResponse::compatibility().serialize(),
        ACCEPTED_RESPONSE
    );
}

#[test]
fn captured_board_frames_deserialize_handle_and_serialize() -> Result<(), Box<dyn Error>> {
    let mut protocol = BoardProtocol::new();

    for raw in [STARTUP, STARTUP, STARTUP] {
        let request = BoardClientRequest::deserialize(&raw)?;
        assert!(matches!(
            request,
            BoardClientRequest::Startup { unknown } if unknown == [0; 10]
        ));
        assert_eq!(protocol.handle(request)?, None);
    }

    let first_request = BoardClientRequest::deserialize(&FIRST_EXCHANGE)?;
    assert!(matches!(
        first_request,
        BoardClientRequest::Exchange {
            mode: BoardExchangeMode::Initialization,
            reserved: 0,
            unknown_output,
        } if unknown_output == [0; 8]
    ));
    let Some(first_response) = protocol.handle(first_request)? else {
        return Err("the first exchange did not produce a typed response".into());
    };
    assert!(matches!(first_response, BoardResponse::Status { .. }));
    assert_eq!(first_response.serialize(), ACCEPTED_RESPONSE);
    protocol.accept_delivered_response(first_response)?;

    let steady_request = BoardClientRequest::deserialize(&STEADY_EXCHANGE)?;
    assert!(matches!(
        steady_request,
        BoardClientRequest::Exchange {
            mode: BoardExchangeMode::Value38,
            reserved: 0,
            unknown_output: [0x04, 0x06, 0x04, 0x06, 0x04, 0x05, 0x04, 0x05],
        }
    ));
    let Some(steady_response) = protocol.handle(steady_request)? else {
        return Err("the steady exchange did not produce a typed response".into());
    };
    assert_eq!(steady_response.serialize(), ACCEPTED_RESPONSE);
    protocol.accept_delivered_response(steady_response)?;
    assert_eq!(protocol.state(), ProtocolState::Ready);
    Ok(())
}

#[test]
fn startup_reply_stays_frozen_until_the_protocol_is_ready() -> Result<(), Box<dyn Error>> {
    let mut protocol = BoardProtocol::new();
    for raw in [STARTUP, STARTUP, STARTUP] {
        assert_eq!(
            protocol.handle(BoardClientRequest::deserialize(&raw)?)?,
            None
        );
    }

    let first = protocol
        .handle(BoardClientRequest::deserialize(&FIRST_EXCHANGE)?)?
        .ok_or("the first exchange did not produce a typed response")?;
    protocol.accept_delivered_response(first)?;
    protocol.apply_operator_event(OperatorInputEvent::new(
        OperatorAction::SelectUp,
        OperatorInputState::Pressed,
    ));

    let matching = protocol
        .handle(BoardClientRequest::deserialize(&STEADY_EXCHANGE)?)?
        .ok_or("the matching exchange did not produce a typed response")?;
    assert_eq!(matching, first);
    protocol.accept_delivered_response(matching)?;
    assert_eq!(protocol.state(), ProtocolState::Ready);

    let ready = protocol
        .handle(BoardClientRequest::deserialize(&STEADY_EXCHANGE)?)?
        .ok_or("the ready exchange did not produce a typed response")?;
    assert_eq!(ready.serialize(), [0x80, 0x0e, 0x01, 0, 0, 0, 0, 0x0f]);
    Ok(())
}

#[test]
fn injected_operator_events_produce_typed_exact_responses() -> Result<(), Box<dyn Error>> {
    let mut protocol = ready_protocol()?;

    let select_up = exchange_after_event(
        &mut protocol,
        OperatorInputEvent::new(OperatorAction::SelectUp, OperatorInputState::Pressed),
    )?;
    assert!(matches!(
        select_up,
        BoardResponse::Status {
            header: BoardResponseHeader::Accepted,
            select_up: OperatorInputState::Pressed,
            select_down: OperatorInputState::Released,
            test: OperatorInputState::Released,
            enter: OperatorInputState::Released,
            service: OperatorInputState::Released,
            coin_counter: 0,
            unknown_bytes_5_to_6: [0, 0],
        }
    ));
    assert_eq!(select_up.serialize(), [0x80, 0x0e, 0x01, 0, 0, 0, 0, 0x0f]);

    let released = exchange_after_event(
        &mut protocol,
        OperatorInputEvent::new(OperatorAction::SelectUp, OperatorInputState::Released),
    )?;
    assert_eq!(released.serialize(), ACCEPTED_RESPONSE);

    let select_down = exchange_after_event(
        &mut protocol,
        OperatorInputEvent::new(OperatorAction::SelectDown, OperatorInputState::Pressed),
    )?;
    assert_eq!(
        select_down.serialize(),
        [0x80, 0x0d, 0x01, 0, 0, 0, 0, 0x0e]
    );
    let released = exchange_after_event(
        &mut protocol,
        OperatorInputEvent::new(OperatorAction::SelectDown, OperatorInputState::Released),
    )?;
    assert_eq!(released.serialize(), ACCEPTED_RESPONSE);

    let test = exchange_after_event(
        &mut protocol,
        OperatorInputEvent::new(OperatorAction::Test, OperatorInputState::Pressed),
    )?;
    assert_eq!(test.serialize(), [0x80, 0x0b, 0x01, 0, 0, 0, 0, 0x0c]);
    let released = exchange_after_event(
        &mut protocol,
        OperatorInputEvent::new(OperatorAction::Test, OperatorInputState::Released),
    )?;
    assert_eq!(released.serialize(), ACCEPTED_RESPONSE);

    let enter = exchange_after_event(
        &mut protocol,
        OperatorInputEvent::new(OperatorAction::Enter, OperatorInputState::Pressed),
    )?;
    assert_eq!(enter.serialize(), [0x80, 0x07, 0x01, 0, 0, 0, 0, 0x08]);
    let released = exchange_after_event(
        &mut protocol,
        OperatorInputEvent::new(OperatorAction::Enter, OperatorInputState::Released),
    )?;
    assert_eq!(released.serialize(), ACCEPTED_RESPONSE);

    let service = exchange_after_event(
        &mut protocol,
        OperatorInputEvent::new(OperatorAction::Service, OperatorInputState::Pressed),
    )?;
    assert_eq!(service.serialize(), [0x80, 0x0f, 0, 0, 0, 0, 0, 0x0f]);
    let released = exchange_after_event(
        &mut protocol,
        OperatorInputEvent::new(OperatorAction::Service, OperatorInputState::Released),
    )?;
    assert_eq!(released.serialize(), ACCEPTED_RESPONSE);

    let coin = exchange_after_event(
        &mut protocol,
        OperatorInputEvent::new(OperatorAction::Coin, OperatorInputState::Pressed),
    )?;
    assert!(matches!(
        coin,
        BoardResponse::Status {
            select_up: OperatorInputState::Released,
            select_down: OperatorInputState::Released,
            test: OperatorInputState::Released,
            enter: OperatorInputState::Released,
            service: OperatorInputState::Released,
            coin_counter: 1,
            ..
        }
    ));
    assert_eq!(coin.serialize(), [0x80, 0x0f, 0x01, 0x01, 0, 0, 0, 0x11]);
    let coin_release = exchange_after_event(
        &mut protocol,
        OperatorInputEvent::new(OperatorAction::Coin, OperatorInputState::Released),
    )?;
    assert_eq!(coin_release.serialize(), coin.serialize());

    let held_counter = protocol
        .handle(BoardClientRequest::deserialize(&STEADY_EXCHANGE)?)?
        .ok_or("the steady exchange did not produce a typed response")?;
    assert_eq!(held_counter.serialize(), coin.serialize());
    Ok(())
}

fn ready_protocol() -> Result<BoardProtocol, Box<dyn Error>> {
    let mut protocol = BoardProtocol::new();
    for raw in [STARTUP, STARTUP, STARTUP] {
        let request = BoardClientRequest::deserialize(&raw)?;
        if protocol.handle(request)?.is_some() {
            return Err("a startup write produced an unexpected response".into());
        }
    }
    let first = protocol
        .handle(BoardClientRequest::deserialize(&FIRST_EXCHANGE)?)?
        .ok_or("the first exchange did not produce a typed response")?;
    protocol.accept_delivered_response(first)?;
    let matching = protocol
        .handle(BoardClientRequest::deserialize(&STEADY_EXCHANGE)?)?
        .ok_or("the matching exchange did not produce a typed response")?;
    protocol.accept_delivered_response(matching)?;
    Ok(protocol)
}

fn exchange_after_event(
    protocol: &mut BoardProtocol,
    event: OperatorInputEvent,
) -> Result<BoardResponse, Box<dyn Error>> {
    protocol.apply_operator_event(event);
    let response = protocol
        .handle(BoardClientRequest::deserialize(&STEADY_EXCHANGE)?)?
        .ok_or("the event exchange did not produce a typed response")?;
    protocol.accept_delivered_response(response)?;
    Ok(response)
}
