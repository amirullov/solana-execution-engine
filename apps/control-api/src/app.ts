import { executionIntentSchema } from '@execution/contracts';
import Fastify, { type FastifyInstance } from 'fastify';

import type { AppConfig } from './config.js';
import { JupiterProviderError, JupiterQuoteClient, quoteRequestSchema } from './jupiter.js';
import { IdempotencyConflictError, MemoryExecutionStore, type ExecutionStore } from './store.js';

export interface AppDependencies {
  store?: ExecutionStore;
  quoteClient?: Pick<JupiterQuoteClient, 'getQuote'>;
}

export function buildApp(config: AppConfig, dependencies: AppDependencies = {}): FastifyInstance {
  const store = dependencies.store ?? new MemoryExecutionStore();
  const quoteClient = dependencies.quoteClient ?? new JupiterQuoteClient(config);
  const app = Fastify({
    logger: config.APP_ENV === 'test' ? false : { level: config.LOG_LEVEL },
    requestIdHeader: 'x-request-id',
  });

  app.get('/health', () => ({
    service: 'control-api',
    status: 'ok',
    executionMode: config.EXECUTION_MODE,
  }));

  app.get('/ready', async (_request, reply) => {
    const ready = await store.ready();
    return reply.status(ready ? 200 : 503).send({
      service: 'control-api',
      status: ready ? 'ready' : 'degraded',
      dependencies: { postgres: ready ? 'ready' : 'unavailable' },
    });
  });

  app.addHook('onClose', async () => store.close());

  app.post('/v1/intents', async (request, reply) => {
    const parsed = executionIntentSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.status(400).send({
        error: 'invalid_intent',
        issues: parsed.error.issues,
      });
    }

    try {
      const saved = await store.saveIntent(parsed.data);
      return await reply.status(saved.created ? 202 : 200).send({
        id: saved.id,
        status: 'received',
        executionMode: config.EXECUTION_MODE,
        duplicate: !saved.created,
      });
    } catch (error) {
      if (error instanceof IdempotencyConflictError) {
        return reply.status(409).send({ error: 'idempotency_conflict' });
      }
      request.log.error({ err: error }, 'failed to persist execution intent');
      return reply.status(503).send({ error: 'storage_unavailable' });
    }
  });

  app.post('/v1/quotes', async (request, reply) => {
    const parsed = quoteRequestSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply
        .status(400)
        .send({ error: 'invalid_quote_request', issues: parsed.error.issues });
    }
    if (parsed.data.cluster !== 'mainnet-beta') {
      return reply.status(422).send({ error: 'quote_cluster_not_supported' });
    }

    try {
      const savedQuote = await quoteClient.getQuote(parsed.data);
      const id = await store.saveQuote(savedQuote);
      const quote = await store.readQuote(id);
      if (!quote) throw new Error('persisted quote could not be read back');
      return await reply.status(200).send(quote);
    } catch (error) {
      if (error instanceof JupiterProviderError) {
        request.log.warn(
          { statusCode: error.statusCode },
          'Jupiter quote provider rejected request',
        );
        return reply.status(502).send({ error: 'quote_provider_unavailable' });
      }
      request.log.error({ err: error }, 'failed to obtain or persist Jupiter quote');
      return reply.status(503).send({ error: 'quote_service_unavailable' });
    }
  });

  app.get<{ Params: { id: string } }>('/v1/quotes/:id', async (request, reply) => {
    const quote = await store.readQuote(request.params.id);
    if (!quote) return reply.status(404).send({ error: 'quote_not_found' });
    return reply.send(quote);
  });

  return app;
}
