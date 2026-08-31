use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;
use tokio::net::TcpListener;
use tracing::info;

use crate::config::AonNetConfig;
use crate::logging::AdminHub;
use crate::runtime_settings::{RuntimeSettings, SettingsError};
use crate::storage::{Storage, StorageError};

mod central;
mod gameplay;
mod matching;
mod power_on;
pub(crate) mod quest_rotation;
mod tower;
mod transport;

static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);
const GAME_SESSION_ID: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionPhase {
    Identity,
    Confirmation,
    Confirmed,
}

impl SessionPhase {
    const fn name(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Confirmation => "confirmation",
            Self::Confirmed => "confirmed",
        }
    }
}

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("cannot listen on {address}: {source}")]
    Bind {
        address: String,
        #[source]
        source: std::io::Error,
    },
    #[error("AON.Net HTTP server failed: {0}")]
    Serve(std::io::Error),
    #[error("AON.Net TCP listener cannot accept a connection: {0}")]
    ConnectionAccept(std::io::Error),
    #[error("AON.Net storage initialization failed: {0}")]
    Storage(#[from] StorageError),
    #[error("AON.Net settings initialization failed: {0}")]
    Settings(#[from] SettingsError),
}

pub async fn serve(config: AonNetConfig, admin_hub: Arc<AdminHub>) -> Result<(), ServerError> {
    let bind_ip = config.server.bind_ip;
    let http_address = SocketAddr::new(bind_ip, config.server.http_port);
    let database_address = SocketAddr::new(bind_ip, config.server.game_port);
    let matching_address = SocketAddr::new(bind_ip, config.server.matching_port);
    let [relay_1_port, relay_2_port, relay_3_port] = config.server.relay_ports;
    let relay_1_address = SocketAddr::new(bind_ip, relay_1_port);
    let relay_2_address = SocketAddr::new(bind_ip, relay_2_port);
    let relay_3_address = SocketAddr::new(bind_ip, relay_3_port);
    let gameplay_address = SocketAddr::new(bind_ip, config.server.gameplay_port);
    let storage = Arc::new(Storage::open(&config.server.database_path)?);
    let settings = Arc::new(RuntimeSettings::new(
        Arc::clone(&storage),
        Arc::clone(&admin_hub),
        config.power_on.shop_name.clone(),
    )?);
    let online = Arc::new(crate::online::OnlineState::new(
        config.server.gameplay_advertise_host.clone(),
        config.server.gameplay_advertise_port.get(),
    ));
    let central = Arc::new(central::CentralServices::new(
        GAME_SESSION_ID,
        storage,
        Arc::clone(&settings),
        Arc::clone(&online),
        config.announcements,
    ));
    let app = power_on::router(config.power_on, Arc::clone(&settings))
        .merge(crate::admin::backend::router(settings, admin_hub));

    let http_listener = bind_listener(http_address).await?;
    let database_listener = bind_listener(database_address).await?;
    let matching_listener = bind_listener(matching_address).await?;
    let relay_1_listener = bind_listener(relay_1_address).await?;
    let relay_2_listener = bind_listener(relay_2_address).await?;
    let relay_3_listener = bind_listener(relay_3_address).await?;
    let gameplay_listener = bind_listener(gameplay_address).await?;
    info!(
        %http_address,
        %database_address,
        %matching_address,
        %relay_1_address,
        %relay_2_address,
        %relay_3_address,
        %gameplay_address,
        gameplay_advertise_host = %config.server.gameplay_advertise_host,
        gameplay_advertise_port = config.server.gameplay_advertise_port.get(),
        "AON.Net is ready"
    );

    let http_server = async move {
        axum::serve(http_listener, app)
            .await
            .map_err(ServerError::Serve)
    };
    let database_server = tower::serve_connections(
        database_listener,
        tower::ServiceKind::Database,
        Arc::clone(&central),
    );
    let matching_server =
        matching::serve_connections(matching_listener, Arc::clone(&central), Arc::clone(&online));
    let relay_1_server = tower::serve_connections(
        relay_1_listener,
        tower::ServiceKind::RelayControl("relay-1"),
        Arc::clone(&central),
    );
    let relay_2_server = tower::serve_connections(
        relay_2_listener,
        tower::ServiceKind::RelayControl("relay-2"),
        Arc::clone(&central),
    );
    let relay_3_server = tower::serve_connections(
        relay_3_listener,
        tower::ServiceKind::RelayControl("relay-3"),
        central,
    );
    let gameplay_server = gameplay::serve_connections(gameplay_listener, GAME_SESSION_ID, online);
    tokio::try_join!(
        http_server,
        database_server,
        matching_server,
        relay_1_server,
        relay_2_server,
        relay_3_server,
        gameplay_server
    )?;
    Ok(())
}

fn next_connection_id() -> u64 {
    NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
}

async fn bind_listener(address: std::net::SocketAddr) -> Result<TcpListener, ServerError> {
    TcpListener::bind(address)
        .await
        .map_err(|source| ServerError::Bind {
            address: address.to_string(),
            source,
        })
}
