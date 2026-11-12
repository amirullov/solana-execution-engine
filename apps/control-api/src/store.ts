import { randomUUID } from 'node:crypto';
import { readdir, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { Pool, type PoolClient } from 'pg';

import type { ExecutionIntent } from '@execution/contracts';

import type { QuotePersistence, QuoteResult, SavedQuote } from './jupiter.js';

export interface IntentSaveResult {
  id: string;
  created: boolean;
}

export interface ExecutionStore extends QuotePersistence {
  ready(): Promise<boolean>;
  saveIntent(intent: ExecutionIntent): Promise<IntentSaveResult>;
  readQuote(id: string): Promise<QuoteResult | null>;
  close(): Promise<void>;
}

export class PostgresExecutionStore implements ExecutionStore {
  readonly #pool: Pool;

  constructor(connectionString: string) {
    this.#pool = new Pool({ connectionString, max: 10, connectionTimeoutMillis: 2_000 });
  }

  async migrate(): Promise<void> {
    const migrationsPath = resolve(
      dirname(fileURLToPath(import.meta.url)),
      '../../../infra/postgres/migrations',
    );
    const files = (await readdir(migrationsPath)).filter((file) => file.endsWith('.sql')).sort();
    const client = await this.#pool.connect();
    try {
      await client.query(`CREATE TABLE IF NOT EXISTS schema_migrations (
        version TEXT PRIMARY KEY,
        applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
      )`);

      for (const file of files) {
        const source = await readFile(resolve(migrationsPath, file), 'utf8');
        const sql = source.replace(/^\s*BEGIN\s*;/i, '').replace(/COMMIT\s*;\s*$/i, '');
        await client.query('BEGIN');
        try {
          await client.query('SELECT pg_advisory_xact_lock(194857391)');
          const alreadyApplied = await client.query(
            'SELECT 1 FROM schema_migrations WHERE version = $1',
            [file],
          );
          if (alreadyApplied.rowCount) {
            await client.query('COMMIT');
            continue;
          }
          await client.query(sql);
          await client.query('INSERT INTO schema_migrations (version) VALUES ($1)', [file]);
          await client.query('COMMIT');
        } catch (error) {
          await rollbackIfOpen(client);
          throw error;
        }
      }
    } finally {
      client.release();
    }
  }

  async ready(): Promise<boolean> {
    try {
      await this.#pool.query('SELECT 1');
      return true;
    } catch {
      return false;
    }
  }

  async saveIntent(intent: ExecutionIntent): Promise<IntentSaveResult> {
    const client = await this.#pool.connect();
    try {
      await client.query('BEGIN');
      const inserted = await client.query<{ id: string }>(
        `INSERT INTO execution_intents
          (id, idempotency_key, cluster, input_mint, output_mint, amount_atomic,
           max_slippage_bps, status, request_payload)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'received', $8::jsonb)
         ON CONFLICT (idempotency_key) DO NOTHING
         RETURNING id`,
        [
          randomUUID(),
          intent.idempotencyKey,
          intent.cluster,
          intent.inputMint,
          intent.outputMint,
          intent.amountAtomic,
          intent.maxSlippageBps,
          JSON.stringify(intent),
        ],
      );

      if (inserted.rowCount === 0) {
        const existing = await client.query<{
          id: string;
          cluster: string;
          input_mint: string;
          output_mint: string;
          amount_atomic: string;
          max_slippage_bps: number;
        }>(
          `SELECT id, cluster, input_mint, output_mint, amount_atomic::text, max_slippage_bps
           FROM execution_intents WHERE idempotency_key = $1 FOR UPDATE`,
          [intent.idempotencyKey],
        );
        const row = existing.rows[0];
        if (!row || !sameIntent(row, intent)) {
          await client.query('ROLLBACK');
          throw new IdempotencyConflictError();
        }
        await client.query('COMMIT');
        return { id: row.id, created: false };
      }

      const id = inserted.rows[0]?.id;
      if (!id) throw new Error('PostgreSQL did not return the inserted intent id');

      const eventPayload = JSON.stringify({ intentId: id, state: 'received' });
      await client.query(
        `INSERT INTO intent_events (intent_id, event_type, event_payload)
         VALUES ($1, 'intent.received', $2::jsonb)`,
        [id, eventPayload],
      );
      await client.query(
        `INSERT INTO outbox_events (id, aggregate_type, aggregate_id, event_type, payload)
         VALUES ($1, 'execution_intent', $2, 'intent.received', $3::jsonb)`,
        [randomUUID(), id, eventPayload],
      );
      await client.query('COMMIT');
      return { id, created: true };
    } catch (error) {
      await rollbackIfOpen(client);
      throw error;
    } finally {
      client.release();
    }
  }

  async saveQuote(quote: SavedQuote): Promise<string> {
    const result = await this.#pool.query<{ id: string }>(
      `INSERT INTO quote_requests
        (id, cluster, input_mint, output_mint, input_amount_atomic,
         output_amount_atomic, minimum_output_atomic, provider, provider_request_id,
         provider_context_slot, fee_bps, fee_mint, price_impact_pct,
         response_sha256, response_payload)
       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15::jsonb)
       RETURNING id`,
      [
        randomUUID(),
        quote.cluster,
        quote.inputMint,
        quote.outputMint,
        quote.inputAmountAtomic,
        quote.outputAmountAtomic,
        quote.minimumOutputAtomic ?? null,
        quote.provider,
        quote.providerRequestId ?? null,
        quote.contextSlot ?? null,
        quote.feeBps ?? null,
        quote.feeMint ?? null,
        quote.priceImpactPct ?? null,
        quote.responseSha256,
        JSON.stringify(quote.rawResponse),
      ],
    );
    const id = result.rows[0]?.id;
    if (!id) throw new Error('PostgreSQL did not return the quote id');
    return id;
  }

  async readQuote(id: string): Promise<QuoteResult | null> {
    const result = await this.#pool.query<{
      id: string;
      cluster: QuoteResult['cluster'];
      input_mint: string;
      output_mint: string;
      input_amount_atomic: string;
      output_amount_atomic: string;
      minimum_output_atomic: string | null;
      provider: string;
      provider_request_id: string | null;
      provider_context_slot: string | null;
      fee_bps: number | null;
      fee_mint: string | null;
      price_impact_pct: string | null;
      response_sha256: string;
      created_at: Date;
    }>(
      `SELECT id, cluster, input_mint, output_mint, input_amount_atomic::text,
              output_amount_atomic::text, minimum_output_atomic::text, provider, provider_request_id,
              provider_context_slot, fee_bps, fee_mint, price_impact_pct, response_sha256, created_at
       FROM quote_requests WHERE id = $1`,
      [id],
    );
    const row = result.rows[0];
    if (!row) return null;
    return {
      id: row.id,
      cluster: row.cluster,
      inputMint: row.input_mint,
      outputMint: row.output_mint,
      inputAmountAtomic: row.input_amount_atomic,
      outputAmountAtomic: row.output_amount_atomic,
      ...(row.minimum_output_atomic === null
        ? {}
        : { minimumOutputAtomic: row.minimum_output_atomic }),
      provider: row.provider,
      ...(row.provider_request_id === null ? {} : { providerRequestId: row.provider_request_id }),
      ...(row.provider_context_slot === null
        ? {}
        : { contextSlot: Number(row.provider_context_slot) }),
      ...(row.fee_bps === null ? {} : { feeBps: row.fee_bps }),
      ...(row.fee_mint === null ? {} : { feeMint: row.fee_mint }),
      ...(row.price_impact_pct === null ? {} : { priceImpactPct: row.price_impact_pct }),
      responseSha256: row.response_sha256,
      receivedAt: row.created_at.toISOString(),
    };
  }

  async close(): Promise<void> {
    await this.#pool.end();
  }
}

