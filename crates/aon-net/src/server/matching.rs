use std::net::SocketAddr;
use std::sync::Arc;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::central::{CentralServiceError, CentralServices};
use super::next_connection_id;
use super::transport::read_frame;
use super::{ServerError, SessionPhase};
use crate::online::{MatchOutcome, OnlineError, OnlineState};
use crate::protocol::station::{
    LobbyRegistration, MatchingRequest, MatchingResponse, StationProtocolError,
    deserialize_matching_request,
};
use crate::protocol::tower::{TowerProtocolError, TowerRequest};

#[derive(Debug, Error)]
enum MatchingConnectionError {
    #[error("matching connection I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("central matching protocol failed: {0}")]
    TowerProtocol(#[from] TowerProtocolError),
    #[error("central matching request failed: {0}")]
    Central(#[from] CentralServiceError),
    #[error("Station matching protocol failed: {0}")]
    StationProtocol(#[from] StationProtocolError),
    #[error("online matching operation failed: {0}")]
    Online(#[from] OnlineError),
    #[error("Station sent request {request:?} in matching session state {state}")]
    Unexpected {
        request: MatchingRequest,
        state: &'static str,
    },
    #[error("Station sent lobby lookup before lobby registration")]
    Registration,
}

pub(super) async fn serve_connections(
    listener: TcpListener,
    central: Arc<CentralServices>,
    online: Arc<OnlineState>,
) -> Result<(), ServerError> {
    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .map_err(ServerError::ConnectionAccept)?;
        let connection_id = next_connection_id();
        info!(%peer, connection_id, service = "matching", "accepted matching connection");
        let central = Arc::clone(&central);
        let online = Arc::clone(&online);
        tokio::spawn(async move {
            if let Err(error) =
                handle_connection(stream, peer, connection_id, &central, &online).await
            {
                warn!(%peer, connection_id, service = "matching", %error, "matching connection ended with an error");
            } else {
                info!(%peer, connection_id, service = "matching", "matching connection closed");
            }
        });
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    connection_id: u64,
    central: &CentralServices,
    online: &OnlineState,
) -> Result<(), MatchingConnectionError> {
    let (assignment_tx, mut assignment_rx) = mpsc::unbounded_channel();
    let mut session_phase = SessionPhase::Identity;
    let mut registration: Option<LobbyRegistration> = None;
    let mut endpoint_assigned = false;
    let result = async {
        loop {
            tokio::select! {
                biased;
                assignment = assignment_rx.recv() => {
                    if let Some(assignment) = assignment {
                        stream
                            .write_all(&MatchingResponse::EndpointAssignment(assignment).serialize()?)
                            .await?;
                        info!(%peer, connection_id, "sent Station gameplay endpoint assignment");
                        endpoint_assigned = true;
                    }
                }
				input = read_frame(&mut stream) => {
                    let Some(input) = input? else { break };
					let message_type = u16::from_be_bytes([input[0], input[1]]);
					debug!(
						%peer,
						connection_id,
						message_type = format_args!("0x{message_type:04X}"),
						payload = %hex_payload(&input[4..]),
						"received Station matching frame"
					);
                    let request = deserialize_matching_request(&input)?;
                    match request {
                        MatchingRequest::Central(request)
                            if matches!(
                                (&request, session_phase),
                                (TowerRequest::InitialIdentity { .. }, SessionPhase::Identity)
                                    | (TowerRequest::SessionConfirm { .. }, SessionPhase::Confirmation)
                                    | (TowerRequest::MatchingConfigurationRequest {}, SessionPhase::Confirmed)
                            ) =>
                        {
                            let next_phase = match request {
                                TowerRequest::InitialIdentity { .. } => SessionPhase::Confirmation,
                                TowerRequest::SessionConfirm { .. } => SessionPhase::Confirmed,
                                _ => session_phase,
                            };
                            let response = central.respond(request)?;
                            stream
                                .write_all(&MatchingResponse::Central(response).serialize()?)
                                .await?;
                            session_phase = next_phase;
                        }
                        MatchingRequest::Activation { reserved }
                            if session_phase == SessionPhase::Confirmed =>
                        {
                            if reserved == 0 {
                                info!(%peer, connection_id, "activated Station matching session");
                            } else {
                                warn!(%peer, connection_id, reserved, "Station matching activation has an unexpected reserved value");
                            }
                            let response = MatchingResponse::ActivationConfiguration(
                                central.matching_activation_configuration()?,
                            );
                            stream.write_all(&response.serialize()?).await?;
                            info!(%peer, connection_id, "sent Station matching activation configuration");
                        }
                        MatchingRequest::LobbyRegistration(lobby)
                            if session_phase == SessionPhase::Confirmed =>
                        {
                            info!(
                                %peer,
                                connection_id,
                                mode = lobby.mode,
                                record_id = lobby.record_id,
                                matching_quest_index = lobby.matching_quest_index,
                                alternate_quest_index = lobby.alternate_quest_index,
                                location = lobby.location,
                                lobby_values = ?lobby.lobby_values,
                                "registered Station in global lobby"
                            );
                            registration = Some(lobby);
                            stream
                        .write_all(&MatchingResponse::LobbyPrompt {}.serialize()?)
                                .await?;
                        }
                        MatchingRequest::LobbyLookup(lookup)
                            if session_phase == SessionPhase::Confirmed =>
                        {
                            if endpoint_assigned {
                                let marker = lookup.player_or_lobby_key.participant_marker();
                                let assignment = online.exchange_participant_record(
                                    connection_id,
                                    lookup.player_or_lobby_key,
                                )?;
                                stream
                                    .write_all(
                                        &MatchingResponse::EndpointAssignment(assignment)
                                            .serialize()?,
                                    )
                                    .await?;
                                debug!(
                                    %peer,
                                    connection_id,
                                    marker = format_args!("0x{:02X}", marker.get()),
                                    "relayed Station participant exchange record"
                                );
                                continue;
                            }
                            let registration = registration.clone().ok_or(MatchingConnectionError::Registration)?;
                            let outcome = online.queue_match(
                                connection_id,
                                registration,
                                lookup.clone(),
                                assignment_tx.clone(),
                            )?;
                            match outcome {
                                MatchOutcome::Assembling {
                                    assignment,
                                    waiting_count,
                                } => {
                                    let player_count = assignment.participants.as_slice().len();
                                    stream
                                        .write_all(
                                            &MatchingResponse::EndpointAssignment(assignment)
                                                .serialize()?,
                                        )
                                        .await?;
                                    info!(
                                        %peer,
                                        connection_id,
                                        elapsed_wait_seconds = lookup.elapsed_wait_seconds,
                                        remaining_wait_seconds = lookup.remaining_wait_seconds,
                                        waiting_count,
                                        player_count,
                                        "sent partial gameplay party assignment"
                                    );
                                }
                                MatchOutcome::PartyCreated { owner_key, player_count } => {
                                    info!(
                                        %peer,
                                        connection_id,
                                        owner_key = %owner_key,
                                        player_count,
                                        "created global gameplay party"
                                    );
                                }
                            }
                        }
                        request => {
                            return Err(MatchingConnectionError::Unexpected {
                                request,
                                state: session_phase.name(),
                            });
                        }
                    }
                }
            }
        }
        debug!(%peer, connection_id, "matching peer reached end of stream");
        Ok(())
    }
    .await;
    online.remove_waiter(connection_id)?;
    result
}

fn hex_payload(payload: &[u8]) -> String {
    use std::fmt::Write;

    let mut output = String::with_capacity(payload.len() * 2);
    for byte in payload {
        let _ = write!(output, "{byte:02X}");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::protocol::station::EndpointHost;
    use crate::storage::Storage;

    async fn read_message_type(stream: &mut TcpStream) -> Result<u16, std::io::Error> {
        let frame = read_frame(stream)
            .await?
            .ok_or_else(|| std::io::Error::other("matching peer closed before its response"))?;
        Ok(u16::from_be_bytes([frame[0], frame[1]]))
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
        let directory = tempfile::tempdir()?;
        let online = Arc::new(OnlineState::new(
            EndpointHost::new("gameservers.aonnet".to_owned())?,
            33442,
        ));
        let storage = Arc::new(Storage::open(&directory.path().join("aon-net.db"))?);
        let settings = crate::runtime_settings::RuntimeSettings::for_tests(Arc::clone(&storage))?;
        let central = CentralServices::new(
            session_id,
            storage,
            settings,
            Arc::clone(&online),
            Vec::new(),
        );

        let result = handle_connection(server, peer, 1, &central, &online).await;

        assert!(result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn endpoint_assignment_continues_the_participant_exchange()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (client, accepted) = tokio::join!(TcpStream::connect(address), listener.accept());
        let mut client = client?;
        let (server, peer) = accepted?;
        let session_id = 0x1234_5678_u32;
        let directory = tempfile::tempdir()?;
        let online = Arc::new(OnlineState::new(
            EndpointHost::new("gameservers.aonnet".to_owned())?,
            33442,
        ));
        let storage = Arc::new(Storage::open(&directory.path().join("aon-net.db"))?);
        let central = Arc::new(CentralServices::new(
            session_id,
            Arc::clone(&storage),
            crate::runtime_settings::RuntimeSettings::for_tests(storage)?,
            Arc::clone(&online),
            Vec::new(),
        ));
        let server_task = tokio::spawn({
            let central = Arc::clone(&central);
            let online = Arc::clone(&online);
            async move { handle_connection(server, peer, 1, &central, &online).await }
        });

        client
            .write_all(&[0x00, 0x01, 0x00, 0x06, 0x01, 0x07, 0, 0, 0, 0])
            .await?;
        assert_eq!(read_message_type(&mut client).await?, 0x02);

        let mut confirmation = vec![0x00, 0x03, 0x00, 0x04];
        confirmation.extend_from_slice(&session_id.to_be_bytes());
        client.write_all(&confirmation).await?;
        assert_eq!(read_message_type(&mut client).await?, 0x04);

        let mut registration = vec![0; 4 + 0x15c];
        registration[..4].copy_from_slice(&[0x00, 0x09, 0x01, 0x5c]);
        registration[8..10].copy_from_slice(&75_u16.to_be_bytes());
        client.write_all(&registration).await?;
        assert_eq!(read_message_type(&mut client).await?, 0x0a);

        let mut lookup = vec![0; 4 + 0x24];
        lookup[..4].copy_from_slice(&[0x00, 0x0b, 0x00, 0x24]);
        lookup[4..6].copy_from_slice(&5_u16.to_be_bytes());
        client.write_all(&lookup).await?;
        client.write_all(&lookup).await?;
        assert_eq!(read_message_type(&mut client).await?, 0x0c);
        assert_eq!(read_message_type(&mut client).await?, 0x0c);

        let early_close =
            tokio::time::timeout(Duration::from_millis(250), read_frame(&mut client)).await;
        assert!(early_close.is_err());

        client.shutdown().await?;
        server_task.await??;
        Ok(())
    }
}
