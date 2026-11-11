#![forbid(unsafe_code)]

use std::{env, net::SocketAddr};

use axum::{Json, Router, routing::get};
use serde::Serialize;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Serialize)]
struct HealthResponse {
    service: &'static str,
    status: &'static str,
    source_mode: &'static str,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let host = env::var("MARKET_DATA_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned());
    let port = env::var("MARKET_DATA_PORT")
        .unwrap_or_else(|_| "3002".to_owned())
        .parse::<u16>()?;
    let address: SocketAddr = format!("{host}:{port}").parse()?;
    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(health));

    let listener = tokio::net::TcpListener::bind(address).await?;
    info!(%address, "market-data foundation service listening without upstream connections");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        service: "market-data",
        status: "ok",
        source_mode: "disconnected",
    })
}
