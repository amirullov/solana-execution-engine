#![forbid(unsafe_code)]

use std::{
    env,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use reqwest::Client;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::time::sleep;
#[cfg(unix)]
use tracing::error;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    rpc: RpcClient,
    cluster: String,
    source_mode: &'static str,
    metrics: Arc<Metrics>,
}

#[derive(Default)]
struct Metrics {
    rpc_successes: AtomicU64,
    rpc_errors: AtomicU64,
    last_finalized_slot: AtomicU64,
    last_success_unix: AtomicU64,
    stream_updates: AtomicU64,
    stream_reconnects: AtomicU64,
}

#[derive(Serialize)]
struct HealthResponse {
    service: &'static str,
    status: &'static str,
    cluster: String,
    source_mode: &'static str,
    last_finalized_slot: u64,
}

#[derive(Clone)]
struct RpcClient {
    client: Client,
    endpoint: String,
    next_id: Arc<AtomicU64>,
}

#[derive(Debug, Clone)]
struct BlockFact {
    slot: i64,
    blockhash: String,
    parent_slot: i64,
    source: &'static str,
}

impl RpcClient {
    fn new(endpoint: String) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: Client::builder().timeout(Duration::from_secs(8)).build()?,
            endpoint,
            next_id: Arc::new(AtomicU64::new(1)),
        })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let response = self
            .client
            .post(&self.endpoint)
            .json(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .send()
            .await
            .map_err(|error| format!("RPC transport failed for {method}: {error}"))?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|error| format!("RPC returned invalid JSON for {method}: {error}"))?;
        if !status.is_success() {
            return Err(format!("RPC HTTP status {status} for {method}"));
        }
        if let Some(error) = body.get("error") {
            let code = error
                .get("code")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            return Err(format!("RPC method {method} failed with code {code}"));
        }
        body.get("result")
            .cloned()
            .ok_or_else(|| format!("RPC response has no result for {method}"))
    }

    async fn finalized_slot(&self) -> Result<u64, String> {
        self.call("getSlot", json!([{ "commitment": "finalized" }]))
            .await?
            .as_u64()
            .ok_or_else(|| "getSlot returned a non-integer result".to_owned())
    }

    async fn first_available_block(&self) -> Result<u64, String> {
        self.call("getFirstAvailableBlock", json!([]))
            .await?
            .as_u64()
            .ok_or_else(|| "getFirstAvailableBlock returned a non-integer result".to_owned())
    }

    async fn blocks(&self, start: u64, end: u64) -> Result<Vec<u64>, String> {
        let value = self
            .call(
                "getBlocks",
                json!([start, end, { "commitment": "finalized" }]),
            )
            .await?;
        value
            .as_array()
            .ok_or_else(|| "getBlocks returned a non-array result".to_owned())?
            .iter()
            .map(|slot| {
                slot.as_u64()
                    .ok_or_else(|| "getBlocks returned an invalid slot".to_owned())
            })
            .collect()
    }

    async fn block(&self, slot: u64) -> Result<Option<BlockFact>, String> {
        let result = self
            .call(
                "getBlock",
                json!([
                    slot,
                    {
                        "commitment": "finalized",
                        "encoding": "json",
                        "transactionDetails": "none",
                        "rewards": false
                    }
                ]),
            )
            .await?;
        if result.is_null() {
            return Ok(None);
        }
        let blockhash = result
            .get("blockhash")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("getBlock({slot}) omitted blockhash"))?
            .to_owned();
        let parent_slot = result
            .get("parentSlot")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("getBlock({slot}) omitted parentSlot"))?;
        Ok(Some(BlockFact {
            slot: i64::try_from(slot).map_err(|_| "slot exceeds PostgreSQL BIGINT".to_owned())?,
            blockhash,
            parent_slot: i64::try_from(parent_slot)
                .map_err(|_| "parent slot exceeds PostgreSQL BIGINT".to_owned())?,
            source: "solana_rpc",
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    dotenvy::dotenv().ok();
    let host = env::var("MARKET_DATA_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned());
    let port = env::var("MARKET_DATA_PORT")
        .unwrap_or_else(|_| "3002".to_owned())
        .parse::<u16>()?;
    let address: SocketAddr = format!("{host}:{port}").parse()?;
    let database_url = env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required")?;
    let rpc_url =
        env::var("SOLANA_RPC_URL").unwrap_or_else(|_| "https://api.devnet.solana.com".to_owned());
    let cluster = env::var("SOLANA_CLUSTER").unwrap_or_else(|_| "devnet".to_owned());
    if !matches!(cluster.as_str(), "localnet" | "devnet" | "mainnet-beta") {
        return Err("SOLANA_CLUSTER must be localnet, devnet, or mainnet-beta".into());
    }

    let pool = PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url)
        .await?;
    sqlx::query("SELECT 1").execute(&pool).await?;
    let rpc = RpcClient::new(rpc_url)?;
    let metrics_state = Arc::new(Metrics::default());
    let source_mode =
        if env::var("YELLOWSTONE_ENDPOINT").is_ok_and(|value| !value.trim().is_empty()) {
            "rpc_backfill+yellowstone"
        } else {
            "rpc_backfill"
        };
    let state = AppState {
        pool,
        rpc,
        cluster,
        source_mode,
        metrics: metrics_state,
    };

    tokio::spawn(rpc_backfill_loop(state.clone()));
    #[cfg(unix)]
    if let Ok(endpoint) = env::var("YELLOWSTONE_ENDPOINT")
        && !endpoint.trim().is_empty()
    {
        let token = env::var("YELLOWSTONE_X_TOKEN").ok();
        tokio::spawn(yellowstone_loop(state.clone(), endpoint, token));
    }
    #[cfg(windows)]
    if source_mode == "rpc_backfill+yellowstone" {
        warn!("Yellowstone gRPC is disabled on Windows; run the service in the Linux container");
    }

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/metrics", get(metrics_endpoint))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(address).await?;
    info!(%address, "market-data service listening; transaction execution is not enabled");
    axum::serve(listener, app).await?;
    Ok(())
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        service: "market-data",
        status: "ok",
        cluster: state.cluster,
        source_mode: state.source_mode,
        last_finalized_slot: state.metrics.last_finalized_slot.load(Ordering::Relaxed),
    })
}

