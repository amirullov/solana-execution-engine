import { randomUUID } from 'node:crypto';

import { executionIntentSchema } from '@execution/contracts';
import Fastify, { type FastifyInstance } from 'fastify';

import type { AppConfig } from './config.js';

export function buildApp(config: AppConfig): FastifyInstance {
  const app = Fastify({
    logger: config.APP_ENV === 'test' ? false : { level: config.LOG_LEVEL },
    requestIdHeader: 'x-request-id',
  });

  app.get('/health', () => ({
    service: 'control-api',
    status: 'ok',
    executionMode: config.EXECUTION_MODE,
  }));

  app.get('/ready', () => ({
    service: 'control-api',
    status: 'ready',
    dependencies: 'not_required_in_foundation',
  }));

  app.post('/v1/intents', (request, reply) => {
    const parsed = executionIntentSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.status(400).send({
        error: 'invalid_intent',
        issues: parsed.error.issues,
      });
    }

    return reply.status(202).send({
      id: randomUUID(),
      status: 'simulation_only',
      executionMode: config.EXECUTION_MODE,
    });
  });

  return app;
}
