import { describe, expect, it, vi } from 'vitest';

import { buildApp } from './app.js';
import { loadConfig } from './config.js';
import type { SavedQuote } from './jupiter.js';
import { MemoryExecutionStore } from './store.js';

const config = loadConfig({ APP_ENV: 'test', EXECUTION_MODE: 'disabled' });

describe('control-api', () => {
  it('reports fail-closed execution mode', async () => {
    const app = buildApp(config, { store: new MemoryExecutionStore() });
    const response = await app.inject({ method: 'GET', url: '/health' });

    expect(response.statusCode).toBe(200);
    expect(response.json()).toMatchObject({ status: 'ok', executionMode: 'disabled' });
    await app.close();
  });

  it('accepts idempotent intents while execution remains disabled', async () => {
    const app = buildApp(config, { store: new MemoryExecutionStore() });
    const payload = {
      idempotencyKey: 'order-0001',
      cluster: 'devnet',
      inputMint: 'So11111111111111111111111111111111111111112',
      outputMint: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
      amountAtomic: '1000000',
      maxSlippageBps: 50,
    };
    const response = await app.inject({
      method: 'POST',
      url: '/v1/intents',
      payload,
    });

    expect(response.statusCode).toBe(202);
    expect(response.json()).toMatchObject({ status: 'received', executionMode: 'disabled' });

    const duplicate = await app.inject({ method: 'POST', url: '/v1/intents', payload });
    expect(duplicate.statusCode).toBe(200);
    expect(duplicate.json()).toMatchObject({ duplicate: true });

    const conflict = await app.inject({
      method: 'POST',
      url: '/v1/intents',
      payload: { ...payload, amountAtomic: '1000001' },
    });
    expect(conflict.statusCode).toBe(409);
    await app.close();
  });

  it('rejects a malformed amount', async () => {
    const app = buildApp(config, { store: new MemoryExecutionStore() });
    const response = await app.inject({
      method: 'POST',
      url: '/v1/intents',
      payload: {
        idempotencyKey: 'order-0002',
        inputMint: 'So11111111111111111111111111111111111111112',
        outputMint: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
        amountAtomic: '1.5',
        maxSlippageBps: 50,
      },
    });

    expect(response.statusCode).toBe(400);
    expect(response.json()).toMatchObject({ error: 'invalid_intent' });
    await app.close();
  });

  it('persists and returns a read-only quote', async () => {
    const savedQuote: SavedQuote = {
      cluster: 'mainnet-beta',
      inputMint: 'So11111111111111111111111111111111111111112',
      outputMint: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
      inputAmountAtomic: '1000000',
      outputAmountAtomic: '2200000',
      provider: 'fixture-router',
      responseSha256: 'a'.repeat(64),
      rawResponse: { requestId: 'fixture-request' },
    };
    const getQuote = vi.fn(() => Promise.resolve(savedQuote));
    const app = buildApp(config, {
      store: new MemoryExecutionStore(),
      quoteClient: { getQuote },
    });
    const response = await app.inject({
      method: 'POST',
      url: '/v1/quotes',
      payload: {
        cluster: 'mainnet-beta',
        inputMint: savedQuote.inputMint,
        outputMint: savedQuote.outputMint,
        amountAtomic: savedQuote.inputAmountAtomic,
      },
    });

    expect(response.statusCode).toBe(200);
    expect(response.json()).toMatchObject({
      inputAmountAtomic: '1000000',
      outputAmountAtomic: '2200000',
      provider: 'fixture-router',
    });
    expect(getQuote).toHaveBeenCalledTimes(1);
    await app.close();
  });

  it('rejects unsupported quote clusters before contacting the provider', async () => {
    const getQuote = vi.fn(() => Promise.reject(new Error('must not run')));
    const app = buildApp(config, {
      store: new MemoryExecutionStore(),
      quoteClient: { getQuote },
    });
    const response = await app.inject({
      method: 'POST',
      url: '/v1/quotes',
      payload: {
        cluster: 'devnet',
        inputMint: 'So11111111111111111111111111111111111111112',
        outputMint: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
        amountAtomic: '1000000',
      },
    });

    expect(response.statusCode).toBe(422);
    expect(getQuote).not.toHaveBeenCalled();
    await app.close();
  });
});