async fn ready(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    let db_ready = sqlx::query("SELECT 1").execute(&state.pool).await.is_ok();
    let last_success = state.metrics.last_success_unix.load(Ordering::Relaxed);
    let source_ready = last_success > 0 && unix_now().saturating_sub(last_success) <= 60;
    let ready = db_ready && source_ready;
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(HealthResponse {
            service: "market-data",
            status: if ready { "ready" } else { "degraded" },
            cluster: state.cluster,
            source_mode: state.source_mode,
            last_finalized_slot: state.metrics.last_finalized_slot.load(Ordering::Relaxed),
        }),
    )
}

async fn metrics_endpoint(State(state): State<AppState>) -> String {
    format!(
        "market_data_rpc_successes_total {}\nmarket_data_rpc_errors_total {}\nmarket_data_last_finalized_slot {}\nmarket_data_yellowstone_updates_total {}\nmarket_data_yellowstone_reconnects_total {}\n",
        state.metrics.rpc_successes.load(Ordering::Relaxed),
        state.metrics.rpc_errors.load(Ordering::Relaxed),
        state.metrics.last_finalized_slot.load(Ordering::Relaxed),
        state.metrics.stream_updates.load(Ordering::Relaxed),
        state.metrics.stream_reconnects.load(Ordering::Relaxed),
    )
}

