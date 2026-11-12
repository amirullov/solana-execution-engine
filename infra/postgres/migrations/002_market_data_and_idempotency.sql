BEGIN;

CREATE TABLE IF NOT EXISTS quote_requests (
    id UUID PRIMARY KEY,
    cluster TEXT NOT NULL CHECK (cluster IN ('localnet', 'devnet', 'mainnet-beta')),
    input_mint TEXT NOT NULL,
    output_mint TEXT NOT NULL,
    input_amount_atomic NUMERIC(39, 0) NOT NULL CHECK (input_amount_atomic > 0),
    output_amount_atomic NUMERIC(39, 0) NOT NULL CHECK (output_amount_atomic >= 0),
    minimum_output_atomic NUMERIC(39, 0),
    provider TEXT NOT NULL,
    provider_request_id TEXT,
    provider_context_slot BIGINT,
    fee_bps INTEGER,
    fee_mint TEXT,
    price_impact_pct TEXT,
    response_sha256 TEXT NOT NULL,
    response_payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS quote_requests_pair_created_idx
    ON quote_requests (input_mint, output_mint, created_at DESC);

CREATE TABLE IF NOT EXISTS intent_events (
    id BIGSERIAL PRIMARY KEY,
    intent_id UUID NOT NULL REFERENCES execution_intents(id),
    event_type TEXT NOT NULL,
    event_payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS intent_events_intent_id_idx
    ON intent_events (intent_id, id);

CREATE TABLE IF NOT EXISTS slot_gaps (
    id BIGSERIAL PRIMARY KEY,
    cluster TEXT NOT NULL DEFAULT 'mainnet-beta' CHECK (cluster IN ('localnet', 'devnet', 'mainnet-beta')),
    start_slot BIGINT NOT NULL,
    end_slot BIGINT NOT NULL,
    reason TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'backfilling', 'resolved', 'unavailable')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at TIMESTAMPTZ,
    CHECK (end_slot >= start_slot)
);

ALTER TABLE slot_gaps
    ADD COLUMN IF NOT EXISTS cluster TEXT NOT NULL DEFAULT 'mainnet-beta';

DROP INDEX IF EXISTS slot_gaps_open_idx;
CREATE INDEX IF NOT EXISTS slot_gaps_open_idx
    ON slot_gaps (cluster, created_at)
    WHERE status IN ('open', 'backfilling');

CREATE TABLE IF NOT EXISTS slot_canonicality (
    cluster TEXT NOT NULL DEFAULT 'mainnet-beta' CHECK (cluster IN ('localnet', 'devnet', 'mainnet-beta')),
    slot BIGINT NOT NULL,
    blockhash TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('observed', 'confirmed', 'finalized', 'orphaned')),
    parent_slot BIGINT,
    source TEXT NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (cluster, slot, blockhash)
);

ALTER TABLE slot_canonicality
    ADD COLUMN IF NOT EXISTS cluster TEXT NOT NULL DEFAULT 'mainnet-beta';

ALTER TABLE slot_canonicality
    DROP CONSTRAINT IF EXISTS slot_canonicality_pkey;

ALTER TABLE slot_canonicality
    ADD CONSTRAINT slot_canonicality_pkey PRIMARY KEY (cluster, slot, blockhash);

ALTER TABLE slot_observations
    ADD COLUMN IF NOT EXISTS cluster TEXT NOT NULL DEFAULT 'mainnet-beta';

ALTER TABLE slot_observations
    DROP CONSTRAINT IF EXISTS slot_observations_pkey;

ALTER TABLE slot_observations
    ADD CONSTRAINT slot_observations_pkey PRIMARY KEY (cluster, slot, blockhash, commitment);

DROP INDEX IF EXISTS slot_canonicality_finalized_idx;
CREATE INDEX IF NOT EXISTS slot_canonicality_finalized_idx
    ON slot_canonicality (cluster, slot DESC)
    WHERE status = 'finalized';

COMMIT;
