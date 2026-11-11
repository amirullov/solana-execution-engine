BEGIN;

CREATE TABLE IF NOT EXISTS execution_intents (
    id UUID PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    cluster TEXT NOT NULL CHECK (cluster IN ('localnet', 'devnet', 'mainnet-beta')),
    input_mint TEXT NOT NULL,
    output_mint TEXT NOT NULL,
    amount_atomic NUMERIC(39, 0) NOT NULL CHECK (amount_atomic > 0),
    max_slippage_bps INTEGER NOT NULL CHECK (max_slippage_bps BETWEEN 0 AND 10000),
    status TEXT NOT NULL CHECK (status IN ('received', 'validated', 'simulated', 'rejected', 'expired')),
    request_payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS quote_snapshots (
    id UUID PRIMARY KEY,
    intent_id UUID NOT NULL REFERENCES execution_intents(id),
    provider TEXT NOT NULL,
    provider_request_id TEXT,
    context_slot BIGINT,
    input_amount_atomic NUMERIC(39, 0) NOT NULL,
    expected_output_atomic NUMERIC(39, 0) NOT NULL,
    minimum_output_atomic NUMERIC(39, 0) NOT NULL,
    price_impact_bps INTEGER,
    response_payload JSONB NOT NULL,
    response_sha256 TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS quote_snapshots_intent_created_idx
    ON quote_snapshots (intent_id, created_at DESC);

CREATE TABLE IF NOT EXISTS chain_checkpoints (
    consumer_name TEXT PRIMARY KEY,
    commitment TEXT NOT NULL CHECK (commitment IN ('processed', 'confirmed', 'finalized')),
    slot BIGINT NOT NULL CHECK (slot >= 0),
    blockhash TEXT,
    parent_slot BIGINT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS slot_observations (
    slot BIGINT NOT NULL,
    blockhash TEXT NOT NULL,
    parent_slot BIGINT,
    commitment TEXT NOT NULL CHECK (commitment IN ('processed', 'confirmed', 'finalized', 'dead')),
    observed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (slot, blockhash, commitment)
);

CREATE TABLE IF NOT EXISTS outbox_events (
    id UUID PRIMARY KEY,
    aggregate_type TEXT NOT NULL,
    aggregate_id UUID NOT NULL,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    published_at TIMESTAMPTZ,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0)
);

CREATE INDEX IF NOT EXISTS outbox_unpublished_idx
    ON outbox_events (created_at)
    WHERE published_at IS NULL;

COMMIT;

