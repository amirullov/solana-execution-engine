# Solana Execution Engine

Production-oriented основа для безопасного исполнения swap-намерений в Solana. Это второй самостоятельный продукт Portfolio Web3; репозиторий Solana Sentinel не изменён.

> Текущий статус: **Stage 1 / Foundation**. Выполнение транзакций, подпись и broadcast намеренно отсутствуют. Единственный режим — `EXECUTION_MODE=disabled`; API принимает только simulation intents.

## Что уже создано

- Rust workspace: доменная модель/policy, `execution-engine`, `market-data`;
- TypeScript workspace: control API и общие Zod-контракты;
- PostgreSQL 18 + Redis 8 через Docker Compose;
- начальная SQL-схема для intents, quote snapshots, checkpoints, slot observations и outbox;
- health/readiness endpoints и структурированные логи;
- GitHub Actions для TypeScript, Rust и Docker Compose;
- unit/API tests, форматирование, lint и строгий typecheck;
- [полный production-grade SDD](docs/SDD.ru.md) и четыре ADR.

## Границы безопасности Foundation

- нет приватных ключей и переменной окружения для них;
- нет signer adapter;
- нет `sendTransaction`, Jupiter `/execute` или другого broadcast-кода;
- Rust policy всегда возвращает запрет исполнения;
- конфигурация с `EXECUTION_MODE`, отличным от `disabled`, не запускается.

## Требования

- Node.js 24+
- pnpm 11.25+
- Rust 1.98.1
- Docker с Compose

По умолчанию инфраструктура использует host-порты `55433` (PostgreSQL) и `56379` (Redis), чтобы не конфликтовать с Sentinel и другими локальными проектами.

## Быстрый старт

```bash
cp .env.example .env
pnpm install
pnpm db:up
pnpm check
pnpm --filter @execution/control-api dev
```

Rust-сервисы запускаются отдельно:

```bash
cargo run -p execution-engine
cargo run -p market-data
```

Проверка API:

```bash
curl http://127.0.0.1:3000/health
curl http://127.0.0.1:3001/health
curl http://127.0.0.1:3002/health
```

## Структура

```text
apps/control-api/             TypeScript control plane
packages/contracts/           versioned wire contracts
crates/engine-core/           Rust domain model and fail-closed policy
services/execution-engine/    Rust simulation-only data plane
services/market-data/         Rust ingestion process shell (disconnected)
infra/postgres/migrations/    durable schema
docs/SDD.ru.md                production design and roadmap
docs/adr/                     architecture decisions
```

## Следующий безопасный шаг

Stage 2 добавляет read-only RPC/Yellowstone ingestion, canonical slot ledger, RPC backfill и quote-only Jupiter v2 adapter. Даже на этом этапе signing и real-money execution остаются запрещены.