export class MemoryExecutionStore implements ExecutionStore {
  readonly #intents = new Map<string, { id: string; intent: ExecutionIntent }>();
  readonly #quotes = new Map<string, QuoteResult>();

  ready(): Promise<boolean> {
    return Promise.resolve(true);
  }

  saveIntent(intent: ExecutionIntent): Promise<IntentSaveResult> {
    const existing = this.#intents.get(intent.idempotencyKey);
    if (existing) {
      if (!sameIntent(existing.intent, intent)) throw new IdempotencyConflictError();
      return Promise.resolve({ id: existing.id, created: false });
    }
    const id = randomUUID();
    this.#intents.set(intent.idempotencyKey, { id, intent });
    return Promise.resolve({ id, created: true });
  }

  saveQuote(quote: SavedQuote): Promise<string> {
    const id = randomUUID();
    this.#quotes.set(id, quoteResultFromSaved(id, quote));
    return Promise.resolve(id);
  }

  readQuote(id: string): Promise<QuoteResult | null> {
    return Promise.resolve(this.#quotes.get(id) ?? null);
  }

  close(): Promise<void> {
    return Promise.resolve();
  }
}

export class IdempotencyConflictError extends Error {
  constructor() {
    super('idempotency key already exists for a different intent');
    this.name = 'IdempotencyConflictError';
  }
}

function sameIntent(
  left: {
    cluster: string;
    input_mint?: string;
    inputMint?: string;
    output_mint?: string;
    outputMint?: string;
    amount_atomic?: string;
    amountAtomic?: string;
    max_slippage_bps?: number;
    maxSlippageBps?: number;
  },
  right: ExecutionIntent,
): boolean {
  return (
    left.cluster === right.cluster &&
    (left.input_mint ?? left.inputMint) === right.inputMint &&
    (left.output_mint ?? left.outputMint) === right.outputMint &&
    (left.amount_atomic ?? left.amountAtomic) === right.amountAtomic &&
    (left.max_slippage_bps ?? left.maxSlippageBps) === right.maxSlippageBps
  );
}

function quoteResultFromSaved(id: string, quote: SavedQuote): QuoteResult {
  return {
    id,
    cluster: quote.cluster,
    inputMint: quote.inputMint,
    outputMint: quote.outputMint,
    inputAmountAtomic: quote.inputAmountAtomic,
    outputAmountAtomic: quote.outputAmountAtomic,
    ...(quote.minimumOutputAtomic === undefined
      ? {}
      : { minimumOutputAtomic: quote.minimumOutputAtomic }),
    provider: quote.provider,
    ...(quote.providerRequestId === undefined
      ? {}
      : { providerRequestId: quote.providerRequestId }),
    ...(quote.contextSlot === undefined ? {} : { contextSlot: quote.contextSlot }),
    ...(quote.feeBps === undefined ? {} : { feeBps: quote.feeBps }),
    ...(quote.feeMint === undefined ? {} : { feeMint: quote.feeMint }),
    ...(quote.priceImpactPct === undefined ? {} : { priceImpactPct: quote.priceImpactPct }),
    responseSha256: quote.responseSha256,
    receivedAt: new Date().toISOString(),
  };
}

async function rollbackIfOpen(client: PoolClient): Promise<void> {
  try {
    await client.query('ROLLBACK');
  } catch {
    // Preserve the original database error.
  }
}
