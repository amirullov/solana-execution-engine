import { describe, expect, it } from 'vitest';

import { buildApp } from './app.js';
import { loadConfig } from './config.js';

const config = loadConfig({ APP_ENV: 'test', EXECUTION_MODE: 'disabled' });

describe('control-api', () => {
  it('reports fail-closed execution mode', async () => {
    const app = buildApp(config);
    const response = await app.inject({ method: 'GET', url: '/health' });

    expect(response.statusCode).toBe(200);
    expect(response.json()).toMatchObject({ status: 'ok', executionMode: 'disabled' });
    await app.close();
  });

  it('accepts only a simulation intent', async () => {
    const app = buildApp(config);
    const response = await app.inject({
      method: 'POST',
      url: '/v1/intents',
      payload: {
        idempotencyKey: 'order-0001',
        cluster: 'devnet',
        inputMint: 'So11111111111111111111111111111111111111112',
        outputMint: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
        amountAtomic: '1000000',
        maxSlippageBps: 50,
      },
    });

    expect(response.statusCode).toBe(202);
    expect(response.json()).toMatchObject({ status: 'simulation_only', executionMode: 'disabled' });
    await app.close();
  });

  it('rejects a malformed amount', async () => {
    const app = buildApp(config);
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
});