async fn rpc_backfill_loop(state: AppState) {
    let initial_backfill = env::var("MARKET_DATA_INITIAL_BACKFILL_SLOTS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(150)
        .clamp(1, 10_000);
    let mut interval = Duration::from_secs(2);
    loop {
        match rpc_backfill_once(&state, initial_backfill).await {
            Ok(slot) => {
                state.metrics.rpc_successes.fetch_add(1, Ordering::Relaxed);
                state
                    .metrics
                    .last_finalized_slot
                    .store(slot, Ordering::Relaxed);
                state
                    .metrics
                    .last_success_unix
                    .store(unix_now(), Ordering::Relaxed);
                interval = Duration::from_secs(2);
            }
            Err(error) => {
                state.metrics.rpc_errors.fetch_add(1, Ordering::Relaxed);
                warn!(%error, "RPC backfill pass failed; the next bounded retry will run shortly");
                interval = (interval * 2).min(Duration::from_secs(30));
            }
        }
        sleep(interval).await;
    }
}

async fn rpc_backfill_once(state: &AppState, initial_backfill: u64) -> Result<u64, String> {
    let tip = state.rpc.finalized_slot().await?;
    let first_available = state.rpc.first_available_block().await?;
    let checkpoint_name = format!("market-data:{}:finalized", state.cluster);
    let checkpoint: Option<i64> = sqlx::query_scalar(
        "SELECT slot FROM chain_checkpoints WHERE consumer_name = $1 AND commitment = 'finalized'",
    )
    .bind(&checkpoint_name)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| format!("failed to read durable finalized checkpoint: {error}"))?;

    let requested_start = checkpoint.map_or_else(
        || tip.saturating_sub(initial_backfill).max(first_available),
        |slot| u64::try_from(slot.saturating_add(1)).unwrap_or(tip.saturating_add(1)),
    );
    let start = requested_start.max(first_available);
    if requested_start < first_available {
        let gap_start = i64::try_from(requested_start).unwrap_or(i64::MAX);
        let gap_end = i64::try_from(first_available.saturating_sub(1)).unwrap_or(i64::MAX);
        insert_gap(
            &state.pool,
            &state.cluster,
            gap_start,
            gap_end,
            "rpc_retention_exceeded",
            "unavailable",
        )
        .await?;
    }
    if start > tip {
        return Ok(tip);
    }

    let mut range_start = start;
    while range_start <= tip {
        let range_end = range_start.saturating_add(99).min(tip);
        let block_slots = state.rpc.blocks(range_start, range_end).await?;
        for slot in block_slots {
            match state.rpc.block(slot).await? {
                Some(fact) => {
                    record_block(&state.pool, &state.cluster, &fact, "finalized").await?;
                }
                None => {
                    return Err(format!(
                        "finalized RPC block data is unavailable at slot {slot}"
                    ));
                }
            }
        }
        save_checkpoint(&state.pool, &checkpoint_name, range_end, None).await?;
        range_start = range_end.saturating_add(1);
    }
    resolve_backfilled_gaps(
        &state.pool,
        &state.cluster,
        i64::try_from(tip).unwrap_or(i64::MAX),
    )
    .await?;
    Ok(tip)
}

async fn record_block(
    pool: &PgPool,
    cluster: &str,
    fact: &BlockFact,
    commitment: &str,
) -> Result<(), String> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| format!("failed to begin slot write transaction: {error}"))?;
    let finalized_conflict: Option<String> = sqlx::query_scalar(
        "SELECT blockhash FROM slot_canonicality WHERE cluster = $1 AND slot = $2 AND status = 'finalized' AND blockhash <> $3 LIMIT 1 FOR UPDATE",
    )
    .bind(cluster)
    .bind(fact.slot)
    .bind(&fact.blockhash)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| format!("failed to check finalized slot hash: {error}"))?;
    if finalized_conflict.is_some() {
        transaction
            .rollback()
            .await
            .map_err(|error| format!("failed to roll back conflicting finalized slot: {error}"))?;
        insert_gap(
            pool,
            cluster,
            fact.slot,
            fact.slot,
            "finalized_hash_conflict",
            "unavailable",
        )
        .await?;
        return Err(format!(
            "conflicting finalized hash observed at slot {}",
            fact.slot
        ));
    }

    sqlx::query(
        "UPDATE slot_canonicality SET status = 'orphaned' WHERE cluster = $1 AND slot = $2 AND blockhash <> $3 AND status <> 'finalized'",
    )
    .bind(cluster)
    .bind(fact.slot)
    .bind(&fact.blockhash)
    .execute(&mut *transaction)
    .await
    .map_err(|error| format!("failed to mark replaced slot hash orphaned: {error}"))?;

    sqlx::query(
        "INSERT INTO slot_canonicality (cluster, slot, blockhash, status, parent_slot, source) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (cluster, slot, blockhash) DO UPDATE SET status = CASE WHEN slot_canonicality.status = 'finalized' OR EXCLUDED.status = 'finalized' THEN 'finalized' WHEN slot_canonicality.status = 'confirmed' OR EXCLUDED.status = 'confirmed' THEN 'confirmed' ELSE 'observed' END, parent_slot = EXCLUDED.parent_slot, source = EXCLUDED.source, observed_at = NOW()",
    )
    .bind(cluster)
    .bind(fact.slot)
    .bind(&fact.blockhash)
    .bind(commitment)
    .bind(fact.parent_slot)
    .bind(fact.source)
    .execute(&mut *transaction)
    .await
    .map_err(|error| format!("failed to persist slot canonicality: {error}"))?;

    sqlx::query(
        "INSERT INTO slot_observations (cluster, slot, blockhash, parent_slot, commitment) VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
    )
    .bind(cluster)
    .bind(fact.slot)
    .bind(&fact.blockhash)
    .bind(fact.parent_slot)
    .bind(commitment)
    .execute(&mut *transaction)
    .await
    .map_err(|error| format!("failed to persist slot observation: {error}"))?;
    if commitment == "finalized" {
        let checkpoint_name = format!("market-data:{cluster}:finalized");
        sqlx::query(
            "INSERT INTO chain_checkpoints (consumer_name, commitment, slot, blockhash, parent_slot) VALUES ($1, 'finalized', $2, $3, $4) ON CONFLICT (consumer_name) DO UPDATE SET commitment = 'finalized', slot = GREATEST(chain_checkpoints.slot, EXCLUDED.slot), blockhash = CASE WHEN EXCLUDED.slot >= chain_checkpoints.slot THEN EXCLUDED.blockhash ELSE chain_checkpoints.blockhash END, parent_slot = CASE WHEN EXCLUDED.slot >= chain_checkpoints.slot THEN EXCLUDED.parent_slot ELSE chain_checkpoints.parent_slot END, updated_at = NOW()",
        )
        .bind(checkpoint_name)
        .bind(fact.slot)
        .bind(&fact.blockhash)
        .bind(fact.parent_slot)
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("failed to persist finalized slot checkpoint: {error}"))?;
    }
    transaction
        .commit()
        .await
        .map_err(|error| format!("failed to commit slot observation: {error}"))
}

