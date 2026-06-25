mod api;

use agent_runtime::storage::Storage;
use std::{net::SocketAddr, sync::Arc};

const DEFAULT_DATABASE_URL: &str = "sqlite://general-agent.db?mode=rwc";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url =
        std::env::var("GENERAL_AGENT_DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.into());
    let storage = Storage::connect(&database_url).await?;
    let app = api::router(Arc::new(api::AppState::new(storage)));
    let addr = SocketAddr::from(([127, 0, 0, 1], 49321));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!("agent server listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
