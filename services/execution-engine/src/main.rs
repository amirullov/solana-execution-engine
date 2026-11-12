#![forbid(unsafe_code)]

use std::{
    env,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use engine_core::{ExecutionIntent, ExecutionPolicy};
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::info;
use tracing_subscriber::EnvFilter;
mod transaction_inspection;
use transaction_inspection::{InspectionError, TransactionInspection, inspect_transaction};

const MAX_TRANSACTION_BYTES: usize = 4_096;
const MAX_RPC_RESPONSE_BYTES: usize = 1_048_576;
const MAX_SIMULATION_LOG_LINES: usize = 100;
const MAX_SIMULATION_LOG_LINE_BYTES: usize = 1_024;

#[derive(Clone)]
struct AppState {
    policy: ExecutionPolicy,
    simulator: RpcSimulator,
}

#[derive(Clone)]
struct RpcSimulator {
    client: Client,
    endpoint: String,
    next_id: Arc<AtomicU64>,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct TransactionSimulationRequest {
    transaction_base64: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TransactionSimulationResponse {
    simulation_passed: bool,
    execution_allowed: bool,
    inspection: TransactionInspection,
    context_slot: Option<u64>,
    error: Option<Value>,
    units_consumed: Option<u64>,
    logs: Vec<String>,
    logs_truncated: bool,
}

impl RpcSimulator {
    fn new(endpoint: String) -> Result<Self, Box<dyn std::error::Error>> {
        let url = reqwest::Url::parse(&endpoint)?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err("SOLANA_RPC_URL must be an absolute HTTP(S) URL".into());
        }
        Ok(Self {
            client: Client::builder().timeout(Duration::from_secs(10)).build()?,
            endpoint,
            next_id: Arc::new(AtomicU64::new(1)),
        })
    }

    async fn simulate(
        &self,
        transaction: &[u8],
        inspection: TransactionInspection,
    ) -> Option<TransactionSimulationResponse> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = build_simulation_request(id, transaction);

        let response = self
            .client
            .post(&self.endpoint)
            .json(&request)
            .send()
            .await
            .ok()?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|length| length > MAX_RPC_RESPONSE_BYTES as u64)
        {
            return None;
        }
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.ok()?;
            if body.len().saturating_add(chunk.len()) > MAX_RPC_RESPONSE_BYTES {
                return None;
            }
            body.extend_from_slice(&chunk);
        }
        let body: Value = serde_json::from_slice(&body).ok()?;
        if body.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || body.get("id").and_then(Value::as_u64) != Some(id)
            || body.get("error").is_some()
        {
            return None;
        }
        let result = body.get("result")?;
        let value = result.get("value")?;
        let raw_error = value.get("err")?;
        let error = (!raw_error.is_null()).then(|| raw_error.clone());
        let raw_logs = value
            .get("logs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let logs_truncated = raw_logs.len() > MAX_SIMULATION_LOG_LINES
            || raw_logs.iter().any(|line| {
                line.as_str()
                    .is_some_and(|line| line.len() > MAX_SIMULATION_LOG_LINE_BYTES)
            });
        let logs = raw_logs
            .iter()
            .take(MAX_SIMULATION_LOG_LINES)
            .filter_map(Value::as_str)
            .map(|line| line.chars().take(MAX_SIMULATION_LOG_LINE_BYTES).collect())
            .collect();

        Some(TransactionSimulationResponse {
            simulation_passed: error.is_none(),
            execution_allowed: false,
            inspection,
            context_slot: result
                .get("context")
                .and_then(|context| context.get("slot"))
                .and_then(Value::as_u64),
            error,
            units_consumed: value.get("unitsConsumed").and_then(Value::as_u64),
            logs,
            logs_truncated,
        })
    }
}

fn build_simulation_request(id: u64, transaction: &[u8]) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "simulateTransaction",
        "params": [
            STANDARD.encode(transaction),
            {
                "commitment": "confirmed",
                "encoding": "base64",
                "replaceRecentBlockhash": true,
                "sigVerify": false
            }
        ]
    })
}

