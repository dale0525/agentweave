mod api;

use std::{net::SocketAddr, sync::Arc};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let app = api::router(Arc::new(api::AppState));
    let addr = SocketAddr::from(([127, 0, 0, 1], 49321));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!("agent server listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
