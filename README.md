# Solana Execution Engine

Второй продукт Portfolio Web3: отдельная система для безопасного управления swap-намерениями и данными Solana. Проект хранится отдельно от Solana Sentinel; Sentinel не изменяется.

Полная спецификация на русском языке находится в [`docs/SDD.ru.md`](docs/SDD.ru.md), решения по архитектуре — в [`docs/adr/`](docs/adr/).

## Текущее состояние

Реализована основа и начат Stage 2 с read-only интеграциями:

- Rust workspace с доменной моделью, fail-closed policy и сервисами `execution-engine` и `market-data`;
- TypeScript API и общие Zod-контракты;
- PostgreSQL 18 и Redis 8 в Docker Compose;
- durable idempotency для intents, audit/outbox записи и SQL migrations;
- Jupiter Swap API v2 quote-only через `/order`, с проверкой пары/суммы, SHA-256 исходного ответа и сохранением результата; провайдер сейчас ограничен `mainnet-beta`;
- finalized RPC backfill с durable checkpoint и canonical slot facts; Yellowstone confirmed stream с replay включается переменной `YELLOWSTONE_ENDPOINT` на Linux;
- health, readiness, базовые metrics, структурные логи, GitHub Actions и тесты;
- production SDD с угрозами, trade-offs, измеримыми критериями, этапами и рисками.

**Проект ещё не готов к production.** Нет реальной симуляции транзакций с проверкой инструкций и ALT, полноценного policy rollout, signer, broadcaster, confirmer, production runbooks, SLO measurements и независимого security audit.

## Защита активов

- `EXECUTION_MODE` допускает только `disabled`;
- в коде нет приватных ключей, signer adapter, Jupiter `/execute`, `sendTransaction` или другого broadcast пути;
- quote endpoint только читает котировку и сохраняет её доказательство;
- Rust endpoint `/v1/simulate` сейчас выполняет fail-closed policy check, но не заявляет об on-chain simulation;
- Stage 4 devnet execution и Stage 5 mainnet canary не включены.

## Требования

- Node.js 24+
- pnpm 11.25+
- Rust 1.98.1
- Docker с Compose

PostgreSQL и Redis используют локальные порты `55433` и `56379`, чтобы не конфликтовать с Sentinel и другими сервисами.

## Запуск локально

В PowerShell из корня репозитория:

```powershell
Copy-Item .env.example .env
pnpm install
pnpm db:up
pnpm db:migrate
pnpm check
```

Затем в отдельных окнах:

```powershell
pnpm --filter @execution/control-api dev
```

```powershell
cargo run -p execution-engine
```

```powershell
cargo run -p market-data
```

Сервис market-data использует RPC URL и кластер из `.env`. Yellowstone gRPC запускается внутри Linux окружения; локальный Windows запуск остаётся RPC-only.

## API

- `GET /health`, `GET /ready`
- `POST /v1/intents` — принять durable idempotent intent; повтор того же ключа возвращает исходную запись, другое содержимое с тем же ключом даёт `409`;
- `POST /v1/quotes` — запросить и сохранить Jupiter quote-only для `mainnet-beta`;
- `GET /v1/quotes/{id}` — прочитать нормализованную котировку;
- `POST /v1/simulate` в Rust-сервисе — fail-closed policy check без создания транзакции.

По умолчанию `JUPITER_API_KEY` пустой. Если Jupiter требует ключ для выбранного тарифа, укажите его в `.env`.

## Проверки

`pnpm check` выполняет форматирование, ESLint, TypeScript typecheck/tests/build, Rust fmt/Clippy/tests и проверку Docker Compose.

Интеграционный запуск с реальными RPC/Yellowstone credentials, end-to-end tests, нагрузочные SLO проверки и security audit остаются отдельными production gates из SDD.
