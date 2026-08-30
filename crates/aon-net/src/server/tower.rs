use std::sync::Arc;

use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

use super::central::{CentralServiceError, CentralServices};
use super::transport::read_frame;
use super::{ServerError, SessionPhase};
use crate::protocol::tower::{
    TowerProtocolError, TowerRequest, deserialize_tower_request, serialize_tower_response,
};

#[derive(Clone, Copy, Debug)]
pub(super) enum ServiceKind {
    Database,
    RelayControl(&'static str),
}

impl ServiceKind {
    fn name(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::RelayControl(name) => name,
        }
    }

    fn permits(self, request: &TowerRequest) -> bool {
        matches!(
            request,
            TowerRequest::InitialIdentity { .. } | TowerRequest::SessionConfirm { .. }
        ) || match self {
            Self::Database => matches!(
                request,
                TowerRequest::AnnouncementRequest { .. }
                    | TowerRequest::CardDataUpload { .. }
                    | TowerRequest::ServiceRecordRequest { .. }
                    | TowerRequest::DatabaseStatusRequest { .. }
            ),
            Self::RelayControl(_) => matches!(
                request,
                TowerRequest::AnnouncementRequest { .. }
                    | TowerRequest::ServiceRecordRequest { .. }
                    | TowerRequest::RelayStatusRequest { .. }
                    | TowerRequest::PartyQuestScheduleRequest { .. }
            ),
        }
    }
}

#[derive(Debug, Error)]
pub(super) enum TowerConnectionError {
    #[error("Tower connection I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Tower protocol failed: {0}")]
    Protocol(#[from] TowerProtocolError),
    #[error("central service request failed: {0}")]
    Central(#[from] CentralServiceError),
    #[error("request {request:?} is not valid for service {service}")]
    WrongService {
        service: &'static str,
        request: TowerRequest,
    },
    #[error("request {request:?} is not valid in Tower session state {state}")]
    UnexpectedState {
        request: TowerRequest,
        state: &'static str,
    },
}

pub(super) async fn serve_connections(
    listener: TcpListener,
    service: ServiceKind,
    central: Arc<CentralServices>,
) -> Result<(), ServerError> {
    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .map_err(ServerError::ConnectionAccept)?;
        let service_name = service.name();
        info!(%peer, service = service_name, "accepted Tower service connection");
        let central = Arc::clone(&central);
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, service, &central).await {
                warn!(%peer, service = service_name, %error, "Tower service connection ended with an error");
            } else {
                info!(%peer, service = service_name, "Tower service connection closed");
            }
        });
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    service: ServiceKind,
    central: &CentralServices,
) -> Result<(), TowerConnectionError> {
    let mut session_phase = SessionPhase::Identity;
    while let Some(frame) = read_frame(&mut stream).await? {
        let request = deserialize_tower_request(&frame)?;
        if !service.permits(&request) {
            return Err(TowerConnectionError::WrongService {
                service: service.name(),
                request,
            });
        }
        let valid_state = matches!(
            (&request, session_phase),
            (TowerRequest::InitialIdentity { .. }, SessionPhase::Identity)
                | (
                    TowerRequest::SessionConfirm { .. },
                    SessionPhase::Confirmation
                )
        ) || (session_phase == SessionPhase::Confirmed
            && !matches!(
                &request,
                TowerRequest::InitialIdentity { .. } | TowerRequest::SessionConfirm { .. }
            ));
        if !valid_state {
            return Err(TowerConnectionError::UnexpectedState {
                request,
                state: session_phase.name(),
            });
        }
        let next_phase = match request {
            TowerRequest::InitialIdentity { .. } => SessionPhase::Confirmation,
            TowerRequest::SessionConfirm { .. } => SessionPhase::Confirmed,
            _ => session_phase,
        };
        let response = central.respond(request)?;
        stream
            .write_all(&serialize_tower_response(&response)?)
            .await?;
        session_phase = next_phase;
    }
    Ok(())
}