async fn save_checkpoint(
    pool: &PgPool,
    name: &str,
    slot: u64,
    blockhash: Option<&str>,
) -> Result<(), String> {
    let slot = i64::try_from(slot).map_err(|_| "slot exceeds PostgreSQL BIGINT")?;
    sqlx::query(
        "INSERT INTO chain_checkpoints (consumer_name, commitment, slot, blockhash, parent_slot) VALUES ($1, 'finalized', $2, $3, NULL) ON CONFLICT (consumer_name) DO UPDATE SET commitment = 'finalized', slot = GREATEST(chain_checkpoints.slot, EXCLUDED.slot), blockhash = CASE WHEN EXCLUDED.slot >= chain_checkpoints.slot THEN EXCLUDED.blockhash ELSE chain_checkpoints.blockhash END, parent_slot = CASE WHEN EXCLUDED.slot >= chain_checkpoints.slot THEN EXCLUDED.parent_slot ELSE chain_checkpoints.parent_slot END, updated_at = NOW()",
    )
    .bind(name)
    .bind(slot)
    .bind(blockhash)
    .execute(pool)
    .await
    .map_err(|error| format!("failed to save chain checkpoint: {error}"))?;
    Ok(())
}

async fn insert_gap(
    pool: &PgPool,
    cluster: &str,
    start: i64,
    end: i64,
    reason: &str,
    status: &str,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO slot_gaps (cluster, start_slot, end_slot, reason, status) SELECT $1, $2, $3, $4, $5 WHERE NOT EXISTS (SELECT 1 FROM slot_gaps WHERE cluster = $1 AND start_slot = $2 AND end_slot = $3 AND reason = $4 AND status = $5)",
    )
    .bind(cluster)
    .bind(start)
    .bind(end)
    .bind(reason)
    .bind(status)
    .execute(pool)
    .await
    .map_err(|error| format!("failed to persist slot gap: {error}"))?;
    Ok(())
}

async fn resolve_backfilled_gaps(
    pool: &PgPool,
    cluster: &str,
    through_slot: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE slot_gaps SET status = 'resolved', resolved_at = NOW() WHERE cluster = $1 AND status IN ('open', 'backfilling') AND reason LIKE 'yellowstone_%' AND end_slot <= $2",
    )
    .bind(cluster)
    .bind(through_slot)
    .execute(pool)
    .await
    .map_err(|error| format!("failed to resolve backfilled slot gaps: {error}"))?;
    Ok(())
}

