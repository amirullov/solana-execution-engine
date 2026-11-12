import { loadConfig, loadProjectEnv } from './config.js';
import { PostgresExecutionStore } from './store.js';

loadProjectEnv();
const config = loadConfig();
if (!config.DATABASE_URL) throw new Error('DATABASE_URL is required to apply migrations');

const store = new PostgresExecutionStore(config.DATABASE_URL);
try {
  await store.migrate();
} finally {
  await store.close();
}
