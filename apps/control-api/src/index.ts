import { buildApp } from './app.js';
import { loadConfig, loadProjectEnv } from './config.js';
import { PostgresExecutionStore } from './store.js';

loadProjectEnv();
const config = loadConfig();
if (!config.DATABASE_URL) throw new Error('DATABASE_URL is required outside injected tests');
const store = new PostgresExecutionStore(config.DATABASE_URL);
await store.migrate();
const app = buildApp(config, { store });

const close = async (signal: string): Promise<void> => {
  app.log.info({ signal }, 'shutting down');
  await app.close();
  process.exitCode = 0;
};

process.once('SIGINT', () => void close('SIGINT'));
process.once('SIGTERM', () => void close('SIGTERM'));

await app.listen({ host: config.CONTROL_API_HOST, port: config.CONTROL_API_PORT });