#[cfg(unix)]
async fn yellowstone_loop(state: AppState, endpoint: String, token: Option<String>) {
    use futures::StreamExt;

    let mut retry_delay = Duration::from_secs(1);
    loop {
        let start_slot = load_stream_start_slot(&state).await;
        match connect_yellowstone(&endpoint, token.as_deref(), start_slot).await {
            Ok(mut stream) => {
                retry_delay = Duration::from_secs(1);
                info!(cluster = %state.cluster, "Yellowstone confirmed slot stream connected with replay enabled");
                while let Some(event) = stream.next().await {
                    match event {
                        Ok(event) => handle_yellowstone_event(&state, event).await,
                        Err(error) => {
                            warn!(%error, "Yellowstone stream failed; reconnect loop will restart");
                            break;
                        }
                    }
                }
            }
            Err(error) => {
                warn!(%error, ?retry_delay, "Yellowstone connection or subscription failed");
            }
        }
        sleep(retry_delay).await;
        retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
    }
}

#[cfg(unix)]
async fn load_stream_start_slot(state: &AppState) -> Option<u64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT slot FROM chain_checkpoints WHERE consumer_name = $1 AND commitment = 'finalized'",
    )
    .bind(format!("market-data:{}:finalized", state.cluster))
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .and_then(|slot| u64::try_from(slot).ok())
    .map(|slot| slot.saturating_sub(20))
}

#[cfg(unix)]
async fn connect_yellowstone(
    endpoint: &str,
    token: Option<&str>,
    start_slot: Option<u64>,
) -> Result<yellowstone_grpc_client::ReconnectStream, String> {
    use yellowstone_grpc_client::{GeyserGrpcClient, ReconnectConfig, ReconnectPolicy};

    let builder = GeyserGrpcClient::build_from_shared(endpoint.to_owned())
        .map_err(|error| format!("invalid Yellowstone endpoint: {error}"))?;
    let builder = builder
        .x_token(token)
        .map_err(|error| format!("invalid Yellowstone authentication token: {error}"))?;
    let reconnect_config =
        ReconnectConfig::default().with_policy(ReconnectPolicy::Replay { slots_behind: 20 });
    let mut client = builder
        .set_reconnect_config(reconnect_config)
        .connect()
        .await
        .map_err(|error| format!("Yellowstone connection failed: {error}"))?;
    let request = build_subscribe_request(start_slot);
    let (_, stream) = client
        .subscribe_with_reconnect(Some(request))
        .await
        .map_err(|error| format!("Yellowstone subscription failed: {error}"))?;
    Ok(stream)
}

#[cfg(unix)]
#[allow(
    clippy::zero_sized_map_values,
    reason = "Yellowstone protobuf defines blocks_meta as a map with an empty message value"
)]
fn build_subscribe_request(
    start_slot: Option<u64>,
) -> yellowstone_grpc_proto::prelude::SubscribeRequest {
    use {
        std::collections::HashMap,
        yellowstone_grpc_proto::prelude::{
            CommitmentLevel, SubscribeRequest, SubscribeRequestFilterBlocksMeta,
            SubscribeRequestFilterSlots,
        },
    };

    SubscribeRequest {
        slots: HashMap::from([(
            "slot_status".to_owned(),
            SubscribeRequestFilterSlots {
                filter_by_commitment: Some(true),
                interslot_updates: Some(false),
            },
        )]),
        blocks_meta: HashMap::from([(
            "block_meta".to_owned(),
            SubscribeRequestFilterBlocksMeta {},
        )]),
        commitment: Some(CommitmentLevel::Confirmed as i32),
        from_slot: start_slot,
        ..Default::default()
    }
}

#[cfg(unix)]
async fn handle_yellowstone_event(
    state: &AppState,
    event: yellowstone_grpc_client::ReconnectEvent,
) {
    use yellowstone_grpc_client::ReconnectEvent;

    match event {
        ReconnectEvent::Update(update) => {
            state.metrics.stream_updates.fetch_add(1, Ordering::Relaxed);
            handle_yellowstone_update(state, update).await;
        }
        ReconnectEvent::Disconnected(disconnected) => {
            state
                .metrics
                .stream_reconnects
                .fetch_add(1, Ordering::Relaxed);
            handle_yellowstone_disconnect(state, disconnected).await;
        }
    }
}