fn decode_transaction(encoded: &str) -> Option<Vec<u8>> {
    if encoded.is_empty() || encoded.len() > MAX_TRANSACTION_BYTES.div_ceil(3) * 4 {
        return None;
    }
    let transaction = STANDARD.decode(encoded).ok()?;
    if transaction.is_empty()
        || transaction.len() > MAX_TRANSACTION_BYTES
        || STANDARD.encode(&transaction) != encoded
    {
        return None;
    }
    Some(transaction)
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
    let rpc_url =
        env::var("SOLANA_RPC_URL").unwrap_or_else(|_| "https://api.devnet.solana.com".to_owned());
    let state = AppState {
        policy: ExecutionPolicy::default(),
        simulator: RpcSimulator::new(rpc_url)?,
    };
    let app = Router::new()
        .layer(DefaultBodyLimit::max(8 * 1024))
        .route("/health", get(health))
        .route("/ready", get(health))
        .route("/v1/simulate", post(simulate))
        .route("/v1/simulate-transaction", post(simulate_transaction))
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

async fn simulate_transaction(
    State(state): State<AppState>,
    Json(request): Json<TransactionSimulationRequest>,
) -> (StatusCode, Json<Value>) {
    let Some(transaction) = decode_transaction(&request.transaction_base64) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_transaction_encoding" })),
        );
    };
    let inspection = match inspect_transaction(&transaction) {
        Ok(inspection) => inspection,
        Err(InspectionError::InvalidTransaction) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "transaction_decode_failed" })),
            );
        }
        Err(InspectionError::AddressLookupTablesUnresolved) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": "address_lookup_tables_unresolved" })),
            );
        }
        Err(InspectionError::V1ResourceLimitsMissing) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": "v1_resource_limits_missing" })),
            );
        }
    };
    if let Some(response) = state.simulator.simulate(&transaction, inspection).await {
        (StatusCode::OK, Json(json!(response)))
    } else {
        tracing::warn!("read-only transaction simulation failed or RPC response was invalid");
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "simulation_provider_unavailable" })),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::transaction_inspection::TransactionInspection;
    use super::{
        MAX_TRANSACTION_BYTES, RpcSimulator, build_simulation_request, decode_transaction,
        ensure_foundation_mode,
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::sync::{Arc, Mutex};

    #[test]
    fn disabled_mode_is_safe() {
        assert!(ensure_foundation_mode("disabled").is_ok());
    }

    #[test]
    fn live_mode_is_rejected() {
        assert!(ensure_foundation_mode("live").is_err());
    }

    #[test]
    fn transaction_encoding_is_canonical_base64_and_within_the_protocol_cap() {
        let encoded = STANDARD.encode([0_u8; MAX_TRANSACTION_BYTES]);
        assert_eq!(
            decode_transaction(&encoded)
                .expect("valid transaction")
                .len(),
            MAX_TRANSACTION_BYTES
        );
        assert!(decode_transaction("AQ").is_none());
        assert!(decode_transaction(&STANDARD.encode([0_u8; MAX_TRANSACTION_BYTES + 1])).is_none());
    }

    #[test]
    fn simulation_request_cannot_verify_signatures_or_change_state() {
        let request = build_simulation_request(7, &[1, 2, 3]);
        assert_eq!(request["method"], "simulateTransaction");
        assert_eq!(request["params"][1]["encoding"], "base64");
        assert_eq!(request["params"][1]["sigVerify"], false);
        assert_eq!(request["params"][1]["replaceRecentBlockhash"], true);
        assert_eq!(request["params"][0], STANDARD.encode([1, 2, 3]));
    }

    #[tokio::test]
    async fn simulation_response_is_observational_and_never_execution_approval() {
        use axum::{Json, Router, extract::State, routing::post};
        use serde_json::json;

        async fn mock_rpc(
            State(requests): State<Arc<Mutex<Vec<serde_json::Value>>>>,
            Json(request): Json<serde_json::Value>,
        ) -> Json<serde_json::Value> {
            requests.lock().expect("request lock").push(request);
            Json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "context": { "slot": 42 },
                    "value": { "err": null, "logs": ["ok"], "unitsConsumed": 123 }
                }
            }))
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock RPC listener");
        let address = listener.local_addr().expect("mock RPC address");
        let app = Router::new()
            .route("/", post(mock_rpc))
            .with_state(Arc::clone(&requests));
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let simulator =
            RpcSimulator::new(format!("http://{address}/")).expect("valid mock RPC URL");
        let inspection = TransactionInspection {
            format: "legacy",
            static_account_keys: Vec::new(),
            instructions: Vec::new(),
            address_lookup_table_count: 0,
            v1_resource_limits: None,
        };
        let response = simulator
            .simulate(&[1, 2, 3], inspection)
            .await
            .expect("simulation response");
        server.abort();

        assert!(response.simulation_passed);
        assert!(!response.execution_allowed);
        assert_eq!(response.context_slot, Some(42));
        assert_eq!(response.units_consumed, Some(123));
        assert_eq!(response.logs, vec!["ok".to_owned()]);
        let requests = requests.lock().expect("request lock");
        assert_eq!(requests[0]["method"], "simulateTransaction");
        assert_eq!(requests[0]["params"][1]["sigVerify"], false);
    }
}
