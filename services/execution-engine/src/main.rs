#![forbid(unsafe_code)]

use std::{env, net::SocketAddr};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use engine_core::{ExecutionIntent, ExecutionPolicy};
use serde::Serialize;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    policy: ExecutionPolicy,
}

#[derive(Serialize)]
struct HealthResponse {
    service: &'static str,
    status: &'static str,
    execution_mode: &'static str,
}

#[derive(Serialize)]
struct SimulationResponse {
    intent_id: uuid::Uuid,
    executable: bool,
    reason: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    init_tracing();
    let execution_mode = env::var("EXECUTION_MODE").unwrap_or_else(|_| "disabled".to_owned());
    ensure_foundation_mode(&execution_mode)?;

    let host = env::var("EXECUTION_ENGINE_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned());
    let port = env::var("EXECUTION_ENGINE_PORT")
        .unwrap_or_else(|_| "3001".to_owned())
        .parse::<u16>()?;
    let address: SocketAddr = format!("{host}:{port}").parse()?;
    let state = AppState {
        policy: ExecutionPolicy::default(),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(health))
        .route("/v1/simulate", post(simulate))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(address).await?;
    info!(%address, "execution engine listening with execution disabled");
    axum::serve(listener, app).await?;
    Ok(())
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
}

fn ensure_foundation_mode(mode: &str) -> Result<(), String> {
    if mode != "disabled" {
        return Err(format!(
            "EXECUTION_MODE={mode} is forbidden: Foundation supports only disabled"
        ));
    }
    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        service: "execution-engine",
        status: "ok",
        execution_mode: "disabled",
    })
}

async fn simulate(
    State(state): State<AppState>,
    Json(intent): Json<ExecutionIntent>,
) -> (StatusCode, Json<SimulationResponse>) {
    let result = state.policy.evaluate(&intent);
    let reason = result
        .err()
        .map_or_else(|| "not_applicable".to_owned(), |denial| denial.to_string());
    (
        StatusCode::OK,
        Json(SimulationResponse {
            intent_id: intent.id,
            executable: false,
            reason,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::ensure_foundation_mode;

    #[test]
    fn disabled_mode_is_safe() {
        assert!(ensure_foundation_mode("disabled").is_ok());
    }

    #[test]
    fn live_mode_is_rejected() {
        assert!(ensure_foundation_mode("live").is_err());
    }
}
