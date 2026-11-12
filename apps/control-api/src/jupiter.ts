import { createHash } from 'node:crypto';

import { z } from 'zod';

import type { AppConfig } from './config.js';

export const quoteRequestSchema = z
  .object({
    cluster: z.enum(['localnet', 'devnet', 'mainnet-beta']).default('mainnet-beta'),
    inputMint: z.string().regex(/^[1-9A-HJ-NP-Za-km-z]{32,44}$/),
    outputMint: z.string().regex(/^[1-9A-HJ-NP-Za-km-z]{32,44}$/),
    amountAtomic: z
      .string()
      .regex(/^[1-9][0-9]*$/)
      .refine(
        (amount) => BigInt(amount) <= 18_446_744_073_709_551_615n,
        'amount must fit in Solana u64',
      ),
  })
  .strict()
  .refine((request) => request.inputMint !== request.outputMint, {
    message: 'inputMint and outputMint must differ',
    path: ['outputMint'],
  });

const jupiterOrderSchema = z.object({
  requestId: z.string().optional(),
  inputMint: z.string(),
  inAmount: z.string().regex(/^[0-9]+$/),
  outputMint: z.string(),
  outAmount: z.string().regex(/^[0-9]+$/),
  otherAmountThreshold: z
    .string()
    .regex(/^[0-9]+$/)
    .optional(),
  router: z.string().min(1),
  feeBps: z.number().int().nonnegative().optional(),
  feeMint: z.string().optional(),
  priceImpactPct: z.string().optional(),
  contextSlot: z.number().int().nonnegative().optional(),
  transaction: z.union([z.string(), z.null()]).optional(),
});

export type QuoteRequest = z.infer<typeof quoteRequestSchema>;

export interface QuoteResult {
  id: string;
  cluster: QuoteRequest['cluster'];
  inputMint: string;
  outputMint: string;
  inputAmountAtomic: string;
  outputAmountAtomic: string;
  minimumOutputAtomic?: string;
  provider: string;
  providerRequestId?: string;
  contextSlot?: number;
  feeBps?: number;
  feeMint?: string;
  priceImpactPct?: string;
  responseSha256: string;
  receivedAt: string;
}

export interface SavedQuote extends Omit<QuoteResult, 'id' | 'receivedAt'> {
  rawResponse: Record<string, unknown>;
}

export interface QuotePersistence {
  saveQuote(quote: SavedQuote): Promise<string>;
}

export type FetchLike = typeof fetch;

export class JupiterQuoteClient {
  readonly #baseUrl: URL;
  readonly #apiKey?: string;
  readonly #fetch: FetchLike;

  constructor(config: AppConfig, fetcher: FetchLike = fetch) {
    this.#baseUrl = new URL(config.JUPITER_BASE_URL);
    if (config.JUPITER_API_KEY) this.#apiKey = config.JUPITER_API_KEY;
    this.#fetch = fetcher;
  }

  async getQuote(request: QuoteRequest): Promise<SavedQuote> {
    if (request.cluster !== 'mainnet-beta') {
      throw new JupiterProviderError('Jupiter quote provider currently supports mainnet-beta only');
    }
    const url = new URL(
      'order',
      this.#baseUrl.href.endsWith('/') ? this.#baseUrl : `${this.#baseUrl.href}/`,
    );
    url.search = new URLSearchParams({
      inputMint: request.inputMint,
      outputMint: request.outputMint,
      amount: request.amountAtomic,
    }).toString();

    const headers = new Headers({ accept: 'application/json' });
    if (this.#apiKey) headers.set('x-api-key', this.#apiKey);

    const response = await this.#fetch(url, {
      method: 'GET',
      headers,
      signal: AbortSignal.timeout(5_000),
    });

    if (!response.ok) {
      throw new JupiterProviderError(
        `Jupiter quote request failed with HTTP ${response.status.toString()}`,
        response.status,
      );
    }

    const rawText = await response.text();
    let rawResponse: unknown;
    try {
      rawResponse = JSON.parse(rawText);
    } catch {
      throw new JupiterProviderError('Jupiter returned invalid JSON');
    }

    const parsed = jupiterOrderSchema.safeParse(rawResponse);
    if (!parsed.success) {
      throw new JupiterProviderError('Jupiter response did not match the supported schema');
    }

    const order = parsed.data;
    if (
      order.inputMint !== request.inputMint ||
      order.outputMint !== request.outputMint ||
      order.inAmount !== request.amountAtomic
    ) {
      throw new JupiterProviderError(
        'Jupiter response does not match the requested pair and amount',
      );
    }
    if (order.transaction !== undefined && order.transaction !== null && order.transaction !== '') {
      throw new JupiterProviderError('Quote-only request unexpectedly returned a transaction');
    }

    return {
      cluster: request.cluster,
      inputMint: order.inputMint,
      outputMint: order.outputMint,
      inputAmountAtomic: order.inAmount,
      outputAmountAtomic: order.outAmount,
      ...(order.otherAmountThreshold === undefined
        ? {}
        : { minimumOutputAtomic: order.otherAmountThreshold }),
      provider: order.router,
      ...(order.requestId === undefined ? {} : { providerRequestId: order.requestId }),
      ...(order.contextSlot === undefined ? {} : { contextSlot: order.contextSlot }),
      ...(order.feeBps === undefined ? {} : { feeBps: order.feeBps }),
      ...(order.feeMint === undefined ? {} : { feeMint: order.feeMint }),
      ...(order.priceImpactPct === undefined ? {} : { priceImpactPct: order.priceImpactPct }),
      responseSha256: createHash('sha256').update(rawText).digest('hex'),
      rawResponse: rawResponse as Record<string, unknown>,
    };
  }
}

export class JupiterProviderError extends Error {
  readonly statusCode?: number;

  constructor(message: string, statusCode?: number) {
    super(message);
    this.name = 'JupiterProviderError';
    if (statusCode !== undefined) this.statusCode = statusCode;
  }
}
