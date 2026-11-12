import { describe, expect, it, vi } from 'vitest';

import { loadConfig } from './config.js';
import { JupiterProviderError, JupiterQuoteClient, quoteRequestSchema } from './jupiter.js';

const request = quoteRequestSchema.parse({
  inputMint: 'So11111111111111111111111111111111111111112',
  outputMint: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
  amountAtomic: '9007199254740993',
});

const response = {
  requestId: 'req-123',
  inputMint: request.inputMint,
  inAmount: request.amountAtomic,
  outputMint: request.outputMint,
  outAmount: '21000000000000000',
  otherAmountThreshold: '20900000000000000',
  router: 'metis',
  feeBps: 5,
  feeMint: request.outputMint,
  priceImpactPct: '0.001',
  contextSlot: 400,
  transaction: null,
};

describe('JupiterQuoteClient', () => {
  it('requests quote-only without a taker and preserves atomic integers as strings', async () => {
    const fetcher = vi.fn<typeof fetch>(() =>
      Promise.resolve(new Response(JSON.stringify(response))),
    );
    const client = new JupiterQuoteClient(loadConfig({ APP_ENV: 'test' }), fetcher);
    const quote = await client.getQuote(request);

    const [url] = fetcher.mock.calls[0] ?? [];
    if (!url) throw new Error('Jupiter fetch was not called');
    const parsedUrl = new URL(
      url instanceof URL ? url.href : url instanceof Request ? url.url : url,
    );
    expect(parsedUrl.pathname).toBe('/swap/v2/order');
    expect(parsedUrl.searchParams.get('taker')).toBeNull();
    expect(quote.inputAmountAtomic).toBe(request.amountAtomic);
    expect(quote.outputAmountAtomic).toBe(response.outAmount);
    expect(quote.minimumOutputAtomic).toBe(response.otherAmountThreshold);
    expect(quote.responseSha256).toMatch(/^[a-f0-9]{64}$/);
  });

  it('rejects devnet quotes instead of labeling a mainnet quote as devnet', async () => {
    const fetcher = vi.fn<typeof fetch>();
    const client = new JupiterQuoteClient(loadConfig({ APP_ENV: 'test' }), fetcher);

    await expect(client.getQuote({ ...request, cluster: 'devnet' })).rejects.toThrow(
      'mainnet-beta only',
    );
    expect(fetcher).not.toHaveBeenCalled();
  });

  it('fails closed when the provider returns a transaction for a quote-only request', async () => {
    const fetcher = vi.fn<typeof fetch>(() =>
      Promise.resolve(new Response(JSON.stringify({ ...response, transaction: 'AQID' }))),
    );
    const client = new JupiterQuoteClient(loadConfig({ APP_ENV: 'test' }), fetcher);

    await expect(client.getQuote(request)).rejects.toBeInstanceOf(JupiterProviderError);
  });

  it('rejects a response with a different pair or amount', async () => {
    const fetcher = vi.fn<typeof fetch>(() =>
      Promise.resolve(new Response(JSON.stringify({ ...response, inAmount: '1' }))),
    );
    const client = new JupiterQuoteClient(loadConfig({ APP_ENV: 'test' }), fetcher);

    await expect(client.getQuote(request)).rejects.toThrow('does not match');
  });

  it('does not leak upstream response bodies from HTTP errors', async () => {
    const fetcher = vi.fn<typeof fetch>(() =>
      Promise.resolve(new Response('secret detail', { status: 429 })),
    );
    const client = new JupiterQuoteClient(loadConfig({ APP_ENV: 'test' }), fetcher);

    await expect(client.getQuote(request)).rejects.toThrow('HTTP 429');
  });
});