#[cfg(unix)]
async fn handle_yellowstone_update(
    state: &AppState,
    update: yellowstone_grpc_proto::prelude::SubscribeUpdate,
) {
    use yellowstone_grpc_proto::prelude::{SlotStatus, subscribe_update::UpdateOneof};

    match update.update_oneof {
        Some(UpdateOneof::BlockMeta(meta)) => {
            let Ok(slot) = i64::try_from(meta.slot) else {
                warn!("Yellowstone slot exceeds PostgreSQL BIGINT");
                return;
            };
            let Ok(parent_slot) = i64::try_from(meta.parent_slot) else {
                warn!("Yellowstone parent slot exceeds PostgreSQL BIGINT");
                return;
            };
            let fact = BlockFact {
                slot,
                blockhash: meta.blockhash,
                parent_slot,
                source: "yellowstone_grpc",
            };
            if let Err(error) = record_block(&state.pool, &state.cluster, &fact, "confirmed").await
            {
                error!(%error, "failed to persist Yellowstone block metadata");
            }
        }
        Some(UpdateOneof::Slot(slot_update))
            if SlotStatus::try_from(slot_update.status).ok() == Some(SlotStatus::SlotDead) =>
        {
            let Ok(slot) = i64::try_from(slot_update.slot) else {
                warn!("Yellowstone dead slot exceeds PostgreSQL BIGINT");
                return;
            };
            if let Err(error) = orphan_dead_slot(&state.pool, &state.cluster, slot).await {
                error!(%error, slot, "failed to record Yellowstone dead slot");
            }
        }
        _ => {}
    }
}

#[cfg(unix)]
async fn handle_yellowstone_disconnect(
    state: &AppState,
    disconnected: yellowstone_grpc_client::Disconnected,
) {
    let partial_bank_count = disconnected.partial_banks.len();
    if partial_bank_count > 0 {
        let start_slot = disconnected
            .partial_banks
            .iter()
            .map(|bank| bank.slot)
            .min()
            .unwrap_or_default();
        let end_slot = disconnected
            .partial_banks
            .iter()
            .map(|bank| bank.slot)
            .max()
            .unwrap_or(start_slot);
        if let (Ok(start), Ok(end)) = (i64::try_from(start_slot), i64::try_from(end_slot))
            && let Err(error) = insert_gap(
                &state.pool,
                &state.cluster,
                start,
                end,
                "yellowstone_partial_bank_discarded",
                "open",
            )
            .await
        {
            error!(%error, "failed to persist Yellowstone reconnect gap");
        }
    }
    warn!(
        replay_from_slot = ?disconnected.replay_from_slot,
        partial_bank_count,
        "Yellowstone reconnecting; RPC finalized backfill remains authoritative"
    );
}

#[cfg(unix)]
async fn orphan_dead_slot(pool: &PgPool, cluster: &str, slot: i64) -> Result<(), String> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| format!("failed to begin dead slot transaction: {error}"))?;
    let finalized_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM slot_canonicality WHERE cluster = $1 AND slot = $2 AND status = 'finalized')",
    )
    .bind(cluster)
    .bind(slot)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| format!("failed to check finalized dead slot: {error}"))?;
    if finalized_exists {
        transaction
            .rollback()
            .await
            .map_err(|error| format!("failed to roll back finalized dead slot: {error}"))?;
        insert_gap(
            pool,
            cluster,
            slot,
            slot,
            "finalized_dead_slot_conflict",
            "unavailable",
        )
        .await?;
        return Err(format!(
            "dead slot conflicts with finalized data at slot {slot}"
        ));
    }
    sqlx::query(
        "UPDATE slot_canonicality SET status = 'orphaned', observed_at = NOW() WHERE cluster = $1 AND slot = $2 AND status <> 'finalized'",
    )
    .bind(cluster)
    .bind(slot)
    .execute(&mut *transaction)
    .await
    .map_err(|error| format!("failed to orphan dead slot: {error}"))?;
    sqlx::query(
        "INSERT INTO slot_observations (cluster, slot, blockhash, parent_slot, commitment) VALUES ($1, $2, 'dead-slot', NULL, 'dead') ON CONFLICT DO NOTHING",
    )
    .bind(cluster)
    .bind(slot)
    .execute(&mut *transaction)
    .await
    .map_err(|error| format!("failed to persist dead slot observation: {error}"))?;
    transaction
        .commit()
        .await
        .map_err(|error| format!("failed to commit dead slot observation: {error}"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_rpc_requests_include_commitment_and_disable_block_payloads() {
        let slot_request = json!([{ "commitment": "finalized" }]);
        let block_request = json!([
            42,
            {
                "commitment": "finalized",
                "encoding": "json",
                "transactionDetails": "none",
                "rewards": false
            }
        ]);

        assert_eq!(slot_request[0]["commitment"], "finalized");
        assert_eq!(block_request[1]["transactionDetails"], "none");
        assert_eq!(block_request[1]["rewards"], false);
    }
}
