use std::net::SocketAddr;
use std::sync::Arc;

use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

use super::next_connection_id;
use super::transport::read_frame;
use super::{ServerError, SessionPhase};
use crate::online::{OnlineError, OnlineState};
use crate::protocol::station::{
    GameplayRequest, GameplayResponse, OwnerKey, PartySlot, StationProtocolError,
    deserialize_gameplay_request,
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
}

#[derive(Clone, Copy)]
struct RelayBinding {
    owner_key: OwnerKey,
    party_slot: PartySlot,
}

pub(super) async fn serve_connections(
    listener: TcpListener,
    session_id: u32,
    online: Arc<OnlineState>,
) -> Result<(), ServerError> {
    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .map_err(ServerError::ConnectionAccept)?;
        let connection_id = next_connection_id();
        info!(%peer, connection_id, service = "gameplay", "accepted Station connection");
        let online = Arc::clone(&online);
        tokio::spawn(async move {
            if let Err(error) =
                handle_connection(stream, peer, connection_id, session_id, &online).await
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
    online: &OnlineState,
) -> Result<(), GameplayConnectionError> {
    let mut session_phase = SessionPhase::Identity;
    let mut binding = None;
    let result = async {
        while let Some(input) = read_frame(&mut stream).await? {
            let request = deserialize_gameplay_request(&input)?;
            match request {
                GameplayRequest::InitialIdentity { identity, reserved }
                    if session_phase == SessionPhase::Identity =>
                {
                    info!(%peer, connection_id, ?identity, reserved, "accepted Station gameplay identity");
                    stream
                        .write_all(&GameplayResponse::InitialAccepted {
                            session_id: configured_session_id,
                        }.serialize()?)
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
                    stream
                        .write_all(&GameplayResponse::SessionConfirmed { status: 0 }.serialize()?)
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
                    ) {
                        Ok(flags) => {
                            binding = Some(RelayBinding {
                                owner_key,
                                party_slot,
                            });
                            stream
                                .write_all(&GameplayResponse::RegistrationResult { status: 0 }.serialize()?)
                                .await?;
                            info!(
                                %peer,
                                connection_id,
                                owner_key = %owner_key,
                                party_slot = %party_slot,
                                record_id,
                                active_flags = flags,
                                "Station joined gameplay party"
                            );
                        }
                        Err(error) => {
                            stream
                                .write_all(&GameplayResponse::RegistrationResult { status: 1 }.serialize()?)
                                .await?;
                            return Err(error.into());
                        }
                    }
                }
                GameplayRequest::GameplayBlob { blob } => {
                    let binding = binding.ok_or(GameplayConnectionError::RegistrationRequired)?;
                    let batch = online.relay_blob(
                        binding.owner_key,
                        binding.party_slot,
                        connection_id,
                        blob.clone(),
                    )?;
                    if batch.dropped_records != 0 {
                        warn!(
                            owner_key = %binding.owner_key,
                            party_slot = %binding.party_slot,
                            dropped_records = batch.dropped_records,
                            "gameplay relay queue was full"
                        );
                    }
                    debug!(
                        owner_key = %binding.owner_key,
                        party_slot = %binding.party_slot,
                        input_bytes = blob.as_bytes().len(),
                        output_records = batch.records.len(),
                        flags = batch.flags,
                        "relayed gameplay record"
                    );
                    let leaves_one_player = batch.leaves_one_player();
                    stream
                        .write_all(
                            &GameplayResponse::Envelope {
                                flags: batch.flags,
                                records: batch.records,
                            }
                            .serialize()?,
                        )
                        .await?;
                    if leaves_one_player {
                        info!(
                            owner_key = %binding.owner_key,
                            party_slot = %binding.party_slot,
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
                        owner_key = %binding.owner_key,
                        party_slot = %binding.party_slot,
                        action_18 = value_18,
                        action_1c = value_1c,
                        action_20 = value_20,
                        "accepted stand-alone Station action record"
                    );
                    stream
                .write_all(&GameplayResponse::ActionAccepted {}.serialize()?)
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
        Ok(())
    }
    .await;

    if let Some(binding) = binding {
        online.leave_relay(binding.owner_key, binding.party_slot, connection_id)?;
        info!(
            %peer,
            connection_id,
            owner_key = %binding.owner_key,
            party_slot = %binding.party_slot,
            "Station left gameplay party"
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::station::EndpointHost;

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

        let result = handle_connection(server, peer, 1, session_id, &online).await;

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
