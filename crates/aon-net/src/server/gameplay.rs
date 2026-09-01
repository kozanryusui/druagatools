use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

use super::limits::ConnectionGate;
use super::next_connection_id;
use super::transport::read_frame;
use super::{ServerError, SessionPhase};
use crate::online::{OnlineError, OnlineState, RelayBinding, RelayEnvelope, relay_channels};
use crate::protocol::station::{
    GameplayRequest, GameplayResponse, StationProtocolError, deserialize_gameplay_request,
};

#[derive(Debug, Error)]
enum GameplayConnectionError {
    #[error("gameplay connection I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("gameplay protocol failed: {0}")]
    Protocol(#[from] StationProtocolError),
    #[error("online party operation failed: {0}")]
    Online(#[from] OnlineError),
    #[error("Station returned session ID {actual}, expected {expected}")]
    SessionId { expected: u32, actual: u32 },
    #[error("Station sent request {request:?} in gameplay state {state}")]
    Unexpected {
        request: GameplayRequest,
        state: &'static str,
    },
    #[error("Station sent a gameplay request before endpoint registration")]
    RegistrationRequired,
    #[error("Station did not accept gameplay data within {timeout:?}")]
    DeliveryTimeout { timeout: Duration },
    #[error("Station sent no gameplay data for {timeout:?}")]
    InactivityTimeout { timeout: Duration },
}

pub(super) async fn serve_connections(
    listener: TcpListener,
    session_id: u32,
    online: Arc<OnlineState>,
    connections: ConnectionGate,
    relay_queue_capacity: NonZeroUsize,
    player_timeout: Duration,
) -> Result<(), ServerError> {
    loop {
        let accepted = connections
            .accept(&listener)
            .await
            .map_err(ServerError::ConnectionAccept)?;
        let (stream, peer, connection_permit) = accepted.into_parts();
        let connection_id = next_connection_id();
        info!(%peer, connection_id, service = "gameplay", "accepted Station connection");
        let online = Arc::clone(&online);
        tokio::spawn(async move {
            let _connection_permit = connection_permit;
            if let Err(error) = handle_connection(
                stream,
                peer,
                connection_id,
                session_id,
                relay_queue_capacity,
                player_timeout,
                &online,
            )
            .await
            {
                warn!(%peer, connection_id, service = "gameplay", %error, "Station connection ended with an error");
            } else {
                info!(%peer, connection_id, service = "gameplay", "Station disconnected");
            }
        });
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    connection_id: u64,
    configured_session_id: u32,
    relay_queue_capacity: NonZeroUsize,
    player_timeout: Duration,
    online: &OnlineState,
) -> Result<(), GameplayConnectionError> {
    stream.set_nodelay(true)?;
    let mut session_phase = SessionPhase::Identity;
    let mut binding: Option<RelayBinding> = None;
    let (relay, mut relay_rx) = relay_channels(relay_queue_capacity);
    let inactivity = tokio::time::sleep(player_timeout);
    tokio::pin!(inactivity);
    let result = async {
        loop {
            tokio::select! {
                biased;

                Some(reason) = relay_rx.disconnect.recv(), if binding.is_some() => {
                    if let Some(binding) = binding {
                        warn!(
                            %peer,
                            connection_id,
                            owner_key = %binding.owner_key(),
                            party_slot = %binding.party_slot(),
                            ?reason,
                            "disconnecting Station from gameplay relay"
                        );
                    }
                    break;
                }
                () = &mut inactivity => {
                    return Err(GameplayConnectionError::InactivityTimeout {
                        timeout: player_timeout,
                    });
                }
                Some(envelope) = relay_rx.envelope.recv(), if binding.is_some() => {
                    debug!(
                        connection_id,
                        input_records = envelope.records.len(),
                        flags = envelope.flags.bits(),
                        "pushed gameplay records to Station"
                    );
                    if write_relay_envelope(&mut stream, envelope, player_timeout).await? {
                        break;
                    }
                }
                input = read_frame(&mut stream) => {
                    let Some(input) = input? else {
                        break;
                    };
                    inactivity
                        .as_mut()
                        .reset(tokio::time::Instant::now() + player_timeout);
                    let request = deserialize_gameplay_request(&input)?;
                    match request {
                GameplayRequest::InitialIdentity { identity, reserved }
                    if session_phase == SessionPhase::Identity =>
                {
                    info!(%peer, connection_id, ?identity, reserved, "accepted Station gameplay identity");
                    write_response(
                        &mut stream,
                        GameplayResponse::InitialAccepted {
                            session_id: configured_session_id,
                        },
                        player_timeout,
                    )
                    .await?;
                    session_phase = SessionPhase::Confirmation;
                }
                GameplayRequest::SessionConfirm { session_id }
                    if session_phase == SessionPhase::Confirmation =>
                {
                    if session_id != configured_session_id {
                        return Err(GameplayConnectionError::SessionId {
                            expected: configured_session_id,
                            actual: session_id,
                        });
                    }
                    session_phase = SessionPhase::Confirmed;
                    write_response(
                        &mut stream,
                        GameplayResponse::SessionConfirmed { status: 0 },
                        player_timeout,
                    )
                    .await?;
                    debug!(%peer, connection_id, "confirmed Station gameplay session");
                }
                GameplayRequest::EndpointRegistration {
                    owner_key,
                    party_slot,
                    record_id,
                    ..
                }
                    if session_phase == SessionPhase::Confirmed && binding.is_none() =>
                {
                    match online.join_relay(
                        owner_key,
                        party_slot,
                        record_id,
                        connection_id,
                        relay.clone(),
                    ) {
                        Ok(join) => {
                            binding = Some(join.binding);
                            write_response(
                                &mut stream,
                                GameplayResponse::RegistrationResult { status: 0 },
                                player_timeout,
                            )
                            .await?;
                            info!(
                                %peer,
                                connection_id,
                                owner_key = %owner_key,
                                party_slot = %party_slot,
                                record_id,
                                active_flags = join.flags.bits(),
                                "Station joined gameplay party"
                            );
                        }
                        Err(error) => {
                            write_response(
                                &mut stream,
                                GameplayResponse::RegistrationResult { status: 1 },
                                player_timeout,
                            )
                            .await?;
                            return Err(error.into());
                        }
                    }
                }
                GameplayRequest::GameplayBlob { blob } => {
                    let binding = binding.ok_or(GameplayConnectionError::RegistrationRequired)?;
                    let outcome = online.relay_blob(binding, blob.clone())?;
                    if outcome.disconnected_players != 0 {
                        warn!(
                            owner_key = %binding.owner_key(),
                            party_slot = %binding.party_slot(),
                            disconnected_players = outcome.disconnected_players,
                            "disconnecting Stations with full gameplay relay queues"
                        );
                    }
                    debug!(
                        owner_key = %binding.owner_key(),
                        party_slot = %binding.party_slot(),
                        input_bytes = blob.as_bytes().len(),
                        output_records = outcome.response.records.len(),
                        flags = outcome.response.flags.bits(),
                        "relayed gameplay record"
                    );
                    let has_sole_survivor = write_relay_envelope(
                        &mut stream,
                        outcome.response,
                        player_timeout,
                    )
                    .await?;
                    if has_sole_survivor {
                        info!(
                            owner_key = %binding.owner_key(),
                            party_slot = %binding.party_slot(),
                            "closing gameplay relay for sole surviving Station"
                        );
                        break;
                    }
                }
                GameplayRequest::ActionRecord {
                    value_18,
                    value_1c,
                    value_20,
                    ..
                } => {
                    let binding = binding.ok_or(GameplayConnectionError::RegistrationRequired)?;
                    debug!(
                        owner_key = %binding.owner_key(),
                        party_slot = %binding.party_slot(),
                        action_18 = value_18,
                        action_1c = value_1c,
                        action_20 = value_20,
                        "accepted stand-alone Station action record"
                    );
                    write_response(
                        &mut stream,
                        GameplayResponse::ActionAccepted {},
                        player_timeout,
                    )
                    .await?;
                }
                request => {
                    return Err(GameplayConnectionError::Unexpected {
                        request,
                        state: if session_phase != SessionPhase::Confirmed {
                            session_phase.name()
                        } else if binding.is_none() {
                            "registration"
                        } else {
                            "active"
                        },
                    });
                }
                    }
                }
            }
        }
        Ok(())
    }
    .await;

    if let Some(binding) = binding {
        online.leave_relay(binding)?;
        info!(
            %peer,
            connection_id,
            owner_key = %binding.owner_key(),
            party_slot = %binding.party_slot(),
            "Station left gameplay party"
        );
    }
    result
}

async fn write_relay_envelope(
    stream: &mut TcpStream,
    envelope: RelayEnvelope,
    player_timeout: Duration,
) -> Result<bool, GameplayConnectionError> {
    let has_sole_survivor = envelope.flags.has_sole_survivor();
    write_response(
        stream,
        GameplayResponse::Envelope {
            flags: envelope.flags,
            records: envelope.records,
        },
        player_timeout,
    )
    .await?;
    Ok(has_sole_survivor)
}

async fn write_response(
    stream: &mut TcpStream,
    response: GameplayResponse,
    player_timeout: Duration,
) -> Result<(), GameplayConnectionError> {
    let frame = response.serialize()?;
    write_frame(stream, &frame, player_timeout).await
}

async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &[u8],
    player_timeout: Duration,
) -> Result<(), GameplayConnectionError> {
    tokio::time::timeout(player_timeout, writer.write_all(frame))
        .await
        .map_err(|_| GameplayConnectionError::DeliveryTimeout {
            timeout: player_timeout,
        })??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::station::EndpointHost;

    #[tokio::test]
    async fn gameplay_write_times_out_when_the_receiver_stalls() {
        let (mut writer, _reader) = tokio::io::duplex(1);

        let result = write_frame(&mut writer, &[0, 1], Duration::from_millis(10)).await;

        assert!(matches!(
            result,
            Err(GameplayConnectionError::DeliveryTimeout { .. })
        ));
    }

    #[tokio::test]
    async fn session_confirmation_requires_initial_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (client, accepted) = tokio::join!(TcpStream::connect(address), listener.accept());
        let mut client = client?;
        let (server, peer) = accepted?;
        let session_id = 0x1234_5678_u32;
        let mut request = vec![0x00, 0x03, 0x00, 0x04];
        request.extend_from_slice(&session_id.to_be_bytes());
        client.write_all(&request).await?;
        client.shutdown().await?;
        let online = OnlineState::new(EndpointHost::new("gameservers.aonnet".to_owned())?, 33442);

        let result = handle_connection(
            server,
            peer,
            1,
            session_id,
            NonZeroUsize::MIN,
            Duration::from_secs(10),
            &online,
        )
        .await;

        assert!(matches!(
            result,
            Err(GameplayConnectionError::Unexpected {
                state: "identity",
                ..
            })
        ));
        Ok(())
    }
}
