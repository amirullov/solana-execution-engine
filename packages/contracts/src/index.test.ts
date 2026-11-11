import { describe, expect, it } from 'vitest';

import { executionIntentSchema, executionModeSchema } from './index.js';

const validIntent = {
  idempotencyKey: 'order-0001',
  cluster: 'devnet',
  inputMint: 'So11111111111111111111111111111111111111112',
  outputMint: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
  amountAtomic: '1000000',
  maxSlippageBps: 50,
};

describe('contracts', () => {
  it('accepts a simulation intent', () => {
    expect(executionIntentSchema.parse(validIntent)).toMatchObject(validIntent);
  });

  it('rejects an unsafe execution mode', () => {
    expect(() => executionModeSchema.parse('live')).toThrow();
  });

  it('rejects floating-point token amounts', () => {
    expect(() => executionIntentSchema.parse({ ...validIntent, amountAtomic: '1.5' })).toThrow();
  });
});
