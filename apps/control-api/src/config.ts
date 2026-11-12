import { z } from 'zod';

import { existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const configSchema = z.object({
  APP_ENV: z.enum(['local', 'test', 'staging', 'production']).default('local'),
  LOG_LEVEL: z.enum(['fatal', 'error', 'warn', 'info', 'debug', 'trace', 'silent']).default('info'),
  EXECUTION_MODE: z.literal('disabled').default('disabled'),
  CONTROL_API_HOST: z.string().default('127.0.0.1'),
  CONTROL_API_PORT: z.coerce.number().int().min(1).max(65_535).default(3000),
  DATABASE_URL: z.url().optional(),
  REDIS_URL: z.url().optional(),
  JUPITER_BASE_URL: z.url().default('https://api.jup.ag/swap/v2'),
  JUPITER_API_KEY: z.preprocess(
    (value) => (value === '' ? undefined : value),
    z.string().min(1).optional(),
  ),
});

export type AppConfig = z.infer<typeof configSchema>;

export function loadConfig(environment: NodeJS.ProcessEnv = process.env): AppConfig {
  return configSchema.parse(environment);
}

export function loadProjectEnv(): void {
  const envFile = resolve(dirname(fileURLToPath(import.meta.url)), '../../../.env');
  if (existsSync(envFile)) process.loadEnvFile(envFile);
}
