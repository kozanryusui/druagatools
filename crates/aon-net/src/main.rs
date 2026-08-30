use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use tracing::error;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> ExitCode {
    let admin_hub = Arc::new(aon_net::AdminHub::new(2_000));
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .with(admin_hub.log_layer())
        .init();

    match run(admin_hub).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!(%error, "AON.Net stopped");
            ExitCode::FAILURE
        }
    }
}

async fn run(admin_hub: Arc<aon_net::AdminHub>) -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("aon-net.toml"));
    let config = aon_net::load_config(&path)?;
    aon_net::serve(config, admin_hub).await?;
    Ok(())
}
